package dev.okarin.rustygrade.storage

import android.util.Base64
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import kotlinx.coroutines.coroutineScope
import org.json.JSONArray
import org.json.JSONObject
import uniffi.rustygrade_ffi.TrackInfo

data class LyricQuery(
    val title: String,
    val artist: String,
    val album: String = "",
    val durationSecs: Double = 0.0,
) {
    companion object {
        fun fromTrack(track: TrackInfo): LyricQuery = LyricQuery(
            title = track.title,
            artist = track.artist,
            album = track.album,
            durationSecs = track.durationSecs,
        )
    }
}

data class LyricCandidate(
    val source: String,
    val title: String,
    val artist: String,
    val album: String?,
    val durationSecs: Double?,
    val synced: Boolean,
    val text: String,
    val preview: String,
    val score: Int,
)

/**
 * Multi-provider online lyrics lookup ported from Rustygrade Linux:
 * **LRCLIB**, **NetEase Cloud Music**, **KuGou**, and **Lyrics.ovh**.
 */
object OnlineLyricsLookup {
    private val TIMESTAMP_REGEX = Regex("""\[\d{1,2}:\d{2}(?:[.:]\d{1,3})?\]""")
    private val NON_LYRIC_MARKERS = listOf("纯音乐，请欣赏", "纯音乐,请欣赏", "此歌曲为没有填词的纯音乐")

    /** Queries all 4 providers in parallel and returns ranked, deduplicated candidates. */
    suspend fun searchAll(query: LyricQuery): List<LyricCandidate> = coroutineScope {
        val cleanTitle = LookupNormalize.collapseSpaces(query.title)
        if (cleanTitle.isEmpty()) return@coroutineScope emptyList()

        val lrcLib = async(Dispatchers.IO) { runCatching { searchLrcLib(query) }.getOrDefault(emptyList()) }
        val netease = async(Dispatchers.IO) { runCatching { searchNetEase(query) }.getOrDefault(emptyList()) }
        val kugou = async(Dispatchers.IO) { runCatching { searchKuGou(query) }.getOrDefault(emptyList()) }
        val ovh = async(Dispatchers.IO) { runCatching { searchLyricsOvh(query) }.getOrDefault(emptyList()) }

        val combined = lrcLib.await() + netease.await() + kugou.await() + ovh.await()
        deduplicateAndRank(combined)
    }

    /** Returns the highest-scoring candidate suitable for automatic batch fetching, or `null`. */
    suspend fun fetchBest(query: LyricQuery): LyricCandidate? =
        searchAll(query).firstOrNull { it.score >= 42 }

    internal fun deduplicateAndRank(candidates: List<LyricCandidate>): List<LyricCandidate> {
        val seen = HashSet<String>()
        return candidates
            .sortedWith(compareByDescending<LyricCandidate> { it.score }.thenByDescending { it.synced })
            .filter { c ->
                val sig = "${c.source}:${c.synced}:${c.preview.take(48).lowercase()}"
                seen.add(sig)
            }
            .take(16)
    }

    internal fun hasTimestamps(text: String): Boolean = TIMESTAMP_REGEX.containsMatchIn(text)

    internal fun isInstrumentalStub(text: String): Boolean {
        val stripped = TIMESTAMP_REGEX.replace(text, "").trim()
        if (stripped.length < 12) return true
        return NON_LYRIC_MARKERS.any { stripped.contains(it) }
    }

    internal fun buildPreview(text: String): String =
        text.removePrefix("\uFEFF")
            .lineSequence()
            .map { TIMESTAMP_REGEX.replace(it.removePrefix("\uFEFF"), "").trim() }
            .filter { it.isNotEmpty() && !it.startsWith("[") }
            .take(3)
            .joinToString(" / ")
            .take(140)

    // ---- LRCLIB -------------------------------------------------------------------------------

