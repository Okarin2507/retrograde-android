package dev.okarin.rustygrade.storage

import java.io.ByteArrayOutputStream
import java.net.HttpURLConnection
import java.net.URI
import java.net.URL
import java.net.URLEncoder
import java.text.Normalizer
import java.util.Locale
import kotlin.math.abs

/**
 * Text normalization, matching/scoring heuristics, and safe HTTP helpers shared by the online
 * lyrics and cover art lookup providers (ported from Rustygrade Linux).
 */
internal object LookupNormalize {
    const val USER_AGENT = "Rustygrade/1.0.0 (Android; +https://github.com/Okarin2507/rustygrade-mobile)"
    const val DEFAULT_TIMEOUT_MS = 8_000
    const val MAX_TEXT_BYTES = 2 * 1024 * 1024
    const val MAX_COVER_BYTES = 8 * 1024 * 1024
    const val MAX_THUMB_BYTES = 2 * 1024 * 1024

    private val GENERIC_ALBUMS = setOf(
        "unknown album",
        "unknown",
        "singles",
        "single",
        "misc",
        "various",
        "untitled",
        "no album",
        "music",
        "downloads",
        "download",
    )

    private val GENERIC_ARTISTS = setOf(
        "unknown artist",
        "unknown",
        "various artists",
        "various",
        "va",
        "v.a.",
        "v/a",
        "compilation",
    )

    private val FEATURE_SPLITS = listOf(
        " feat. ",
        " feat ",
        " ft. ",
        " ft ",
        " featuring ",
        " & ",
        " x ",
        " × ",
        ", ",
        "; ",
        " / ",
        "、",
        "・",
    )

    private val ALBUM_NOISE_PATTERNS = listOf(
        Regex("""\s*[\(\[]\s*(?:\d{4}\s+)?(?:remaster(?:ed)?|deluxe|expanded|special|anniversary|collector'?s|bonus|explicit|clean|single|ep|version|edition|limited|standard|digital|hi-?res|flac|mp3|disc\s*\d+|cd\s*\d+|ost|original\s+soundtrack)[^\)\]]*[\)\]]""", RegexOption.IGNORE_CASE),
        Regex("""\s*[-–—]\s*(?:single|ep|remaster(?:ed)?(?:\s+\d{4})?|deluxe(?:\s+edition)?|disc\s*\d+|cd\s*\d+)$""", RegexOption.IGNORE_CASE),
    )

    private val ALLOWED_IMAGE_DOMAINS = listOf(
        "mzstatic.com",
        "dzcdn.net",
        "coverartarchive.org",
        "archive.org",
        "discogs.com",
    )

    private val MBID_REGEX = Regex(
        "^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$",
    )

    fun collapseSpaces(s: String): String =
        s.trim().replace(Regex("\\s+"), " ")

    fun isGenericAlbum(album: String): Boolean {
        val c = collapseSpaces(album).lowercase(Locale.ROOT)
        return c.isEmpty() || c in GENERIC_ALBUMS
    }

    fun isGenericArtist(artist: String): Boolean {
        val c = collapseSpaces(artist).lowercase(Locale.ROOT)
        return c.isEmpty() || c in GENERIC_ARTISTS
    }

    /** Extracts the primary artist before `feat.`, `ft.`, `&`, `,`, `/`, etc. */
    fun primaryArtist(artist: String): String {
        var s = collapseSpaces(artist)
        // Strip parenthesized featuring credit: "Artist (feat. Guest)" -> "Artist"
        s = s.replace(Regex("""\s*[\(\[]\s*(?:feat\.?|ft\.?|featuring)\b[^\)\]]*[\)\]]""", RegexOption.IGNORE_CASE), "")
        val lower = s.lowercase(Locale.ROOT)
        var cut = s.length
        for (sep in FEATURE_SPLITS) {
            val idx = lower.indexOf(sep)
            if (idx in 1 until cut) cut = idx
        }
        return collapseSpaces(s.substring(0, cut))
    }

