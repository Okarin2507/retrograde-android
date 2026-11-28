package dev.okarin.rustygrade.media

import android.content.Context
import android.net.Uri
import androidx.media3.common.MediaItem
import androidx.media3.common.MediaMetadata
import java.io.File
import uniffi.rustygrade_ffi.AlbumSummary
import uniffi.rustygrade_ffi.TrackInfo

/**
 * Cover files as `content://` URIs another process can load (`file://` inside the private app
 * directory is unreadable for Android Auto and the system media UI).
 */
object ArtworkUris {
    // Plain values only: holding a Context in a static field leaks it. Written once at startup on
    // the main thread, read from any thread.
    @Volatile
    private var coversDir: File? = null

    @Volatile
    private var authority: String? = null

    fun init(app: Context) {
        val appContext = app.applicationContext
        coversDir = runCatching { File(appContext.filesDir, CoverProvider.COVERS_DIR).canonicalFile }.getOrNull()
        authority = CoverProvider.authority(appContext.packageName)
    }

    fun of(path: String?): Uri? {
        if (path == null) return null
        val file = File(path)
        val covers = coversDir
        val authority = authority
        if (covers == null || authority == null) return Uri.fromFile(file)
        // Only plain files directly in the cover cache are served; anything else stays a file URI.
        val parent = file.parentFile?.let { runCatching { it.canonicalFile }.getOrNull() }
        return if (parent == covers) {
            Uri.Builder().scheme("content").authority(authority).appendPath(file.name).build()
        } else {
            Uri.fromFile(file)
        }
    }
}

/**
 * `mediaId` is the track's `file_path`: that is what the Rust queue matches when the player moves
 * on by itself (`on_player_advanced`). The playable URI is `file://` or the SAF `content://`.
 */
fun TrackInfo.toMediaItem(): MediaItem = MediaItem.Builder()
    .setMediaId(filePath)
    .setUri(playUri)
    .setMediaMetadata(
        MediaMetadata.Builder()
            .setTitle(title)
            .setArtist(artist)
            .setAlbumTitle(album)
            .setTrackNumber(trackNumber.takeIf { it > 0 })
            .setDiscNumber(discNumber)
            .setDurationMs((durationSecs * 1000).toLong())
            .setArtworkUri(ArtworkUris.of(thumbPath))
            .setIsPlayable(true)
            .setIsBrowsable(false)
            .build(),
    )
    .build()

/** Browse-tree ids for Android Auto and other controllers. */
object BrowseIds {
    const val ROOT = "root"
    private const val ALBUM = "album:"
    private const val TRACK = "track:"

    fun album(key: String) = ALBUM + key
    fun track(albumKey: String, index: Int) = "$TRACK$index:$albumKey"

    fun albumKey(mediaId: String): String? = mediaId.takeIf { it.startsWith(ALBUM) }?.removePrefix(ALBUM)

    /** `track:<index>:<album key>` -> (album key, index). */
    fun trackRef(mediaId: String): Pair<String, Int>? {
        if (!mediaId.startsWith(TRACK)) return null
        val rest = mediaId.removePrefix(TRACK)
        val sep = rest.indexOf(':')
        if (sep <= 0) return null
        val index = rest.substring(0, sep).toIntOrNull() ?: return null
        return rest.substring(sep + 1) to index
    }
}

fun AlbumSummary.toBrowseItem(): MediaItem = MediaItem.Builder()
    .setMediaId(BrowseIds.album(key))
    .setMediaMetadata(
        MediaMetadata.Builder()
            .setTitle(name)
            .setArtist(artist)
            .setArtworkUri(ArtworkUris.of(thumbPath))
            .setIsBrowsable(true)
            .setIsPlayable(false)
            .setMediaType(MediaMetadata.MEDIA_TYPE_ALBUM)
            .build(),
    )
    .build()

fun TrackInfo.toBrowseItem(albumKey: String, index: Int): MediaItem =
    toMediaItem().buildUpon().setMediaId(BrowseIds.track(albumKey, index)).build()
