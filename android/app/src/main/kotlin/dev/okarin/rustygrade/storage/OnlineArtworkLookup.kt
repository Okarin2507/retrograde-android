package dev.okarin.rustygrade.storage

import java.io.File
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.withContext
import org.json.JSONObject

data class ArtworkProviders(
    val itunes: Boolean = true,
    val deezer: Boolean = true,
    val musicbrainz: Boolean = true,
)

data class ArtworkQuery(
    val album: String,
    val artist: String,
    val trackTitle: String = "",
    val year: Int = 0,
    val providers: ArtworkProviders = ArtworkProviders(),
)

data class ArtworkCandidate(
    val source: String,
    val title: String,
    val artist: String,
    val year: String?,
    val thumbUrl: String,
    val fullUrls: List<String>,
    val score: Int,
)

/**
 * Multi-provider online cover art lookup ported from Rustygrade Linux:
 * **iTunes** (including JP store for CJK releases), **Deezer**, and
 * **MusicBrainz + Cover Art Archive**.
 */
object OnlineArtworkLookup {
    /** Queries all providers in parallel and returns ranked, deduplicated cover candidates. */
    suspend fun searchAll(query: ArtworkQuery): List<ArtworkCandidate> = coroutineScope {
        val hasAlbum = !LookupNormalize.isGenericAlbum(query.album)
        val hasTrack = LookupNormalize.collapseSpaces(query.trackTitle).isNotEmpty()
        if (!hasAlbum && !hasTrack) return@coroutineScope emptyList()

        val p = query.providers
        val itunes = if (p.itunes) async(Dispatchers.IO) { runCatching { searchItunes(query) }.getOrDefault(emptyList()) } else null
        val deezer = if (p.deezer) async(Dispatchers.IO) { runCatching { searchDeezer(query) }.getOrDefault(emptyList()) } else null
        val mb = if (p.musicbrainz) async(Dispatchers.IO) { runCatching { searchMusicBrainz(query) }.getOrDefault(emptyList()) } else null

        val all = ArrayList<ArtworkCandidate>()
        itunes?.let { all += it.await() }
        deezer?.let { all += it.await() }
        mb?.let { all += it.await() }
        deduplicateAndRank(all)
    }

    /** Downloads a candidate's thumbnail bytes (bounded & domain-checked) for the Cover Picker UI. */
    suspend fun downloadThumbnailBytes(url: String): ByteArray? = withContext(Dispatchers.IO) {
        LookupNormalize.httpGetBytes(
            url = url,
            maxBytes = LookupNormalize.MAX_THUMB_BYTES,
            requireAllowedImageDomain = true,
        )
    }

    /**
     * Downloads the highest-resolution cover available from [candidate] into [destDir] as a real
     * image file (`.jpg`/`.png`/`.webp`) verified by magic bytes. Returns `null` if all URLs fail.
     */
    suspend fun downloadCoverToFile(candidate: ArtworkCandidate, destDir: File): File? = withContext(Dispatchers.IO) {
        for (url in candidate.fullUrls) {
            val bytes = LookupNormalize.httpGetBytes(
                url = url,
                maxBytes = LookupNormalize.MAX_COVER_BYTES,
                requireAllowedImageDomain = true,
            ) ?: continue
            val ext = LookupNormalize.sniffImageExtension(bytes) ?: continue
            val out = File.createTempFile("online-cover-", ".$ext", destDir)
            try {
                out.writeBytes(bytes)
                return@withContext out
            } catch (_: Exception) {
                out.delete()
            }
        }
        null
    }

    /** Batch helper: searches and downloads the best matching cover (>= score 45) to a temp file. */
    suspend fun fetchBestToFile(query: ArtworkQuery, destDir: File): File? {
        val best = searchAll(query).filter { it.score >= 45 }.take(3)
        for (cand in best) {
            val file = downloadCoverToFile(cand, destDir)
            if (file != null) return file
        }
        return null
    }

    internal fun deduplicateAndRank(candidates: List<ArtworkCandidate>): List<ArtworkCandidate> {
        val seen = HashSet<String>()
        return candidates
            .sortedByDescending { it.score }
            .filter { seen.add(it.thumbUrl) }
            .take(18)
    }

    // ---- iTunes Search API --------------------------------------------------------------------

