//! `RustygradeEngine`: the single object Kotlin talks to for library, scanning, collections,
//! stats and settings. Every call returns a coarse snapshot so Compose never crosses the FFI per row.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use rustygrade_core::library::LibraryStore;
use rustygrade_core::settings::{
    KEY_SAF_TREES, KEY_USB_BIT_PERFECT, KEY_USE_MEDIA_STORE,
    normalize_replay_gain, sanitize_eq_gains,
};
use rustygrade_core::{
    Album, AppSettings, ArtistSummary, CollectionsStore, CoverPaths, FileOpener, LibrarySort,
    PlayStats, PlaylistCovers, SafFile, ScanOptions, ScanProgress, ScanSources, Scanner,
    TrackIndex, TrackLookup, build_track_index, canonical_library_sort, contains_folded,
    deduplicate_and_sweep_covers as core_deduplicate_and_sweep_covers, delete_unreferenced_covers,
    ensure_default_smart_playlists, fold, migrate_playlist_covers, search_albums, search_artists,
    sort_library,
};
use serde_json::Value;

use crate::dto::*;
use crate::error::EngineError;

mod fd_opener;
mod state;
#[cfg(test)]
mod tests;

pub(crate) use fd_opener::FdOpener;

const MAX_TRACK_HITS: usize = 200;

/// Receives scan progress. Called from a worker thread (not the caller's thread).
#[uniffi::export(with_foreign)]
pub trait ScanProgressListener: Send + Sync {
    fn on_progress(&self, event: ScanProgressEvent);
}

/// Opens a SAF document by URI and returns a detached file descriptor, or `None` when it cannot be
/// opened. Kotlin must pass ownership with `ParcelFileDescriptor.detachFd()`: Rust closes the fd.
/// Called concurrently from scan worker threads.
#[uniffi::export(with_foreign)]
pub trait UriOpener: Send + Sync {
    fn open_fd(&self, uri: String) -> Option<i32>;
}

pub(crate) struct LibraryData {
    /// The engine generation this data was loaded under (read *before* the file was read).
    pub(crate) generation: u64,
    pub(crate) albums: Vec<Album>,
    /// Album key -> index into `albums` (the first album with that key, like a linear `find`).
    album_index: HashMap<String, usize>,
    artists: Vec<ArtistSummary>,
    restored_from_backup: bool,
    track_index: OnceLock<TrackIndex>,
}

impl LibraryData {
    /// Track id -> `(album index, track index)` into `albums`; built once per library generation.
    /// Positions, not clones: copying `(Track, Album)` per track was quadratic in an album's size.
    pub(crate) fn track_index(&self) -> &TrackIndex {
        self.track_index.get_or_init(|| build_track_index(&self.albums))
    }

    /// Id -> track lookup over this library.
    pub(crate) fn lookup(&self) -> TrackLookup<'_> {
        TrackLookup::new(&self.albums, self.track_index())
    }

    /// The album stored under `key`, in O(1).
    pub(crate) fn album(&self, key: &str) -> Option<&Album> {
        self.album_index.get(key).map(|&i| &self.albums[i])
    }

    pub(super) fn new(generation: u64, albums: Vec<Album>, artists: Vec<ArtistSummary>, restored_from_backup: bool) -> Self {
        let mut album_index = HashMap::with_capacity(albums.len());
        for (i, a) in albums.iter().enumerate() {
            album_index.entry(a.name.clone()).or_insert(i);
        }
        Self {
            generation,
            albums,
            album_index,
            artists,
            restored_from_backup,
            track_index: OnceLock::new(),
        }
    }
}

struct ScanGuard<'a>(&'a AtomicBool);

impl Drop for ScanGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

#[derive(uniffi::Object)]
pub struct RustygradeEngine {
    cancel: AtomicBool,
    scanning: AtomicBool,
    cache: RwLock<Option<Arc<LibraryData>>>,
    /// Serialises cache-miss loads so concurrent callers parse `library.json` once, not once each.
    load_lock: Mutex<()>,
    /// Bumped by every invalidation so a load that raced a scan never caches stale data.
    generation: AtomicU64,
    /// Serialises settings read-modify-write cycles.
    pub(crate) settings_lock: Mutex<()>,
    /// Serialises read-modify-write cycles of `native-collections.json`.
    pub(crate) collections_lock: Mutex<()>,
}

