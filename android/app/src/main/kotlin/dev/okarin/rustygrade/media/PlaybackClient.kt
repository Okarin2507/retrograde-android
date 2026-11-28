package dev.okarin.rustygrade.media

import android.content.ComponentName
import android.content.Context
import android.os.Bundle
import androidx.core.content.ContextCompat
import androidx.media3.session.MediaController
import androidx.media3.session.SessionCommand
import androidx.media3.session.SessionToken
import com.google.common.util.concurrent.ListenableFuture

/** The UI's handle on the media service: transport via `MediaController`, queue ops via custom commands. */
class PlaybackClient(private val context: Context) {
    private val lock = Any()
    private var future: ListenableFuture<MediaController>? = null

    /** Binds (and thereby starts) the media service. */
    fun connect() {
        synchronized(lock) { connectLocked() }
    }

    private fun connectLocked(): ListenableFuture<MediaController> =
        future ?: MediaController.Builder(
            context,
            SessionToken(context, ComponentName(context, RustygradeMediaService::class.java)),
        ).buildAsync().also { future = it }

    fun release() {
        synchronized(lock) {
            future?.let { MediaController.releaseFuture(it) }
            future = null
        }
    }

    fun playAlbum(key: String, startIndex: Int? = null) =
        command(PlaybackCommands.PLAY_ALBUM, Bundle().apply {
            putString(PlaybackCommands.ARG_KEY, key)
            if (startIndex != null) putInt(PlaybackCommands.ARG_INDEX, startIndex)
        })

    fun playAlbumShuffled(key: String) =
        command(PlaybackCommands.PLAY_ALBUM_SHUFFLED, Bundle().apply {
            putString(PlaybackCommands.ARG_KEY, key)
        })

    fun playTracks(ids: List<String>, startIndex: Int? = null) =
        command(PlaybackCommands.PLAY_TRACKS, idsBundle(ids, startIndex))

    fun playTracksShuffled(ids: List<String>) =
        command(PlaybackCommands.PLAY_TRACKS_SHUFFLED, idsBundle(ids, null))

    fun playNext(ids: List<String>) = command(PlaybackCommands.PLAY_NEXT, idsBundle(ids, 0))
    fun enqueue(ids: List<String>) = command(PlaybackCommands.ENQUEUE, idsBundle(ids, 0))
    fun removeAt(index: Int) = command(PlaybackCommands.REMOVE_AT, Bundle().apply { putInt(PlaybackCommands.ARG_INDEX, index) })
    fun jumpTo(index: Int) = command(PlaybackCommands.JUMP_TO, Bundle().apply { putInt(PlaybackCommands.ARG_INDEX, index) })
    /** Drag and drop in the queue: the track at [from] goes to [to]. */
    fun moveItem(from: Int, to: Int) = command(PlaybackCommands.MOVE_ITEM, Bundle().apply {
        putInt(PlaybackCommands.ARG_INDEX, from)
        putInt(PlaybackCommands.ARG_TO, to)
    })
    fun pause() = withController { it.pause() }
    fun reloadCurrent() = command(PlaybackCommands.RELOAD_CURRENT, Bundle.EMPTY)
    fun clearQueue() = command(PlaybackCommands.CLEAR_QUEUE, Bundle.EMPTY)
    fun cycleRepeat() = command(PlaybackCommands.CYCLE_REPEAT, Bundle.EMPTY)
    fun refreshFx() = command(PlaybackCommands.REFRESH_FX, Bundle.EMPTY)
    fun syncLibrary() = command(PlaybackCommands.SYNC_LIBRARY, Bundle.EMPTY)
    fun requestUsbPermission() = command(PlaybackCommands.USB_REQUEST_PERMISSION, Bundle.EMPTY)
    fun setUsbVolume(normalized: Float) =
        command(PlaybackCommands.SET_USB_VOLUME, Bundle().apply { putFloat(PlaybackCommands.ARG_FLOAT, normalized) })

    fun togglePlay() = withController { if (it.isPlaying) it.pause() else it.play() }
    fun next() = withController { it.seekToNext() }
    fun previous() = withController { it.seekToPrevious() }
    fun seekTo(positionMs: Long) = withController { it.seekTo(positionMs) }
    fun toggleShuffle() = withController { it.shuffleModeEnabled = !it.shuffleModeEnabled }

    private fun idsBundle(ids: List<String>, index: Int?) = Bundle().apply {
        putStringArrayList(PlaybackCommands.ARG_IDS, ArrayList(ids))
        if (index != null) putInt(PlaybackCommands.ARG_INDEX, index)
    }

    private fun command(action: String, args: Bundle) = withController {
        it.sendCustomCommand(SessionCommand(action, Bundle.EMPTY), args)
    }

    private fun withController(block: (MediaController) -> Unit) {
        val f = synchronized(lock) { future } ?: return
        f.addListener({
            if (f.isCancelled) return@addListener
            val controller = runCatching { f.get() }.getOrNull()
            if (controller != null && controller.isConnected) {
                block(controller)
                return@addListener
            }
            // The service was stopped (or killed) while the UI lived on: connect again. Only the
            // first command to notice does it; the others attach to the new connection.
            val retry = synchronized(lock) {
                if (future === f) {
                    MediaController.releaseFuture(f)
                    future = null
                }
                connectLocked()
            }
            retry.addListener({
                runCatching { retry.get() }.getOrNull()?.takeIf { it.isConnected }?.let(block)
            }, ContextCompat.getMainExecutor(context))
        }, ContextCompat.getMainExecutor(context))
    }
}
