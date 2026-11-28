package dev.okarin.rustygrade

import android.app.Application
import android.content.Intent
import android.net.Uri
import androidx.lifecycle.AndroidViewModel
import androidx.compose.ui.graphics.Color
import androidx.lifecycle.viewModelScope
import dev.okarin.rustygrade.media.PlaybackHub
import dev.okarin.rustygrade.storage.BackupArchive
import dev.okarin.rustygrade.ui.theme.Orbit
import dev.okarin.rustygrade.ui.theme.colorOfRgb
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.distinctUntilChanged
import dev.okarin.rustygrade.media.PlaybackClient
import dev.okarin.rustygrade.storage.ArtworkProviders
import dev.okarin.rustygrade.storage.ArtworkQuery
import dev.okarin.rustygrade.storage.ContentResolverOpener
import dev.okarin.rustygrade.storage.LibraryScanner
import dev.okarin.rustygrade.storage.LyricQuery
import dev.okarin.rustygrade.storage.OnlineArtworkLookup
import dev.okarin.rustygrade.storage.OnlineLyricsLookup
import dev.okarin.rustygrade.storage.ReplayGainScanner
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicBoolean
import uniffi.rustygrade_ffi.BackupBundleDto
import uniffi.rustygrade_ffi.BackupPreviewDto
import uniffi.rustygrade_ffi.LibrarySnapshot
import uniffi.rustygrade_ffi.ScanSummary
import uniffi.rustygrade_ffi.SettingsSnapshot

sealed interface ScanUi {
    data object Idle : ScanUi
    data class Running(val phase: String, val scanned: UInt, val total: UInt) : ScanUi
    data class Done(val summary: ScanSummary) : ScanUi
    data class Failed(val message: String) : ScanUi
}

sealed interface BackupUi {
    data object Idle : BackupUi
    data object Busy : BackupUi
    data class Done(val message: String) : BackupUi
    data class Failed(val message: String) : BackupUi
}

sealed interface FetchUi {
    data object Idle : FetchUi
    data class Running(
        val kind: String,
        val done: Int,
        val total: Int,
        val found: Int,
        val detail: String = "",
    ) : FetchUi
    data class Done(val message: String) : FetchUi
    data class Failed(val message: String) : FetchUi
}

data class PendingBackupImport(
    val uri: Uri,
    val bundle: BackupBundleDto,
    val preview: BackupPreviewDto,
)

class LibraryViewModel(app: Application) : AndroidViewModel(app) {
    private val engine = (app as RustygradeApp).engine
    private val scanner = LibraryScanner(app, engine)

    /** Transport and queue commands to the media service (also binds, and so starts, it). */
    val playback = PlaybackClient(app).also { it.connect() }

    private val _library = MutableStateFlow<LibrarySnapshot?>(null)
    val library: StateFlow<LibrarySnapshot?> = _library

    private val _scan = MutableStateFlow<ScanUi>(ScanUi.Idle)
    val scan: StateFlow<ScanUi> = _scan

    private val _backupUi = MutableStateFlow<BackupUi>(BackupUi.Idle)
    val backupUi: StateFlow<BackupUi> = _backupUi

    private val _fetchUi = MutableStateFlow<FetchUi>(FetchUi.Idle)
    val fetchUi: StateFlow<FetchUi> = _fetchUi

    private val _pendingReplace = MutableStateFlow<PendingBackupImport?>(null)
    val pendingReplace: StateFlow<PendingBackupImport?> = _pendingReplace

    private val _collectionsRefreshTick = MutableStateFlow(0)
    val collectionsRefreshTick: StateFlow<Int> = _collectionsRefreshTick

    private val _settings = MutableStateFlow<SettingsSnapshot?>(null)
    val settings: StateFlow<SettingsSnapshot?> = _settings

    private var scanJob: Job? = null
    private var backupJob: Job? = null
    private var fetchJob: Job? = null

    /** Accent and glow for the whole UI: the playing cover's palette, or Orbit orange. */
    data class AccentColors(val accent: Color = Orbit.DefaultAccent, val glow: Color = Orbit.DefaultGlow)

    private val _accent = MutableStateFlow(AccentColors())
    val accent: StateFlow<AccentColors> = _accent
    // Cleared on the main thread (reload) while the palette collector fills it on IO.
    private val paletteCache = ConcurrentHashMap<String, AccentColors>()