    /** Strips edition/remaster/disc suffixes from an album title for broader search recall. */
    fun cleanAlbumTitle(album: String): String {
        var cur = collapseSpaces(album)
        for (re in ALBUM_NOISE_PATTERNS) {
            cur = re.replace(cur, "")
        }
        return collapseSpaces(cur).ifEmpty { collapseSpaces(album) }
    }

    /** Strips feat./remaster/live annotations from a song title for broader lyric search recall. */
    fun cleanTrackTitle(title: String): String {
        var cur = collapseSpaces(title)
        cur = cur.replace(
            Regex("""\s*[\(\[]\s*(?:feat\.?|ft\.?|featuring|remaster(?:ed)?|official|audio|lyric|ver\.?|version|tv\s*size|short\s*ver|live\b)[^\)\]]*[\)\]]""", RegexOption.IGNORE_CASE),
            "",
        )
        cur = cur.replace(
            Regex("""\s*[-–—]\s*(?:remaster(?:ed)?(?:\s+\d{4})?|single\s+version|radio\s+edit|live(?:\s+at\b.*)?)$""", RegexOption.IGNORE_CASE),
            "",
        )
        return collapseSpaces(cur).ifEmpty { collapseSpaces(title) }
    }

    /** True when [s] contains Han/Kanji, Hiragana, Katakana, or Hangul characters. */
    fun containsCjk(s: String): Boolean {
        var i = 0
        while (i < s.length) {
            val cp = s.codePointAt(i)
            i += Character.charCount(cp)
            val isCjk = (cp in 0x3040..0x30FF) || // Hiragana & Katakana
                (cp in 0x31F0..0x31FF) || // Katakana Phonetic Extensions
                (cp in 0x3400..0x4DBF) || // CJK Unified Ideographs Ext A
                (cp in 0x4E00..0x9FFF) || // CJK Unified Ideographs
                (cp in 0xAC00..0xD7AF) || // Hangul Syllables
                (cp in 0xFF66..0xFF9F) // Halfwidth Katakana
            if (isCjk) return true
        }
        return false
    }

    /** NFKC + lowercase + keep letters/digits/spaces for fuzzy comparison. */
    fun normalizeForMatch(s: String): String {
        val nfkc = Normalizer.normalize(s, Normalizer.Form.NFKC).lowercase(Locale.ROOT)
        val out = StringBuilder(nfkc.length)
        var prevSpace = true
        for (ch in nfkc) {
            if (ch.isLetterOrDigit()) {
                out.append(ch)
                prevSpace = false
            } else if (!prevSpace) {
                out.append(' ')
                prevSpace = true
            }
        }
        return out.toString().trim()
    }

    fun tokenSimilarity(a: String, b: String): Double {
        val na = normalizeForMatch(a)
        val nb = normalizeForMatch(b)
        if (na.isEmpty() || nb.isEmpty()) return 0.0
        if (na == nb) return 1.0
        if (na.contains(nb) || nb.contains(na)) {
            val ratio = minOf(na.length, nb.length).toDouble() / maxOf(na.length, nb.length).toDouble()
            return 0.72 + 0.25 * ratio
        }
        val ta = na.split(' ').filter { it.isNotEmpty() }.toSet()
        val tb = nb.split(' ').filter { it.isNotEmpty() }.toSet()
        if (ta.isEmpty() || tb.isEmpty()) return 0.0
        val inter = ta.intersect(tb).size.toDouble()
        val union = ta.union(tb).size.toDouble()
        val jaccard = inter / union
        val contain = inter / minOf(ta.size, tb.size).toDouble()
        return maxOf(jaccard, contain * 0.85)
    }

    /** Escapes Lucene special syntax characters for MusicBrainz queries. */
    fun escapeLucene(s: String): String {
        val specials = setOf('+', '-', '&', '|', '!', '(', ')', '{', '}', '[', ']', '^', '"', '~', '*', '?', ':', '\\', '/')
        val out = StringBuilder(s.length + 8)
        for (ch in s) {
            if (ch in specials) out.append('\\')
            out.append(ch)
        }
        return out.toString()
    }

    fun isValidMbid(mbid: String): Boolean = MBID_REGEX.matches(mbid.trim())