    private fun searchLrcLib(query: LyricQuery): List<LyricCandidate> {
        val title = LookupNormalize.cleanTrackTitle(query.title)
        val artist = if (LookupNormalize.isGenericArtist(query.artist)) "" else LookupNormalize.primaryArtist(query.artist)
        val primaryUrl = buildString {
            append("https://lrclib.net/api/search?track_name=")
            append(LookupNormalize.urlEncode(title))
            if (artist.isNotEmpty()) {
                append("&artist_name=")
                append(LookupNormalize.urlEncode(artist))
            }
        }
        val first = LookupNormalize.httpGetText(primaryUrl)?.let { parseLrcLibJson(it, query) }.orEmpty()
        if (first.isNotEmpty()) return first

        val fallbackQ = LookupNormalize.collapseSpaces("$artist $title")
        if (fallbackQ.isEmpty()) return emptyList()
        val fallbackUrl = "https://lrclib.net/api/search?q=${LookupNormalize.urlEncode(fallbackQ)}"
        return LookupNormalize.httpGetText(fallbackUrl)?.let { parseLrcLibJson(it, query) }.orEmpty()
    }

    internal fun parseLrcLibJson(jsonText: String, query: LyricQuery): List<LyricCandidate> {
        val arr = runCatching { JSONArray(jsonText) }.getOrNull() ?: return emptyList()
        val out = ArrayList<LyricCandidate>()
        for (i in 0 until minOf(arr.length(), 10)) {
            val obj = arr.optJSONObject(i) ?: continue
            if (obj.optBoolean("instrumental", false)) continue
            val synced = obj.optString("syncedLyrics", "").trim()
            val plain = obj.optString("plainLyrics", "").trim()
            val chosen = synced.ifEmpty { plain }
            if (chosen.isEmpty() || isInstrumentalStub(chosen)) continue
            val isSynced = synced.isNotEmpty() && hasTimestamps(synced)
            val title = obj.optString("trackName", obj.optString("name", query.title))
            val artist = obj.optString("artistName", "")
            val album = obj.optString("albumName", "").ifEmpty { null }
            val dur = obj.optDouble("duration", Double.NaN).takeIf { !it.isNaN() && it > 0.0 }
            val score = LookupNormalize.scoreLyricCandidate(
                queryTitle = query.title,
                queryArtist = query.artist,
                queryAlbum = query.album,
                queryDurationSecs = query.durationSecs,
                candTitle = title,
                candArtist = artist,
                candAlbum = album,
                candDurationSecs = dur,
                synced = isSynced,
                sourceBonus = 6,
            )
            out += LyricCandidate(
                source = "LRCLIB",
                title = title,
                artist = artist,
                album = album,
                durationSecs = dur,
                synced = isSynced,
                text = chosen,
                preview = buildPreview(chosen),
                score = score,
            )
        }
        return out
    }

    // ---- NetEase Cloud Music ------------------------------------------------------------------

    private fun searchNetEase(query: LyricQuery): List<LyricCandidate> {
        val title = LookupNormalize.cleanTrackTitle(query.title)
        val artist = if (LookupNormalize.isGenericArtist(query.artist)) "" else LookupNormalize.primaryArtist(query.artist)
        val keyword = LookupNormalize.collapseSpaces("$title $artist")
        if (keyword.isEmpty()) return emptyList()
        val headers = mapOf("Referer" to "https://music.163.com/")
        val searchUrl = "https://music.163.com/api/search/get?type=1&s=${LookupNormalize.urlEncode(keyword)}&limit=6"
        val searchBody = LookupNormalize.httpGetText(searchUrl, headers = headers) ?: return emptyList()
        val songs = parseNetEaseSearchSongs(searchBody)
        if (songs.isEmpty()) return emptyList()

        // Rank songs by metadata first, then fetch lyrics for top 3.
        val ranked = songs.sortedByDescending { s ->
            LookupNormalize.scoreLyricCandidate(
                queryTitle = query.title,
                queryArtist = query.artist,
                queryAlbum = query.album,
                queryDurationSecs = query.durationSecs,
                candTitle = s.title,
                candArtist = s.artist,
                candAlbum = s.album,
                candDurationSecs = s.durationSecs,
                synced = true,
            )
        }.take(3)

        val out = ArrayList<LyricCandidate>()
        for (song in ranked) {
            val lrcUrl = "https://music.163.com/api/song/lyric?id=${song.id}&lv=1&tv=1"
            val lrcBody = LookupNormalize.httpGetText(lrcUrl, headers = headers) ?: continue
            val lyricText = parseNetEaseLyricJson(lrcBody) ?: continue
            if (isInstrumentalStub(lyricText)) continue
            val isSynced = hasTimestamps(lyricText)
            val score = LookupNormalize.scoreLyricCandidate(
                queryTitle = query.title,
                queryArtist = query.artist,
                queryAlbum = query.album,
                queryDurationSecs = query.durationSecs,
                candTitle = song.title,
                candArtist = song.artist,
                candAlbum = song.album,
                candDurationSecs = song.durationSecs,
                synced = isSynced,
                sourceBonus = 4,
            )
            out += LyricCandidate(
                source = "NETEASE",
                title = song.title,
                artist = song.artist,
                album = song.album,
                durationSecs = song.durationSecs,
                synced = isSynced,
                text = lyricText,
                preview = buildPreview(lyricText),
                score = score,
            )
        }
        return out
    }