#[uniffi::export]
impl RustygradeEngine {
    /// `app_dir` is `Context.filesDir`; it is where `library.json`, `covers/` etc. live.
    #[uniffi::constructor]
    pub fn new(app_dir: String) -> Arc<Self> {
        if !app_dir.is_empty() {
            CoverPaths::set_app_dir(PathBuf::from(app_dir));
        }
        Arc::new(Self {
            cancel: AtomicBool::new(false),
            scanning: AtomicBool::new(false),
            cache: RwLock::new(None),
            load_lock: Mutex::new(()),
            generation: AtomicU64::new(0),
            settings_lock: Mutex::new(()),
            collections_lock: Mutex::new(()),
        })
    }

    // ---- library --------------------------------------------------------------------------

    pub fn load_library_snapshot(&self, sort: String) -> Result<LibrarySnapshot, EngineError> {
        let data = self.library()?;
        let canonical = canonical_library_sort(&sort);
        let sort = LibrarySort::from_key(canonical);
        let stats = if sort.uses_stats() { PlayStats::load_locked() } else { PlayStats::default() };
        // One pass over the play stats serves both orders.
        let (album_order, artist_order) = sort_library(&data.albums, &data.artists, sort, &stats);
        let albums = album_order.into_iter().map(|i| AlbumSummary::from(&data.albums[i])).collect();
        let artists = artist_order
            .into_iter()
            .map(|i| ArtistInfo::new(&data.artists[i], &data.albums))
            .collect();
        Ok(LibrarySnapshot {
            albums,
            artists,
            track_count: data.albums.iter().map(|a| a.tracks.len()).sum::<usize>() as u32,
            sort: canonical.to_owned(),
            restored_from_backup: data.restored_from_backup,
        })
    }

    pub fn get_album_detail(&self, key: String) -> Result<Option<AlbumDetail>, EngineError> {
        let data = self.library()?;
        Ok(data.album(&key).map(|a| AlbumDetail {
            album: AlbumSummary::from(a),
            tracks: a.tracks.iter().map(TrackInfo::from).collect(),
        }))
    }

    pub fn get_artist_detail(&self, name: String) -> Result<Option<ArtistDetail>, EngineError> {
        let data = self.library()?;
        let wanted = fold(name.trim());
        Ok(data.artists.iter().find(|a| fold(&a.name) == wanted).map(|a| ArtistDetail {
            artist: ArtistInfo::new(a, &data.albums),
            albums: a.albums(&data.albums).map(AlbumSummary::from).collect(),
        }))
    }

    /// Every track of the library in one call (album order, then track order), so Kotlin does not
    /// need one `get_album_detail` round trip per album.
    pub fn all_tracks(&self) -> Result<Vec<TrackInfo>, EngineError> {
        let data = self.library()?;
        Ok(data.albums.iter().flat_map(|a| a.tracks.iter()).map(TrackInfo::from).collect())
    }

    /// Resolves track ids (favorites, playlists, queue) to full track records, in the given order.
    /// Ids that no longer exist in the library are skipped.
    pub fn get_tracks(&self, ids: Vec<String>) -> Result<Vec<TrackInfo>, EngineError> {
        let data = self.library()?;
        let lookup = data.lookup();
        Ok(ids.iter().filter_map(|id| lookup.get(id).map(TrackInfo::from)).collect())
    }

    pub fn search_library(&self, query: String) -> Result<SearchResults, EngineError> {
        let q = fold(query.trim());
        if q.is_empty() {
            return Ok(SearchResults {
                albums: vec![],
                artists: vec![],
                tracks: vec![],
                playlists: vec![],
                smart_playlists: vec![],
            });
        }
        let data = self.library()?;
        let albums = search_albums(&data.albums, &q)
            .into_iter()
            .map(|i| AlbumSummary::from(&data.albums[i]))
            .collect();
        let artists = search_artists(&data.artists, &q)
            .into_iter()
            .map(|i| ArtistInfo::new(&data.artists[i], &data.albums))
            .collect();
        let tracks = data
            .albums
            .iter()
            .flat_map(|a| a.tracks.iter())
            .filter(|t| contains_folded(&t.title, &q) || contains_folded(&t.artist, &q))
            .take(MAX_TRACK_HITS)
            .map(TrackInfo::from)
            .collect();
        let mut col = {
            let _guard = self.collections_lock.lock().unwrap_or_else(|p| p.into_inner());
            CollectionsStore::load()
        };
        ensure_default_smart_playlists(&mut col);
        let playlists = col
            .playlists
            .into_iter()
            .filter(|p| contains_folded(&p.name, &q))
            .map(|p| PlaylistInfo {
                id: p.id,
                name: p.name,
                cover_path: CoverPaths::to_local_path(p.cover_art.as_deref())
                    .map(|path| path.to_string_lossy().into_owned()),
                track_ids: p.track_ids,
            })
            .collect();
        let smart_playlists = col
            .smart
            .unwrap_or_default()
            .into_iter()
            .filter(|s| contains_folded(&s.name, &q))
            .map(|s| SmartPlaylistInfo {
                id: s.id,
                name: s.name,
                match_all: s.match_all,
                rules: s
                    .rules
                    .into_iter()
                    .map(|r| SmartRuleInfo { field: r.field, op: r.op, value: r.value })
                    .collect(),
                sort: s.sort,
                limit: s.limit as u32,
            })
            .collect();
        Ok(SearchResults { albums, artists, tracks, playlists, smart_playlists })
    }