    /**
     * Settings and library writes run one at a time, each followed by its own refresh, so a slow
     * earlier write can never publish a snapshot over a newer one. A [Mutex] is not reentrant:
     * nothing that takes this lock may be called while holding it ([reloadNow] does not).
     */
    private val stateLock = Mutex()

    init {
        reload()
        // Finished results are one-shot notices: they clear themselves, so navigating back to a
        // screen never shows a stale "DONE" again.
        autoClear(_scan, SCAN_RESULT_MS, ScanUi.Idle) { it is ScanUi.Done || it is ScanUi.Failed }
        autoClear(_fetchUi, NOTICE_MS, FetchUi.Idle) { it is FetchUi.Done || it is FetchUi.Failed }
        autoClear(_backupUi, NOTICE_MS, BackupUi.Idle) { it is BackupUi.Done || it is BackupUi.Failed }
        if (startupSweepDone.compareAndSet(false, true)) {
            viewModelScope.launch(Dispatchers.IO) {
                val res = runCatching { engine.deduplicateAndSweepCovers() }.getOrNull()
                if (res != null && (res.migratedAlbums > 0u || res.migratedPlaylists > 0u)) {
                    runCatching { stateLock.withLock { reloadNow() } }
                    if (res.migratedPlaylists > 0u) _collectionsRefreshTick.value += 1
                }
            }
        }
        // Dynamic Cover Accent: recompute only when the track's cover or the setting changes.
        viewModelScope.launch {
            combine(PlaybackHub.state, _settings) { state, settings ->
                val cover = state?.current?.let { it.fullPath ?: it.thumbPath }
                if (settings?.accentFromCover == false) null else cover
            }.distinctUntilChanged().collect { cover ->
                _accent.value = if (cover == null) AccentColors() else withContext(Dispatchers.IO) {
                    paletteCache.getOrPut(cover) {
                        val p = uniffi.rustygrade_ffi.coverPalette(cover)
                        AccentColors(colorOfRgb(p.accent.toLong()), colorOfRgb(p.glow.toLong()))
                    }
                }
            }
        }
    }

    /** Re-reads settings and the sorted library snapshot (cheap: one FFI call each). */
    fun reload() {
        // A cover that was rewritten keeps its path: its cached palette would be stale.
        paletteCache.clear()
        viewModelScope.launch {
            try {
                withContext(Dispatchers.IO) { stateLock.withLock { reloadNow() } }
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                reportFailure(e, "Could not load the library")
            }
        }
    }

    /** Shows a failed background write, unless a scan is running (its own state wins). */
    private fun reportFailure(e: Throwable, fallback: String) {
        if (_scan.value !is ScanUi.Running) _scan.value = ScanUi.Failed(e.message ?: fallback)
    }

    fun rescan() = startScan(beforeScan = null)

    fun resyncCovers() = startScan(resyncCovers = true, beforeScan = null)

    /** Saves the picked tree first, then scans: the scan must see the new folder. */
    fun addTreeAndRescan(uri: String) = startScan { engine.addSafTree(uri) }

    fun removeTreeAndRescan(uri: String) = startScan {
        engine.removeSafTree(uri)
        // The folder is gone from the library: give the persisted grant back to the system.
        runCatching {
            getApplication<Application>().contentResolver.releasePersistableUriPermission(
                Uri.parse(uri),
                Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_WRITE_URI_PERMISSION,
            )
        }
    }

    fun reportError(message: String) {
        _scan.value = ScanUi.Failed(message)
    }

    fun setSort(sort: String) = mutate("Could not change the sort order") { engine.setLibrarySort(sort) }

    fun setView(view: String) = mutate("Could not change the view") { engine.setLibraryView(view) }

    fun setAccentFromCover(enabled: Boolean) = mutate("Could not save the setting") { engine.setAccentFromCover(enabled) }

    fun setUseMediaStore(enabled: Boolean) = mutate("Could not save the setting") { engine.setUseMediaStore(enabled) }

    fun setScanOnStartup(enabled: Boolean) = mutate("Could not save the setting") { engine.setScanOnStartup(enabled) }

    fun setAutoReplayGain(enabled: Boolean) {
        val wasEnabled = _settings.value?.autoReplayGain == true
        viewModelScope.launch(Dispatchers.IO) {
            try {
                stateLock.withLock {
                    engine.setAutoReplayGain(enabled)
                    reloadNow()
                }
                if (!wasEnabled && enabled) {
                    // `fetchJob` is main-thread state: start the scan from there.
                    withContext(Dispatchers.Main) { scanReplayGain(forceAll = false, silentIfUpToDate = true) }
                }
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                reportFailure(e, "Could not save the setting")
            }
        }
    }

