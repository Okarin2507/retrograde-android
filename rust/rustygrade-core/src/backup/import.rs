use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::cover_names::{
    dedupe_preserving_order, extract_cover_basename, is_normalized_content_cover,
    stored_playlist_cover_name, validate_cover_filename,
};
use super::merge::{
    build_stats_ci_index, merge_playlists, merge_settings, merge_smart_playlists,
    merge_stat_entry_indexed,
};
use super::path_mapper::{MapOutcome, TrackPathMapper};
use super::remap::append_backup_source_roots;
use super::{
    BackupBundle, BackupCoverFile, BackupError, BackupImportOptions,
    MAX_BACKUP_COVER_BYTES, MAX_BACKUP_COVER_COUNT, MAX_BACKUP_COVERS_TOTAL_BYTES,
    MAX_BACKUP_JSON_BYTES, PLAYLIST_COVER_PREFIX,
};
use crate::collections::{CollectionsData, CollectionsStore, PlaylistData, SmartPlaylistData};
use crate::covers::{ImageLimits, PlaylistCovers, delete_unreferenced_covers};
use crate::library::LibraryStore;
use crate::library_view::ensure_default_smart_playlists;
use crate::models::Album;
use crate::paths::CoverPaths;
use crate::settings::AppSettings;
use crate::stats::{PlayStat, PlayStats};

struct PreparedCollections {
    liked: Vec<String>,
    playlists: Vec<PreparedPlaylist>,
    smart: Option<Vec<SmartPlaylistData>>,
    extra: serde_json::Map<String, Value>,
}

struct PreparedPlaylist {
    data: PlaylistData,
    raw_cover_basename: Option<String>,
}

/// Validated, pre-remapped import ready to apply with minimal lock hold times.
pub struct PreparedImport {
    collections: Option<PreparedCollections>,
    stats: Option<HashMap<String, PlayStat>>,
    settings: Option<AppSettings>,
    covers: Vec<BackupCoverFile>,
    source_roots: Vec<String>,
    replace: bool,
    pub tracks_exact: usize,
    pub tracks_remapped: usize,
    pub tracks_unmatched: usize,
}