    /// Removes whole albums (and their cached covers). Returns how many were removed.
    pub fn delete_albums(&self, keys: Vec<String>) -> Result<u32, EngineError> {
        let removed = LibraryStore::delete_albums(&keys)?;
        self.invalidate();
        Ok(removed as u32)
    }

    /// Migrates legacy covers to single-file content-addressed covers (`c_<hash>.<ext>`)
    /// and removes unreferenced cover files from the cache.
    pub fn deduplicate_and_sweep_covers(&self) -> Result<CoverDedupSummary, EngineError> {
        if self.scanning.load(Ordering::SeqCst) {
            return Ok(CoverDedupSummary::default());
        }
        let mut migrated_playlists = 0u32;
        let mut replaced_pl_paths = Vec::new();
        let col_snapshot = {
            let _guard = self.collections_lock.lock().unwrap_or_else(|p| p.into_inner());
            let mut col = CollectionsStore::load();
            let before: Vec<Option<String>> =
                col.playlists.iter().map(|p| p.cover_art.clone()).collect();
            if migrate_playlist_covers(&mut col) {
                for (pl, prev) in col.playlists.iter().zip(before.iter()) {
                    if &pl.cover_art != prev {
                        migrated_playlists += 1;
                        if let Some(old_path) = CoverPaths::to_local_path(prev.as_deref()) {
                            let thumb = PlaylistCovers::thumb_for(&old_path);
                            if thumb != old_path {
                                replaced_pl_paths.push(thumb);
                            }
                            replaced_pl_paths.push(old_path);
                        }
                    }
                }
                CollectionsStore::save(&col);
            }
            col
        };
        let res = core_deduplicate_and_sweep_covers(Some(&col_snapshot), None)?;
        let mut extra_removed = 0u32;
        if !replaced_pl_paths.is_empty() {
            let candidates: Vec<PathBuf> = replaced_pl_paths.into_iter().filter(|p| p.exists()).collect();
            if !candidates.is_empty() {
                let library = LibraryStore::load_raw().unwrap_or_default();
                let current_col = {
                    let _guard = self.collections_lock.lock().unwrap_or_else(|p| p.into_inner());
                    CollectionsStore::load()
                };
                delete_unreferenced_covers(&candidates, &library, &current_col);
                extra_removed = candidates.iter().filter(|p| !p.exists()).count() as u32;
            }
        }
        if res.migrated_albums > 0 || !res.changed_paths.is_empty() {
            self.invalidate();
        }
        Ok(CoverDedupSummary {
            migrated_albums: res.migrated_albums as u32,
            migrated_playlists,
            removed_files: res.removed_files as u32 + extra_removed,
        })
    }

    // ---- scanning -------------------------------------------------------------------------

