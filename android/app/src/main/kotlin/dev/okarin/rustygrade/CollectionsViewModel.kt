package dev.okarin.rustygrade

import android.app.Application
import android.net.Uri
import android.provider.DocumentsContract
import android.provider.OpenableColumns
import android.widget.Toast
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import dev.okarin.rustygrade.storage.BackupArchive
import dev.okarin.rustygrade.storage.PlaylistFiles
import dev.okarin.rustygrade.storage.PlaylistImportKind
import dev.okarin.rustygrade.ui.nav.PlaylistKind
import java.io.OutputStream
import java.io.PushbackInputStream
import java.nio.charset.StandardCharsets
import java.util.Locale
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import uniffi.rustygrade_ffi.CollectionsSnapshot
import uniffi.rustygrade_ffi.PlaylistInfo
import uniffi.rustygrade_ffi.SmartPlaylistInfo

sealed interface PlaylistIoUi {
    data object Idle : PlaylistIoUi
    data class Busy(val message: String) : PlaylistIoUi
    data class Done(val message: String) : PlaylistIoUi
    data class Failed(val message: String) : PlaylistIoUi
}

/**
 * Favorites, playlists and smart playlists. Every action runs on IO, then re-reads the snapshot,
 * so [collections] and [liked] are always what is on disk.
 */
class CollectionsViewModel(app: Application) : AndroidViewModel(app) {
    private val engine = (app as RustygradeApp).engine

    private val _collections = MutableStateFlow<CollectionsSnapshot?>(null)
    val collections: StateFlow<CollectionsSnapshot?> = _collections

    /** One write+refresh at a time, so a slow refresh can never overwrite a newer one. */
    private val order = Mutex()

    private val _liked = MutableStateFlow<Set<String>>(emptySet())
    val liked: StateFlow<Set<String>> = _liked

    private val _ioUi = MutableStateFlow<PlaylistIoUi>(PlaylistIoUi.Idle)
    val ioUi: StateFlow<PlaylistIoUi> = _ioUi

    private var importJob: Job? = null
    private var exportJob: Job? = null

    init {
        reload()
        // Import/export results clear themselves instead of staying up until dismissed.
        viewModelScope.launch {
            _ioUi.collectLatest { state ->
                if (state is PlaylistIoUi.Done || state is PlaylistIoUi.Failed) {
                    delay(IO_NOTICE_MS)
                    _ioUi.compareAndSet(state, PlaylistIoUi.Idle)
                }
            }
        }
    }

    fun clearIoStatus() {
        if (_ioUi.value !is PlaylistIoUi.Busy) {
            _ioUi.value = PlaylistIoUi.Idle
        }
    }

    fun reload() {
        viewModelScope.launch(Dispatchers.IO) { order.withLock { attempt { refresh() } } }
    }

    private fun refresh() {
        val snapshot = engine.loadCollections()
        _collections.value = snapshot
        _liked.value = snapshot.liked.toSet()
    }

    /**
     * Runs [write], then refreshes. [onMain] only runs when the write succeeded (its results, such as
     * a created playlist, are not there otherwise); [onFailure] runs when it did not.
     */
    private fun act(onMain: (() -> Unit)? = null, onFailure: (() -> Unit)? = null, write: () -> Unit) {
        viewModelScope.launch {
            val ok = kotlinx.coroutines.withContext(Dispatchers.IO) {
                order.withLock {
                    // A failing engine call must not take the app down; the next refresh shows the truth.
                    val ok = attempt { write() }
                    attempt { refresh() }
                    ok
                }
            }
            if (ok) onMain?.invoke() else onFailure?.invoke()
        }
    }

    /** Like `runCatching { }.isSuccess`, but a cancellation still cancels. */
    private inline fun attempt(block: () -> Unit): Boolean = try {
        block()
        true
    } catch (e: kotlinx.coroutines.CancellationException) {
        throw e
    } catch (_: Throwable) {
        false
    }

    fun toggleLiked(trackId: String) = setLiked(trackId, trackId !in _liked.value)

    fun setLiked(trackId: String, liked: Boolean) {
        // Optimistic: the heart flips at once, the snapshot confirms right after.
        _liked.value = if (liked) _liked.value + trackId else _liked.value - trackId
        act { engine.setLiked(trackId, liked) }
    }

    fun createPlaylist(name: String, onDone: (PlaylistInfo) -> Unit = {}) {
        var created: PlaylistInfo? = null
        act(onMain = { created?.let(onDone) }) { created = engine.createPlaylist(name) }
    }

    fun renamePlaylist(id: Long, name: String) = act { engine.renamePlaylist(id, name) }

