package dev.okarin.rustygrade.ui.components

/** Id of the playing track, provided once by the app shell so list rows do not each collect it. */
val LocalNowPlayingId = androidx.compose.runtime.compositionLocalOf<String?> { null }

/** Favorite track ids, provided once by the app shell. */
val LocalLikedIds = androidx.compose.runtime.compositionLocalOf<Set<String>> { emptySet() }

/** Whether audio is playing right now (not just loaded), for pulsing "now playing" marks. */
val LocalIsPlaying = androidx.compose.runtime.compositionLocalOf { false }