    private fun searchItunes(query: ArtworkQuery): List<ArtworkCandidate> {
        val albumClean = LookupNormalize.cleanAlbumTitle(query.album)
        val artistClean = if (LookupNormalize.isGenericArtist(query.artist)) "" else LookupNormalize.primaryArtist(query.artist)
        val searchSubject = if (!LookupNormalize.isGenericAlbum(query.album)) albumClean else LookupNormalize.cleanTrackTitle(query.trackTitle)
        val term = LookupNormalize.collapseSpaces("$artistClean $searchSubject")
        if (term.isEmpty()) return emptyList()

        val results = ArrayList<ArtworkCandidate>()
        val usUrl = "https://itunes.apple.com/search?term=${LookupNormalize.urlEncode(term)}&entity=album&limit=10"
        LookupNormalize.httpGetText(usUrl)?.let { results += parseItunesJson(it, query) }

        // Japanese/CJK metadata often resolves much better in the JP storefront.
        if (LookupNormalize.containsCjk(query.album) || LookupNormalize.containsCjk(query.artist)) {
            val jpUrl = "https://itunes.apple.com/search?term=${LookupNormalize.urlEncode(term)}&entity=album&country=jp&limit=10"
            LookupNormalize.httpGetText(jpUrl)?.let { results += parseItunesJson(it, query) }
        }
        return results
    }

    internal fun parseItunesJson(jsonText: String, query: ArtworkQuery): List<ArtworkCandidate> {
        val root = runCatching { JSONObject(jsonText) }.getOrNull() ?: return emptyList()
        val arr = root.optJSONArray("results") ?: return emptyList()
        val out = ArrayList<ArtworkCandidate>()
        for (i in 0 until arr.length()) {
            val obj = arr.optJSONObject(i) ?: continue
            val art100 = obj.optString("artworkUrl100", "").trim()
            if (art100.isEmpty()) continue
            val thumb = upscaleItunesArt(art100, 300)
            val full1200 = upscaleItunesArt(art100, 1200)
            val full600 = upscaleItunesArt(art100, 600)
            if (!LookupNormalize.validateDownloadUrl(thumb)) continue

            val title = obj.optString("collectionName", obj.optString("trackName", ""))
            val artist = obj.optString("artistName", "")
            val year = obj.optString("releaseDate", "").take(4).ifBlank { null }
            val score = LookupNormalize.scoreArtworkCandidate(
                queryAlbum = if (!LookupNormalize.isGenericAlbum(query.album)) query.album else query.trackTitle,
                queryArtist = query.artist,
                queryYear = query.year,
                candAlbum = title,
                candArtist = artist,
                candYear = year,
                sourceBonus = 6,
            )
            out += ArtworkCandidate(
                source = "ITUNES",
                title = title,
                artist = artist,
                year = year,
                thumbUrl = thumb,
                fullUrls = listOf(full1200, full600, thumb).distinct(),
                score = score,
            )
        }
        return out
    }

    internal fun upscaleItunesArt(url: String, size: Int): String =
        url.replace(Regex("""/\d+x\d+bb\."""), "/${size}x${size}bb.")

    // ---- Deezer API ---------------------------------------------------------------------------

    private fun searchDeezer(query: ArtworkQuery): List<ArtworkCandidate> {
        val albumClean = LookupNormalize.cleanAlbumTitle(query.album)
        val artistClean = if (LookupNormalize.isGenericArtist(query.artist)) "" else LookupNormalize.primaryArtist(query.artist)
        val subject = if (!LookupNormalize.isGenericAlbum(query.album)) albumClean else LookupNormalize.cleanTrackTitle(query.trackTitle)
        val term = LookupNormalize.collapseSpaces("$artistClean $subject")
        if (term.isEmpty()) return emptyList()

        val albumUrl = "https://api.deezer.com/search/album?q=${LookupNormalize.urlEncode(term)}&limit=10"
        val fromAlbums = LookupNormalize.httpGetText(albumUrl)?.let { parseDeezerJson(it, query, isTrackSearch = false) }.orEmpty()
        if (fromAlbums.isNotEmpty()) return fromAlbums

        val trackUrl = "https://api.deezer.com/search/track?q=${LookupNormalize.urlEncode(term)}&limit=8"
        return LookupNormalize.httpGetText(trackUrl)?.let { parseDeezerJson(it, query, isTrackSearch = true) }.orEmpty()
    }

