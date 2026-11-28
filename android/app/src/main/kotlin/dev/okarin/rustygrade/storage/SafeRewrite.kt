package dev.okarin.rustygrade.storage

import java.io.File
import java.io.InputStream
import java.io.OutputStream
import kotlinx.coroutines.CancellationException

/** Where a file's bytes come from and go back to (a MediaStore row, a SAF document, a plain file). */
interface RewriteTarget {
    fun openRead(): InputStream

    /** Must truncate: writing a shorter file over a longer one may not leave the old tail behind. */
    fun openWriteTruncating(): OutputStream
}

/**
 * Edits a file that Rust cannot open for writing: copy it to a temp file, let [edit] change the
 * copy, then write the result back. The original bytes are kept until the write-back succeeded
 * and are written back if it fails halfway, so a failed edit never leaves a damaged song.
 */
object SafeRewrite {
    class RewriteException(message: String, cause: Throwable? = null) : Exception(message, cause)

    fun rewrite(target: RewriteTarget, workDir: File, extension: String, edit: (File) -> Unit) {
        workDir.mkdirs()
        // Both temp files are created inside the try: if the second allocation fails, the first is still removed.
        var work: File? = null
        var backup: File? = null
        try {
            val workFile = File.createTempFile("tag-edit-", ".$extension", workDir).also { work = it }
            val backupFile = File.createTempFile("tag-orig-", ".$extension", workDir).also { backup = it }
            target.openRead().use { input -> backupFile.outputStream().use { input.copyTo(it) } }
            backupFile.copyTo(workFile, overwrite = true)

            try {
                edit(workFile)
            } catch (e: CancellationException) {
                throw e // nothing was written to the original yet
            } catch (e: Exception) {
                // Nothing was written to the original yet.
                throw RewriteException("The tags could not be edited: ${e.message}", e)
            }

            try {
                workFile.inputStream().use { src -> target.openWriteTruncating().use { src.copyTo(it) } }
            } catch (e: CancellationException) {
                // Cancelled halfway through the write-back: put the original back, then cancel.
                if (!tryRestore(target, backupFile)) {
                    throw RewriteException("Writing the file was cancelled and the original could NOT be restored", e)
                }
                throw e
            } catch (e: Exception) {
                restore(target, backupFile, e)
            }
        } finally {
            work?.delete()
            backup?.delete()
        }
    }

    private fun tryRestore(target: RewriteTarget, backup: File): Boolean = try {
        backup.inputStream().use { src -> target.openWriteTruncating().use { src.copyTo(it) } }
        true
    } catch (_: Exception) {
        false
    }

    private fun restore(target: RewriteTarget, backup: File, cause: Exception): Nothing {
        val restored = tryRestore(target, backup)
        throw RewriteException(
            if (restored) "Writing the file failed; the original was restored: ${cause.message}"
            else "Writing the file failed and the original could NOT be restored: ${cause.message}",
            cause,
        )
    }
}