    internal data class NetEaseSong(
        val id: Long,
        val title: String,
        val artist: String,
        val album: String?,
        val durationSecs: Double?,
    )

    internal fun parseNetEaseSearchSongs(jsonText: String): List<NetEaseSong> {
        val root = runCatching { JSONObject(jsonText) }.getOrNull() ?: return emptyList()
        val songs = root.optJSONObject("result")?.optJSONArray("songs") ?: return emptyList()
        val out = ArrayList<NetEaseSong>()
        for (i in 0 until songs.length()) {
            val s = songs.optJSONObject(i) ?: continue
            val id = s.optLong("id", 0L)
            if (id <= 0L) continue
            val title = s.optString("name", "")
            val artistsArr = s.optJSONArray("artists")
            val artist = if (artistsArr != null && artistsArr.length() > 0) {
                (0 until artistsArr.length())
                    .mapNotNull { artistsArr.optJSONObject(it)?.optString("name")?.takeIf(String::isNotBlank) }
                    .joinToString(", ")
            } else {
                ""
            }
            val album = s.optJSONObject("album")?.optString("name")?.ifBlank { null }
            val durMs = s.optLong("duration", 0L)
            out += NetEaseSong(id, title, artist, album, if (durMs > 0) durMs / 1000.0 else null)
        }
        return out
    }

    internal fun parseNetEaseLyricJson(jsonText: String): String? {
        val root = runCatching { JSONObject(jsonText) }.getOrNull() ?: return null
        val lrc = root.optJSONObject("lrc")?.optString("lyric", "")?.trim().orEmpty()
        return lrc.ifEmpty { null }
    }

    // ---- KuGou --------------------------------------------------------------------------------

