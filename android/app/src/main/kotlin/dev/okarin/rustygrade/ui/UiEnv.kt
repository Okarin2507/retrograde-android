package dev.okarin.rustygrade.ui

import androidx.compose.runtime.Stable
import androidx.compose.runtime.staticCompositionLocalOf
import dev.okarin.rustygrade.CollectionsViewModel
import dev.okarin.rustygrade.LibraryViewModel
import dev.okarin.rustygrade.media.PlaybackClient
import dev.okarin.rustygrade.ui.nav.NavState
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import uniffi.rustygrade_ffi.RustygradeEngine

/**
 * Everything a screen needs, passed as one composition local so screens keep a plain signature.
 * Engine calls block: run them with [io], never directly in composition.
 */
@Stable
class UiEnv(
    val engine: RustygradeEngine,
    /** The play queue (paged queue listing, etc.). Blocking calls: use [io]. */
    val player: uniffi.rustygrade_ffi.PlayerEngine,
    val library: LibraryViewModel,
    val collections: CollectionsViewModel,
    val nav: NavState,
    /** Asks for the audio permission if missing, then scans. */
    val scan: () -> Unit,
    /** Opens the system folder picker (SAF); the result is saved and scanned. */
    val addFolder: () -> Unit,
) {
    val playback: PlaybackClient get() = library.playback
}

val LocalEnv = staticCompositionLocalOf<UiEnv> { error("UiEnv is not provided") }

/** Runs a blocking engine call off the main thread. */
suspend fun <T> io(block: () -> T): T = withContext(Dispatchers.IO) { block() }
