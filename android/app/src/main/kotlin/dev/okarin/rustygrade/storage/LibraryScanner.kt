package dev.okarin.rustygrade.storage

import android.content.Context
import android.net.Uri
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.withContext
import kotlin.coroutines.coroutineContext
import uniffi.rustygrade_ffi.RustygradeEngine
import uniffi.rustygrade_ffi.SafDocument
import uniffi.rustygrade_ffi.ScanProgressEvent
import uniffi.rustygrade_ffi.ScanProgressListener
import uniffi.rustygrade_ffi.ScanRequest
import uniffi.rustygrade_ffi.ScanSummary

/** Gathers the user's sources (MediaStore + granted SAF trees) and runs one Rust scan over them. */
class LibraryScanner(private val context: Context, private val engine: RustygradeEngine) {

    /** Blocking Rust scan, moved off the main thread. Cancel with [RustygradeEngine.cancelScan]. */
    suspend fun scan(
        pruneMissing: Boolean = false,
        resyncCovers: Boolean = false,
        report: (ScanProgressEvent) -> Unit,
    ): ScanSummary = withContext(Dispatchers.IO) {
        val settings = engine.loadSettings()

        var files: List<String> = emptyList()
        var roots: List<String> = emptyList()
        if (settings.useMediaStore) {
            MediaStoreSource.query(context)?.let {
                files = it.files
                roots = it.roots
            }
        }

        val safFiles = ArrayList<SafDocument>()
        val safRoots = ArrayList<String>()
        // The walk is plain blocking code: it polls this job to notice a cancel between directories.
        val job = coroutineContext[Job]
        for (tree in settings.safTrees) {
            coroutineContext.ensureActive()
            SafSource.walk(context, Uri.parse(tree)) { job?.isActive != false }?.let {
                safFiles += it.documents
                safRoots += it.rootDocumentId
            }
        }

        // A cancel during the (Kotlin-side) folder walk must not start the Rust scan.
        coroutineContext.ensureActive()

        engine.scanSources(
            ScanRequest(
                dirs = settings.libraryFolders,
                files = files,
                roots = roots,
                safFiles = safFiles,
                safRoots = safRoots,
                pruneMissing = if (resyncCovers) false else pruneMissing,
                force = resyncCovers,
                resyncCovers = resyncCovers,
            ),
            object : ScanProgressListener {
                override fun onProgress(event: ScanProgressEvent) = report(event)
            },
            ContentResolverOpener(context.contentResolver),
        )
    }
}