/// Validates all JSON payloads and cover files and remaps all track IDs in memory before any
/// disk writes or store locks are taken.
pub fn validate_and_prepare_import(
    bundle: BackupBundle,
    albums: &[Album],
    options: &BackupImportOptions,
) -> Result<PreparedImport, BackupError> {
    if bundle.collections_json.is_none()
        && bundle.stats_json.is_none()
        && bundle.settings_json.is_none()
    {
        return Err(BackupError::EmptyBackup);
    }

    for (what, raw) in [
        ("native-collections.json", &bundle.collections_json),
        ("native-stats.json", &bundle.stats_json),
        ("native-settings.json", &bundle.settings_json),
    ] {
        if raw.as_ref().is_some_and(|r| r.len() > MAX_BACKUP_JSON_BYTES) {
            return Err(BackupError::TooLarge { what, limit: "16 MiB".to_owned() });
        }
    }
    if bundle.playlist_covers.len() > MAX_BACKUP_COVER_COUNT {
        return Err(BackupError::TooLarge {
            what: "The playlist cover set",
            limit: format!("{MAX_BACKUP_COVER_COUNT} files"),
        });
    }

    let parsed_collections = match &bundle.collections_json {
        Some(raw) => {
            let parsed: CollectionsData =
                serde_json::from_str(raw).map_err(|e| BackupError::InvalidJson {
                    file: "native-collections.json",
                    message: e.to_string(),
                })?;
            Some(parsed)
        }
        None => None,
    };

    let parsed_stats = match &bundle.stats_json {
        Some(raw) => {
            let parsed: PlayStats =
                serde_json::from_str(raw).map_err(|e| BackupError::InvalidJson {
                    file: "native-stats.json",
                    message: e.to_string(),
                })?;
            Some(parsed)
        }
        None => None,
    };

    let parsed_settings = match &bundle.settings_json {
        Some(raw) => Some(AppSettings::parse_lenient(raw).ok_or_else(|| {
            BackupError::InvalidJson {
                file: "native-settings.json",
                message: "Expected a JSON object".to_owned(),
            }
        })?),
        None => None,
    };

    let mut total_cover_bytes = 0usize;
    let mut validated_covers = Vec::with_capacity(bundle.playlist_covers.len());
    for cover in bundle.playlist_covers {
        validate_cover_filename(&cover.name)?;
        if cover.bytes.len() > MAX_BACKUP_COVER_BYTES {
            return Err(BackupError::InvalidCover {
                name: cover.name,
                reason: "cover exceeds 10 MiB limit",
            });
        }
        total_cover_bytes = total_cover_bytes.saturating_add(cover.bytes.len());
        if total_cover_bytes > MAX_BACKUP_COVERS_TOTAL_BYTES {
            return Err(BackupError::InvalidCover {
                name: cover.name,
                reason: "total playlist covers exceed 50 MiB limit",
            });
        }
        if !ImageLimits::is_sane_bytes(&cover.bytes) {
            return Err(BackupError::InvalidCover {
                name: cover.name,
                reason: "corrupt or unsupported image",
            });
        }
        validated_covers.push(cover);
    }

    let mut needed_covers = HashSet::new();
    if let Some(col) = &parsed_collections {
        for p in &col.playlists {
            if let Some(raw) = extract_cover_basename(p.cover_art.as_deref())
                && validate_cover_filename(&raw).is_ok()
            {
                let stored = stored_playlist_cover_name(&raw);
                if let Some(thumb_name) = PlaylistCovers::thumb_for(Path::new(&stored))
                    .file_name()
                    .and_then(|n| n.to_str())
                {
                    needed_covers.insert(thumb_name.to_owned());
                }
                needed_covers.insert(stored);
            }
        }
    }

    let mut seen_covers = HashSet::new();
    let mut covers = Vec::with_capacity(validated_covers.len());
    for mut cover in validated_covers {
        let stored_name = stored_playlist_cover_name(&cover.name);
        if needed_covers.contains(&stored_name) && seen_covers.insert(stored_name.clone()) {
            cover.name = stored_name;
            covers.push(cover);
        }
    }

    let mut source_roots = Vec::new();
    if let Some(override_root) = options.source_root_override.as_deref() {
        let trimmed = override_root.trim();
        if !trimmed.is_empty() {
            source_roots.push(trimmed.to_owned());
        }
    }
    if let Some(s) = &parsed_settings {
        for folder in &s.library_folders {
            let trimmed = folder.trim();
            if !trimmed.is_empty() && !source_roots.iter().any(|r| r == trimmed) {
                source_roots.push(trimmed.to_owned());
            }
        }
    }

    let mapper = TrackPathMapper::from_albums(albums, &source_roots);
    let mut track_outcomes: HashMap<String, MapOutcome> = HashMap::new();
    let mut resolve_track = |raw_id: &str| -> String {
        track_outcomes
            .entry(raw_id.to_owned())
            .or_insert_with(|| mapper.map_id(raw_id))
            .target_id()
            .to_owned()
    };

    let collections = parsed_collections.map(|imp_col| {
        let liked = dedupe_preserving_order(imp_col.liked.iter().map(|id| resolve_track(id)));
        let playlists = imp_col
            .playlists
            .into_iter()
            .map(|mut p| {
                p.track_ids = dedupe_preserving_order(
                    p.track_ids.iter().map(|id| resolve_track(id)),
                );
                let raw_cover_basename = extract_cover_basename(p.cover_art.as_deref())
                    .filter(|b| validate_cover_filename(b).is_ok())
                    .map(|b| stored_playlist_cover_name(&b));
                p.cover_art = None;
                PreparedPlaylist {
                    data: p,
                    raw_cover_basename,
                }
            })
            .collect();
        PreparedCollections {
            liked,
            playlists,
            smart: imp_col.smart,
            extra: imp_col.extra,
        }
    });

    let stats = parsed_stats.map(|imp_stats| {
        let mut mapped_tracks: HashMap<String, PlayStat> =
            HashMap::with_capacity(imp_stats.tracks.len());
        let mut ci_index: HashMap<String, String> =
            HashMap::with_capacity(imp_stats.tracks.len());
        for (raw_id, stat) in imp_stats.tracks {
            let dst_id = resolve_track(&raw_id);
            merge_stat_entry_indexed(&mut mapped_tracks, &mut ci_index, dst_id, stat);
        }
        mapped_tracks
    });

    let (mut tracks_exact, mut tracks_remapped, mut tracks_unmatched) = (0, 0, 0);
    for outcome in track_outcomes.values() {
        match outcome {
            MapOutcome::Exact(_) => tracks_exact += 1,
            MapOutcome::Remapped(_) => tracks_remapped += 1,
            MapOutcome::Unmatched(_) => tracks_unmatched += 1,
        }
    }

    Ok(PreparedImport {
        collections,
        stats,
        settings: parsed_settings,
        covers,
        source_roots,
        replace: options.replace,
        tracks_exact,
        tracks_remapped,
        tracks_unmatched,
    })
}

