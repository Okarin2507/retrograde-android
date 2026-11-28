package dev.okarin.rustygrade.media

import android.content.Context
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.util.Log
import androidx.annotation.OptIn
import androidx.media3.common.C
import androidx.media3.common.MediaItem
import androidx.media3.common.PlaybackException
import androidx.media3.common.Player
import androidx.media3.exoplayer.ExoPlayer
import androidx.media3.common.util.UnstableApi
import androidx.media3.session.MediaSession
import com.google.common.util.concurrent.Futures
import com.google.common.util.concurrent.ListenableFuture
import com.google.common.util.concurrent.SettableFuture
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors
import java.util.concurrent.RejectedExecutionException
import java.util.concurrent.atomic.AtomicReference
import uniffi.rustygrade_ffi.AdvanceKind
import uniffi.rustygrade_ffi.PlayerEngine
import uniffi.rustygrade_ffi.PlayerState
import uniffi.rustygrade_ffi.RestoredSession
import uniffi.rustygrade_ffi.RustygradeEngine
import uniffi.rustygrade_ffi.SettingsSnapshot
import uniffi.rustygrade_ffi.TrackInfo
import uniffi.rustygrade_ffi.UsbDriverException
import uniffi.rustygrade_ffi.crossfadeGains
import uniffi.rustygrade_ffi.eqPlan
import uniffi.rustygrade_ffi.replayGainDb
import uniffi.rustygrade_ffi.replayGainFactor

/**
 * Glue between the Rust queue ([PlayerEngine]) and ExoPlayer. Main thread only.
 *
 * Rust owns the queue. ExoPlayer holds only a window `[previous, current, next]`, which is enough
 * for gapless playback and for the previous/next buttons to be offered by the lockscreen and
 * Bluetooth controllers. After every Rust call the window is reconciled with the returned state.
 */