    fun setArtworkSources(itunes: Boolean, deezer: Boolean, musicbrainz: Boolean) =
        mutate("Could not save artwork sources") { engine.setArtworkSources(itunes, deezer, musicbrainz) }

    /** One serialized write followed by a refresh of the settings and library snapshots. */
    private fun mutate(failure: String, write: () -> Unit) {
        viewModelScope.launch(Dispatchers.IO) {
            try {
                stateLock.withLock {
                    write()
                    reloadNow()
                }
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                reportFailure(e, failure)
            }
        }
    }

    /** Removes whole albums, then refreshes the grid and the play queue. [onDone] gets the count. */
    fun deleteAlbums(keys: List<String>, onDone: (Int) -> Unit = {}) {
        viewModelScope.launch {
            val removed = try {
                withContext(Dispatchers.IO) {
                    stateLock.withLock { engine.deleteAlbums(keys).toInt().also { reloadNow() } }
                }
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                reportFailure(e, "Could not remove the albums")
                0
            }
            playback.syncLibrary()
            onDone(removed)
        }
    }

    /** Re-reads settings and library; the caller holds [stateLock] (or is a lone writer). */
    private fun reloadNow() {
        val s = engine.loadSettings()
        _settings.value = s
        _library.value = engine.loadLibrarySnapshot(s.librarySort)
    }

    /** Stops the running scan: cancels the coroutine (during the Kotlin-side walk) and the Rust scan. */
    fun cancelScan() {
        scanJob?.cancel()
        engine.cancelScan()
    }

    private fun startScan(resyncCovers: Boolean = false, beforeScan: (() -> Unit)?) {
        if (scanJob?.isActive == true) return
        _scan.value = ScanUi.Running("discovering", 0u, 0u)
        scanJob = viewModelScope.launch {
            _scan.value = try {
                if (beforeScan != null) withContext(Dispatchers.IO) { stateLock.withLock { beforeScan() } }
                val summary = scanner.scan(resyncCovers = resyncCovers) { e ->
                    _scan.value = ScanUi.Running(e.phase, e.scanned, e.total)
                }
                ScanUi.Done(summary)
            } catch (e: CancellationException) {
                _scan.value = ScanUi.Idle
                throw e
            } catch (e: Exception) {
                ScanUi.Failed(e.message ?: "Scan failed")
            }
            reload()
            _collectionsRefreshTick.value += 1 // post-scan remap may update saved favorites/playlists
            playback.syncLibrary() // vanished tracks leave the play queue
            if (_scan.value is ScanUi.Done && _settings.value?.autoReplayGain == true) {
                scanReplayGain(forceAll = false, silentIfUpToDate = true)
            }
        }
    }

    /** Exports collections, stats, settings and playlist covers into [uri] inside `viewModelScope`. */
    fun exportBackup(uri: Uri) {
        if (backupJob?.isActive == true) return
        _backupUi.value = BackupUi.Busy
        backupJob = viewModelScope.launch {
            _backupUi.value = try {
                val coverCount = withContext(Dispatchers.IO) {
                    val bundle = engine.exportBackup()
                    // "wt": a plain "w" may leave the tail of a larger file it overwrites (a corrupt ZIP).
                    getApplication<Application>().contentResolver.openOutputStream(uri, "wt")?.use {
                        BackupArchive.writeZip(it, bundle)
                    } ?: error("Could not write the backup file")
                    bundle.playlistCovers.size
                }
                val coverWord = if (coverCount == 1) "COVER FILE" else "COVER FILES"
                BackupUi.Done(
                    if (coverCount > 0) "EXPORTED BACKUP ZIP ($coverCount $coverWord)" else "EXPORTED BACKUP ZIP",
                )
            } catch (e: CancellationException) {
                _backupUi.value = BackupUi.Idle
                throw e
            } catch (e: Exception) {
                BackupUi.Failed((e.message ?: "EXPORT FAILED").uppercase())
            }
        }
    }

    /**
     * Reads and inspects a backup ZIP before confirming a destructive Replace import, surfacing
     * counts and single-playlist export warnings without re-reading the document twice.
     */
    fun inspectBackupForReplace(uri: Uri) {
        if (backupJob?.isActive == true) return
        _backupUi.value = BackupUi.Busy
        backupJob = viewModelScope.launch {
            try {
                val pending = withContext(Dispatchers.IO) {
                    val bundle = readBackupZip(uri)
                    val preview = engine.inspectBackupBundle(bundle)
                    PendingBackupImport(uri, bundle, preview)
                }
                _backupUi.value = BackupUi.Idle
                _pendingReplace.value = pending
            } catch (e: CancellationException) {
                _backupUi.value = BackupUi.Idle
                throw e
            } catch (e: Exception) {
                _backupUi.value = BackupUi.Failed((e.message ?: "IMPORT FAILED").uppercase())
            }
        }
    }