impl PreparedImport {
    pub fn has_stats(&self) -> bool {
        self.stats.is_some()
    }

    /// Writes validated playlist covers into `CoverPaths::cover_dir()` without holding any store locks.
    pub fn write_covers(&self) -> Result<(HashSet<String>, usize), BackupError> {
        let mut restored_names = HashSet::new();
        if self.covers.is_empty() {
            return Ok((restored_names, 0));
        }
        let cover_dir = CoverPaths::cover_dir();
        fs::create_dir_all(&cover_dir)?;
        let mut count = 0usize;
        for cover in &self.covers {
            let dest = cover_dir.join(&cover.name);
            // Content-addressed covers are immutable for a given hash; avoid letting an archive
            // overwrite an already-verified non-empty file shared by existing playlists.
            if !(is_normalized_content_cover(&cover.name)
                && dest.is_file()
                && fs::metadata(&dest).is_ok_and(|m| m.len() > 0))
            {
                crate::fsutil::write_atomic(&dest, &cover.bytes)?;
            }
            restored_names.insert(cover.name.clone());
            count += 1;
        }
        Ok((restored_names, count))
    }

    /// Applies pre-remapped collections, saves `native-collections.json`, and deletes replaced/orphaned
    /// covers only after the save succeeds.
    pub fn apply_collections(
        &mut self,
        current: &mut CollectionsData,
        restored_names: &HashSet<String>,
    ) -> Result<bool, BackupError> {
        let Some(imp_col) = self.collections.take() else {
            return Ok(false);
        };
        let cover_dir = CoverPaths::cover_dir();
        let mapped_playlists: Vec<PlaylistData> = imp_col
            .playlists
            .into_iter()
            .map(|mut p| {
                p.data.cover_art = p.raw_cover_basename.as_deref().and_then(|base| {
                    let target = cover_dir.join(base);
                    if restored_names.contains(base) || target.is_file() {
                        Some(CoverPaths::to_file_url(&target))
                    } else {
                        None
                    }
                });
                p.data
            })
            .collect();

        let mut old_replaced_covers: Vec<String> = Vec::new();

        if self.replace {
            current.liked = imp_col.liked;
            current.playlists = mapped_playlists;
            current.smart = imp_col.smart;
            ensure_default_smart_playlists(current);
            for (k, v) in imp_col.extra {
                current.extra.insert(k, v);
            }
        } else {
            current.liked = dedupe_preserving_order(
                current.liked.iter().cloned().chain(imp_col.liked),
            );
            merge_playlists(
                &mut current.playlists,
                mapped_playlists,
                &mut old_replaced_covers,
            );
            if let Some(imp_smart) = imp_col.smart {
                let existing = current.smart.get_or_insert_with(Vec::new);
                merge_smart_playlists(existing, imp_smart);
            }
            ensure_default_smart_playlists(current);
            for (k, v) in imp_col.extra {
                current.extra.insert(k, v);
            }
        }

        CollectionsStore::try_save(current)?;

        // Only prune/delete old cover files once the new collections file is safely on disk.
        let library = LibraryStore::load_raw().unwrap_or_default();
        if self.replace {
            let candidates: Vec<PathBuf> = fs::read_dir(&cover_dir)
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.is_file()
                        && p.file_name().and_then(|n| n.to_str()).is_some_and(|name| {
                            name.starts_with(PLAYLIST_COVER_PREFIX)
                                || CoverPaths::is_content_cover_file(p)
                        })
                })
                .collect();
            delete_unreferenced_covers(&candidates, &library, current);
        } else if !old_replaced_covers.is_empty() {
            let mut candidates = Vec::with_capacity(old_replaced_covers.len() * 2);
            for old_url in old_replaced_covers {
                if let Some(full) = CoverPaths::to_local_path(Some(&old_url)) {
                    let thumb = PlaylistCovers::thumb_for(&full);
                    if thumb != full {
                        candidates.push(thumb);
                    }
                    candidates.push(full);
                }
            }
            delete_unreferenced_covers(&candidates, &library, current);
        }
        Ok(true)
    }

    /// Merges/replaces pre-remapped play stats in O(n) and persists `native-stats.json`.
    pub fn apply_stats(&mut self, current: &mut PlayStats) -> Result<bool, BackupError> {
        let Some(mapped_tracks) = self.stats.take() else {
            return Ok(false);
        };
        if self.replace {
            current.tracks = mapped_tracks;
        } else {
            let mut ci_index = build_stats_ci_index(&current.tracks);
            for (dst_id, stat) in mapped_tracks {
                merge_stat_entry_indexed(&mut current.tracks, &mut ci_index, dst_id, stat);
            }
        }
        current.invalidate_index();
        current.try_save()?;
        Ok(true)
    }

    /// Merges portable settings while preserving Android storage/USB fields and recording source root hints.
    pub fn apply_settings(&mut self, current: &mut AppSettings) -> Result<bool, BackupError> {
        let had_settings = self.settings.is_some();
        if let Some(imp_settings) = self.settings.take() {
            merge_settings(current, imp_settings);
        }
        if !self.source_roots.is_empty() {
            append_backup_source_roots(current, &self.source_roots);
        }
        if had_settings || !self.source_roots.is_empty() {
            current.try_save()?;
        }
        Ok(had_settings)
    }
}

