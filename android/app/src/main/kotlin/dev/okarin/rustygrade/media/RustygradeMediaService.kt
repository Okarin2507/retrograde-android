package dev.okarin.rustygrade.media

import android.app.PendingIntent
import android.content.Intent
import android.media.AudioManager
import android.os.Bundle
import androidx.annotation.OptIn
import androidx.core.content.ContextCompat
import androidx.media3.common.C
import androidx.media3.common.MediaItem
import androidx.media3.common.util.UnstableApi
import androidx.media3.session.LibraryResult
import androidx.media3.session.MediaLibraryService
import androidx.media3.session.MediaSession
import androidx.media3.session.SessionCommand
import androidx.media3.session.SessionError
import androidx.media3.session.SessionResult
import com.google.common.collect.ImmutableList
import com.google.common.util.concurrent.Futures
import com.google.common.util.concurrent.ListenableFuture
import com.google.common.util.concurrent.MoreExecutors
import dev.okarin.rustygrade.MainActivity
import dev.okarin.rustygrade.RustygradeApp
import java.util.concurrent.Executors
import java.util.concurrent.RejectedExecutionException
import java.util.concurrent.TimeUnit
import uniffi.rustygrade_ffi.RestoredSession
import uniffi.rustygrade_ffi.SettingsSnapshot

/**
 * Foreground media service: ExoPlayer + the Rust queue behind a `MediaLibrarySession`.
 * The session gives the lockscreen, notification, Bluetooth (AVRCP) and Android Auto their controls.
 */
@OptIn(UnstableApi::class)
class RustygradeMediaService : MediaLibraryService() {
    private var session: MediaLibrarySession? = null
    private lateinit var coordinator: PlaybackCoordinator
    private val io = MoreExecutors.listeningDecorator(Executors.newSingleThreadExecutor())
    private var startupRestore: ListenableFuture<*>? = null

