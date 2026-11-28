//! Backup export and import across Android and desktop (`~/.rustygrade` / `~/.rustygrade`) setups,
//! plus single-playlist ZIP and Extended M3U/M3U8 export/import.

use std::collections::HashSet;

use rustygrade_core::{
    AppSettings, BackupBundle, BackupCoverFile, BackupError, BackupImportOptions,
    CollectionsData, CollectionsStore, PlayStats, PlaylistData, SmartPlaylists, Track,
    TrackPathMapper, backup::{MAX_BACKUP_COVER_COUNT, MAX_BACKUP_JSON_BYTES},
    export_backup_bundle, export_m3u8, export_playlist_backup_bundle, match_m3u_playlist, now_ms,
    remap_stored_collections, remap_stored_stats, sanitize_export_filename,
    saved_backup_source_roots, validate_and_prepare_import,
};

use crate::engine::RustygradeEngine;
use crate::error::EngineError;

#[derive(Debug, Clone, uniffi::Record)]
pub struct BackupCoverEntry {
    pub name: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, Default, uniffi::Record)]
pub struct BackupBundleDto {
    pub collections_json: Option<String>,
    pub stats_json: Option<String>,
    pub settings_json: Option<String>,
    pub playlist_covers: Vec<BackupCoverEntry>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct BackupImportReportDto {
    pub collections_imported: bool,
    pub stats_imported: bool,
    pub settings_imported: bool,
    pub liked_count: u32,
    pub playlist_count: u32,
    pub smart_playlist_count: u32,
    pub stats_track_count: u32,
    pub tracks_exact: u32,
    pub tracks_remapped: u32,
    pub tracks_unmatched: u32,
    pub covers_restored: u32,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct BackupPreviewDto {
    pub has_collections: bool,
    pub has_stats: bool,
    pub has_settings: bool,
    pub liked_count: u32,
    pub playlist_count: u32,
    pub smart_playlist_count: u32,
    pub cover_count: u32,
    pub is_single_playlist_export: bool,
    pub single_playlist_name: Option<String>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct ExportedM3uDto {
    pub suggested_filename: String,
    pub content: String,
    pub track_count: u32,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct M3uImportReportDto {
    pub playlist_id: i64,
    pub playlist_name: String,
    pub matched_exact: u32,
    pub matched_remapped: u32,
    pub matched_metadata: u32,
    pub unmatched: u32,
}

fn map_backup_err(err: BackupError) -> EngineError {
    EngineError::Library(err.to_string())
}

fn bundle_to_dto(bundle: BackupBundle) -> BackupBundleDto {
    BackupBundleDto {
        collections_json: bundle.collections_json,
        stats_json: bundle.stats_json,
        settings_json: bundle.settings_json,
        playlist_covers: bundle
            .playlist_covers
            .into_iter()
            .map(|c| BackupCoverEntry {
                name: c.name,
                bytes: c.bytes,
            })
            .collect(),
    }
}

fn build_exported_m3u<'a>(name: &str, tracks: impl IntoIterator<Item = &'a Track>) -> ExportedM3uDto {
    let collected: Vec<&Track> = tracks.into_iter().collect();
    let content = export_m3u8(name, collected.iter().copied());
    ExportedM3uDto {
        suggested_filename: sanitize_export_filename(name, "m3u8"),
        content,
        track_count: collected.len() as u32,
    }
}

#[uniffi::export]
impl RustygradeEngine {
    /// Exports collections, play stats, settings, and referenced playlist covers as JSON/bytes
    /// compatible with the desktop `~/.rustygrade` directory.
    pub fn export_backup(&self) -> Result<BackupBundleDto, EngineError> {
        let collections = {
            let _guard = self.collections_lock.lock().unwrap_or_else(|p| p.into_inner());
            CollectionsStore::load()
        };
        let settings = {
            let _guard = self.settings_lock.lock().unwrap_or_else(|p| p.into_inner());
            AppSettings::load()
        };
        let stats = PlayStats::load_locked();
        let bundle = export_backup_bundle(&collections, &stats, &settings).map_err(map_backup_err)?;
        Ok(bundle_to_dto(bundle))
    }

    /// Exports a single custom playlist and its cover(s) as a desktop-compatible backup bundle.
    pub fn export_playlist_backup(&self, playlist_id: i64) -> Result<BackupBundleDto, EngineError> {
        let playlist = {
            let _guard = self.collections_lock.lock().unwrap_or_else(|p| p.into_inner());
            CollectionsStore::load()
                .playlists
                .into_iter()
                .find(|p| p.id == playlist_id)
                .ok_or_else(|| EngineError::Library(format!("playlist {playlist_id} not found")))?
        };
        let bundle = export_playlist_backup_bundle(&playlist).map_err(map_backup_err)?;
        Ok(bundle_to_dto(bundle))
    }

    /// Exports Favorites as an Extended M3U8 playlist.
    pub fn export_favorites_m3u8(&self) -> Result<ExportedM3uDto, EngineError> {
        let lib = self.library()?;
        let liked = {
            let _guard = self.collections_lock.lock().unwrap_or_else(|p| p.into_inner());
            CollectionsStore::load().liked
        };
        let lookup = lib.lookup();
        let tracks: Vec<&Track> = liked.iter().filter_map(|id| lookup.get(id)).collect();
        Ok(build_exported_m3u("Favorites", tracks))
    }

    /// Exports a custom playlist as an Extended M3U8 playlist.
    pub fn export_playlist_m3u8(&self, playlist_id: i64) -> Result<ExportedM3uDto, EngineError> {
        let lib = self.library()?;
        let playlist = {
            let _guard = self.collections_lock.lock().unwrap_or_else(|p| p.into_inner());
            CollectionsStore::load()
                .playlists
                .into_iter()
                .find(|p| p.id == playlist_id)
                .ok_or_else(|| EngineError::Library(format!("playlist {playlist_id} not found")))?
        };
        let lookup = lib.lookup();
        let tracks: Vec<&Track> = playlist
            .track_ids
            .iter()
            .filter_map(|id| lookup.get(id))
            .collect();
        Ok(build_exported_m3u(&playlist.name, tracks))
    }

    /// Exports an evaluated smart playlist as an Extended M3U8 playlist.
    pub fn export_smart_playlist_m3u8(&self, smart_id: i64) -> Result<ExportedM3uDto, EngineError> {
        let lib = self.library()?;
        let (smart, liked) = {
            let _guard = self.collections_lock.lock().unwrap_or_else(|p| p.into_inner());
            let col = CollectionsStore::load();
            let smart = col
                .smart
                .unwrap_or_default()
                .into_iter()
                .find(|s| s.id == smart_id)
                .ok_or_else(|| EngineError::Library(format!("smart playlist {smart_id} not found")))?;
            let liked: HashSet<String> = col.liked.into_iter().collect();
            (smart, liked)
        };
        let stats = PlayStats::load_locked();
        let tracks = SmartPlaylists::evaluate_albums(
            &smart,
            &lib.albums,
            |id| stats.get(id),
            &liked,
            now_ms(),
        );
        Ok(build_exported_m3u(&smart.name, tracks.iter()))
    }

    /// Imports an `.m3u` / `.m3u8` playlist, creating a new custom playlist when at least one track matches.
    pub fn import_m3u_playlist(
        &self,
        content: String,
        filename_hint: Option<String>,
        source_root: Option<String>,
    ) -> Result<M3uImportReportDto, EngineError> {
        let lib = self.library()?;
        let mut source_roots = Vec::new();
        if let Some(root) = source_root.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            source_roots.push(root.to_owned());
        }
        let saved_roots = {
            let _set_guard = self.settings_lock.lock().unwrap_or_else(|p| p.into_inner());
            saved_backup_source_roots(&AppSettings::load())
        };
        for root in saved_roots {
            if !source_roots.iter().any(|existing| existing == &root) {
                source_roots.push(root);
            }
        }

        let matched = match_m3u_playlist(
            &content,
            filename_hint.as_deref(),
            &lib.albums,
            &source_roots,
        )
        .map_err(|e| EngineError::Library(e.to_string()))?;

        let playlist_id = {
            let _col_guard = self.collections_lock.lock().unwrap_or_else(|p| p.into_inner());
            let mut col = CollectionsStore::load();
            let id = crate::collections_api::fresh_id(&col);
            col.playlists.push(PlaylistData {
                id,
                name: matched.playlist_name.clone(),
                cover_art: None,
                track_ids: matched.track_ids,
                ..PlaylistData::default()
            });
            CollectionsStore::try_save(&col)
                .map_err(|e| EngineError::Library(format!("could not save collections: {e}")))?;
            id
        };

        Ok(M3uImportReportDto {
            playlist_id,
            playlist_name: matched.playlist_name,
            matched_exact: matched.matched_exact as u32,
            matched_remapped: matched.matched_remapped as u32,
            matched_metadata: matched.matched_metadata as u32,
            unmatched: matched.unmatched as u32,
        })
    }

    /// Inspects a backup bundle before applying it so UI confirmation dialogs can warn
    /// clearly in Replace mode and detect single-playlist exports.
    pub fn inspect_backup_bundle(&self, bundle: BackupBundleDto) -> Result<BackupPreviewDto, EngineError> {
        if bundle.collections_json.is_none()
            && bundle.stats_json.is_none()
            && bundle.settings_json.is_none()
        {
            return Err(map_backup_err(BackupError::EmptyBackup));
        }
        for (what, raw) in [
            ("native-collections.json", &bundle.collections_json),
            ("native-stats.json", &bundle.stats_json),
            ("native-settings.json", &bundle.settings_json),
        ] {
            if raw.as_ref().is_some_and(|r| r.len() > MAX_BACKUP_JSON_BYTES) {
                return Err(map_backup_err(BackupError::TooLarge {
                    what,
                    limit: "16 MiB".to_owned(),
                }));
            }
        }
        if bundle.playlist_covers.len() > MAX_BACKUP_COVER_COUNT {
            return Err(map_backup_err(BackupError::TooLarge {
                what: "The playlist cover set",
                limit: format!("{MAX_BACKUP_COVER_COUNT} files"),
            }));
        }

        let col = match &bundle.collections_json {
            Some(raw) => Some(
                serde_json::from_str::<CollectionsData>(raw).map_err(|e| {
                    map_backup_err(BackupError::InvalidJson {
                        file: "native-collections.json",
                        message: e.to_string(),
                    })
                })?,
            ),
            None => None,
        };
        if let Some(raw) = &bundle.stats_json {
            serde_json::from_str::<PlayStats>(raw).map_err(|e| {
                map_backup_err(BackupError::InvalidJson {
                    file: "native-stats.json",
                    message: e.to_string(),
                })
            })?;
        }
        if let Some(raw) = &bundle.settings_json
            && AppSettings::parse_lenient(raw).is_none()
        {
            return Err(map_backup_err(BackupError::InvalidJson {
                file: "native-settings.json",
                message: "Expected a JSON object".to_owned(),
            }));
        }

        let has_collections = col.is_some();
        let has_stats = bundle.stats_json.is_some();
        let has_settings = bundle.settings_json.is_some();
        let (liked_count, playlist_count, smart_playlist_count, is_single_playlist_export, single_playlist_name) =
            match col {
                Some(c) => {
                    let single = !has_stats
                        && !has_settings
                        && c.playlists.len() == 1
                        && c.liked.is_empty()
                        && c.smart.is_none();
                    let name = single.then(|| c.playlists[0].name.clone());
                    (
                        c.liked.len() as u32,
                        c.playlists.len() as u32,
                        c.smart.as_ref().map_or(0, Vec::len) as u32,
                        single,
                        name,
                    )
                }
                None => (0, 0, 0, false, None),
            };

        Ok(BackupPreviewDto {
            has_collections,
            has_stats,
            has_settings,
            liked_count,
            playlist_count,
            smart_playlist_count,
            cover_count: bundle.playlist_covers.len() as u32,
            is_single_playlist_export,
            single_playlist_name,
        })
    }

    /// Validates and imports a backup bundle (merging into or replacing local collections/stats,
    /// remapping foreign/desktop paths onto local tracks, and preserving Android storage flags).
    /// Covers and track-ID remapping run outside all store locks so playback ticks never stall.
    pub fn import_backup(
        &self,
        bundle: BackupBundleDto,
        replace: bool,
        source_root: Option<String>,
    ) -> Result<BackupImportReportDto, EngineError> {
        let lib = self.library()?;
        let core_bundle = BackupBundle {
            collections_json: bundle.collections_json,
            stats_json: bundle.stats_json,
            settings_json: bundle.settings_json,
            playlist_covers: bundle
                .playlist_covers
                .into_iter()
                .map(|c| BackupCoverFile {
                    name: c.name,
                    bytes: c.bytes,
                })
                .collect(),
        };
        let options = BackupImportOptions {
            replace,
            source_root_override: source_root,
        };
        let mut prepared =
            validate_and_prepare_import(core_bundle, &lib.albums, &options).map_err(map_backup_err)?;
        let (restored_names, covers_restored) = prepared.write_covers().map_err(map_backup_err)?;

        let (collections_imported, liked_count, playlist_count, smart_playlist_count) = {
            let _col_guard = self.collections_lock.lock().unwrap_or_else(|p| p.into_inner());
            let mut collections = CollectionsStore::load();
            let imported = prepared
                .apply_collections(&mut collections, &restored_names)
                .map_err(map_backup_err)?;
            (
                imported,
                collections.liked.len(),
                collections.playlists.len(),
                collections.smart.as_ref().map_or(0, Vec::len),
            )
        };

        let (stats_imported, stats_track_count) = if prepared.has_stats() {
            PlayStats::with_lock(|stats| {
                let imported = prepared.apply_stats(stats)?;
                Ok::<(bool, usize), BackupError>((imported, stats.tracks.len()))
            })
            .map_err(map_backup_err)?
        } else {
            (false, PlayStats::load_locked().tracks.len())
        };

        let settings_imported = {
            let _set_guard = self.settings_lock.lock().unwrap_or_else(|p| p.into_inner());
            let mut settings = AppSettings::load();
            prepared.apply_settings(&mut settings).map_err(map_backup_err)?
        };

        Ok(BackupImportReportDto {
            collections_imported,
            stats_imported,
            settings_imported,
            liked_count: liked_count as u32,
            playlist_count: playlist_count as u32,
            smart_playlist_count: smart_playlist_count as u32,
            stats_track_count: stats_track_count as u32,
            tracks_exact: prepared.tracks_exact as u32,
            tracks_remapped: prepared.tracks_remapped as u32,
            tracks_unmatched: prepared.tracks_unmatched as u32,
            covers_restored: covers_restored as u32,
        })
    }
}

impl RustygradeEngine {
    /// Attempts to remap any unmatched (foreign/desktop) track IDs in favorites, playlists, and
    /// play stats onto the currently scanned local library. Returns how many entries remapped.
    /// Runs after every scan (not exported: Kotlin never needs to call it directly).
    pub(crate) fn remap_unmatched_ids(&self) -> Result<u32, EngineError> {
        let lib = self.library()?;
        if lib.albums.is_empty() {
            return Ok(0);
        }
        let index = lib.track_index();
        let source_roots = {
            let _set_guard = self.settings_lock.lock().unwrap_or_else(|p| p.into_inner());
            saved_backup_source_roots(&AppSettings::load())
        };

        // Quick pre-check: avoid allocating TrackPathMapper when all saved IDs already exist locally.
        let has_foreign_col = {
            let _col_guard = self.collections_lock.lock().unwrap_or_else(|p| p.into_inner());
            let col = CollectionsStore::load();
            col.liked.iter().any(|id| !index.contains_key(id))
                || col
                    .playlists
                    .iter()
                    .any(|p| p.track_ids.iter().any(|id| !index.contains_key(id)))
        };

        let has_foreign_stats = PlayStats::with_lock(|stats| {
            stats.tracks.keys().any(|id| !index.contains_key(id))
        });

        if !has_foreign_col && !has_foreign_stats {
            return Ok(0);
        }

        let mapper = TrackPathMapper::from_albums(&lib.albums, &source_roots);
        let mut total = 0usize;

        if has_foreign_col {
            let _col_guard = self.collections_lock.lock().unwrap_or_else(|p| p.into_inner());
            let mut col = CollectionsStore::load();
            let n = remap_stored_collections(&mut col, &mapper);
            if n > 0 {
                CollectionsStore::try_save(&col).map_err(|e| EngineError::Library(e.to_string()))?;
                total += n;
            }
        }

        if has_foreign_stats {
            total += PlayStats::with_lock(|stats| -> Result<usize, EngineError> {
                let n = remap_stored_stats(stats, &mapper);
                if n > 0 {
                    stats
                        .try_save()
                        .map_err(|e| EngineError::Library(e.to_string()))?;
                }
                Ok(n)
            })?;
        }

        Ok(total as u32)
    }
}

#[cfg(test)]
mod tests {
    use rustygrade_core::testing::{Fixture, TestEnv, write_flac, write_png};

    use super::*;
    use crate::dto::ScanRequest;

    #[test]
    fn ffi_export_and_import_roundtrip_and_desktop_migration() {
        let env = TestEnv::new("ffi-backup");
        write_flac(
            &env.music.join("Artist/Album/01.flac"),
            &Fixture::new("Song One", "Artist", "Album", 1).with_cover(),
        );
        let engine = RustygradeEngine::new(env.app_dir.to_string_lossy().into_owned());
        let req = ScanRequest {
            dirs: vec![env.music.to_string_lossy().into_owned()],
            ..ScanRequest::default()
        };
        engine.scan_sources(req, None, None).unwrap();

        let local_track_id = engine
            .get_album_detail("Album".into())
            .unwrap()
            .unwrap()
            .tracks[0]
            .id
            .clone();

        engine.add_saf_tree("content://tree/primary%3AMusic".into());
        engine.set_usb_bit_perfect(true);

        // Migrate a desktop ~/.rustygrade backup into this scanned Android library.
        let desktop_bundle = BackupBundleDto {
            collections_json: Some(
                r#"{
                    "liked": ["/home/okarin/Music/Artist/Album/01.flac"],
                    "playlists": [{
                        "id": 99,
                        "name": "Desktop Mix",
                        "trackIds": ["/home/okarin/Music/Artist/Album/01.flac"]
                    }]
                }"#
                .into(),
            ),
            stats_json: Some(
                r#"{"Tracks":{"/home/okarin/Music/Artist/Album/01.flac":{"Count":12,"Last":9000}}}"#
                    .into(),
            ),
            settings_json: Some(
                r#"{"Volume":0.7,"CrossfadeSeconds":5,"EqEnabled":true,"EqPreset":"WARM","LibraryFolders":["/home/okarin/Music"]}"#
                    .into(),
            ),
            playlist_covers: vec![],
        };

        let report = engine.import_backup(desktop_bundle, false, None).unwrap();
        assert_eq!(report.tracks_remapped, 1);
        assert_eq!(report.liked_count, 1);
        assert_eq!(report.playlist_count, 1);
        assert_eq!(report.stats_track_count, 1);

        let col = engine.load_collections();
        assert_eq!(col.liked, vec![local_track_id.clone()]);
        assert_eq!(col.playlists[0].track_ids, vec![local_track_id.clone()]);
        let stats = engine.load_stats();
        assert_eq!(stats.tracks[0].track_id, local_track_id);
        assert_eq!(stats.tracks[0].count, 12);

        let s = engine.load_settings();
        assert_eq!(s.crossfade_seconds, 5);
        assert!(s.eq_enabled && s.eq_preset == "WARM");
        assert_eq!(s.saf_trees, vec!["content://tree/primary%3AMusic"]);
        assert!(s.usb_bit_perfect);

        // Add a playlist cover locally, export, then import with replace into clean stores.
        let cover_img = env.root.join("pl.png");
        write_png(&cover_img, [20, 180, 220]);
        engine
            .set_playlist_cover(99, Some(cover_img.to_string_lossy().into_owned()))
            .unwrap();
        let exported = engine.export_backup().unwrap();
        assert!(!exported.playlist_covers.is_empty());

        let r2 = engine.import_backup(exported, true, None).unwrap();
        assert_eq!(r2.tracks_exact, 1);
        assert_eq!(r2.covers_restored, 1);
    }

    #[test]
    fn ffi_import_before_scan_auto_remaps_after_first_scan() {
        let env = TestEnv::new("ffi-backup-prescan");
        write_flac(
            &env.music.join("flat_track.flac"),
            &Fixture::new("Flat Song", "Artist", "Singles", 1),
        );
        let engine = RustygradeEngine::new(env.app_dir.to_string_lossy().into_owned());

        let desktop_bundle = BackupBundleDto {
            collections_json: Some(
                r#"{
                    "liked": ["/home/okarin/DesktopMusic/flat_track.flac"],
                    "playlists": [{
                        "id": 7,
                        "name": "Early Mix",
                        "trackIds": ["/home/okarin/DesktopMusic/flat_track.flac"]
                    }]
                }"#
                .into(),
            ),
            stats_json: Some(
                r#"{"Tracks":{"/home/okarin/DesktopMusic/flat_track.flac":{"Count":4,"Last":777}}}"#
                    .into(),
            ),
            ..Default::default()
        };

        let pre = engine
            .import_backup(desktop_bundle, false, Some("/home/okarin/DesktopMusic".into()))
            .unwrap();
        assert_eq!(pre.tracks_unmatched, 1);

        // Now perform the first scan; unmatched desktop IDs must remap automatically!
        let req = ScanRequest {
            dirs: vec![env.music.to_string_lossy().into_owned()],
            ..ScanRequest::default()
        };
        engine.scan_sources(req, None, None).unwrap();

        let local_id = engine
            .get_album_detail("Singles".into())
            .unwrap()
            .unwrap()
            .tracks[0]
            .id
            .clone();
        let col = engine.load_collections();
        assert_eq!(col.liked, vec![local_id.clone()]);
        assert_eq!(col.playlists[0].track_ids, vec![local_id.clone()]);
        let stats = engine.load_stats();
        assert_eq!(stats.tracks.len(), 1);
        assert_eq!(stats.tracks[0].track_id, local_id);
        assert_eq!(stats.tracks[0].count, 4);
    }