    private fun searchKuGou(query: LyricQuery): List<LyricCandidate> {
        val title = LookupNormalize.cleanTrackTitle(query.title)
        val artist = if (LookupNormalize.isGenericArtist(query.artist)) "" else LookupNormalize.primaryArtist(query.artist)
        val keyword = LookupNormalize.collapseSpaces("$artist $title")
        if (keyword.isEmpty()) return emptyList()

        val searchUrl = "https://mobileservice.kugou.com/api/v3/search/song?format=json&keyword=${LookupNormalize.urlEncode(keyword)}&page=1&pagesize=6"
        val searchBody = LookupNormalize.httpGetText(searchUrl) ?: return emptyList()
        val root = runCatching { JSONObject(searchBody) }.getOrNull() ?: return emptyList()
        val info = root.optJSONObject("data")?.optJSONArray("info") ?: return emptyList()

        val out = ArrayList<LyricCandidate>()
        for (i in 0 until minOf(info.length(), 3)) {
            val s = info.optJSONObject(i) ?: continue
            val hash = s.optString("hash", "").trim()
            if (hash.isEmpty()) continue
            val sTitle = s.optString("songname", title)
            val sArtist = s.optString("singername", artist)
            val sAlbum = s.optString("album_name", "").ifBlank { null }
            val durS = s.optDouble("duration", 0.0).takeIf { it > 0.0 }
            val durMs = ((durS ?: query.durationSecs) * 1000.0).toLong().coerceAtLeast(0L)

            val krcSearchUrl = "https://krcs.kugou.com/search?ver=1&man=yes&client=mobi&keyword=&duration=$durMs&hash=${LookupNormalize.urlEncode(hash)}"
            val krcBody = LookupNormalize.httpGetText(krcSearchUrl) ?: continue
            val krcRoot = runCatching { JSONObject(krcBody) }.getOrNull() ?: continue
            val cand = krcRoot.optJSONArray("candidates")?.optJSONObject(0) ?: continue
            val id = cand.optString("id", "").trim()
            val accesskey = cand.optString("accesskey", "").trim()
            if (id.isEmpty() || accesskey.isEmpty()) continue

            val dlUrl = "https://lyrics.kugou.com/download?ver=1&client=pc&id=${LookupNormalize.urlEncode(id)}&accesskey=${LookupNormalize.urlEncode(accesskey)}&fmt=lrc&charset=utf8"
            val dlBody = LookupNormalize.httpGetText(dlUrl) ?: continue
            val b64 = runCatching { JSONObject(dlBody).optString("content", "") }.getOrNull().orEmpty()
            if (b64.isEmpty()) continue
            val decoded = runCatching {
                String(Base64.decode(b64, Base64.DEFAULT), Charsets.UTF_8).removePrefix("\uFEFF").trim()
            }.getOrNull().orEmpty()
            if (decoded.isEmpty() || isInstrumentalStub(decoded)) continue
            val isSynced = hasTimestamps(decoded)
            val score = LookupNormalize.scoreLyricCandidate(
                queryTitle = query.title,
                queryArtist = query.artist,
                queryAlbum = query.album,
                queryDurationSecs = query.durationSecs,
                candTitle = sTitle,
                candArtist = sArtist,
                candAlbum = sAlbum,
                candDurationSecs = durS,
                synced = isSynced,
                sourceBonus = 2,
            )
            out += LyricCandidate(
                source = "KUGOU",
                title = sTitle,
                artist = sArtist,
                album = sAlbum,
                durationSecs = durS,
                synced = isSynced,
                text = decoded,
                preview = buildPreview(decoded),
                score = score,
            )
        }
        return out
    }

    // ---- Lyrics.ovh (plain text fallback) -----------------------------------------------------

    private fun searchLyricsOvh(query: LyricQuery): List<LyricCandidate> {
        if (LookupNormalize.isGenericArtist(query.artist)) return emptyList()
        val artist = LookupNormalize.primaryArtist(query.artist)
        val title = LookupNormalize.cleanTrackTitle(query.title)
        if (artist.isEmpty() || title.isEmpty()) return emptyList()
        val url = "https://api.lyrics.ovh/v1/${encodePathSegment(artist)}/${encodePathSegment(title)}"
        val body = LookupNormalize.httpGetText(url, timeoutMs = 6_000) ?: return emptyList()
        val text = runCatching { JSONObject(body).optString("lyrics", "").trim() }.getOrNull().orEmpty()
        if (text.isEmpty() || isInstrumentalStub(text)) return emptyList()
        val score = LookupNormalize.scoreLyricCandidate(
            queryTitle = query.title,
            queryArtist = query.artist,
            queryAlbum = query.album,
            queryDurationSecs = query.durationSecs,
            candTitle = title,
            candArtist = artist,
            candAlbum = null,
            candDurationSecs = null,
            synced = false,
            sourceBonus = -2,
        )
        return listOf(
            LyricCandidate(
                source = "OVH",
                title = title,
                artist = artist,
                album = null,
                durationSecs = null,
                synced = false,
                text = text,
                preview = buildPreview(text),
                score = score,
            ),
        )
    }

    private fun encodePathSegment(s: String): String =
        LookupNormalize.urlEncode(s).replace("+", "%20")
}
