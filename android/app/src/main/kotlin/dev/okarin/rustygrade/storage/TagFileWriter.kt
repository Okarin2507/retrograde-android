package dev.okarin.rustygrade.storage

import android.content.ContentResolver
import android.content.Context
import android.content.IntentSender
import android.content.pm.PackageManager
import android.media.MediaScannerConnection
import android.net.Uri
import android.os.Build
import android.provider.MediaStore
import dev.okarin.rustygrade.media.PlaybackClient
import dev.okarin.rustygrade.media.PlaybackHub
import java.io.File
import java.io.InputStream
import java.io.OutputStream
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.delay
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeoutOrNull
import uniffi.rustygrade_ffi.EngineException
import uniffi.rustygrade_ffi.RustygradeEngine
import uniffi.rustygrade_ffi.SafDocument
import uniffi.rustygrade_ffi.ScanRequest
import uniffi.rustygrade_ffi.TagEditInfo
import uniffi.rustygrade_ffi.TrackInfo
import uniffi.rustygrade_ffi.editTagsInFile

/**
 * Writes tags into music files under Android's storage rules.
 *
 * Rust cannot open these files for writing, so each file is copied to a temp file, edited there
 * (`edit_tags_in_file`), and written back through the system ([SafeRewrite]: the original is
 * restored if the write fails). API 30+ needs the user to approve the write once per batch
 * ([writeAccessRequest]); SAF documents need the folder to have been picked with write access.
 * Afterwards the touched files are re-read (a forced scan of just those files).
 */