@OptIn(UnstableApi::class)
class PlaybackCoordinator(
    private val context: Context,
    initial: PlayerHandle,
    private val factory: PlayerFactory,
    private val library: RustygradeEngine,
    private val engine: PlayerEngine,
    private val equalizer: EqualizerController,
    private val usb: UsbDacController,
    /**
     * Where disk and FFI work (session saves, play-count polls, settings reads) runs: never the main
     * thread. The service passes its single-thread executor, so these calls also stay in order.
     */
    private val io: ExecutorService,
) : Player.Listener {

    private val main = Handler(Looper.getMainLooper())

    /**
     * DAC hardware volume / ReplayGain writes are EP0 control transfers (up to ~1 s on some DACs):
     * they run here, in order, never on the main thread.
     */
    private val usbControl: ExecutorService = Executors.newSingleThreadExecutor { r -> Thread(r, "RustyUsbControl") }
    /** The one volume target still to send: a set replaces it, a step moves it (see [stepUsbHardwareVolume]). */
    private val pendingUsbVolume = AtomicReference<Float?>(null)
    /** The volume target a transfer on [usbControl] is sending right now. */
    @Volatile
    private var inFlightUsbVolume: Float? = null
    private val pendingUsbReplayGainDb = AtomicReference<Float?>(null)

    /** The player that is heard. After a crossfade the two players swap roles. */
    private var primary: PlayerHandle = initial
    private var spare: PlayerHandle? = null
    private val exo: ExoPlayer get() = primary.exo
    private val replayGain: ReplayGainAudioProcessor get() = primary.replayGain

    /** Called when the player behind the session changes (the crossfade swapped players). */
    var onPlayerSwapped: ((ExoPlayer) -> Unit)? = null

    /** Latest state from Rust; what the session reports for shuffle, repeat and skip commands. */
    var state: PlayerState? = null
        private set

    /** A queue is loaded (playing, paused or restored). */
    val hasQueue: Boolean get() = state?.current != null

    /** Called whenever [state] changes so the session player can refresh its reported state. */
    var onStateChanged: (() -> Unit)? = null

    /**
     * Audio settings. Null until the first read lands (the service reads them off the main thread
     * and hands them over with [applySettings]); until then playback runs with plain defaults.
     */
    private var fx: SettingsSnapshot? = null
    private val crossfadeSeconds: Int get() = fx?.crossfadeSeconds ?: 0
    /** Track sample rate last evaluated against the USB DAC; null forces re-sync. */
    private var lastUsbRate: Int? = null
    /** Tracks current track's scanned bit depth for [UsbDirectAudioSink] (read on audio thread). */
    @Volatile
    private var currentTrackBits: Int = 0
    /** True while handing a USB disconnect/unsupported-rate fallback so onPlayerError does not skip. */
    @Volatile
    private var handlingUsbFallback: Boolean = false
    private var failures = 0
    private var ticks = 0
    private var released = false

    private val ticker = object : Runnable {
        override fun run() {
            if (released) return
            tick()
            if (exo.isPlaying) main.postDelayed(this, TICK_MS)
        }
    }

    /**
     * Pauses playback when a headphone-class output connects or disconnects (any output mode).
     * While bit-perfect is on, USB outputs appearing in / vanishing from AudioManager are ignored
     * altogether: they follow our own DAC claims and kernel reconnects (track-change rate
     * fallbacks), with no bound on how late. Physical USB plugs and unplugs then pause through the
     * UsbManager broadcasts instead (see [start]). Right after bit-perfect is switched off, the
     * controller's own-change window still covers the kernel driver coming back.
     */
    private val routes = AudioRouteWatcher(
        context,
        main,
        suppressUsb = { fx?.usbBitPerfect == true || usb.suppressesUsbRouteEvents() },
    ) { connected, _ -> pauseForOutputChange(if (connected) "output connected" else "output disconnected") }

    /**
     * Pauses (never stops: the queue and position stay) because the audio route changed, so
     * playback does not carry on through a newly connected device or out of the speaker. Nothing
     * resumes it automatically: a pending [rebuildPrimary] carries over the paused state.
     */
    private fun pauseForOutputChange(reason: String) {
        if (released) return
        if (exo.playWhenReady || spare?.exo?.playWhenReady == true) Log.i(TAG, "Pausing: $reason")
        fsm.cancel()
        exo.playWhenReady = false
    }

    private fun onUsbChanged() {
        if (!released && fx?.usbBitPerfect == true) {
            lastUsbRate = null
            applyFxSettings()
        }
    }

    /** Physical USB DAC unplug while our driver owned it: pause playback and hand back to OS audio. */
    private fun onUsbDetached() {
        if (released) return
        handlingUsbFallback = false
        pauseForOutputChange("USB DAC detached")
        if (primary.usbDirect) {
            rebuildPrimary(usbDirect = false, closeUsbFirst = false, forcePause = true)
        }
        PlaybackHub.setUsb(if (fx?.usbBitPerfect == true) UsbStatus.NoDevice else UsbStatus.Off)
        onStateChanged?.invoke()
        applyNonUsbFx()
    }

    fun start() {
        exo.addListener(this)
        usb.start(
            onChanged = { main.post { onUsbChanged() } },
            onDetachedWhileActive = { main.post { onUsbDetached() } },
            // Bit-perfect only (otherwise AudioManager reports the route change). Posted before the
            // onChanged that opens the session, so the rebuild to the USB player starts paused.
            onAudioDeviceAttached = { main.post { if (fx?.usbBitPerfect == true) pauseForOutputChange("USB audio device attached") } },
            onAudioDeviceDetached = { main.post { if (fx?.usbBitPerfect == true) pauseForOutputChange("USB audio device detached") } },
        )
        routes.start()
        applyFxSettings()
    }

    fun release() {
        released = true
        routes.stop()
        main.removeCallbacksAndMessages(null)
        saveSession(force = true) // queued before the service shuts the executor down
        exo.removeListener(this)
        dropSpare()
        spare?.exo?.removeListener(spareListener)
        spare?.exo?.release()
        primary.exo.release()
        equalizer.release()
        usbControl.shutdown()
        usb.release()
        PlaybackHub.setPlaying(false)
    }

    fun requestUsbPermission() {
        if (!released) usb.requestPermissionNow()
    }

    /**
     * Completes once the DAC took the volume (or a later target replaced it). A burst of sets and
     * steps (a slider drag, a held volume key) collapses to one pending target, sent in call order.
     */
    fun setUsbHardwareVolume(normalized: Float): ListenableFuture<*> {
        pendingUsbVolume.set(normalized.coerceIn(0f, 1f))
        return drainUsbVolume()
    }

    /** Moves the newest target (pending, else in flight, else the DAC's known volume) by [delta]. */
    fun stepUsbHardwareVolume(delta: Float): ListenableFuture<*> {
        val known = PlaybackHub.usbVolume.value?.normalized ?: return Futures.immediateFuture(Unit)
        pendingUsbVolume.updateAndGet { p -> ((p ?: inFlightUsbVolume ?: known) + delta).coerceIn(0f, 1f) }
        return drainUsbVolume()
    }

    /**
     * Sends the pending volume target. Every set/step queues one of these after updating the target,
     * so a call whose target an earlier task already took (or a newer one replaced) still completes
     * after that transfer, and the newest target is always sent last.
     */
    private fun drainUsbVolume(): ListenableFuture<*> = onUsbControl {
        val target = pendingUsbVolume.get() ?: return@onUsbControl false
        inFlightUsbVolume = target
        try {
            // A newer target arrived meanwhile: its own task, queued after this one, sends it.
            if (!pendingUsbVolume.compareAndSet(target, null)) return@onUsbControl false
            usb.setHardwareVolume(target)
        } finally {
            inFlightUsbVolume = null
        }
    }

    /**
     * Runs a DAC control [write] on [usbControl]. The returned future completes there once the write
     * ran, whatever it threw; a change refreshes the session player through a post to main (the
     * write already published the new volume to [PlaybackHub], which the session player reads).
     */
    private fun onUsbControl(write: () -> Boolean): ListenableFuture<*> {
        val done = SettableFuture.create<Unit>()
        if (released) {
            done.set(Unit)
            return done
        }
        try {
            usbControl.execute {
                try {
                    val changed = try { write() } catch (_: Exception) { false }
                    if (changed) main.post { if (!released) onStateChanged?.invoke() }
                } finally {
                    done.set(Unit)
                }
            }
        } catch (_: RejectedExecutionException) {
            done.set(Unit)
        }
        return done
    }

    // ---- commands -----------------------------------------------------------------------------

    fun playAlbum(key: String, start: Int?) = playFresh(engine.playAlbum(key, start?.toUInt()))

    fun playAlbumShuffled(key: String) = playFresh(engine.playAlbumShuffled(key))

    fun playTracks(ids: List<String>, start: Int?) = playFresh(engine.playTracks(ids, start?.toUInt()))

    fun playTracksShuffled(ids: List<String>) = playFresh(engine.playTracksShuffled(ids))

    fun playNext(ids: List<String>) = edit(engine.playNext(ids))

    fun enqueue(ids: List<String>) = edit(engine.enqueue(ids))

    fun removeAt(index: Int) = reconcile(engine.removeAt(index.toUInt()))

    /** Drag and drop in the queue sheet; the playing track keeps playing. */
    fun moveItem(from: Int, to: Int) = reconcile(engine.moveItem(from.toUInt(), to.toUInt()))

    /** Loads the current track again at the same position, paused as it was (its file was rewritten). */
    fun reloadCurrent() {
        val s = state ?: return
        fsm.cancel()
        val wasPlaying = exo.playWhenReady
        syncWindow(s, forceReload = true, startMs = exo.currentPosition.coerceAtLeast(0))
        exo.playWhenReady = wasPlaying
    }

    /** A seek by the user: a fade towards the next track no longer fits the position. */
    fun onUserSeek() = fsm.cancel()

    /** There is something playing or about to play (the service must stay up). */
    fun isActive(): Boolean = exo.playWhenReady && exo.mediaItemCount > 0

    fun clearQueue() {
        fsm.cancel()
        publish(engine.clearQueue())
        exo.stop()
        exo.clearMediaItems()
        saveSession()
    }

    fun jumpTo(index: Int) = moveTo(engine.jumpTo(index.toUInt()), autoPlay = true)

    fun skipNext() = moveTo(engine.stepNext(false), autoPlay = false)

    /** The desktop rule: past 3 s "previous" restarts the track, otherwise it steps back. */
    fun skipPrevious() {
        if (exo.currentPosition > RESTART_THRESHOLD_MS || state?.canPrevious != true) {
            fsm.cancel() // restarting the outgoing track: a fade towards the next one no longer fits
            exo.seekTo(0)
            return
        }
        stepPrevious()
    }

    fun stepPrevious() = moveTo(engine.stepPrev(), autoPlay = false)

    fun setShuffle(enabled: Boolean) = reconcile(engine.setShuffle(enabled))

    fun setRepeat(mode: Int) = reconcile(engine.setRepeat(mode))

    fun cycleRepeat() = reconcile(engine.cycleRepeat())

    /** A scan, ReplayGain update or delete finished: fold it into the queue and refresh FX. */
    fun syncLibrary() {
        val s = engine.state()
        if (s.revision != state?.revision) {
            reconcile(s)
        } else if (s.current != state?.current) {
            publish(s)
            applyReplayGain(s.current)
        }
    }

    /** Re-reads EQ / ReplayGain / USB settings after the UI changed them. */
    fun refreshFx() {
        submitIo {
            val settings = library.loadSettings()
            main.post { applySettings(settings) }
        }
    }

    /** Takes freshly read settings (main thread) and applies EQ, ReplayGain, USB and crossfade. */
    fun applySettings(settings: SettingsSnapshot) {
        if (released) return
        val previous = fx
        // Switching bit-perfect on or off moves playback to another output path: pause first so
        // the player rebuild in applyFxSettings() carries over the paused state. Not on the first
        // read, nor when only other settings changed.
        if (previous != null && previous.usbBitPerfect != settings.usbBitPerfect) {
            pauseForOutputChange("bit-perfect ${if (settings.usbBitPerfect) "on" else "off"}")
        }
        fx = settings
        lastUsbRate = null
        applyFxSettings() // also tells Rust the crossfade length / direct output, once it changed
        updateFadeWatch()
    }

    // ---- restore & external controllers -------------------------------------------------------

    /** Prepares the saved queue paused at the saved position. Ignored if playback already started. */
    fun applyRestored(restored: RestoredSession) {
        if (released || state?.current != null) return
        fsm.reset()
        // The snapshot inside `restored` was taken on the I/O thread, possibly before the crossfade
        // setting reached Rust (applySettings): read the state again, now that settings are applied.
        val s = try {
            engine.state().takeIf { it.current != null } ?: restored.state
        } catch (_: Exception) {
            restored.state
        }
        publish(s)
        syncWindow(s, forceReload = true, startMs = (restored.positionS * 1000).toLong())
        exo.playWhenReady = false
        applyFx(s.current)
        pushPosition()
    }

    /** `onPlaybackResumption`: the saved session as a window the session can hand to the player. */
    fun resumptionItems(restored: RestoredSession?): MediaSession.MediaItemsWithStartPosition? {
        if (released) return null
        val s = restored?.state ?: state ?: return null
        val cur = s.current ?: return null
        val window = windowOf(s)
        fsm.cancel()
        fsm.reset()
        publish(s)
        exo.repeatMode = if (s.loopCurrent) Player.REPEAT_MODE_ONE else Player.REPEAT_MODE_OFF
        applyFx(cur)
        val startMs = restored?.let { (it.positionS * 1000).toLong() } ?: exo.currentPosition
        return MediaSession.MediaItemsWithStartPosition(
            window.map { it.toMediaItem() },
            if (s.previous != null) 1 else 0,
            startMs,
        )
    }

    /** `onSetMediaItems` from a browse tree (Android Auto, Assistant): album or track ids. */
    fun resolveBrowseItems(items: List<MediaItem>): MediaSession.MediaItemsWithStartPosition? {
        val first = items.firstOrNull()?.mediaId ?: return null
        val s = BrowseIds.albumKey(first)?.let { engine.playAlbum(it, null) }
            ?: BrowseIds.trackRef(first)?.let { (key, idx) -> engine.playAlbum(key, idx.toUInt()) }
            ?: return null
        val cur = s.current ?: return null
        fsm.cancel()
        fsm.reset()
        publish(s)
        exo.repeatMode = if (s.loopCurrent) Player.REPEAT_MODE_ONE else Player.REPEAT_MODE_OFF
        applyFx(cur)
        return MediaSession.MediaItemsWithStartPosition(
            windowOf(s).map { it.toMediaItem() },
            if (s.previous != null) 1 else 0,
            0L,
        )
    }

    /** [force] queues the write even after [release] (the final save of a shutting-down service). */
    fun saveSession(force: Boolean = false) {
        // Player values are read here, on the main thread; only the FFI write goes to the executor.
        val pos = try {
            // A finished queue is saved at the start of its last track, not at its end.
            if (exo.playbackState == Player.STATE_ENDED) 0L else exo.currentPosition.coerceAtLeast(0)
        } catch (_: Exception) {
            return
        }
        val seconds = pos / 1000.0
        submitIo(force) { engine.saveSession(seconds) }
    }

    /** Runs [block] on the I/O executor; failures are dropped (a missed save must not stop playback). */
    private fun submitIo(force: Boolean = false, block: () -> Unit) {
        if (released && !force) return
        try {
            io.execute {
                try {
                    block()
                } catch (_: Exception) {
                }
            }
        } catch (_: RejectedExecutionException) {
            // The service is shutting down: nothing left to persist to.
        }
    }

    // ---- Player.Listener ----------------------------------------------------------------------

    override fun onMediaItemTransition(mediaItem: MediaItem?, reason: Int) {
        when (reason) {
            Player.MEDIA_ITEM_TRANSITION_REASON_AUTO -> onAutoAdvance(mediaItem ?: return)
            Player.MEDIA_ITEM_TRANSITION_REASON_REPEAT -> {
                engine.noteTrackStarted()
                applyFx(state?.current)
            }
            // SEEK / PLAYLIST_CHANGED are our own window edits: Rust already knows.
            else -> Unit
        }
    }

    override fun onPositionDiscontinuity(oldPosition: Player.PositionInfo, newPosition: Player.PositionInfo, reason: Int) {
        // Seeks (also while paused, when the 1 s ticker is not running) and track changes: the UI
        // must show the new position at once instead of the last report.
        pushPosition()
    }

    override fun onIsPlayingChanged(isPlaying: Boolean) {
        PlaybackHub.setPlaying(isPlaying)
        main.removeCallbacks(ticker)
        if (isPlaying) {
            failures = 0
            PlaybackHub.setError(null)
            main.postDelayed(ticker, TICK_MS)
        } else {
            // Paused (or stopped) in the middle of a fade: give up the fade, the track keeps its place.
            fsm.cancel()
            saveSession()
        }
        updateFadeWatch()
    }

    override fun onPlaybackStateChanged(playbackState: Int) {
        if (playbackState == Player.STATE_ENDED) {
            if (fsm.isFading) {
                fsm.finishNow() // the outgoing track ended a hair before the fade did
                return
            }
            val following = state?.next
            if (following != null) {
                // The window has no next item while a crossfade applies: a track that ends without a
                // fade (the user seeked into its last seconds) moves on without overlap.
                main.post { advanceWithoutOverlap(following) }
                return
            }
            // The queue ran out (repeat off): leave the last track ready, counted position reset.
            PlaybackHub.setPosition(0, exo.duration.coerceAtLeast(0))
            submitIo { engine.saveSession(0.0) }
            // ExoPlayer would replay from the *start of the playlist* (our "previous" item) on the
            // next play(): put the last track back, paused, so play() replays that one.
            main.post {
                val s = state ?: return@post
                if (!released && exo.playbackState == Player.STATE_ENDED) {
                    syncWindow(s, forceReload = true)
                    exo.playWhenReady = false
                    engine.noteTrackStarted() // playing it again is a new play
                    saveSession()
                }
            }
        }
    }

    override fun onPlayerError(error: PlaybackException) {
        if (handlingUsbFallback || isUsbStreamFailure(error)) {
            // The USB stream error handler is switching back to system audio / pausing on unplug.
            return
        }
        fsm.cancel() // the second player of a fade must not keep playing under a failed primary
        failures++
        val s = state
        if (failures <= MAX_FAILURES && s?.canNext == true) {
            // An unreadable file must not stop the music: move on to the next track.
            skipNext()
            exo.prepare()
            exo.play()
        } else {
            PlaybackHub.setError(error.message ?: "Playback failed")
        }
    }

    // ---- internals ----------------------------------------------------------------------------

    /**
     * True for an error raised by [UsbDirectAudioSink]: its handler already falls back to system
     * audio or pauses, so the track must not be skipped as unreadable. ExoPlayer wraps the sink's
     * exception, so the cause chain is walked.
     */
    private fun isUsbStreamFailure(error: PlaybackException): Boolean {
        var cause: Throwable? = error.cause
        var depth = 0
        while (cause != null && depth++ < 8) {
            if (cause is UsbStreamFailure || cause is UsbDriverException) return true
            cause = cause.cause
        }
        return false
    }

    private fun advanceWithoutOverlap(next: TrackInfo) {
        if (released || exo.playbackState != Player.STATE_ENDED) return
        val r = engine.onPlayerAdvanced(next.filePath)
        publish(r.state)
        syncWindow(r.state, forceReload = true)
        exo.playWhenReady = r.kind != AdvanceKind.STOP
        applyFx(r.state.current)
        fsm.reset()
        saveSession()
    }

    private fun onAutoAdvance(item: MediaItem) {
        failures = 0
        fsm.reset()
        val r = engine.onPlayerAdvanced(item.mediaId)
        publish(r.state)
        when (r.kind) {
            AdvanceKind.FOLLOWED -> syncWindow(r.state, forceReload = false)
            AdvanceKind.RELOAD -> {
                // The queue changed right at the boundary: load the track that is really meant.
                val wasPlaying = exo.playWhenReady
                syncWindow(r.state, forceReload = true)
                exo.playWhenReady = wasPlaying
            }
            AdvanceKind.STOP -> {
                syncWindow(r.state, forceReload = true)
                exo.playWhenReady = false
            }
        }
        applyFx(r.state.current)
        saveSession()
    }

    private fun playFresh(s: PlayerState) {
        fsm.cancel()
        fsm.reset()
        PlaybackHub.setError(null)
        publish(s)
        val cur = s.current ?: return
        syncWindow(s, forceReload = true)
        exo.playWhenReady = true
        applyFx(cur)
        saveSession()
    }

    private fun edit(r: uniffi.rustygrade_ffi.QueueEditResult) {
        if (r.startedNewQueue) playFresh(r.state) else reconcile(r.state)
    }

    /** The queue changed but the current track did not (shuffle, repeat, hand queue, removal). */
    private fun reconcile(s: PlayerState) {
        // An edit may change what comes next: stop any fade towards the old next track.
        // (also when a fade no longer applies, e.g. the crossfade was switched off mid-fade)
        if (fsm.isFading &&
            (s.current?.id != state?.current?.id || s.next?.id != state?.next?.id || !s.crossfadeApplies || crossfadeSeconds <= 0)
        ) {
            fsm.cancel()
        }
        val old = state
        publish(s)
        val cur = s.current
        if (cur == null) {
            exo.stop()
            exo.clearMediaItems()
            saveSession()
            return
        }
        // Removing the playing track makes another one current: that needs a reload.
        val sameCurrent = exo.currentMediaItem?.mediaId == cur.filePath
        syncWindow(s, forceReload = !sameCurrent)
        if (!sameCurrent) {
            exo.playWhenReady = old != null && exo.playWhenReady
            applyFx(cur)
        }
        saveSession()
    }

    /** Moves to the track Rust just made current, reusing the neighbour ExoPlayer already holds. */
    private fun moveTo(s: PlayerState, autoPlay: Boolean) {
        val old = state
        if (old != null && s.revision == old.revision) return // nothing moved (end of queue)
        fsm.cancel()
        fsm.reset()
        publish(s)
        val cur = s.current ?: return
        val ci = exo.currentMediaItemIndex
        val ahead = if (exo.hasNextMediaItem()) exo.getMediaItemAt(ci + 1).mediaId else null
        val behind = if (ci > 0) exo.getMediaItemAt(ci - 1).mediaId else null
        when {
            ahead == cur.filePath && old?.next?.filePath == cur.filePath -> exo.seekToNextMediaItem()
            behind == cur.filePath && old?.previous?.filePath == cur.filePath -> exo.seekToPreviousMediaItem()
            else -> {
                syncWindow(s, forceReload = true)
                if (autoPlay) exo.playWhenReady = true
                applyFx(cur)
                saveSession()
                return
            }
        }
        syncWindow(s, forceReload = false)
        if (autoPlay) exo.playWhenReady = true
        applyFx(cur)
        saveSession()
    }

    /** While a crossfade applies the window does not hold the next track (the fade starts it elsewhere). */
    private fun nextInWindow(s: PlayerState): TrackInfo? =
        if (s.crossfadeApplies && crossfadeSeconds > 0) null else s.next

    private fun windowOf(s: PlayerState): List<TrackInfo> = listOfNotNull(s.previous, s.current, nextInWindow(s))

    /** ExoPlayer's playlist as a [WindowList] of [TrackInfo]. */
    private val exoWindow = object : WindowList<TrackInfo> {
        override val count get() = exo.mediaItemCount
        override val currentIndex get() = exo.currentMediaItemIndex
        override fun idAt(index: Int) = exo.getMediaItemAt(index).mediaId
        override fun currentId() = exo.currentMediaItem?.mediaId
        override fun setAll(items: List<TrackInfo>, startIndex: Int, startMs: Long) {
            exo.setMediaItems(items.map { it.toMediaItem() }, startIndex, startMs)
            exo.prepare()
        }
        override fun insert(index: Int, item: TrackInfo) = exo.addMediaItem(index, item.toMediaItem())
        override fun append(item: TrackInfo) = exo.addMediaItem(item.toMediaItem())
        override fun remove(fromIndex: Int, toIndexExclusive: Int) = exo.removeMediaItems(fromIndex, toIndexExclusive)
    }
    private val reconciler = WindowReconciler(exoWindow) { it.filePath }

    /** Makes ExoPlayer's playlist match `[previous, current, next]` without interrupting playback. */
    private fun syncWindow(s: PlayerState, forceReload: Boolean, startMs: Long = 0L) {
        val cur = s.current ?: return
        exo.repeatMode = if (s.loopCurrent) Player.REPEAT_MODE_ONE else Player.REPEAT_MODE_OFF
        // While a crossfade applies the window must not hold the next track: ExoPlayer would roll
        // into it by itself, and the fade starts it on the second player instead.
        reconciler.reconcile(WindowSpec(s.previous, cur, nextInWindow(s)), forceReload, startMs)
    }

    private fun publish(s: PlayerState) {
        state = s
        currentTrackId = s.current?.id
        currentTrackBits = s.current?.bitsPerSample ?: 0
        PlaybackHub.setState(s)
        onStateChanged?.invoke()
        updateFadeWatch()
    }

    private fun tick() {
        val pos = exo.currentPosition.coerceAtLeast(0)
        val dur = if (exo.duration != C.TIME_UNSET) exo.duration else ((state?.current?.durationSecs ?: 0.0) * 1000).toLong()
        PlaybackHub.setPosition(pos, dur)
        // Play counting needs a poll at least every ~1 s; seeks are ignored by the tracker.
        reportPosition(PositionSample(currentTrackId, pos / 1000.0, dur / 1000.0, exo.isPlaying))
        if (++ticks % SAVE_EVERY_TICKS == 0) saveSession()
    }

    private class PositionSample(val trackId: String?, val posS: Double, val durS: Double, val playing: Boolean)

    private val pendingPosition = AtomicReference<PositionSample?>(null)

    /** The track Rust considers current; written on main, read by the executor to drop stale samples. */
    @Volatile
    private var currentTrackId: String? = null

    /**
     * Hands the latest position to the executor. Only one drain task is ever queued: while the
     * executor is busy, newer samples replace the waiting one instead of piling up behind it.
     */
    private fun reportPosition(sample: PositionSample) {
        if (pendingPosition.getAndSet(sample) != null) return // a queued task will pick this one up
        submitIo {
            val latest = pendingPosition.getAndSet(null) ?: return@submitIo
            // The queue moved on to another track while this sample waited: it no longer applies.
            if (latest.trackId != currentTrackId) return@submitIo
            engine.onPosition(latest.posS, latest.durS, latest.playing)
        }
    }

    private fun pushPosition() {
        val dur = ((state?.current?.durationSecs ?: 0.0) * 1000).toLong()
        PlaybackHub.setPosition(exo.currentPosition.coerceAtLeast(0), dur)
    }

    // ---- crossfade ----------------------------------------------------------------------------

    private var fadeNext: TrackInfo? = null
    private var fadeCurrentFactor = 1f
    private var fadeNextFactor = 1f

    /** Watches the second player: if the incoming track cannot be played the fade is given up. */
    private val spareListener = object : Player.Listener {
        override fun onPlaybackStateChanged(playbackState: Int) {
            if (playbackState == Player.STATE_READY && awaitingSpareReady && spare?.exo?.playbackState == Player.STATE_READY) {
                onSpareReady()
            }
        }

        override fun onPlayerError(error: PlaybackException) {
            // The outgoing track plays on; when it ends it moves on through the normal error path.
            // Not retried: a spare that cannot play would otherwise restart the fade over and over.
            if (fsm.isFading) fsm.abandon()
            failures++
            if (failures > MAX_FAILURES) PlaybackHub.setError(error.message ?: "Playback failed")
        }
    }

    /** The incoming player was started and has not buffered yet: the fade clock waits for it. */
    private var awaitingSpareReady = false

    private val spareStall = Runnable {
        // The incoming track never became ready: play the outgoing one out, no crossfade for it.
        if (awaitingSpareReady && !released) fsm.abandon()
    }

    private fun clearSpareWait() {
        awaitingSpareReady = false
        main.removeCallbacks(spareStall)
    }

    private fun onSpareReady() {
        clearSpareWait()
        fsm.rebase(SystemClock.elapsedRealtime()) // the curve starts now, and still ends with the track
        main.removeCallbacks(fadeTicker)
        main.post(fadeTicker)
    }

    private val fadeHost: CrossfadeHost = object : CrossfadeHost {
        override fun startSpare() {
            val next = state?.next ?: return
            fadeNext = next
            fadeCurrentFactor = state?.current?.let { gainFactor(it) } ?: 1f
            fadeNextFactor = gainFactor(next)
            val incoming = spare ?: factory.create(handlesAudioFocus = false).also {
                spare = it
                it.exo.addListener(spareListener)
            }
            incoming.replayGain.gain = maxOf(fadeNextFactor, 1f)
            incoming.exo.volume = 0f
            incoming.exo.setMediaItems(listOf(next.toMediaItem()), 0, 0L)
            incoming.exo.prepare()
            incoming.exo.playWhenReady = true
            main.removeCallbacks(fadeTicker)
            // The fade clock starts when the incoming player is ready, not now: a slow open must not
            // eat the fade. A stalled or failed open falls back to the plain track change.
            awaitingSpareReady = true
            main.removeCallbacks(spareStall)
            main.postDelayed(spareStall, SPARE_READY_TIMEOUT_MS)
            if (incoming.exo.playbackState == Player.STATE_READY) onSpareReady()
        }

        override fun applyFade(fadeOut: Double, fadeIn: Double) {
            exo.volume = (minOf(fadeCurrentFactor, 1f) * fadeOut).toFloat()
            spare?.exo?.volume = (minOf(fadeNextFactor, 1f) * fadeIn).toFloat()
        }

        override fun commit() = finishCrossfade()

        override fun abort() {
            main.removeCallbacks(fadeTicker)
            dropSpare()
            applyReplayGain(state?.current)
            updateFadeWatch() // a cancelled fade (seek, skip, edit) may have to start again later
        }
    }

    /** Stops the second player and forgets the track it was fading in. */
    private fun dropSpare() {
        clearSpareWait()
        spare?.exo?.run {
            playWhenReady = false
            stop()
            clearMediaItems()
            volume = 1f
        }
        spare?.replayGain?.gain = 1f
        fadeNext = null
    }

    private val fsm: CrossfadeFsm = CrossfadeFsm(fadeHost) { t ->
        val g = crossfadeGains(t)
        g.fadeOut to g.fadeIn
    }

    private val fadeTicker: Runnable = object : Runnable {
        override fun run() {
            if (released) return
            fsm.tick(SystemClock.elapsedRealtime())
            if (fsm.isFading) main.postDelayed(this, FADE_TICK_MS)
        }
    }

    /** Polls for the start of a fade, but only while one could start: playing, with a fade due. */
    private val fadeWatch: Runnable = object : Runnable {
        override fun run() {
            if (released) return
            val s = state
            val duration = exo.duration
            if (s != null && duration != C.TIME_UNSET) {
                fsm.onProgress(
                    positionMs = exo.currentPosition,
                    durationMs = duration,
                    applies = s.crossfadeApplies && s.next != null,
                    fadeSeconds = crossfadeSeconds,
                    playing = exo.isPlaying,
                    nowMs = SystemClock.elapsedRealtime(),
                )
            }
            if (exo.isPlaying && !fsm.isFading && state?.crossfadeApplies == true && crossfadeSeconds > 0) {
                // Poll fast only when a fade is close; a long way from the end once a second is plenty.
                val remaining = if (duration != C.TIME_UNSET) duration - exo.currentPosition else Long.MAX_VALUE
                val soon = remaining <= crossfadeSeconds * 1000L + 2000L
                main.postDelayed(this, if (soon) WATCH_MS else TICK_MS)
            }
        }
    }

    private fun updateFadeWatch() {
        main.removeCallbacks(fadeWatch)
        val armed = !released && exo.isPlaying && state?.crossfadeApplies == true && crossfadeSeconds > 0
        if (armed) main.postDelayed(fadeWatch, WATCH_MS)
    }

    /** The fade is over: the second player becomes the player, and Rust moves on to its track. */
    private fun finishCrossfade() {
        main.removeCallbacks(fadeTicker)
        clearSpareWait()
        val next = fadeNext
        val incoming = spare
        if (next == null || incoming == null) {
            dropSpare()
            return
        }
        val r = engine.onPlayerAdvanced(next.filePath)
        if (r.kind != AdvanceKind.FOLLOWED) {
            // The queue changed right at the boundary: drop the fade and load what is really meant.
            dropSpare()
            publish(r.state)
            syncWindow(r.state, forceReload = true)
            exo.playWhenReady = r.kind == AdvanceKind.RELOAD
            applyFx(r.state.current)
            fsm.reset()
            saveSession()
            return
        }
        incoming.exo.removeListener(spareListener)
        val outgoing = swapIn(incoming)
        spare = outgoing
        outgoing.exo.addListener(spareListener) // it is the spare now
        fadeNext = null
        failures = 0
        publish(r.state)
        syncWindow(r.state, forceReload = false)
        applyFx(r.state.current)
        fsm.reset()
        PlaybackHub.setPlaying(primary.exo.isPlaying)
        pushPosition()
        main.removeCallbacks(ticker)
        if (primary.exo.isPlaying) main.postDelayed(ticker, TICK_MS)
        updateFadeWatch()
        saveSession()
    }

    // ---- audio effects ------------------------------------------------------------------------

    private fun applyFxSettings() {
        val fx = fx ?: return // settings not read yet: applySettings() runs this once they are
        val track = state?.current
        val rate = track?.sampleRate ?: 0
        currentTrackBits = track?.bitsPerSample ?: 0

        // When turning Direct USB off while the USB sink is actively playing, release the USB player
        // BEFORE closing the USB session so no in-flight AudioSink write races with close().
        if (!fx.usbBitPerfect && primary.usbDirect) {
            lastUsbRate = null
            PlaybackHub.setUsb(UsbStatus.Off)
            ensureOutputMode(usbDirect = false, closeUsbFirst = true)
            applyNonUsbFx()
            return
        }

        // A session sync() must drop (stale device, unsupported rate) closes through the player:
        // the USB player is released before the session, as above. Synchronous, not posted.
        val status = usb.sync(enabled = fx.usbBitPerfect, trackSampleRate = rate) { reconnectKernel ->
            if (primary.usbDirect) {
                rebuildPrimary(usbDirect = false, closeUsbFirst = true, reconnectKernel = reconnectKernel)
            } else {
                usb.closeSession(reconnectKernel)
            }
        }
        lastUsbRate = rate
        PlaybackHub.setUsb(status)
        onStateChanged?.invoke()
        val wantUsbDirect = usb.hasOpenSession()
        ensureOutputMode(usbDirect = wantUsbDirect, closeUsbFirst = !wantUsbDirect)
        applyNonUsbFx()
    }

    private fun applyNonUsbFx() {
        val fx = fx ?: return
        val directActive = usb.hasOpenSession()
        equalizer.apply(eqPlan(fx.eqEnabled, directActive, fx.eqGains), fx.eqEnabled && !directActive)
        applyReplayGain(state?.current)
        val key = fx.crossfadeSeconds to directActive
        if (key != lastCrossfadeKey) {
            lastCrossfadeKey = key
            val s = engine.setCrossfade(fx.crossfadeSeconds, directActive)
            if (state?.current != null) reconcile(s)
            updateFadeWatch()
        }
    }

    private var lastCrossfadeKey: Pair<Int, Boolean>? = null

    /**
     * Switches between the direct userspace USB AudioSink ([UsbDirectAudioSink]) and Android's
     * standard `DefaultAudioSink`. Posted to the main looper so it is safe inside player callbacks.
     */
    private fun ensureOutputMode(usbDirect: Boolean, closeUsbFirst: Boolean = false) {
        if (primary.usbDirect == usbDirect) return
        main.post {
            if (released) return@post
            val wanted = (fx?.usbBitPerfect == true) && usb.hasOpenSession()
            if (primary.usbDirect != wanted) {
                rebuildPrimary(usbDirect = wanted, closeUsbFirst = closeUsbFirst && !wanted)
            }
        }
    }

    /** The sink of the current USB direct [primary] (null otherwise). Main thread only. */
    private var currentUsbSink: UsbDirectAudioSink? = null

    /** Builds a [UsbDirectAudioSink] wired to our Rust UAC1/UAC2 USBFS driver. */
    private fun createUsbDirectSink(): UsbDirectAudioSink {
        lateinit var sink: UsbDirectAudioSink
        sink = UsbDirectAudioSink(
            driverProvider = { usb.activeDriver() },
            sourceBitsProvider = { currentTrackBits },
            onStreamConfigured = { spec ->
                val updated = usb.onStreamConfigured(spec)
                if (updated != null) {
                    main.post {
                        if (!released && usb.hasOpenSession()) {
                            PlaybackHub.setUsb(updated)
                        }
                    }
                }
            },
            onStreamError = { err ->
                handlingUsbFallback = true
                main.post {
                    if (released) return@post
                    handlingUsbFallback = false
                    // A released player's sink failing late (its session already closed or
                    // replaced) must not tear down the current output.
                    if (sink !== currentUsbSink) return@post
                    if (err is UsbDriverException.Disconnected || !usb.hasOpenSession()) {
                        onUsbDetached()
                    } else if (err is UsbDriverException.UnsupportedRate) {
                        val trackRate = state?.current?.sampleRate ?: 0
                        val supported = usb.supportedRates()
                        val deviceName = (PlaybackHub.usb.value as? UsbStatus.Active)?.device
                            ?: (PlaybackHub.usb.value as? UsbStatus.Ready)?.device
                            ?: "USB DAC"
                        if (primary.usbDirect) {
                            rebuildPrimary(usbDirect = false, closeUsbFirst = true)
                        } else {
                            usb.closeSession(reconnectKernel = true)
                        }
                        PlaybackHub.setUsb(UsbStatus.UnsupportedRate(deviceName, trackRate, supported))
                        applyNonUsbFx()
                    } else {
                        if (primary.usbDirect) {
                            rebuildPrimary(usbDirect = false, closeUsbFirst = true)
                        } else {
                            usb.closeSession(reconnectKernel = true)
                        }
                        PlaybackHub.setUsb(UsbStatus.Failed(err.message ?: "USB audio stream error"))
                        applyNonUsbFx()
                    }
                }
            },
        )
        return sink
    }

    /**
     * Moves playback (window, position, play state) to a fresh player backed by either
     * [UsbDirectAudioSink] or `DefaultAudioSink`.
     */
    private fun rebuildPrimary(
        usbDirect: Boolean,
        closeUsbFirst: Boolean = false,
        forcePause: Boolean = false,
        reconnectKernel: Boolean = true,
    ) {
        fsm.cancel()
        val old = primary
        val items = List(old.exo.mediaItemCount) { old.exo.getMediaItemAt(it) }
        val index = old.exo.currentMediaItemIndex
        val positionMs = old.exo.currentPosition.coerceAtLeast(0)
        val playWhenReady = if (forcePause) false else old.exo.playWhenReady

        if (closeUsbFirst && old.usbDirect) {
            // Release the old USB direct player FIRST so its AudioSink stops writing before we
            // close the USB session and reattach the kernel snd-usb-audio driver.
            old.exo.removeListener(this)
            old.exo.playWhenReady = false
            old.exo.stop()
            old.exo.release()
            currentUsbSink = null
            usb.closeSession(reconnectKernel = reconnectKernel)
            onStateChanged?.invoke()

            val fresh = factory.create(handlesAudioFocus = true, usbDirectSink = null)
            fresh.exo.repeatMode = if (state?.loopCurrent == true) Player.REPEAT_MODE_ONE else Player.REPEAT_MODE_OFF
            if (items.isNotEmpty()) {
                fresh.exo.setMediaItems(items, index, positionMs)
                fresh.exo.prepare()
            }
            fresh.exo.playWhenReady = playWhenReady
            primary = fresh
            primary.exo.addListener(this)
            onPlayerSwapped?.invoke(primary.exo)
            applyReplayGain(state?.current)
            PlaybackHub.setPlaying(fresh.exo.isPlaying)
            updateFadeWatch()
            return
        }

        val sink = if (usbDirect) createUsbDirectSink() else null
        val fresh = factory.create(handlesAudioFocus = false, usbDirectSink = sink)
        currentUsbSink = sink
        fresh.exo.repeatMode = old.exo.repeatMode
        fresh.exo.volume = if (usbDirect) 1f else old.exo.volume
        fresh.replayGain.gain = if (usbDirect) 1f else old.replayGain.gain
        if (items.isNotEmpty()) {
            fresh.exo.setMediaItems(items, index, positionMs)
            fresh.exo.prepare()
        }
        // Before the swap: a player that is not set to play requests no audio focus, and the old
        // one gives its focus up inside swapIn.
        fresh.exo.playWhenReady = playWhenReady
        swapIn(fresh)
        old.exo.release()
        PlaybackHub.setPlaying(fresh.exo.isPlaying)
        updateFadeWatch()
    }

    /**
     * Makes [incoming] the player that is heard: listeners, audio focus and the session follow it.
     * The old primary is stopped and returned (the crossfade keeps it as the spare).
     */
    private fun swapIn(incoming: PlayerHandle): PlayerHandle {
        val outgoing = primary
        outgoing.exo.removeListener(this)
        primary = incoming
        primary.exo.addListener(this)
        // Audio focus moves with the player that is heard: take it before letting the old one go,
        // so the abandon of the old focus request cannot pause the new primary.
        primary.exo.setAudioAttributes(PlayerFactory.MUSIC, true)
        outgoing.exo.setAudioAttributes(PlayerFactory.MUSIC, false)
        outgoing.exo.playWhenReady = false
        outgoing.exo.stop()
        outgoing.exo.clearMediaItems()
        outgoing.exo.volume = 1f
        outgoing.replayGain.gain = 1f
        onPlayerSwapped?.invoke(primary.exo)
        return outgoing
    }

    private fun applyFx(track: TrackInfo?) {
        currentTrackBits = track?.bitsPerSample ?: 0
        if (fx?.usbBitPerfect == true && track != null && track.sampleRate != lastUsbRate) {
            // Verify whether the connected DAC supports this track's sample rate natively (or
            // reclaim the DAC if the previous track fell back due to an unsupported rate).
            applyFxSettings()
        } else {
            applyReplayGain(track)
        }
    }

    /**
     * In standard output mode, attenuation goes through the player volume (exact at the track
     * boundary) and boost goes through [ReplayGainAudioProcessor].
     *
     * In Direct USB bit-perfect mode, PCM stays 100% untouched (`exo.volume = 1f`,
     * `replayGain.gain = 1f`) and the ReplayGain dB offset is applied through the USB DAC's own
     * UAC Feature Unit hardware volume control when supported (bypassed only on fixed-level DACs).
     */
    private fun applyReplayGain(track: TrackInfo?) {
        if (usb.hasOpenSession()) {
            exo.volume = 1f
            replayGain.gain = 1f
            if (usb.hasHardwareVolume()) {
                // Latest wins: a fast run of track changes sends only the gain of the last one.
                pendingUsbReplayGainDb.set(if (track != null) gainDb(track) else 0f)
                onUsbControl { pendingUsbReplayGainDb.getAndSet(null)?.let { usb.setReplayGainDb(it) } ?: false }
            }
            return
        }
        val factor = if (track == null) 1f else gainFactor(track)
        exo.volume = minOf(factor, 1f)
        replayGain.gain = maxOf(factor, 1f)
    }

    /** Linear ReplayGain factor for [track]; unity until the settings are known. */
    private fun gainFactor(track: TrackInfo): Float = fx?.let { replayGainFactor(it.replayGain, track) } ?: 1f

    /** Effective ReplayGain offset in dB for [track] (0 dB when off or untagged). */
    private fun gainDb(track: TrackInfo): Float = fx?.let { replayGainDb(it.replayGain, track) } ?: 0f

    private companion object {
        const val TAG = "RustyPlayback"
        const val SPARE_READY_TIMEOUT_MS = 3000L
        const val TICK_MS = 1000L
        const val WATCH_MS = 250L
        const val FADE_TICK_MS = 40L
        const val SAVE_EVERY_TICKS = 15
        const val RESTART_THRESHOLD_MS = 3000L
        const val MAX_FAILURES = 3
    }
}