    #[test]
    fn ffi_m3u_export_and_import_roundtrip_and_zero_match_error() {
        let env = TestEnv::new("ffi-m3u");
        write_flac(
            &env.music.join("Artist/Album/01.flac"),
            &Fixture::new("Song One", "Artist", "Album", 1).with_seconds(195),
        );
        write_flac(
            &env.music.join("Artist/Album/02.flac"),
            &Fixture::new("Song Two", "Artist", "Album", 2).with_seconds(210),
        );
        let engine = RustygradeEngine::new(env.app_dir.to_string_lossy().into_owned());
        let req = ScanRequest {
            dirs: vec![env.music.to_string_lossy().into_owned()],
            ..ScanRequest::default()
        };
        engine.scan_sources(req, None, None).unwrap();

        let tracks = engine.get_album_detail("Album".into()).unwrap().unwrap().tracks;
        engine.set_liked(tracks[0].id.clone(), true);
        let custom = engine.create_playlist("My / Mix?".into());
        engine.add_to_playlist(custom.id, vec![tracks[0].id.clone(), tracks[1].id.clone()]);
        let smart_id = engine.load_collections().smart[0].id;

        let fav_m3u = engine.export_favorites_m3u8().unwrap();
        assert_eq!(fav_m3u.suggested_filename, "Favorites.m3u8");
        assert_eq!(fav_m3u.track_count, 1);
        assert!(fav_m3u.content.contains("#PLAYLIST:Favorites\n"));
        assert!(fav_m3u.content.contains(&tracks[0].file_path));

        let custom_m3u = engine.export_playlist_m3u8(custom.id).unwrap();
        assert_eq!(custom_m3u.suggested_filename, "My _ Mix.m3u8");
        assert_eq!(custom_m3u.track_count, 2);
        assert!(custom_m3u.content.contains("#EXTALB:Album\n"));

        let smart_m3u = engine.export_smart_playlist_m3u8(smart_id).unwrap();
        assert!(smart_m3u.content.starts_with("#EXTM3U\n"));

        // Import desktop M3U8 with Windows path + metadata fallback.
        let desktop_m3u = "\u{FEFF}#EXTM3U\r\n#PLAYLIST:Desktop Imports\r\n#EXTINF:195,Artist - Song One\r\nC:\\Users\\okarin\\Music\\Artist\\Album\\01.flac\r\n#EXTINF:210,Artist - Song Two\r\n#EXTALB:Album\r\n/moved/elsewhere/renamed.mp3\r\n#EXTINF:100,Missing - Track\r\n/missing/nope.flac\r\n";
        let report = engine
            .import_m3u_playlist(desktop_m3u.into(), Some("fallback.m3u8".into()), None)
            .unwrap();
        assert_eq!(report.playlist_name, "Desktop Imports");
        assert_eq!(report.matched_exact, 0);
        assert_eq!(report.matched_remapped, 1);
        assert_eq!(report.matched_metadata, 1);
        assert_eq!(report.unmatched, 1);

        let imported = engine.playlist(report.playlist_id).unwrap();
        assert_eq!(imported.track_ids, vec![tracks[0].id.clone(), tracks[1].id.clone()]);

        // Zero-match M3U fails without creating an empty playlist.
        let before_count = engine.load_collections().playlists.len();
        let err = engine
            .import_m3u_playlist(
                "#EXTM3U\n#EXTINF:90,Nobody - Ghost\n/nope/ghost.flac\n".into(),
                None,
                None,
            )
            .unwrap_err();
        assert!(err.to_string().contains("matched"), "{err}");
        assert_eq!(engine.load_collections().playlists.len(), before_count);
    }