    fun dismissPendingReplace() {
        _pendingReplace.value = null
    }

    fun confirmPendingImport(replace: Boolean, sourceRoot: String?) {
        val pending = _pendingReplace.value ?: return
        _pendingReplace.value = null
        applyBackupBundle(replace, sourceRoot) { pending.bundle }
    }

    /**
     * Imports a backup ZIP inside `viewModelScope`, refreshing library, collections and playback
     * even if the user rotates or switches tabs mid-operation.
     */
    fun importBackup(uri: Uri, replace: Boolean, sourceRoot: String?) =
        applyBackupBundle(replace, sourceRoot) { withContext(Dispatchers.IO) { readBackupZip(uri) } }

    /** Runs one import job: [bundle] supplies the archive, then it is applied and everything refreshed. */
    private fun applyBackupBundle(replace: Boolean, sourceRoot: String?, bundle: suspend () -> BackupBundleDto) {
        if (backupJob?.isActive == true) return
        _backupUi.value = BackupUi.Busy
        val rootHint = sourceRoot?.trim()?.ifEmpty { null }
        backupJob = viewModelScope.launch {
            try {
                applyBundleSuspend(bundle(), replace, rootHint)
            } catch (e: CancellationException) {
                _backupUi.value = BackupUi.Idle
                throw e
            } catch (e: Exception) {
                _backupUi.value = BackupUi.Failed((e.message ?: "IMPORT FAILED").uppercase())
            }
        }
    }

    private fun readBackupZip(uri: Uri): BackupBundleDto = BackupArchive.readZip {
        getApplication<Application>().contentResolver.openInputStream(uri)
            ?: error("Could not open the selected archive")
    }

    private suspend fun applyBundleSuspend(bundle: BackupBundleDto, replace: Boolean, rootHint: String?) {
        val r = withContext(Dispatchers.IO) {
            stateLock.withLock { engine.importBackup(bundle, replace, rootHint) }
        }
        withContext(Dispatchers.IO) { stateLock.withLock { reloadNow() } }
        _collectionsRefreshTick.value += 1
        playback.refreshFx()
        playback.syncLibrary()
        _backupUi.value = BackupUi.Done(
            "IMPORTED // ${r.playlistCount} PLAYLISTS // ${r.likedCount} LIKED // ${r.statsTrackCount} STATS // " +
                "${r.tracksRemapped} REMAPPED, ${r.tracksExact} EXACT, ${r.tracksUnmatched} UNMATCHED",
        )
    }

    // ---- online metadata fetch & EBU R128 ReplayGain scanner ----------------------------------

    fun cancelFetch() {
        fetchJob?.cancel()
        fetchJob = null
        if (_fetchUi.value is FetchUi.Running) _fetchUi.value = FetchUi.Idle
    }

    /** Batch-fetches lyrics from LRCLIB / NetEase / KuGou / Lyrics.ovh for tracks lacking lyrics. */
    fun fetchMissingLyrics() {
        if (fetchJob?.isActive == true) return
        val app = getApplication<Application>()
        _fetchUi.value = FetchUi.Running("LYRICS", 0, 0, 0, "INSPECTING LIBRARY…")
        fetchJob = viewModelScope.launch {
            try {
                val allTracks = withContext(Dispatchers.IO) { engine.allTracks() }
                val opener = ContentResolverOpener(app.contentResolver)
                val missing = withContext(Dispatchers.IO) {
                    allTracks.filter { t ->
                        ensureActive()
                        runCatching { engine.lyrics(t.id, opener)?.lines?.isNotEmpty() == true }.getOrDefault(false).not()
                    }
                }
                if (missing.isEmpty()) {
                    _fetchUi.value = FetchUi.Done("ALL ${allTracks.size} TRACKS ALREADY HAVE LYRICS")
                    return@launch
                }
                var found = 0
                for ((idx, track) in missing.withIndex()) {
                    ensureActive()
                    _fetchUi.value = FetchUi.Running("LYRICS", idx, missing.size, found, track.title)
                    val best = OnlineLyricsLookup.fetchBest(LyricQuery.fromTrack(track))
                    if (best != null) {
                        val saved = withContext(Dispatchers.IO) {
                            runCatching { engine.saveTrackLyrics(track.id, best.text) != null }.getOrDefault(false)
                        }
                        if (saved) found++
                    }
                }
                _fetchUi.value = FetchUi.Done("LYRICS // $found/${missing.size} FETCHED & CACHED")
            } catch (e: CancellationException) {
                _fetchUi.value = FetchUi.Idle
                throw e
            } catch (e: Exception) {
                _fetchUi.value = FetchUi.Failed((e.message ?: "LYRICS FETCH FAILED").uppercase())
            }
        }
    }