    fun deletePlaylist(id: Long, onDone: () -> Unit = {}) = act(onDone) { engine.deletePlaylist(id) }

    /** [onDone] gets how many tracks were new to the playlist. */
    fun addToPlaylist(id: Long, trackIds: List<String>, onDone: (Int) -> Unit = {}) {
        var added = 0u
        act(onMain = { onDone(added.toInt()) }) { added = engine.addToPlaylist(id, trackIds) }
    }

    fun removeFromPlaylist(id: Long, trackId: String) = act { engine.removeFromPlaylist(id, trackId) }

    fun movePlaylistTrack(id: Long, from: Int, to: Int) =
        act { engine.reorderPlaylistTrack(id, from.toUInt(), to.toUInt()) }

    fun moveLikedTrack(from: Int, to: Int) =
        act { engine.reorderLikedTrack(from.toUInt(), to.toUInt()) }

    /** `path == null` removes the cover. [onDone] gets an error message, or null on success. */
    fun setPlaylistCover(id: Long, path: String?, deleteSourceAfter: Boolean = false, onDone: (String?) -> Unit = {}) {
        var error: String? = null
        act(onFailure = { onDone(error ?: "Could not set the cover") }, onMain = { onDone(error) }) {
            try {
                engine.setPlaylistCover(id, path)
            } catch (e: Exception) {
                error = e.message ?: "Could not set the cover"
            } finally {
                // A temp copy of the picked image: removed even if the screen is already gone.
                if (deleteSourceAfter && path != null) java.io.File(path).delete()
            }
        }
    }

    /** Creates (`id == 0`) or updates a smart playlist; [onDone] gets its id. */
    fun saveSmart(playlist: SmartPlaylistInfo, onDone: (Long) -> Unit = {}) {
        var id = 0L
        act(onMain = { onDone(id) }) { id = engine.saveSmartPlaylist(playlist) }
    }

    fun deleteSmart(id: Long, onDone: () -> Unit = {}) = act(onDone) { engine.deleteSmartPlaylist(id) }

    /**
     * Imports a standalone `.m3u` / `.m3u8` playlist or playlist `.zip` archive (collections only;
     * stats and settings are never modified from the Playlists tab).
     */
    fun importPlaylistFile(uri: Uri) {
        if (importJob?.isActive == true) return
        _ioUi.value = PlaylistIoUi.Busy("IMPORTING PLAYLIST…")
        importJob = viewModelScope.launch {
            val app = getApplication<Application>()
            try {
                val message = withContext(Dispatchers.IO) {
                    val resolver = app.contentResolver
                    val displayName = resolveDisplayName(app, uri)
                    val mimeType = runCatching { resolver.getType(uri) }.getOrNull()
                    val raw = resolver.openInputStream(uri) ?: error("Could not open the selected file")
                    PushbackInputStream(raw, 32).use { pushback ->
                        val header = ByteArray(32)
                        var headerLen = 0
                        while (headerLen < header.size) {
                            val n = pushback.read(header, headerLen, header.size - headerLen)
                            if (n < 0) break
                            headerLen += n
                        }
                        val kind = PlaylistFiles.detectImportKind(displayName, mimeType, header, headerLen)
                            ?: error("Unsupported file type. Choose a .m3u8, .m3u, or .zip file.")
                        if (headerLen > 0) {
                            pushback.unread(header, 0, headerLen)
                        }

                        when (kind) {
                            PlaylistImportKind.M3U -> {
                                val isLegacyM3u = displayName?.trim()?.lowercase(Locale.ROOT)?.endsWith(".m3u") == true
                                val content = PlaylistFiles.readBoundedText(pushback, allowWindows1252Fallback = isLegacyM3u)
                                val r = order.withLock {
                                    engine.importM3uPlaylist(content, displayName, null).also { refresh() }
                                }
                                val matched = r.matchedExact + r.matchedRemapped + r.matchedMetadata
                                "IMPORTED \"${r.playlistName}\" // $matched MATCHED " +
                                    "(${r.matchedExact} EXACT, ${r.matchedRemapped} REMAPPED, ${r.matchedMetadata} METADATA) · " +
                                    "${r.unmatched} UNMATCHED"
                            }
                            PlaylistImportKind.ZIP -> {
                                val bundle = BackupArchive.readZip(pushback) {
                                    resolver.openInputStream(uri) ?: error("Could not reopen the selected archive")
                                }
                                require(bundle.collectionsJson != null) {
                                    "Archive does not contain playlists (native-collections.json)"
                                }
                                val collectionsOnly = bundle.copy(statsJson = null, settingsJson = null)
                                val r = order.withLock {
                                    engine.importBackup(collectionsOnly, false, null).also { refresh() }
                                }
                                val coversNote = if (r.coversRestored > 0u) ", ${r.coversRestored} COVERS" else ""
                                "IMPORTED ZIP // ${r.playlistCount} PLAYLISTS · " +
                                    "${r.tracksExact} EXACT, ${r.tracksRemapped} REMAPPED, ${r.tracksUnmatched} UNMATCHED$coversNote"
                            }
                        }
                    }
                }
                _ioUi.value = PlaylistIoUi.Done(message)
            } catch (e: CancellationException) {
                _ioUi.value = PlaylistIoUi.Idle
                throw e
            } catch (e: Exception) {
                val msg = (e.message ?: "Playlist import failed").uppercase(Locale.ROOT)
                _ioUi.value = PlaylistIoUi.Failed(msg)
            }
        }
    }