    override fun onCreate() {
        super.onCreate()
        val app = application as RustygradeApp

        // A fixed session id lets the equalizer attach once and survive track changes (and cover the
        // second player used during a crossfade).
        // `ERROR` (-1) means no session could be made: then ExoPlayer keeps its own and there is no EQ.
        val audioSessionId = (getSystemService(AUDIO_SERVICE) as AudioManager).generateAudioSessionId()
            .takeIf { it != AudioManager.ERROR } ?: C.AUDIO_SESSION_ID_UNSET
        val factory = PlayerFactory(this, audioSessionId)
        val handle = factory.create(handlesAudioFocus = true) // ducking for notifications, pause for calls

        coordinator = PlaybackCoordinator(
            this, handle, factory, app.engine, app.player,
            EqualizerController(audioSessionId), UsbDacController(this), io,
        )
        coordinator.start()

        // Settings first, then the saved queue: both come back through the main executor in this order,
        // so EQ / ReplayGain / crossfade are known by the time the restored queue is prepared.
        val settings = io.submit<SettingsSnapshot?> {
            try { app.engine.loadSettings() } catch (_: Exception) { null }
        }
        Futures.addCallback(
            settings,
            object : com.google.common.util.concurrent.FutureCallback<SettingsSnapshot?> {
                override fun onSuccess(result: SettingsSnapshot?) {
                    if (result != null) coordinator.applySettings(result)
                }
                override fun onFailure(t: Throwable) = Unit
            },
            ContextCompat.getMainExecutor(this),
        )

        val sessionPlayer = RustygradePlayer(handle.exo, coordinator)
        val openApp = PendingIntent.getActivity(
            this, 0, Intent(this, MainActivity::class.java),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        session = MediaLibrarySession.Builder(this, sessionPlayer, LibraryCallback())
            .setId("rustygrade")
            .setSessionActivity(openApp)
            .build()

        // Bring back the last queue, paused at the saved position (heavy first load: off the main thread).
        val restored = io.submit<RestoredSession?> {
            try { app.player.restoreSession() } catch (_: Exception) { null }
        }
        startupRestore = restored
        Futures.addCallback(
            restored,
            object : com.google.common.util.concurrent.FutureCallback<RestoredSession?> {
                override fun onSuccess(result: RestoredSession?) {
                    if (result != null) coordinator.applyRestored(result)
                }
                override fun onFailure(t: Throwable) = Unit
            },
            ContextCompat.getMainExecutor(this),
        )
    }

    override fun onGetSession(controllerInfo: MediaSession.ControllerInfo): MediaLibrarySession? = session

    override fun onTaskRemoved(rootIntent: Intent?) {
        // Queued on the I/O executor (it is not shut down here: the service may keep playing).
        coordinator.saveSession()
        // Keep playing in the background; stop the service only when there is nothing to hear.
        if (!coordinator.isActive()) stopSelf()
    }

    override fun onDestroy() {
        startupRestore?.cancel(false)
        // The session goes first: no controller may reach the player while it is being released.
        session?.run {
            release()
            session = null
        }
        // The coordinator queues its final session save; the executor then drains and stops, and a
        // short wait lets that save land before the process can be killed.
        if (::coordinator.isInitialized) coordinator.release()
        io.shutdown()
        try {
            io.awaitTermination(1500, TimeUnit.MILLISECONDS)
        } catch (_: InterruptedException) {
            Thread.currentThread().interrupt()
        }
        super.onDestroy()
    }

    private inner class LibraryCallback : MediaLibrarySession.Callback {
        private val app get() = application as RustygradeApp

        /** The service is exported: only this app's own controllers may use the queue commands. */
        private fun isOwn(controller: MediaSession.ControllerInfo) = controller.packageName == packageName

        override fun onConnect(
            session: MediaSession,
            controller: MediaSession.ControllerInfo,
        ): MediaSession.ConnectionResult {
            // Android Auto, Bluetooth and the system UI get the standard commands, not our custom ones.
            val commands = MediaSession.ConnectionResult.DEFAULT_SESSION_AND_LIBRARY_COMMANDS.buildUpon()
                .apply { if (isOwn(controller)) PlaybackCommands.all.forEach { add(it) } }
                .build()
            return MediaSession.ConnectionResult.accept(
                commands,
                MediaSession.ConnectionResult.DEFAULT_PLAYER_COMMANDS,
            )
        }

        override fun onCustomCommand(
            session: MediaSession,
            controller: MediaSession.ControllerInfo,
            customCommand: SessionCommand,
            args: Bundle,
        ): ListenableFuture<SessionResult> {
            if (!isOwn(controller)) {
                return Futures.immediateFuture(SessionResult(SessionError.ERROR_PERMISSION_DENIED))
            }
            val ids = args.getStringArrayList(PlaybackCommands.ARG_IDS).orEmpty()
            val hasIndex = args.containsKey(PlaybackCommands.ARG_INDEX)
            val optionalIndex = if (hasIndex) args.getInt(PlaybackCommands.ARG_INDEX) else null
            val index = optionalIndex ?: 0
            val to = args.getInt(PlaybackCommands.ARG_TO, index)
            // Indices become unsigned in Rust: a negative one would turn into a huge position.
            val badOptionalIndex = when (customCommand.customAction) {
                PlaybackCommands.PLAY_ALBUM, PlaybackCommands.PLAY_TRACKS -> optionalIndex != null && optionalIndex < 0
                else -> false
            }
            val usesRequiredIndex = when (customCommand.customAction) {
                PlaybackCommands.REMOVE_AT, PlaybackCommands.JUMP_TO, PlaybackCommands.MOVE_ITEM -> true
                else -> false
            }
            if (badOptionalIndex || (usesRequiredIndex && (index < 0 || to < 0))) {
                return Futures.immediateFuture(SessionResult(SessionError.ERROR_BAD_VALUE))
            }
            try {
                when (customCommand.customAction) {
                    PlaybackCommands.PLAY_ALBUM ->
                        coordinator.playAlbum(args.getString(PlaybackCommands.ARG_KEY).orEmpty(), optionalIndex)
                    PlaybackCommands.PLAY_ALBUM_SHUFFLED ->
                        coordinator.playAlbumShuffled(args.getString(PlaybackCommands.ARG_KEY).orEmpty())
                    PlaybackCommands.PLAY_TRACKS -> coordinator.playTracks(ids, optionalIndex)
                    PlaybackCommands.PLAY_TRACKS_SHUFFLED -> coordinator.playTracksShuffled(ids)
                    PlaybackCommands.PLAY_NEXT -> coordinator.playNext(ids)
                    PlaybackCommands.ENQUEUE -> coordinator.enqueue(ids)
                    PlaybackCommands.REMOVE_AT -> coordinator.removeAt(index)
                    PlaybackCommands.JUMP_TO -> coordinator.jumpTo(index)
                    PlaybackCommands.CLEAR_QUEUE -> coordinator.clearQueue()
                    PlaybackCommands.RELOAD_CURRENT -> coordinator.reloadCurrent()
                    PlaybackCommands.MOVE_ITEM -> coordinator.moveItem(index, to)
                    PlaybackCommands.CYCLE_REPEAT -> coordinator.cycleRepeat()
                    PlaybackCommands.REFRESH_FX -> coordinator.refreshFx()
                    PlaybackCommands.SYNC_LIBRARY -> coordinator.syncLibrary()
                    PlaybackCommands.USB_REQUEST_PERMISSION -> coordinator.requestUsbPermission()
                    PlaybackCommands.SET_USB_VOLUME ->
                        coordinator.setUsbHardwareVolume(args.getFloat(PlaybackCommands.ARG_FLOAT, 0.5f))
                    else -> return Futures.immediateFuture(SessionResult(SessionError.ERROR_NOT_SUPPORTED))
                }
            } catch (e: Exception) {
                return Futures.immediateFuture(SessionResult(SessionError.ERROR_UNKNOWN))
            }
            return Futures.immediateFuture(SessionResult(SessionResult.RESULT_SUCCESS))
        }

        override fun onGetLibraryRoot(
            session: MediaLibrarySession,
            browser: MediaSession.ControllerInfo,
            params: LibraryParams?,
        ): ListenableFuture<LibraryResult<MediaItem>> {
            val root = MediaItem.Builder()
                .setMediaId(BrowseIds.ROOT)
                .setMediaMetadata(
                    androidx.media3.common.MediaMetadata.Builder()
                        .setTitle("Rustygrade")
                        .setIsBrowsable(true)
                        .setIsPlayable(false)
                        .build(),
                )
                .build()
            return Futures.immediateFuture(LibraryResult.ofItem(root, params))
        }

        override fun onGetChildren(
            session: MediaLibrarySession,
            browser: MediaSession.ControllerInfo,
            parentId: String,
            page: Int,
            pageSize: Int,
            params: LibraryParams?,
        ): ListenableFuture<LibraryResult<ImmutableList<MediaItem>>> {
            if (page < 0 || pageSize <= 0) {
                return Futures.immediateFuture(LibraryResult.ofError(SessionError.ERROR_BAD_VALUE))
            }
            return browse { children(parentId, page, pageSize, params) }
        }

        private fun children(
            parentId: String,
            page: Int,
            pageSize: Int,
            params: LibraryParams?,
        ): LibraryResult<ImmutableList<MediaItem>> {
            val engine = app.engine
            val items: List<MediaItem> = when {
                parentId == BrowseIds.ROOT ->
                    engine.loadLibrarySnapshot("added_desc").albums.map { it.toBrowseItem() }
                BrowseIds.albumKey(parentId) != null -> {
                    val key = BrowseIds.albumKey(parentId)!!
                    engine.getAlbumDetail(key)?.tracks?.mapIndexed { i, t -> t.toBrowseItem(key, i) }.orEmpty()
                }
                else -> emptyList()
            }
            // pageSize is often Int.MAX_VALUE: do the arithmetic in Long.
            val from = (page.toLong() * pageSize).coerceIn(0L, items.size.toLong()).toInt()
            val to = (from.toLong() + pageSize).coerceIn(from.toLong(), items.size.toLong()).toInt()
            return LibraryResult.ofItemList(ImmutableList.copyOf(items.subList(from, to)), params)
        }

        /** Runs a browse query on the I/O thread; a failing engine call is an error result, not a crash. */
        private fun <T : Any> browse(block: () -> LibraryResult<T>): ListenableFuture<LibraryResult<T>> {
            if (io.isShutdown) return Futures.immediateFuture(LibraryResult.ofError(SessionError.ERROR_UNKNOWN))
            return try {
                io.submit<LibraryResult<T>> {
                    try {
                        block()
                    } catch (e: Exception) {
                        LibraryResult.ofError(SessionError.ERROR_UNKNOWN)
                    }
                }
            } catch (_: RejectedExecutionException) {
                Futures.immediateFuture(LibraryResult.ofError(SessionError.ERROR_UNKNOWN))
            }
        }

        override fun onGetItem(
            session: MediaLibrarySession,
            browser: MediaSession.ControllerInfo,
            mediaId: String,
        ): ListenableFuture<LibraryResult<MediaItem>> = browse {
            val engine = app.engine
            BrowseIds.albumKey(mediaId)?.let { key ->
                engine.getAlbumDetail(key)?.let { return@browse LibraryResult.ofItem(it.album.toBrowseItem(), null) }
            }
            BrowseIds.trackRef(mediaId)?.let { (key, i) ->
                engine.getAlbumDetail(key)?.tracks?.getOrNull(i)?.let {
                    return@browse LibraryResult.ofItem(it.toBrowseItem(key, i), null)
                }
            }
            LibraryResult.ofError(SessionError.ERROR_BAD_VALUE)
        }

        override fun onSetMediaItems(
            mediaSession: MediaSession,
            controller: MediaSession.ControllerInfo,
            mediaItems: MutableList<MediaItem>,
            startIndex: Int,
            startPositionMs: Long,
        ): ListenableFuture<MediaSession.MediaItemsWithStartPosition> {
            // Only our own browse ids (`album:` / `track:`) are playable. Anything else, such as a
            // `file://` or `content://` URI from another app, would be played as given: refuse it.
            val resolved = try {
                coordinator.resolveBrowseItems(mediaItems)
            } catch (e: Exception) {
                null
            }
            return if (resolved != null) {
                Futures.immediateFuture(resolved)
            } else {
                Futures.immediateFailedFuture(UnsupportedOperationException("Unsupported media item"))
            }
        }

        /** The queue lives in Rust: items added by a controller as raw URIs are never accepted. */
        override fun onAddMediaItems(
            mediaSession: MediaSession,
            controller: MediaSession.ControllerInfo,
            mediaItems: MutableList<MediaItem>,
        ): ListenableFuture<MutableList<MediaItem>> =
            Futures.immediateFailedFuture(UnsupportedOperationException("Unsupported media item"))

        /** System "resume" controls (API 33+) and headset play: bring back the last session. */
        override fun onPlaybackResumption(
            mediaSession: MediaSession,
            controller: MediaSession.ControllerInfo,
            isForPlayback: Boolean,
        ): ListenableFuture<MediaSession.MediaItemsWithStartPosition> {
            // A queue is already loaded: resume it as is; the saved file is older than memory.
            // (`hasQueue` is main-thread state, so it is read here, not on the I/O thread.)
            if (coordinator.hasQueue) {
                val items = coordinator.resumptionItems(null)
                return if (items != null) {
                    Futures.immediateFuture(items)
                } else {
                    Futures.immediateFailedFuture(UnsupportedOperationException("Nothing to resume"))
                }
            }
            val restored = try {
                io.submit<RestoredSession?> {
                    try { app.player.restoreSession() } catch (_: Exception) { null }
                }
            } catch (_: RejectedExecutionException) {
                return Futures.immediateFailedFuture(UnsupportedOperationException("Nothing to resume"))
            }
            return Futures.transform(
                restored,
                { r ->
                    // A queue may have been started while the file was read: it is newer than the file.
                    val items = if (coordinator.hasQueue) coordinator.resumptionItems(null) else coordinator.resumptionItems(r)
                    items ?: throw UnsupportedOperationException("Nothing to resume")
                },
                ContextCompat.getMainExecutor(this@RustygradeMediaService),
            )
        }
    }
}