    /** Batch-fetches covers from iTunes / Deezer / MusicBrainz for albums missing artwork. */
    fun fetchMissingCovers() {
        if (fetchJob?.isActive == true) return
        val app = getApplication<Application>()
        _fetchUi.value = FetchUi.Running("COVERS", 0, 0, 0, "INSPECTING ALBUMS…")
        fetchJob = viewModelScope.launch {
            try {
                val missingAlbums = withContext(Dispatchers.IO) {
                    engine.loadLibrarySnapshot("album").albums.filter { it.thumbPath == null && it.fullPath == null }
                }
                if (missingAlbums.isEmpty()) {
                    _fetchUi.value = FetchUi.Done("ALL ALBUMS ALREADY HAVE COVERS")
                    return@launch
                }
                val s = _settings.value
                val providers = ArtworkProviders(
                    itunes = s?.artworkItunes ?: true,
                    deezer = s?.artworkDeezer ?: true,
                    musicbrainz = s?.artworkMusicbrainz ?: true,
                )
                var found = 0
                for ((idx, album) in missingAlbums.withIndex()) {
                    ensureActive()
                    _fetchUi.value = FetchUi.Running("COVERS", idx, missingAlbums.size, found, album.name)
                    val firstTitle = withContext(Dispatchers.IO) {
                        runCatching { engine.getAlbumDetail(album.key)?.tracks?.firstOrNull()?.title }.getOrNull().orEmpty()
                    }
                    val file = OnlineArtworkLookup.fetchBestToFile(
                        ArtworkQuery(
                            album = album.name,
                            artist = album.artist,
                            trackTitle = firstTitle,
                            year = album.year,
                            providers = providers,
                        ),
                        app.cacheDir,
                    )
                    if (file != null) {
                        val applied = withContext(Dispatchers.IO) {
                            try {
                                stateLock.withLock {
                                    engine.applyAlbumCover(album.key, file.absolutePath) != null
                                }
                            } finally {
                                file.delete()
                            }
                        }
                        if (applied) found++
                    }
                }
                if (found > 0) reload()
                _fetchUi.value = FetchUi.Done("COVERS // $found/${missingAlbums.size} ALBUMS UPDATED")
            } catch (e: CancellationException) {
                _fetchUi.value = FetchUi.Idle
                throw e
            } catch (e: Exception) {
                _fetchUi.value = FetchUi.Failed((e.message ?: "COVER FETCH FAILED").uppercase())
            }
        }
    }

    /**
     * Scans tracks with our in-process EBU R128 analyzer (`-18.0 LUFS` ReplayGain 2.0) and saves
     * per-track + pooled album gain/peak directly into `library.json`.
     */
    fun scanReplayGain(forceAll: Boolean = false, silentIfUpToDate: Boolean = false) {
        if (fetchJob?.isActive == true) return
        val app = getApplication<Application>()
        if (!silentIfUpToDate) {
            _fetchUi.value = FetchUi.Running("REPLAYGAIN", 0, 0, 0, "COLLECTING TRACKS…")
        }
        fetchJob = viewModelScope.launch {
            try {
                val allTracks = withContext(Dispatchers.IO) { engine.allTracks() }
                val targets = if (forceAll) {
                    allTracks
                } else {
                    allTracks.filter { it.replayGainTrackDb == null || it.replayGainAlbumDb == null }
                }
                if (targets.isEmpty()) {
                    if (!silentIfUpToDate) {
                        _fetchUi.value = FetchUi.Done("ALL ${allTracks.size} TRACKS ALREADY HAVE REPLAYGAIN")
                    }
                    return@launch
                }
                val rgScanner = ReplayGainScanner(app, engine)
                val summary = rgScanner.scanTracks(targets) { done, total, current ->
                    _fetchUi.value = FetchUi.Running("REPLAYGAIN", done, total, done, current)
                }
                reload()
                playback.syncLibrary()
                playback.refreshFx()
                val skippedText = if (summary.skipped > 0) " (${summary.skipped} SKIPPED)" else ""
                _fetchUi.value = FetchUi.Done(
                    "REPLAYGAIN // ${summary.scanned} TRACKS // ${summary.albums} ALBUMS$skippedText",
                )
            } catch (e: CancellationException) {
                _fetchUi.value = FetchUi.Idle
                throw e
            } catch (e: Exception) {
                _fetchUi.value = FetchUi.Failed((e.message ?: "REPLAYGAIN SCAN FAILED").uppercase())
            }
        }
    }

