package dev.okarin.rustygrade

import android.app.Application
import android.content.Context
import android.content.IntentSender
import android.net.Uri
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import dev.okarin.rustygrade.media.CoverProvider
import dev.okarin.rustygrade.media.PlaybackClient
import dev.okarin.rustygrade.storage.ContentResolverOpener
import dev.okarin.rustygrade.storage.TagFileWriter
import dev.okarin.rustygrade.ui.nav.TagTarget
import dev.okarin.rustygrade.ui.screens.TagForm
import dev.okarin.rustygrade.ui.screens.TagLoad
import dev.okarin.rustygrade.ui.screens.hasChanges
import java.io.File
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import uniffi.rustygrade_ffi.RustygradeEngine
import uniffi.rustygrade_ffi.TagEditInfo
import uniffi.rustygrade_ffi.TrackInfo

/**
 * Holds the active tag-editing session across configuration changes (screen rotation while the
 * system `createWriteRequest` sheet is showing or while files are being rewritten).
 */
class TagEditorViewModel(app: Application) : AndroidViewModel(app) {
    sealed interface LoadState {
        data object Loading : LoadState
        data object Missing : LoadState
        class Ready(val data: TagLoad, val form: TagForm) : LoadState
    }

    data class WriteCompleted(val target: TagTarget, val renamedAlbum: String?)

    private val engine: RustygradeEngine = (app as RustygradeApp).engine

    var currentTarget: TagTarget? by mutableStateOf(null)
        private set
    var loadState: LoadState by mutableStateOf(LoadState.Loading)
        private set
    var coverFile: File? by mutableStateOf(null)
        private set
    var pickingCover: Boolean by mutableStateOf(false)
        private set
    var removeCover: Boolean by mutableStateOf(false)
        private set
    var saving: Boolean by mutableStateOf(false)
        private set
    var progress: Int by mutableIntStateOf(0)
        private set
    var updated: Int? by mutableStateOf(null)
        private set
    var failures: List<Pair<TrackInfo, String>> by mutableStateOf(emptyList())
        private set
    var error: String? by mutableStateOf(null)
        private set
    var pendingEdit: TagEditInfo? by mutableStateOf(null)
        private set
    var pendingWriteRequest: IntentSender? by mutableStateOf(null)
        private set
    var reloadTick: Int by mutableIntStateOf(0)
        private set
    var completedEvent: WriteCompleted? by mutableStateOf(null)
        private set

    private var loadJob: Job? = null
    private var writeJob: Job? = null

    fun ensureLoaded(target: TagTarget) {
        if (currentTarget == target && (loadState !is LoadState.Missing || saving)) {
            return
        }
        if (saving && currentTarget != null) {
            return
        }
        clearSession()
        currentTarget = target
        loadState = LoadState.Loading
        loadJob = viewModelScope.launch {
            val loaded = withContext(Dispatchers.IO) {
                runCatching { loadTagTarget(engine, target) }.getOrNull()
            }
            if (currentTarget != target) return@launch
            loadState = if (loaded == null) {
                LoadState.Missing
            } else {
                LoadState.Ready(loaded, TagForm(loaded.tracks, loaded.album, loaded.lyrics))
            }
        }
    }

    fun pickCover(uri: Uri) {
        if (saving || pickingCover) return
        val app = getApplication<Application>()
        pickingCover = true
        viewModelScope.launch {
            try {
                val file = withContext(Dispatchers.IO) { copyPickedCover(app, uri) }
                if (saving || currentTarget == null) {
                    discard(file)
                    return@launch
                }
                discard(coverFile)
                coverFile = file
                removeCover = false
                error = null
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                error = (e.message ?: "COULD NOT READ THE IMAGE").uppercase()
            } finally {
                pickingCover = false
            }
        }
    }

    fun markRemoveCover() {
        if (saving || pickingCover) return
        discard(coverFile)
        coverFile = null
        removeCover = true
    }

    /** Applies an online-fetched cover file downloaded by the Cover Picker sheet. */
    fun setFetchedCover(file: File) {
        if (saving || pickingCover || currentTarget == null) {
            discard(file)
            return
        }
        discard(coverFile)
        coverFile = file
        removeCover = false
        error = null
    }

