package dev.okarin.rustygrade.media

import android.content.ContentProvider
import android.content.ContentValues
import android.content.Context
import android.database.Cursor
import android.net.Uri
import android.os.ParcelFileDescriptor
import java.io.File
import java.io.FileNotFoundException
import java.io.IOException

/**
 * Read-only access to the cover cache for other apps (Android Auto, the system media UI), which
 * cannot open `file://` paths inside the private app directory.
 *
 * It is exported, so it serves exactly one thing: files that sit directly in `filesDir/covers`,
 * opened read-only. `androidx.core.content.FileProvider` cannot be used here: it refuses to be
 * exported.
 */
class CoverProvider : ContentProvider() {
    override fun onCreate(): Boolean = true

    override fun getType(uri: Uri): String? = resolve(uri)?.let { file ->
        sniffImageMimeType(file) ?: when (file.extension.lowercase()) {
            "png" -> "image/png"
            "webp" -> "image/webp"
            else -> "image/jpeg"
        }
    }

    override fun openFile(uri: Uri, mode: String): ParcelFileDescriptor {
        if (mode != "r") throw FileNotFoundException("covers are read-only")
        val file = resolve(uri) ?: throw FileNotFoundException(uri.toString())
        return ParcelFileDescriptor.open(file, ParcelFileDescriptor.MODE_READ_ONLY)
    }

    /** `content://<authority>/<file name>` -> the file, or null if it is not a plain cover file. */
    private fun resolve(uri: Uri): File? {
        val ctx = context ?: return null
        val name = uri.pathSegments.singleOrNull() ?: return null
        if (name.isEmpty() || name.contains('/') || name.contains('\\') || name.contains('\u0000') || name.startsWith(".")) {
            return null
        }
        return try {
            val dir = File(ctx.filesDir, COVERS_DIR).canonicalFile
            val file = File(dir, name).canonicalFile
            // Canonical paths also defeat symlinks that point out of the cache.
            file.takeIf { it.parentFile == dir && it.isFile }
        } catch (_: java.io.IOException) {
            null
        }
    }

    override fun query(uri: Uri, projection: Array<out String>?, selection: String?, args: Array<out String>?, sort: String?): Cursor? = null
    override fun insert(uri: Uri, values: ContentValues?): Uri? = throw UnsupportedOperationException()
    override fun update(uri: Uri, values: ContentValues?, selection: String?, args: Array<out String>?): Int = throw UnsupportedOperationException()
    override fun delete(uri: Uri, selection: String?, args: Array<out String>?): Int = throw UnsupportedOperationException()

    companion object {
        const val COVERS_DIR = "covers"
        const val MAX_PICKED_COVER_BYTES = 20L * 1024 * 1024
        fun authority(packageName: String) = "$packageName.covers"

        /**
         * Sniffs common image container magic bytes instead of trusting file extensions:
         * PNG (`89 50 4E 47`), JPEG (`FF D8`), WebP (`RIFF....WEBP`), and GIF (`GIF8`).
         */
        internal fun sniffImageMimeType(header: ByteArray, length: Int = header.size): String? {
            val n = length.coerceAtMost(header.size)
            return when {
                n >= 4 &&
                    header[0] == 0x89.toByte() && header[1] == 0x50.toByte() &&
                    header[2] == 0x4E.toByte() && header[3] == 0x47.toByte() -> "image/png"
                n >= 2 &&
                    header[0] == 0xFF.toByte() && header[1] == 0xD8.toByte() -> "image/jpeg"
                n >= 12 &&
                    header[0] == 'R'.code.toByte() && header[1] == 'I'.code.toByte() &&
                    header[2] == 'F'.code.toByte() && header[3] == 'F'.code.toByte() &&
                    header[8] == 'W'.code.toByte() && header[9] == 'E'.code.toByte() &&
                    header[10] == 'B'.code.toByte() && header[11] == 'P'.code.toByte() -> "image/webp"
                n >= 4 &&
                    header[0] == 'G'.code.toByte() && header[1] == 'I'.code.toByte() &&
                    header[2] == 'F'.code.toByte() && header[3] == '8'.code.toByte() -> "image/gif"
                else -> null
            }
        }

        internal fun sniffImageMimeType(file: File): String? = try {
            file.inputStream().use { input ->
                val buf = ByteArray(12)
                var readTotal = 0
                while (readTotal < buf.size) {
                    val read = input.read(buf, readTotal, buf.size - readTotal)
                    if (read <= 0) break
                    readTotal += read
                }
                sniffImageMimeType(buf, readTotal)
            }
        } catch (_: IOException) {
            null
        }

        internal fun supportedCoverExtension(mime: String?): String? = when (mime?.lowercase()) {
            "image/jpeg", "image/jpg" -> "jpg"
            "image/png" -> "png"
            "image/webp" -> "webp"
            else -> null
        }

        /**
         * Copies a picked [uri] into [dir] with a size cap and names the file after sniffing the
         * actual container magic bytes (falling back to the resolver MIME type if unknown).
         */
        internal fun copyPickedImage(
            context: Context,
            uri: Uri,
            dir: File,
            prefix: String,
            maxBytes: Long = MAX_PICKED_COVER_BYTES,
        ): File {
            val resolver = context.contentResolver
            val tmp = File(dir, "$prefix${System.nanoTime()}.tmp")
            val header = ByteArray(12)
            var headerLen = 0
            try {
                val input = resolver.openInputStream(uri) ?: error("Could not read the image")
                input.use { src ->
                    tmp.outputStream().use { dst ->
                        // Count bytes and capture magic header while copying: MIME alone can lie.
                        val buffer = ByteArray(8192)
                        var total = 0L
                        while (true) {
                            val n = src.read(buffer)
                            if (n < 0) break
                            if (headerLen < header.size) {
                                val take = minOf(n, header.size - headerLen)
                                System.arraycopy(buffer, 0, header, headerLen, take)
                                headerLen += take
                            }
                            total += n
                            if (total > maxBytes) {
                                throw IOException("Image is too large (max ${maxBytes / (1024 * 1024)} MB)")
                            }
                            dst.write(buffer, 0, n)
                        }
                    }
                }
                val detectedMime = sniffImageMimeType(header, headerLen) ?: resolver.getType(uri)
                val ext = supportedCoverExtension(detectedMime)
                    ?: error("Unsupported image type: use JPG, PNG or WEBP")
                val out = File(dir, "$prefix${System.nanoTime()}.$ext")
                if (!tmp.renameTo(out)) {
                    tmp.copyTo(out, overwrite = true)
                    tmp.delete()
                }
                return out
            } catch (e: Exception) {
                tmp.delete()
                throw e
            }
        }
    }
}