    /** Scans EBU R128 ReplayGain for a single album (called from AlbumDetailScreen). */
    fun scanAlbumReplayGain(albumKey: String, onDone: (String) -> Unit = {}) {
        if (fetchJob?.isActive == true) {
            onDone("ANOTHER FETCH/SCAN TASK IS RUNNING")
            return
        }
        val app = getApplication<Application>()
        _fetchUi.value = FetchUi.Running("REPLAYGAIN", 0, 0, 0, albumKey)
        fetchJob = viewModelScope.launch {
            try {
                val tracks = withContext(Dispatchers.IO) {
                    engine.getAlbumDetail(albumKey)?.tracks.orEmpty()
                }
                if (tracks.isEmpty()) {
                    _fetchUi.value = FetchUi.Idle
                    onDone("NO TRACKS TO SCAN")
                    return@launch
                }
                val rgScanner = ReplayGainScanner(app, engine)
                val summary = rgScanner.scanTracks(tracks) { done, total, current ->
                    _fetchUi.value = FetchUi.Running("REPLAYGAIN", done, total, done, current)
                }
                reload()
                playback.syncLibrary()
                playback.refreshFx()
                val msg = "REPLAYGAIN SCANNED (${summary.scanned}/${tracks.size} TRACKS)"
                _fetchUi.value = FetchUi.Done(msg)
                onDone(msg)
            } catch (e: CancellationException) {
                _fetchUi.value = FetchUi.Idle
                throw e
            } catch (e: Exception) {
                val msg = (e.message ?: "REPLAYGAIN SCAN FAILED").uppercase()
                _fetchUi.value = FetchUi.Failed(msg)
                onDone(msg)
            }
        }
    }

    // ---- audio settings (the service re-reads them on REFRESH_FX) -----------------------------

    fun setEq(enabled: Boolean, preset: String) = changeAudio {
        val gains = uniffi.rustygrade_ffi.eqPresetGains(preset) ?: engine.loadSettings().eqGains
        engine.setEq(enabled, preset, gains)
    }

    fun setReplayGain(mode: String) = changeAudio { engine.setReplayGain(mode) }

    fun setCrossfadeSeconds(seconds: Int) = changeAudio { engine.setCrossfadeSeconds(seconds) }

    fun setUsbBitPerfect(enabled: Boolean) = changeAudio { engine.setUsbBitPerfect(enabled) }

    private fun changeAudio(write: () -> Unit) {
        viewModelScope.launch(Dispatchers.IO) {
            try {
                stateLock.withLock {
                    write()
                    _settings.value = engine.loadSettings()
                }
                playback.refreshFx()
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                reportFailure(e, "Could not save the audio setting")
            }
        }
    }

    override fun onCleared() {
        playback.release()
        engine.cancelScan()
    }

    private fun <T> autoClear(flow: MutableStateFlow<T>, afterMs: Long, idle: T, finished: (T) -> Boolean) {
        viewModelScope.launch {
            flow.collectLatest { state ->
                if (finished(state)) {
                    delay(afterMs)
                    flow.compareAndSet(state, idle)
                }
            }
        }
    }

    /** Hides the last scan's result now (the user closed it). A running scan is left alone. */
    fun dismissScanResult() {
        val s = _scan.value
        if (s is ScanUi.Done || s is ScanUi.Failed) _scan.compareAndSet(s, ScanUi.Idle)
    }

    companion object {
        private val startupSweepDone = AtomicBoolean(false)
        private val startupScanClaimed = AtomicBoolean(false)

        /** True exactly once per process: "scan on startup" means app start, not every new Activity. */
        fun claimStartupScan(): Boolean = startupScanClaimed.compareAndSet(false, true)

        private const val SCAN_RESULT_MS = 10_000L
        private const val NOTICE_MS = 5_000L
    }
}