    #[test]
    fn ffi_single_playlist_backup_export_and_merge_preserves_existing_data() {
        let env = TestEnv::new("ffi-single-zip");
        write_flac(
            &env.music.join("Artist/Album/01.flac"),
            &Fixture::new("Song One", "Artist", "Album", 1),
        );
        let engine = RustygradeEngine::new(env.app_dir.to_string_lossy().into_owned());
        engine
            .scan_sources(
                ScanRequest {
                    dirs: vec![env.music.to_string_lossy().into_owned()],
                    ..ScanRequest::default()
                },
                None,
                None,
            )
            .unwrap();
        let t_id = engine.get_album_detail("Album".into()).unwrap().unwrap().tracks[0].id.clone();

        engine.set_liked(t_id.clone(), true);
        let p1 = engine.create_playlist("Keep Me".into());
        let p2 = engine.create_playlist("Export Me".into());
        engine.add_to_playlist(p2.id, vec![t_id.clone()]);
        let img = env.root.join("cover.png");
        write_png(&img, [255, 100, 40]);
        engine
            .set_playlist_cover(p2.id, Some(img.to_string_lossy().into_owned()))
            .unwrap();

        let single = engine.export_playlist_backup(p2.id).unwrap();
        assert!(single.stats_json.is_none());
        assert!(single.settings_json.is_none());
        assert!(!single.playlist_covers.is_empty());

        // Delete p2, then import the single-playlist bundle back in merge mode.
        engine.delete_playlist(p2.id);
        assert!(engine.playlist(p2.id).is_none());

        let report = engine.import_backup(single, false, None).unwrap();
        assert_eq!(report.tracks_exact, 1);
        assert_eq!(report.covers_restored, 1);

        let col = engine.load_collections();
        assert_eq!(col.liked, vec![t_id]);
        assert!(col.playlists.iter().any(|p| p.id == p1.id && p.name == "Keep Me"));
        let restored = col.playlists.iter().find(|p| p.name == "Export Me").unwrap();
        assert!(restored.cover_path.as_deref().is_some_and(|p| std::path::Path::new(p).is_file()));
    }

    #[test]
    fn ffi_inspect_backup_bundle_distinguishes_full_and_single_playlist() {
        let env = TestEnv::new("ffi-inspect-backup");
        let engine = RustygradeEngine::new(env.app_dir.to_string_lossy().into_owned());
        let p = engine.create_playlist("Solo List".into());
        let single = engine.export_playlist_backup(p.id).unwrap();
        let preview = engine.inspect_backup_bundle(single).unwrap();
        assert!(preview.is_single_playlist_export);
        assert_eq!(preview.single_playlist_name.as_deref(), Some("Solo List"));
        assert_eq!((preview.playlist_count, preview.liked_count, preview.smart_playlist_count), (1, 0, 0));

        let full = engine.export_backup().unwrap();
        let full_preview = engine.inspect_backup_bundle(full).unwrap();
        assert!(!full_preview.is_single_playlist_export);
        assert!(full_preview.has_stats && full_preview.has_settings && full_preview.has_collections);
    }
}