    /// Scans the given sources and updates `library.json` and the cover cache. Blocking: call it
    /// from `Dispatchers.IO`. Only one scan runs at a time.
    pub fn scan_sources(
        &self,
        request: ScanRequest,
        listener: Option<Arc<dyn ScanProgressListener>>,
        opener: Option<Arc<dyn UriOpener>>,
    ) -> Result<ScanSummary, EngineError> {
        if self.scanning.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_err() {
            return Err(EngineError::ScanInProgress);
        }
        let _guard = ScanGuard(&self.scanning);
        self.cancel.store(false, Ordering::SeqCst);

        let sources = ScanSources {
            dirs: request.dirs.into_iter().map(PathBuf::from).collect(),
            files: request.files.into_iter().map(PathBuf::from).collect(),
            roots: request.roots.into_iter().map(PathBuf::from).collect(),
            saf_files: request
                .saf_files
                .into_iter()
                .map(|d| SafFile {
                    document_id: d.document_id,
                    uri: d.uri,
                    mtime_ms: d.mtime_ms,
                    size: d.size,
                })
                .collect(),
            saf_roots: request.saf_roots,
        };
        let options = ScanOptions {
            prune_missing: if request.resync_covers { false } else { request.prune_missing },
            force: request.force || request.resync_covers,
            resync_covers: request.resync_covers,
            max_threads: None,
        };
        let fd_opener = opener.map(FdOpener);
        let mut on_progress = |p: ScanProgress| {
            if let Some(l) = &listener {
                l.on_progress(ScanProgressEvent {
                    phase: p.phase.to_owned(),
                    scanned: p.scanned as u32,
                    total: p.total as u32,
                    current_file: p.current_file,
                });
            }
        };
        let result = Scanner::scan_sources(
            &sources,
            fd_opener.as_ref().map(|o| o as &dyn FileOpener),
            options,
            Some(&self.cancel),
            Some(&mut on_progress),
        );
        self.invalidate();
        let r = result?;
        if !r.cancelled {
            let _ = self.remap_unmatched_ids();
        }
        Ok(ScanSummary {
            added_tracks: r.added_tracks as u32,
            new_albums: r.new_albums as u32,
            covers: r.covers as u32,
            updated_tracks: r.updated_tracks as u32,
            removed_tracks: r.removed_tracks as u32,
            missing_tracks: r.missing_tracks as u32,
            merged_albums: r.merged_albums as u32,
            cancelled: r.cancelled,
        })
    }

    /// Asks the running scan to stop; it returns early without saving.
    pub fn cancel_scan(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    // ---- collections / stats / settings ---------------------------------------------------

    pub fn load_collections(&self) -> CollectionsSnapshot {
        // Creating the default smart playlists is a write: hold the same lock as every other edit.
        let _guard = self.collections_lock.lock().unwrap_or_else(|p| p.into_inner());
        let mut data = CollectionsStore::load();
        if ensure_default_smart_playlists(&mut data) {
            CollectionsStore::save(&data);
        }
        CollectionsSnapshot {
            liked: data.liked,
            playlists: data
                .playlists
                .into_iter()
                .map(|p| PlaylistInfo {
                    id: p.id,
                    name: p.name,
                    cover_path: CoverPaths::to_local_path(p.cover_art.as_deref())
                        .map(|p| p.to_string_lossy().into_owned()),
                    track_ids: p.track_ids,
                })
                .collect(),
            smart: data
                .smart
                .unwrap_or_default()
                .into_iter()
                .map(|s| SmartPlaylistInfo {
                    id: s.id,
                    name: s.name,
                    match_all: s.match_all,
                    rules: s
                        .rules
                        .into_iter()
                        .map(|r| SmartRuleInfo { field: r.field, op: r.op, value: r.value })
                        .collect(),
                    sort: s.sort,
                    limit: s.limit as u32,
                })
                .collect(),
        }
    }

    pub fn load_settings(&self) -> SettingsSnapshot {
        let s = AppSettings::load();
        let saf_trees = s
            .extra
            .get(KEY_SAF_TREES)
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
            .unwrap_or_default();
        let use_media_store = s.extra.get(KEY_USE_MEDIA_STORE).and_then(Value::as_bool).unwrap_or(true);
        SettingsSnapshot {
            volume: s.volume,
            library_sort: canonical_library_sort(&s.library_sort).to_owned(),
            library_view: s.library_view,
            replay_gain: s.replay_gain,
            accent_from_cover: s.accent_from_cover,
            crossfade_seconds: s.crossfade_seconds,
            eq_enabled: s.eq_enabled,
            eq_preset: s.eq_preset,
            eq_gains: s.eq_gains,
            library_folders: s.library_folders,
            saf_trees,
            use_media_store,
            usb_bit_perfect: s.extra.get(KEY_USB_BIT_PERFECT).and_then(Value::as_bool).unwrap_or(false),
            scan_on_startup: s.scan_on_startup,
            auto_replay_gain: s.auto_replay_gain,
            artwork_itunes: s.artwork_itunes,
            artwork_deezer: s.artwork_deezer,
            artwork_musicbrainz: s.artwork_musicbrainz,
        }
    }

    /// Remembers a SAF tree URI. Atomic with respect to other settings writes.
    pub fn add_saf_tree(&self, uri: String) {
        self.update_saf_trees(|trees| {
            if !trees.contains(&uri) {
                trees.push(uri.clone());
            }
        });
    }

    pub fn remove_saf_tree(&self, uri: String) {
        self.update_saf_trees(|trees| trees.retain(|t| t != &uri));
    }

    /// Equalizer settings only (the service and the UI both write settings, so no full overwrite).
    pub fn set_eq(&self, enabled: bool, preset: String, gains: Vec<f64>) {
        self.update_settings(|s| {
            s.eq_enabled = enabled;
            s.eq_preset = preset;
            s.eq_gains = sanitize_eq_gains(gains);
        });
    }

    /// `"no"`, `"track"` or `"album"`; anything else is stored as `"no"`.
    pub fn set_replay_gain(&self, mode: String) {
        let mode = normalize_replay_gain(&mode).to_owned();
        self.update_settings(|s| s.replay_gain = mode);
    }

    pub fn set_usb_bit_perfect(&self, enabled: bool) {
        self.update_settings(|s| {
            s.extra.insert(KEY_USB_BIT_PERFECT.to_owned(), Value::Bool(enabled));
        });
    }

    pub fn set_scan_on_startup(&self, enabled: bool) {
        self.update_settings(|s| s.scan_on_startup = enabled);
    }

    pub fn set_auto_replay_gain(&self, enabled: bool) {
        self.update_settings(|s| s.auto_replay_gain = enabled);
    }

    pub fn set_artwork_sources(&self, itunes: bool, deezer: bool, musicbrainz: bool) {
        self.update_settings(|s| {
            s.artwork_itunes = itunes;
            s.artwork_deezer = deezer;
            s.artwork_musicbrainz = musicbrainz;
        });
    }
}

/// Test-only helpers (not part of the UniFFI surface).
#[cfg(test)]
impl RustygradeEngine {
    pub fn is_scanning(&self) -> bool {
        self.scanning.load(Ordering::SeqCst)
    }