    fun beginSave(playback: PlaybackClient) {
        val target = currentTarget ?: return
        val ready = loadState as? LoadState.Ready ?: return
        if (saving || pickingCover) return

        error = null
        updated = null
        failures = emptyList()
        val edit = ready.form.toEdit(target is TagTarget.Album, coverFile?.absolutePath, removeCover)
        if (!edit.hasChanges()) {
            error = "NOTHING CHANGED"
            return
        }
        saving = true
        progress = 0
        pendingEdit = edit
        viewModelScope.launch {
            try {
                val writer = TagFileWriter(getApplication(), engine, playback)
                val sender = try {
                    withContext(Dispatchers.IO) { writer.writeAccessRequest(ready.data.tracks) }
                } catch (e: CancellationException) {
                    throw e
                } catch (_: Exception) {
                    null
                }
                if (sender != null) {
                    pendingWriteRequest = sender
                } else {
                    launchWrite(ready.data.tracks, edit, playback)
                }
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                // Whatever went wrong, the screen must not stay stuck in "saving".
                saving = false
                pendingEdit = null
                pendingWriteRequest = null
                error = (e.message ?: "SAVE FAILED").uppercase()
            }
        }
    }

    fun consumeWriteRequest() {
        pendingWriteRequest = null
    }

    fun onAccessDenied() {
        saving = false
        pendingEdit = null
        pendingWriteRequest = null
        error = "WRITE ACCESS DENIED. NOTHING WAS CHANGED"
    }

    fun onSessionResetAfterGrant() {
        saving = false
        pendingEdit = null
        pendingWriteRequest = null
        error = "SESSION RESET BY SYSTEM — TAP SAVE AGAIN (ACCESS GRANTED)"
    }

    fun launchWrite(tracks: List<TrackInfo>, edit: TagEditInfo, playback: PlaybackClient) {
        if (writeJob?.isActive == true) return
        val activeTarget = currentTarget
        if (activeTarget == null) {
            // The session was cleared while the access request was open: nothing to write, not saving.
            saving = false
            pendingEdit = null
            pendingWriteRequest = null
            return
        }
        val writer = TagFileWriter(getApplication(), engine, playback)
        saving = true
        writeJob = viewModelScope.launch {
            progress = 0
            var written = 0
            val failed = ArrayList<Pair<TrackInfo, String>>()
            try {
                val report = writer.write(tracks, edit) { progress = it }
                written = report.written
                failed += report.failures
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                error = (e.message ?: "WRITE FAILED").uppercase()
            } finally {
                pendingEdit = null
                pendingWriteRequest = null
            }
            // The picked cover is kept after a failure, so the user can retry without picking it again.
            if (failed.isEmpty() && error == null) {
                if (edit.lyrics != null) {
                    withContext(Dispatchers.IO) {
                        tracks.forEach { engine.clearCachedLyrics(it.id) }
                    }
                }
                discard(coverFile)
                coverFile = null
            }
            saving = false
            updated = written
            failures = failed
            reloadTick += 1
            if (failed.isEmpty() && error == null) {
                completedEvent = WriteCompleted(activeTarget, edit.album)
            }
        }
    }

    fun consumeCompleted() {
        completedEvent = null
    }

    fun clearSession() {
        if (saving) return
        loadJob?.cancel()
        loadJob = null
        discard(coverFile)
        coverFile = null
        removeCover = false
        progress = 0
        updated = null
        failures = emptyList()
        error = null
        pendingEdit = null
        pendingWriteRequest = null
        completedEvent = null
        currentTarget = null
        loadState = LoadState.Loading
    }

    override fun onCleared() {
        loadJob?.cancel()
        // `viewModelScope` is already cancelled here, so this one delete cannot be launched.
        coverFile?.delete()
        coverFile = null
    }

    /** Deletes a temp cover off the main thread. */
    private fun discard(file: File?) {
        if (file == null) return
        viewModelScope.launch(Dispatchers.IO) { file.delete() }
    }

    private fun loadTagTarget(engine: RustygradeEngine, target: TagTarget): TagLoad? = when (target) {
        is TagTarget.Track -> engine.getTracks(listOf(target.id)).takeIf { it.isNotEmpty() }
            ?.let {
                val opener = ContentResolverOpener(getApplication<Application>().contentResolver)
                val raw = runCatching { engine.rawLyrics(it[0].id, opener) }.getOrNull().orEmpty()
                TagLoad(it, null, it[0].fullPath ?: it[0].thumbPath, raw)
            }
        is TagTarget.Album -> engine.getAlbumDetail(target.key)?.takeIf { it.tracks.isNotEmpty() }
            ?.let { TagLoad(it.tracks, it.album, it.album.fullPath ?: it.album.thumbPath) }
    }

    /** Copies a picked image into the cache dir (the tag writer needs a real jpg/png/webp file path). */
    private fun copyPickedCover(context: Context, uri: Uri): File =
        CoverProvider.copyPickedImage(context, uri, context.cacheDir, "tag-cover-")
}
