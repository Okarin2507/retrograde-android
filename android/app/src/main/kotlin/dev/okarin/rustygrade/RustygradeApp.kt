package dev.okarin.rustygrade

import android.app.Application
import uniffi.rustygrade_ffi.PlayerEngine
import uniffi.rustygrade_ffi.RustygradeEngine

class RustygradeApp : Application() {
    /**
     * Single Rust engine for the whole process (library, scanning, collections, settings). Opening
     * the library is disk work: it is built on first use (thread-safe) and warmed up in the
     * background from [onCreate], so process start does not wait for it.
     */
    val engine: RustygradeEngine by lazy {
        // The constructor points the Rust core at the app-private data dir (`Context.filesDir`).
        RustygradeEngine(filesDir.absolutePath)
    }

    /** The play queue, play counting and session persistence (shares the engine's library). */
    val player: PlayerEngine by lazy { PlayerEngine(engine) }

    override fun onCreate() {
        super.onCreate()
        dev.okarin.rustygrade.media.ArtworkUris.init(this)
        warmUpEngine()
        sweepStaleTagCache()
    }

    /** Opens the Rust library off the main thread. A failure here resurfaces on the first real use. */
    private fun warmUpEngine() {
        Thread(
            {
                try {
                    engine
                    player
                } catch (_: Throwable) {
                    // `lazy` does not cache a failure: the first caller on the main thread retries it.
                }
            },
            "engine-warmup",
        ).apply {
            isDaemon = true
            start()
        }
    }

    /** Cleans up orphaned temp covers/edits older than 24 h left by a prior process kill. */
    private fun sweepStaleTagCache() {
        val dir = cacheDir
        Thread(
            {
                val cutoffMs = System.currentTimeMillis() - 86_400_000L
                dir.listFiles { f -> f.isFile && f.name.startsWith("tag-cover-") }
                    ?.forEach { if (it.lastModified() < cutoffMs) it.delete() }
                java.io.File(dir, "tag-edit").listFiles()
                    ?.forEach { if (it.lastModified() < cutoffMs) it.deleteRecursively() }
            },
            "tag-cache-sweep",
        ).apply {
            isDaemon = true
            priority = Thread.MIN_PRIORITY
            start()
        }
    }
}