    pub fn load_stats(&self) -> StatsSnapshot {
        StatsSnapshot {
            tracks: PlayStats::load_locked()
                .tracks
                .into_iter()
                .map(|(id, s)| PlayStatInfo { track_id: id, count: s.count, last_played_ms: s.last })
                .collect(),
        }
    }

    /// Writes the snapshot over the stored settings; keys this app does not know are preserved.
    pub fn save_settings(&self, settings: SettingsSnapshot) {
        let _guard = self.settings_lock.lock().unwrap_or_else(|p| p.into_inner());
        let mut s = AppSettings::load();
        s.volume = if settings.volume.is_finite() { settings.volume.clamp(0.0, 1.0) } else { 0.5 };
        s.library_sort = canonical_library_sort(&settings.library_sort).to_owned();
        s.library_view = rustygrade_core::settings::normalize_library_view(&settings.library_view).to_owned();
        s.replay_gain = normalize_replay_gain(&settings.replay_gain).to_owned();
        s.accent_from_cover = settings.accent_from_cover;
        s.crossfade_seconds = settings.crossfade_seconds.clamp(0, 12);
        s.eq_enabled = settings.eq_enabled;
        s.eq_preset = settings.eq_preset;
        s.eq_gains = sanitize_eq_gains(settings.eq_gains);
        s.library_folders = settings.library_folders;
        s.scan_on_startup = settings.scan_on_startup;
        s.auto_replay_gain = settings.auto_replay_gain;
        s.artwork_itunes = settings.artwork_itunes;
        s.artwork_deezer = settings.artwork_deezer;
        s.artwork_musicbrainz = settings.artwork_musicbrainz;
        s.extra.insert(
            KEY_SAF_TREES.to_owned(),
            Value::Array(settings.saf_trees.into_iter().map(Value::String).collect()),
        );
        s.extra.insert(KEY_USE_MEDIA_STORE.to_owned(), Value::Bool(settings.use_media_store));
        s.extra.insert(KEY_USB_BIT_PERFECT.to_owned(), Value::Bool(settings.usb_bit_perfect));
        s.save();
    }
}
