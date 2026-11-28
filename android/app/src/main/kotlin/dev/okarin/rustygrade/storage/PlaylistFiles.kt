package dev.okarin.rustygrade.storage

import java.io.ByteArrayOutputStream
import java.io.InputStream
import java.nio.ByteBuffer
import java.nio.charset.CharacterCodingException
import java.nio.charset.Charset
import java.nio.charset.CodingErrorAction
import java.nio.charset.StandardCharsets
import java.util.Locale

enum class PlaylistImportKind { M3U, ZIP }

/** Pure JVM helpers for standalone `.m3u`/`.m3u8` and single-playlist `.zip` import/export. */
object PlaylistFiles {
    const val MAX_M3U_BYTES = 16 * 1024 * 1024
    private const val CHUNK_BYTES = 8192
    private const val MAX_STEM_CHARS = 80
    private val FORBIDDEN_FILENAME_CHARS = charArrayOf('/', '\\', ':', '*', '?', '"', '<', '>', '|')

    private val ZIP_MIMES = setOf("application/zip", "application/x-zip-compressed")
    private val M3U_MIMES = setOf(
        "audio/x-mpegurl",
        "audio/mpegurl",
        "application/vnd.apple.mpegurl",
        "application/x-mpegurl",
    )

    fun detectImportKind(
        displayName: String?,
        mimeType: String?,
        header: ByteArray,
        headerLen: Int = header.size,
    ): PlaylistImportKind? {
        val len = headerLen.coerceIn(0, header.size)
        if (isZipHeader(header, len)) {
            return PlaylistImportKind.ZIP
        }

        val lowerName = displayName?.trim()?.lowercase(Locale.ROOT)
        if (lowerName != null) {
            if (lowerName.endsWith(".zip")) return PlaylistImportKind.ZIP
            if (lowerName.endsWith(".m3u8") || lowerName.endsWith(".m3u")) return PlaylistImportKind.M3U
        }

        val cleanMime = mimeType?.substringBefore(';')?.trim()?.lowercase(Locale.ROOT)
        if (cleanMime != null) {
            if (cleanMime in ZIP_MIMES) return PlaylistImportKind.ZIP
            if (cleanMime in M3U_MIMES) return PlaylistImportKind.M3U
        }

        if (startsWithExtM3u(header, len)) {
            return PlaylistImportKind.M3U
        }
        return null
    }

    fun readBoundedText(
        input: InputStream,
        allowWindows1252Fallback: Boolean = false,
        maxBytes: Int = MAX_M3U_BYTES,
    ): String {
        val out = ByteArrayOutputStream()
        val buf = ByteArray(CHUNK_BYTES)
        var total = 0
        while (true) {
            val n = input.read(buf)
            if (n < 0) break
            if (n == 0) continue
            for (i in 0 until n) {
                require(buf[i] != 0.toByte()) { "Playlist file contains binary data" }
            }
            total += n
            require(total <= maxBytes) { "Playlist file exceeds ${formatLimit(maxBytes)} limit" }
            out.write(buf, 0, n)
        }
        val bytes = out.toByteArray()
        return try {
            decodeStrict(bytes, StandardCharsets.UTF_8)
        } catch (e: CharacterCodingException) {
            if (!allowWindows1252Fallback) {
                throw IllegalArgumentException("Playlist file is not valid UTF-8", e)
            }
            try {
                decodeStrict(bytes, Charset.forName("windows-1252"))
            } catch (e2: CharacterCodingException) {
                throw IllegalArgumentException("Playlist file is not valid UTF-8 or Windows-1252 text", e2)
            }
        }
    }

    private fun decodeStrict(bytes: ByteArray, charset: Charset): String {
        val decoder = charset.newDecoder()
            .onMalformedInput(CodingErrorAction.REPORT)
            .onUnmappableCharacter(CodingErrorAction.REPORT)
        return decoder.decode(ByteBuffer.wrap(bytes)).toString()
    }

    private fun formatLimit(maxBytes: Int): String =
        if (maxBytes >= 1024 * 1024 && maxBytes % (1024 * 1024) == 0) {
            "${maxBytes / (1024 * 1024)} MiB"
        } else {
            "$maxBytes bytes"
        }

    fun sanitizeExportFilename(name: String, extension: String): String {
        val sb = StringBuilder(name.length)
        for (ch in name) {
            val forbidden = ch in FORBIDDEN_FILENAME_CHARS || ch < ' ' || ch == '\u007F'
            sb.append(if (forbidden) '_' else ch)
        }
        val trimmed = sb.toString().trim { it == '.' || it == '_' || it.isWhitespace() }
        val truncated = truncateCodePoints(trimmed, MAX_STEM_CHARS)
        val stem = truncated.trim { it == '.' || it == '_' || it.isWhitespace() }.ifEmpty { "playlist" }
        val cleanExt = extension.trim().trimStart('.')
        return if (cleanExt.isEmpty()) stem else "$stem.$cleanExt"
    }

    private fun truncateCodePoints(s: String, maxCodePoints: Int): String {
        if (s.codePointCount(0, s.length) <= maxCodePoints) return s
        val endOffset = s.offsetByCodePoints(0, maxCodePoints)
        return s.substring(0, endOffset)
    }

    private fun isZipHeader(header: ByteArray, len: Int): Boolean {
        if (len < 4) return false
        if (header[0] != 'P'.code.toByte() || header[1] != 'K'.code.toByte()) return false
        val b2 = header[2].toInt() and 0xFF
        val b3 = header[3].toInt() and 0xFF
        return (b2 == 0x03 && b3 == 0x04) ||
            (b2 == 0x05 && b3 == 0x06) ||
            (b2 == 0x07 && b3 == 0x08)
    }

    private fun startsWithExtM3u(header: ByteArray, len: Int): Boolean {
        var i = 0
        if (len >= 3 &&
            (header[0].toInt() and 0xFF) == 0xEF &&
            (header[1].toInt() and 0xFF) == 0xBB &&
            (header[2].toInt() and 0xFF) == 0xBF
        ) {
            i = 3
        }
        while (i < len && header[i].toInt().toChar().isWhitespace()) {
            i++
        }
        val magic = "#EXTM3U"
        if (len - i < magic.length) return false
        for (j in magic.indices) {
            val c = (header[i + j].toInt() and 0xFF).toChar().uppercaseChar()
            if (c != magic[j]) return false
        }
        return true
    }
}