    /** Enforces HTTPS + trusted cover CDN domain allowlist (matching Linux `artwork_lookup`). */
    fun validateDownloadUrl(rawUrl: String): Boolean {
        val uri = runCatching { URI(rawUrl.trim()) }.getOrNull() ?: return false
        if (!uri.scheme.equals("https", ignoreCase = true)) return false
        val host = uri.host?.lowercase(Locale.ROOT) ?: return false
        return ALLOWED_IMAGE_DOMAINS.any { domain ->
            host == domain || host.endsWith(".$domain")
        }
    }

    /**
     * Scores an online artwork candidate against the target album/artist/year.
     * Tolerates CJK vs romanized cross-script mismatches when the other field matches strongly.
     */
    fun scoreArtworkCandidate(
        queryAlbum: String,
        queryArtist: String,
        queryYear: Int,
        candAlbum: String,
        candArtist: String,
        candYear: String?,
        sourceBonus: Int = 0,
    ): Int {
        val qAlb = cleanAlbumTitle(queryAlbum)
        val cAlb = cleanAlbumTitle(candAlbum)
        val qArt = primaryArtist(queryArtist)
        val cArt = primaryArtist(candArtist)

        val albSim = maxOf(tokenSimilarity(queryAlbum, candAlbum), tokenSimilarity(qAlb, cAlb))
        val artSim = if (isGenericArtist(queryArtist)) {
            0.5
        } else {
            maxOf(tokenSimilarity(queryArtist, candArtist), tokenSimilarity(qArt, cArt))
        }

        // Cross-script CJK <-> Latin tolerance (e.g. Japanese artist tagged in romaji vs kanji on iTunes JP).
        val crossScriptArtist = !isGenericArtist(queryArtist) && (containsCjk(queryArtist) != containsCjk(candArtist))
        val crossScriptAlbum = containsCjk(queryAlbum) != containsCjk(candAlbum)

        var score = (albSim * 55.0).toInt() + (artSim * 35.0).toInt() + sourceBonus
        if (crossScriptArtist && albSim >= 0.85) {
            score += 22
        }
        if (crossScriptAlbum && artSim >= 0.85) {
            score += 18
        }

        // Year tie-breaker
        val parsedYear = candYear?.take(4)?.toIntOrNull()
        if (queryYear > 0 && parsedYear != null) {
            val diff = abs(queryYear - parsedYear)
            score += when {
                diff == 0 -> 8
                diff == 1 -> 4
                diff > 5 -> -6
                else -> 0
            }
        }
        return score.coerceIn(0, 120)
    }

    /**
     * Scores a lyrics candidate by title, artist, album, duration proximity, and synced LRC availability.
     */
    fun scoreLyricCandidate(
        queryTitle: String,
        queryArtist: String,
        queryAlbum: String,
        queryDurationSecs: Double,
        candTitle: String,
        candArtist: String,
        candAlbum: String?,
        candDurationSecs: Double?,
        synced: Boolean,
        sourceBonus: Int = 0,
    ): Int {
        val qTitleClean = cleanTrackTitle(queryTitle)
        val cTitleClean = cleanTrackTitle(candTitle)
        val titleSim = maxOf(tokenSimilarity(queryTitle, candTitle), tokenSimilarity(qTitleClean, cTitleClean))
        val artSim = if (isGenericArtist(queryArtist)) {
            0.5
        } else {
            maxOf(tokenSimilarity(queryArtist, candArtist), tokenSimilarity(primaryArtist(queryArtist), primaryArtist(candArtist)))
        }
        val albSim = if (!candAlbum.isNullOrBlank() && !isGenericAlbum(queryAlbum)) {
            tokenSimilarity(cleanAlbumTitle(queryAlbum), cleanAlbumTitle(candAlbum))
        } else {
            0.0
        }

        val crossScriptArtist = !isGenericArtist(queryArtist) && (containsCjk(queryArtist) != containsCjk(candArtist))
        var score = (titleSim * 52.0).toInt() + (artSim * 30.0).toInt() + (albSim * 10.0).toInt() + sourceBonus
        if (crossScriptArtist && titleSim >= 0.85) {
            score += 16
        }

        if (queryDurationSecs > 1.0 && candDurationSecs != null && candDurationSecs > 1.0) {
            val diff = abs(queryDurationSecs - candDurationSecs)
            score += when {
                diff <= 2.0 -> 14
                diff <= 5.0 -> 8
                diff <= 10.0 -> 2
                diff <= 20.0 -> -8
                else -> -22
            }
        }

        if (synced) score += 16
        return score.coerceIn(0, 150)
    }