/// Validates and applies a backup bundle onto the stores in `CoverPaths::app_dir()`.
#[cfg(test)]
pub fn import_backup_bundle(
    bundle: BackupBundle,
    albums: &[Album],
    options: &BackupImportOptions,
) -> Result<super::BackupImportReport, BackupError> {
    let mut prepared = validate_and_prepare_import(bundle, albums, options)?;
    let (restored_names, covers_restored) = prepared.write_covers()?;

    let mut collections = CollectionsStore::load();
    let collections_imported = prepared.apply_collections(&mut collections, &restored_names)?;

    let stats_imported = PlayStats::with_lock(|stats| prepared.apply_stats(stats))?;
    let stats_track_count = PlayStats::load_locked().tracks.len();

    let mut settings = AppSettings::load();
    let settings_imported = prepared.apply_settings(&mut settings)?;

    Ok(super::BackupImportReport {
        collections_imported,
        stats_imported,
        settings_imported,
        liked_count: collections.liked.len(),
        playlist_count: collections.playlists.len(),
        smart_playlist_count: collections.smart.as_ref().map_or(0, Vec::len),
        stats_track_count,
        tracks_exact: prepared.tracks_exact,
        tracks_remapped: prepared.tracks_remapped,
        tracks_unmatched: prepared.tracks_unmatched,
        covers_restored,
    })
}
