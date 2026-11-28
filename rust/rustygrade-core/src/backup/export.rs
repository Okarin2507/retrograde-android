use std::collections::HashSet;
use std::fs;

use super::cover_names::{extract_cover_basename, validate_cover_filename};
use super::{
    BackupBundle, BackupCoverFile, BackupError, MAX_BACKUP_COVER_BYTES, MAX_BACKUP_COVER_COUNT,
    MAX_BACKUP_COVERS_TOTAL_BYTES,
};
use crate::collections::{CollectionsData, PlaylistData};
use crate::covers::PlaylistCovers;
use crate::paths::CoverPaths;
use crate::settings::AppSettings;
use crate::stats::PlayStats;

/// Exports collections, stats, settings, and referenced local playlist cover files into a bundle.
pub fn export_backup_bundle(
    collections: &CollectionsData,
    stats: &PlayStats,
    settings: &AppSettings,
) -> Result<BackupBundle, BackupError> {
    let collections_json = serde_json::to_string_pretty(collections).map_err(|e| {
        BackupError::InvalidJson {
            file: "native-collections.json",
            message: e.to_string(),
        }
    })?;
    let stats_json = serde_json::to_string_pretty(stats).map_err(|e| BackupError::InvalidJson {
        file: "native-stats.json",
        message: e.to_string(),
    })?;
    let settings_json =
        serde_json::to_string_pretty(settings).map_err(|e| BackupError::InvalidJson {
            file: "native-settings.json",
            message: e.to_string(),
        })?;

    let playlist_covers = collect_playlist_covers(&collections.playlists);

    Ok(BackupBundle {
        collections_json: Some(collections_json),
        stats_json: Some(stats_json),
        settings_json: Some(settings_json),
        playlist_covers,
    })
}

/// Exports a single custom playlist and its referenced cover(s) in desktop-compatible backup format
/// (`liked: []`, `smart: null`, no stats or settings).
pub fn export_playlist_backup_bundle(playlist: &PlaylistData) -> Result<BackupBundle, BackupError> {
    let single = CollectionsData {
        liked: Vec::new(),
        playlists: vec![playlist.clone()],
        smart: None,
        extra: serde_json::Map::new(),
    };
    let collections_json = serde_json::to_string_pretty(&single).map_err(|e| {
        BackupError::InvalidJson {
            file: "native-collections.json",
            message: e.to_string(),
        }
    })?;
    let playlist_covers = collect_playlist_covers(&single.playlists);

    Ok(BackupBundle {
        collections_json: Some(collections_json),
        stats_json: None,
        settings_json: None,
        playlist_covers,
    })
}

fn collect_playlist_covers(playlists: &[PlaylistData]) -> Vec<BackupCoverFile> {
    let cover_dir = CoverPaths::cover_dir();
    let mut seen_names = HashSet::new();
    let mut playlist_covers = Vec::new();
    let mut total_cover_bytes = 0usize;

    for playlist in playlists {
        let Some(basename) = extract_cover_basename(playlist.cover_art.as_deref()) else {
            continue;
        };
        if validate_cover_filename(&basename).is_err() {
            continue;
        }
        let full_path = cover_dir.join(&basename);
        let thumb_path = PlaylistCovers::thumb_for(&full_path);
        for candidate in [full_path, thumb_path] {
            if playlist_covers.len() >= MAX_BACKUP_COVER_COUNT {
                return playlist_covers;
            }
            let Some(name) = candidate.file_name().and_then(|n| n.to_str()).map(str::to_owned) else {
                continue;
            };
            if validate_cover_filename(&name).is_err() || !seen_names.insert(name.clone()) {
                continue;
            }
            let Ok(meta) = fs::metadata(&candidate) else {
                continue;
            };
            if !meta.is_file() || meta.len() > MAX_BACKUP_COVER_BYTES as u64 {
                continue;
            }
            let Ok(bytes) = fs::read(&candidate) else {
                continue;
            };
            if bytes.len() > MAX_BACKUP_COVER_BYTES {
                continue;
            }
            if total_cover_bytes.saturating_add(bytes.len()) > MAX_BACKUP_COVERS_TOTAL_BYTES {
                return playlist_covers;
            }
            total_cover_bytes += bytes.len();
            playlist_covers.push(BackupCoverFile { name, bytes });
        }
    }

    playlist_covers
}