    fun urlEncode(s: String): String = URLEncoder.encode(s, "UTF-8")

    /**
     * Performs a bounded UTF-8 HTTP GET (follows up to 4 redirects, enforces [maxBytes]).
     * Returns `null` on non-2xx or network error.
     */
    fun httpGetText(
        url: String,
        headers: Map<String, String> = emptyMap(),
        timeoutMs: Int = DEFAULT_TIMEOUT_MS,
        maxBytes: Int = MAX_TEXT_BYTES,
    ): String? {
        val bytes = httpGetBytes(url, headers, timeoutMs, maxBytes, requireAllowedImageDomain = false) ?: return null
        return bytes.toString(Charsets.UTF_8)
    }

    /**
     * Performs a bounded HTTP GET with manual redirect following (so redirects are also checked
     * against [validateDownloadUrl] when [requireAllowedImageDomain] is true).
     */
    fun httpGetBytes(
        url: String,
        headers: Map<String, String> = emptyMap(),
        timeoutMs: Int = DEFAULT_TIMEOUT_MS,
        maxBytes: Int = MAX_COVER_BYTES,
        requireAllowedImageDomain: Boolean = false,
    ): ByteArray? {
        var currentUrl = url
        for (hop in 0 until 5) {
            if (requireAllowedImageDomain && !validateDownloadUrl(currentUrl)) return null
            val conn = (URL(currentUrl).openConnection() as? HttpURLConnection) ?: return null
            try {
                conn.requestMethod = "GET"
                conn.instanceFollowRedirects = false
                conn.connectTimeout = timeoutMs
                conn.readTimeout = timeoutMs
                conn.setRequestProperty("User-Agent", USER_AGENT)
                conn.setRequestProperty("Accept", "*/*")
                for ((k, v) in headers) conn.setRequestProperty(k, v)

                val code = conn.responseCode
                if (code in 300..399) {
                    val loc = conn.getHeaderField("Location")?.trim() ?: return null
                    currentUrl = runCatching { URI(currentUrl).resolve(loc).toString() }.getOrNull() ?: return null
                    continue
                }
                if (code !in 200..299) return null
                val contentLength = conn.contentLengthLong
                if (contentLength > maxBytes) return null
                return conn.inputStream.use { input ->
                    val out = ByteArrayOutputStream(if (contentLength in 1..maxBytes) contentLength.toInt() else 16_384)
                    val buf = ByteArray(8_192)
                    var total = 0
                    while (true) {
                        val n = input.read(buf)
                        if (n <= 0) break
                        total += n
                        if (total > maxBytes) return null
                        out.write(buf, 0, n)
                    }
                    out.toByteArray()
                }
            } catch (_: Exception) {
                return null
            } finally {
                conn.disconnect()
            }
        }
        return null
    }

    /** Sniffs JPEG/PNG/WebP magic bytes and returns the matching extension, or `null` if invalid. */
    fun sniffImageExtension(bytes: ByteArray): String? {
        if (bytes.size < 12) return null
        if (bytes[0] == 0xFF.toByte() && bytes[1] == 0xD8.toByte() && bytes[2] == 0xFF.toByte()) {
            return "jpg"
        }
        if (bytes[0] == 0x89.toByte() && bytes[1] == 0x50.toByte() && bytes[2] == 0x4E.toByte() && bytes[3] == 0x47.toByte()) {
            return "png"
        }
        if (bytes[0] == 'R'.code.toByte() && bytes[1] == 'I'.code.toByte() && bytes[2] == 'F'.code.toByte() && bytes[3] == 'F'.code.toByte() &&
            bytes[8] == 'W'.code.toByte() && bytes[9] == 'E'.code.toByte() && bytes[10] == 'B'.code.toByte() && bytes[11] == 'P'.code.toByte()
        ) {
            return "webp"
        }
        return null
    }
}