    internal fun parseDeezerJson(jsonText: String, query: ArtworkQuery, isTrackSearch: Boolean): List<ArtworkCandidate> {
        val root = runCatching { JSONObject(jsonText) }.getOrNull() ?: return emptyList()
        val arr = root.optJSONArray("data") ?: return emptyList()
        val out = ArrayList<ArtworkCandidate>()
        for (i in 0 until arr.length()) {
            val item = arr.optJSONObject(i) ?: continue
            val albObj = if (isTrackSearch) item.optJSONObject("album") ?: continue else item
            // Deezer returns an empty md5_image when the album only has a generic placeholder cover.
            val md5 = albObj.optString("md5_image", "").trim()
            if (md5.isEmpty()) continue
            val xl = albObj.optString("cover_xl", "").trim()
            val big = albObj.optString("cover_big", "").trim()
            val med = albObj.optString("cover_medium", "").trim()
            val thumb = med.ifEmpty { big }.ifEmpty { xl }
            if (thumb.isEmpty() || !LookupNormalize.validateDownloadUrl(thumb)) continue

            val title = albObj.optString("title", "")
            val artist = item.optJSONObject("artist")?.optString("name", "").orEmpty()
            val score = LookupNormalize.scoreArtworkCandidate(
                queryAlbum = if (!LookupNormalize.isGenericAlbum(query.album)) query.album else query.trackTitle,
                queryArtist = query.artist,
                queryYear = query.year,
                candAlbum = title,
                candArtist = artist,
                candYear = null,
                sourceBonus = 4,
            )
            val fullUrls = listOf(xl, big, med).filter { it.isNotEmpty() && LookupNormalize.validateDownloadUrl(it) }.distinct()
            if (fullUrls.isEmpty()) continue
            out += ArtworkCandidate(
                source = "DEEZER",
                title = title,
                artist = artist,
                year = null,
                thumbUrl = thumb,
                fullUrls = fullUrls,
                score = score,
            )
        }
        return out
    }

    // ---- MusicBrainz + Cover Art Archive ------------------------------------------------------

    private fun searchMusicBrainz(query: ArtworkQuery): List<ArtworkCandidate> {
        if (LookupNormalize.isGenericAlbum(query.album)) return emptyList()
        val alb = LookupNormalize.escapeLucene(LookupNormalize.cleanAlbumTitle(query.album))
        val art = if (LookupNormalize.isGenericArtist(query.artist)) {
            ""
        } else {
            LookupNormalize.escapeLucene(LookupNormalize.primaryArtist(query.artist))
        }
        val lucene = if (art.isNotEmpty()) {
            "releasegroup:\"$alb\" AND artist:\"$art\""
        } else {
            "releasegroup:\"$alb\""
        }
        val url = "https://musicbrainz.org/ws/2/release-group/?query=${LookupNormalize.urlEncode(lucene)}&fmt=json&limit=5"
        val body = LookupNormalize.httpGetText(
            url,
            headers = mapOf("Accept" to "application/json"),
            timeoutMs = 7_000,
        ) ?: return emptyList()
        return parseMusicBrainzJson(body, query)
    }

    internal fun parseMusicBrainzJson(jsonText: String, query: ArtworkQuery): List<ArtworkCandidate> {
        val root = runCatching { JSONObject(jsonText) }.getOrNull() ?: return emptyList()
        val groups = root.optJSONArray("release-groups") ?: return emptyList()
        val out = ArrayList<ArtworkCandidate>()
        for (i in 0 until minOf(groups.length(), 4)) {
            val rg = groups.optJSONObject(i) ?: continue
            val mbid = rg.optString("id", "").trim()
            if (!LookupNormalize.isValidMbid(mbid)) continue
            val title = rg.optString("title", "")
            val credits = rg.optJSONArray("artist-credit")
            val artist = if (credits != null && credits.length() > 0) {
                (0 until credits.length())
                    .mapNotNull { credits.optJSONObject(it)?.optString("name")?.takeIf(String::isNotBlank) }
                    .joinToString(", ")
            } else {
                ""
            }
            val year = rg.optString("first-release-date", "").take(4).ifBlank { null }
            val score = LookupNormalize.scoreArtworkCandidate(
                queryAlbum = query.album,
                queryArtist = query.artist,
                queryYear = query.year,
                candAlbum = title,
                candArtist = artist,
                candYear = year,
                sourceBonus = 2,
            )
            val base = "https://coverartarchive.org/release-group/$mbid"
            out += ArtworkCandidate(
                source = "CAA",
                title = title,
                artist = artist,
                year = year,
                thumbUrl = "$base/front-250",
                fullUrls = listOf("$base/front-1200", "$base/front-500", "$base/front"),
                score = score,
            )
        }
        return out
    }
}