class TagFileWriter(
    private val context: Context,
    private val engine: RustygradeEngine,
    private val playback: PlaybackClient,
) {
    class Report(val written: Int, val failures: List<Pair<TrackInfo, String>>)

    private val resolver: ContentResolver get() = context.contentResolver

    /**
     * An intent to launch (`StartIntentSenderForResult`) so the user approves editing these files,
     * or null when no approval is needed: older Android, SAF documents, or already granted.
     */
    fun writeAccessRequest(tracks: List<TrackInfo>): IntentSender? {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.R) return null
        val missing = tracks.filterNot { isSaf(it) }
            .mapNotNull { mediaStoreUri(it.filePath) }
            .filter { uri ->
                context.checkUriPermission(
                    uri, android.os.Process.myPid(), android.os.Process.myUid(),
                    android.content.Intent.FLAG_GRANT_WRITE_URI_PERMISSION,
                ) != PackageManager.PERMISSION_GRANTED
            }
        if (missing.isEmpty()) return null
        return MediaStore.createWriteRequest(resolver, missing).intentSender
    }

    /**
     * [onProgress] gets the number of tracks handled so far, on the main thread. The library is
     * refreshed once, at the end (also when the batch is cancelled: what was written is not lost).
     */
    suspend fun write(tracks: List<TrackInfo>, edit: TagEditInfo, onProgress: (Int) -> Unit = {}): Report = withContext(Dispatchers.IO) {
        // The files under the playing track (and the neighbours ExoPlayer has already opened) must
        // not change while they are read: pause the current one and wait until it has really stopped.
        val hub = PlaybackHub.state.value
        val ids = tracks.mapTo(HashSet()) { it.id }
        val touchesPlaying = hub?.current?.id?.let { it in ids } == true
        val touchesWindow = touchesPlaying || listOfNotNull(hub?.previous, hub?.next).any { it.id in ids }
        if (touchesPlaying) {
            playback.pause()
            if (PlaybackHub.isPlaying.value) {
                // `pause()` is asynchronous (a controller command): wait for the player to confirm.
                withTimeoutOrNull(PAUSE_WAIT_MS) { PlaybackHub.isPlaying.first { !it } }
            }
        }

        val failures = ArrayList<Pair<TrackInfo, String>>()
        val done = ArrayList<TrackInfo>()
        try {
            for ((index, track) in tracks.withIndex()) {
                ensureActive()
                try {
                    val target = targetFor(track)
                    // `extension` of the file NAME: a dot in a parent folder must not count. Only
                    // letters and digits survive: it becomes part of a temp file name.
                    val extension = File(track.filePath).extension.replace(NON_ALNUM, "").ifEmpty { "flac" }
                    SafeRewrite.rewrite(target, File(context.cacheDir, "tag-edit"), extension) { temp ->
                        editTagsInFile(temp.absolutePath, edit)
                    }
                    done += track
                } catch (e: CancellationException) {
                    throw e
                } catch (e: SafeRewrite.RewriteException) {
                    failures += track to (e.message ?: "Could not write the file")
                } catch (e: EngineException) {
                    failures += track to (e.message ?: "Could not edit the tags")
                } catch (e: SecurityException) {
                    failures += track to "No permission to edit this file" + if (isSaf(track)) ": add the folder again to allow editing" else ""
                } catch (e: Exception) {
                    failures += track to (e.message ?: e.javaClass.simpleName)
                }
                withContext(Dispatchers.Main) { onProgress(index + 1) }
            }
        } catch (e: CancellationException) {
            // Files already rewritten are real: let the library and the player learn about them.
            if (done.isNotEmpty()) {
                withContext(NonCancellable) { runCatching { finish(done, edit, touchesWindow) } }
            }
            throw e
        }
        if (done.isNotEmpty()) finish(done, edit, touchesWindow)
        Report(done.size, failures)
    }

    /** Refreshes the library, then always re-syncs the queue; reloads the player if its files changed. */
    private suspend fun finish(done: List<TrackInfo>, edit: TagEditInfo, touchesWindow: Boolean) {
        try {
            refreshLibrary(done, edit)
        } finally {
            playback.syncLibrary()
            // The player still holds the old bytes of a file that was rewritten under it.
            if (touchesWindow) playback.reloadCurrent()
        }
    }

    /** Tells the media database and our library about the changed files. */
    private suspend fun refreshLibrary(done: List<TrackInfo>, edit: TagEditInfo) {
        val coverChanged = edit.coverImagePath != null || edit.removeCover
        val realPaths = done.filterNot { isSaf(it) }.map { it.filePath }
        if (realPaths.isNotEmpty()) MediaScannerConnection.scanFile(context, realPaths.toTypedArray(), null, null)
        // Both the albums the tracks leave and the album they may join (it can already have a cover).
        if (coverChanged) engine.clearAlbumCovers((done.map { it.album } + listOfNotNull(edit.album)).distinct())

        val docs = done.filter { isSaf(it) }.map {
            val (mtime, size) = SafSource.queryMetadata(resolver, Uri.parse(it.playUri))
                ?: (System.currentTimeMillis() to 0L)
            SafDocument(it.filePath.removePrefix("/saf/"), it.playUri, mtime, size)
        }
        val request = ScanRequest(
            dirs = emptyList(), files = realPaths, roots = emptyList(),
            safFiles = docs, safRoots = emptyList(), pruneMissing = false, force = true, resyncCovers = false,
        )
        // A scan may be running: wait for it instead of failing.
        repeat(30) {
            try {
                engine.scanSources(request, null, dev.okarin.rustygrade.storage.ContentResolverOpener(resolver))
                return
            } catch (e: EngineException.ScanInProgress) {
                delay(300) // a scan is running: wait for it; any other error is real and is raised
            }
        }
        error("Library scan still running; rescan to refresh edited tags")
    }

    private fun isSaf(track: TrackInfo) = track.filePath.startsWith("/saf/")

    private fun targetFor(track: TrackInfo): RewriteTarget = when {
        isSaf(track) -> ResolverTarget(resolver, Uri.parse(track.playUri))
        Build.VERSION.SDK_INT >= Build.VERSION_CODES.R ->
            ResolverTarget(resolver, mediaStoreUri(track.filePath) ?: throw SecurityException("Not in the media database"))
        // API 28-29: plain file access (WRITE_EXTERNAL_STORAGE + requestLegacyExternalStorage).
        else -> FileTarget(File(track.filePath))
    }

    private fun mediaStoreUri(path: String): Uri? =
        resolver.query(
            MediaStore.Audio.Media.EXTERNAL_CONTENT_URI,
            arrayOf(MediaStore.Audio.Media._ID),
            "${MediaStore.Audio.Media.DATA} = ?",
            arrayOf(path),
            null,
        )?.use { c ->
            if (c.moveToFirst()) android.content.ContentUris.withAppendedId(MediaStore.Audio.Media.EXTERNAL_CONTENT_URI, c.getLong(0)) else null
        }

    private class ResolverTarget(private val resolver: ContentResolver, private val uri: Uri) : RewriteTarget {
        override fun openRead(): InputStream = resolver.openInputStream(uri) ?: error("Cannot read $uri")

        // "wt" truncates; plain "w" does not on every provider and would leave the old tail behind.
        override fun openWriteTruncating(): OutputStream = resolver.openOutputStream(uri, "wt") ?: error("Cannot write $uri")
    }

    private companion object {
        const val PAUSE_WAIT_MS = 1500L
        val NON_ALNUM = Regex("[^A-Za-z0-9]")
    }

    private class FileTarget(private val file: File) : RewriteTarget {
        override fun openRead(): InputStream = file.inputStream()
        override fun openWriteTruncating(): OutputStream = file.outputStream() // FileOutputStream truncates
    }
}
