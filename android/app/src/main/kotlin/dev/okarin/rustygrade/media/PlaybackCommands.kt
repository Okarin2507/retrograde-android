package dev.okarin.rustygrade.media

import android.os.Bundle
import androidx.media3.session.SessionCommand

/** Custom session commands for queue operations the standard `Player` API cannot express. */
object PlaybackCommands {
    const val PLAY_ALBUM = "dev.okarin.rustygrade.PLAY_ALBUM"
    const val PLAY_ALBUM_SHUFFLED = "dev.okarin.rustygrade.PLAY_ALBUM_SHUFFLED"
    const val PLAY_TRACKS = "dev.okarin.rustygrade.PLAY_TRACKS"
    const val PLAY_TRACKS_SHUFFLED = "dev.okarin.rustygrade.PLAY_TRACKS_SHUFFLED"
    const val PLAY_NEXT = "dev.okarin.rustygrade.PLAY_NEXT"
    const val ENQUEUE = "dev.okarin.rustygrade.ENQUEUE"
    const val REMOVE_AT = "dev.okarin.rustygrade.REMOVE_AT"
    const val JUMP_TO = "dev.okarin.rustygrade.JUMP_TO"
    const val CLEAR_QUEUE = "dev.okarin.rustygrade.CLEAR_QUEUE"
    const val MOVE_ITEM = "dev.okarin.rustygrade.MOVE_ITEM"
    /** The file of the playing track was rewritten: load it again at the same position. */
    const val RELOAD_CURRENT = "dev.okarin.rustygrade.RELOAD_CURRENT"
    const val CYCLE_REPEAT = "dev.okarin.rustygrade.CYCLE_REPEAT"
    /** Settings (EQ, ReplayGain, USB) changed: re-read them. */
    const val REFRESH_FX = "dev.okarin.rustygrade.REFRESH_FX"
    /** A scan or delete changed the library: drop vanished tracks from the queue. */
    const val SYNC_LIBRARY = "dev.okarin.rustygrade.SYNC_LIBRARY"
    /** Explicitly ask Android for USB Host permission for the connected DAC. */
    const val USB_REQUEST_PERMISSION = "dev.okarin.rustygrade.USB_REQUEST_PERMISSION"
    /** Set the connected USB DAC's hardware Feature Unit volume (`ARG_FLOAT`, `0f..1f`). */
    const val SET_USB_VOLUME = "dev.okarin.rustygrade.SET_USB_VOLUME"

    const val ARG_KEY = "key"
    const val ARG_IDS = "ids"
    const val ARG_INDEX = "index"
    const val ARG_TO = "to"
    const val ARG_FLOAT = "float"

    val all: List<SessionCommand> = listOf(
        PLAY_ALBUM, PLAY_ALBUM_SHUFFLED, PLAY_TRACKS, PLAY_TRACKS_SHUFFLED,
        PLAY_NEXT, ENQUEUE, REMOVE_AT, JUMP_TO, CLEAR_QUEUE, MOVE_ITEM,
        RELOAD_CURRENT, CYCLE_REPEAT, REFRESH_FX, SYNC_LIBRARY,
        USB_REQUEST_PERMISSION, SET_USB_VOLUME,
    ).map { SessionCommand(it, Bundle.EMPTY) }
}