    /** Exports Favorites, a custom playlist, or an evaluated smart playlist to an M3U8 file. */
    fun exportPlaylistM3u8(kind: PlaylistKind, uri: Uri) {
        if (exportJob?.isActive == true) return
        exportJob = viewModelScope.launch {
            val app = getApplication<Application>()
            try {
                val count = withContext(Dispatchers.IO) {
                    val exported = when (kind) {
                        PlaylistKind.Favorites -> engine.exportFavoritesM3u8()
                        is PlaylistKind.Custom -> engine.exportPlaylistM3u8(kind.id)
                        is PlaylistKind.Smart -> engine.exportSmartPlaylistM3u8(kind.id)
                    }
                    openTruncatingOutputStream(app, uri).use { out ->
                        out.write(exported.content.toByteArray(StandardCharsets.UTF_8))
                    }
                    exported.trackCount
                }
                val unit = if (count == 1u) "track" else "tracks"
                Toast.makeText(app, "Exported $count $unit to M3U8", Toast.LENGTH_SHORT).show()
            } catch (e: CancellationException) {
                withContext(NonCancellable + Dispatchers.IO) { deleteSafDocument(app, uri) }
                throw e
            } catch (e: Exception) {
                withContext(Dispatchers.IO) { deleteSafDocument(app, uri) }
                Toast.makeText(app, e.message ?: "Could not export M3U8", Toast.LENGTH_LONG).show()
            }
        }
    }

    /** Exports a single custom playlist and its cover(s) as a desktop-compatible backup ZIP. */
    fun exportPlaylistZip(playlistId: Long, uri: Uri) {
        if (exportJob?.isActive == true) return
        exportJob = viewModelScope.launch {
            val app = getApplication<Application>()
            try {
                val coverCount = withContext(Dispatchers.IO) {
                    val bundle = engine.exportPlaylistBackup(playlistId)
                    openTruncatingOutputStream(app, uri).use { out ->
                        BackupArchive.writeZip(out, bundle)
                    }
                    bundle.playlistCovers.size
                }
                val coverLabel = if (coverCount == 1) "1 cover" else "$coverCount covers"
                val msg = if (coverCount > 0) "Exported playlist ZIP ($coverLabel)" else "Exported playlist ZIP"
                Toast.makeText(app, msg, Toast.LENGTH_SHORT).show()
            } catch (e: CancellationException) {
                withContext(NonCancellable + Dispatchers.IO) { deleteSafDocument(app, uri) }
                throw e
            } catch (e: Exception) {
                withContext(Dispatchers.IO) { deleteSafDocument(app, uri) }
                Toast.makeText(app, e.message ?: "Could not export playlist ZIP", Toast.LENGTH_LONG).show()
            }
        }
    }

    private fun resolveDisplayName(app: Application, uri: Uri): String? =
        runCatching {
            app.contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { cursor ->
                val idx = cursor.getColumnIndex(OpenableColumns.DISPLAY_NAME)
                if (idx >= 0 && cursor.moveToFirst()) cursor.getString(idx) else null
            }
        }.getOrNull()?.takeIf { it.isNotBlank() }
            ?: uri.lastPathSegment?.substringAfterLast('/')?.takeIf { it.isNotBlank() }

    private fun openTruncatingOutputStream(app: Application, uri: Uri): OutputStream =
        app.contentResolver.openOutputStream(uri, "wt")
            ?: error("Could not write the export file")

    private fun deleteSafDocument(app: Application, uri: Uri) {
        runCatching { DocumentsContract.deleteDocument(app.contentResolver, uri) }
    }

    private companion object {
        const val IO_NOTICE_MS = 5_000L
    }
}
