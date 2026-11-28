use super::*;

use std::collections::HashSet;
use std::fs;

use serde_json::Value;

use super::merge::{claim_id, merge_playlists, merge_smart_playlists};
use crate::collections::{CollectionsData, CollectionsStore, PlaylistData, SmartPlaylistData};
use crate::covers::PlaylistCovers;
use crate::library::LibraryStore;
use crate::models::Album;
use crate::models::{ReplayGain, Track};
use crate::paths::CoverPaths;
use crate::settings::{AppSettings, KEY_SAF_TREES};
use crate::smart::SmartPlaylists;
use crate::stats::PlayStats;
use crate::testing::{TestEnv, write_png};

fn mk_track(id: &str, file_path: &str, album: &str) -> Track {
    Track {
        id: id.to_owned(),
        title: id.to_owned(),
        artist: "Artist".to_owned(),
        album: album.to_owned(),
        duration: 180.0,
        file_path: file_path.to_owned(),
        play_uri: format!("file://{file_path}"),
        replay_gain: ReplayGain::default(),
        track_number: 1,
        disc_number: 1,
        codec: "FLAC".to_owned(),
        sample_rate: 44100,
        bits_per_sample: 16,
        channels: 2,
        cover_thumb_path: None,
        cover_full_path: None,
    }
}

fn mk_album(name: &str, tracks: Vec<Track>) -> Album {
    Album {
        name: name.to_owned(),
        artist: "Artist".to_owned(),
        cover_thumb_path: None,
        cover_full_path: None,
        year: 2024,
        added_at: 1,
        tracks,
    }
}

#[test]
fn remaps_linux_windows_and_saf_paths_without_false_bare_filename_matches() {
    let albums = vec![mk_album(
        "Alpha",
        vec![
            mk_track(
                "/storage/emulated/0/Music/Rock/Alpha/01.flac",
                "/storage/emulated/0/Music/Rock/Alpha/01.flac",
                "Alpha",
            ),
            mk_track(
                "/saf/primary:Music/Jazz/Beta/01.flac",
                "/saf/primary:Music/Jazz/Beta/01.flac",
                "Beta",
            ),
            mk_track(
                "/storage/emulated/0/Music/flat_unique.flac",
                "/storage/emulated/0/Music/flat_unique.flac",
                "Singles",
            ),
        ],
    )];

    let mapper = TrackPathMapper::from_albums(&albums, &["/home/okarin/MyLibrary".to_owned()]);

    assert_eq!(
        mapper.map_id("/storage/emulated/0/Music/Rock/Alpha/01.flac"),
        MapOutcome::Exact("/storage/emulated/0/Music/Rock/Alpha/01.flac".to_owned())
    );
    // Linux -> Android MediaStore via auto suffix (>= 2 trailing segments).
    assert_eq!(
        mapper.map_id("/home/okarin/Music/Rock/Alpha/01.flac"),
        MapOutcome::Remapped("/storage/emulated/0/Music/Rock/Alpha/01.flac".to_owned())
    );
    // Linux -> Android SAF path.
    assert_eq!(
        mapper.map_id("/home/okarin/Music/Jazz/Beta/01.flac"),
        MapOutcome::Remapped("/saf/primary:Music/Jazz/Beta/01.flac".to_owned())
    );
    // Windows backslash path -> Android SAF path.
    assert_eq!(
        mapper.map_id(r"D:\Tunes\Jazz\Beta\01.flac"),
        MapOutcome::Remapped("/saf/primary:Music/Jazz/Beta/01.flac".to_owned())
    );
    // Ambiguous bare filename in unrelated folder must NOT falsely guess.
    assert_eq!(
        mapper.map_id("/tmp/OtherAlbum/01.flac"),
        MapOutcome::Unmatched("/tmp/OtherAlbum/01.flac".to_owned())
    );
    // Flat file directly under explicit source_root remaps uniquely.
    assert_eq!(
        mapper.map_id("/home/okarin/MyLibrary/flat_unique.flac"),
        MapOutcome::Remapped("/storage/emulated/0/Music/flat_unique.flac".to_owned())
    );
    // Flat file NOT under source_root has only 1 matching segment, so it stays unmatched.
    assert_eq!(
        mapper.map_id("/elsewhere/flat_unique.flac"),
        MapOutcome::Unmatched("/elsewhere/flat_unique.flac".to_owned())
    );
}

#[test]
fn generic_disc_folder_requires_album_parent_match() {
    let albums = vec![mk_album(
        "AlbumW",
        vec![mk_track(
            "/storage/0/Music/Z/AlbumW/CD1/01.flac",
            "/storage/0/Music/Z/AlbumW/CD1/01.flac",
            "AlbumW",
        )],
    )];
    let mapper = TrackPathMapper::from_albums(&albums, &[]);

    // Only CD1/01.flac matches (2 segments), parent AlbumY != AlbumW -> must NOT match.
    assert_eq!(
        mapper.map_id("/home/okarin/Music/X/AlbumY/CD1/01.flac"),
        MapOutcome::Unmatched("/home/okarin/Music/X/AlbumY/CD1/01.flac".to_owned())
    );
    // Same for "Disc 1" / "disc_01".
    assert_eq!(
        mapper.map_id("/home/okarin/Music/X/AlbumY/Disc 1/01.flac"),
        MapOutcome::Unmatched("/home/okarin/Music/X/AlbumY/Disc 1/01.flac".to_owned())
    );
    // AlbumW/CD1/01.flac matches 3 segments -> remapped.
    assert_eq!(
        mapper.map_id("/home/okarin/Music/X/AlbumW/CD1/01.flac"),
        MapOutcome::Remapped("/storage/0/Music/Z/AlbumW/CD1/01.flac".to_owned())
    );
}

#[test]
fn colon_in_linux_folder_name_is_not_treated_as_drive_or_saf_prefix() {
    let albums = vec![mk_album(
        "Re:Zero",
        vec![mk_track(
            "/storage/emulated/0/Music/Re:Zero/01.flac",
            "/storage/emulated/0/Music/Re:Zero/01.flac",
            "Re:Zero",
        )],
    )];
    let mapper = TrackPathMapper::from_albums(&albums, &[]);
    assert_eq!(
        mapper.map_id("/Re:Zero/01.flac"),
        MapOutcome::Remapped("/storage/emulated/0/Music/Re:Zero/01.flac".to_owned())
    );
}

#[test]
fn merge_is_idempotent_and_preserves_android_settings() {
    let _env = TestEnv::new("backup-merge");
    let albums = vec![mk_album(
        "A",
        vec![
            mk_track("/storage/0/Music/A/01.flac", "/storage/0/Music/A/01.flac", "A"),
            mk_track("/storage/0/Music/A/02.flac", "/storage/0/Music/A/02.flac", "A"),
        ],
    )];

    let mut local_settings = AppSettings {
        library_folders: vec!["/android/local".to_owned()],
        ..Default::default()
    };
    local_settings.extra.insert(
        KEY_SAF_TREES.to_owned(),
        Value::Array(vec![Value::String("content://tree/1".to_owned())]),
    );
    local_settings
        .extra
        .insert("AndroidUseMediaStore".to_owned(), Value::Bool(false));
    local_settings
        .extra
        .insert("AndroidUsbBitPerfect".to_owned(), Value::Bool(true));
    local_settings.save();

    let mut local_stats = PlayStats::default();
    local_stats.record_at("/storage/0/Music/A/01.flac", 100);
    local_stats.save();

    let mk_bundle = || BackupBundle {
        collections_json: Some(
            r#"{
                "liked": ["/home/okarin/Music/A/01.flac", "/home/okarin/Music/Missing/99.flac"],
                "playlists": [{
                    "id": 77,
                    "name": "Favorites Mix",
                    "trackIds": ["/home/okarin/Music/A/01.flac", "/home/okarin/Music/A/02.flac"]
                }],
                "smart": null
            }"#
            .to_owned(),
        ),
        stats_json: Some(
            r#"{"Tracks":{"/home/okarin/Music/A/01.flac":{"Count":5,"Last":500}}}"#.to_owned(),
        ),
        settings_json: Some(
            r#"{
                "Volume": 0.85,
                "LibrarySort": "plays_desc",
                "ReplayGain": "album",
                "CrossfadeSeconds": 6,
                "EqEnabled": true,
                "EqPreset": "BASS",
                "EqGains": [4, 3, 0, 0, 0, 0, 0, 0, 0, 0],
                "AccentFromCover": false,
                "LibraryFolders": ["/home/okarin/Music"],
                "AndroidSafTrees": ["content://tree/evil"],
                "AndroidUseMediaStore": true,
                "AndroidUsbBitPerfect": false,
                "DiscordClientId": "12345"
            }"#
            .to_owned(),
        ),
        playlist_covers: Vec::new(),
    };

    let r1 = import_backup_bundle(mk_bundle(), &albums, &BackupImportOptions::default()).unwrap();
    assert!(r1.collections_imported && r1.stats_imported && r1.settings_imported);
    assert_eq!(r1.tracks_remapped, 2);
    assert_eq!(r1.tracks_unmatched, 1);

    // Re-importing the exact same backup must be idempotent (counts do NOT become 10!).
    let _r2 = import_backup_bundle(mk_bundle(), &albums, &BackupImportOptions::default()).unwrap();
    let stats = PlayStats::load();
    assert_eq!(stats.get("/storage/0/Music/A/01.flac").unwrap().count, 5);
    assert_eq!(stats.get("/storage/0/Music/A/01.flac").unwrap().last, 500);

    let col = CollectionsStore::load();
    assert_eq!(
        col.liked,
        vec![
            "/storage/0/Music/A/01.flac".to_owned(),
            "/home/okarin/Music/Missing/99.flac".to_owned(),
        ]
    );
    assert_eq!(col.playlists.len(), 1);
    assert_eq!(col.playlists[0].track_ids.len(), 2);

    let s = AppSettings::load();
    assert_eq!(s.library_folders, vec!["/android/local".to_owned()]);
    assert_eq!(s.crossfade_seconds, 6);
    assert!(s.eq_enabled && s.eq_preset == "BASS");
    assert_eq!(s.replay_gain, "album");
    assert!(!s.accent_from_cover);
    assert_eq!(s.extra.get("DiscordClientId"), Some(&Value::from("12345")), "desktop keys ride along in extra");
    assert_eq!(
        s.extra.get("AndroidSafTrees"),
        Some(&Value::Array(vec![Value::String("content://tree/1".to_owned())]))
    );
    assert_eq!(s.extra.get("AndroidUseMediaStore"), Some(&Value::Bool(false)));
    assert_eq!(s.extra.get("AndroidUsbBitPerfect"), Some(&Value::Bool(true)));
    assert_eq!(
        saved_backup_source_roots(&s),
        vec!["/home/okarin/Music".to_owned()]
    );
}

#[test]
fn merge_mode_deletes_replaced_old_playlist_cover_after_save() {
    let env = TestEnv::new("backup-merge-cover");
    let old_img = env.root.join("cover_old.png");
    let new_img = env.root.join("cover_new.png");
    write_png(&old_img, [40, 200, 120]);
    write_png(&new_img, [220, 40, 120]);
    let old_url = PlaylistCovers::save(&old_img).unwrap();
    let old_full = CoverPaths::to_local_path(Some(&old_url)).unwrap();
    CollectionsStore::save(&CollectionsData {
        playlists: vec![PlaylistData {
            id: 10,
            name: "P".to_owned(),
            cover_art: Some(old_url),
            ..Default::default()
        }],
        ..Default::default()
    });

    let new_url = PlaylistCovers::save(&new_img).unwrap();
    let new_full = CoverPaths::to_local_path(Some(&new_url)).unwrap();
    let new_name = new_full.file_name().unwrap().to_str().unwrap().to_owned();
    let new_bytes = fs::read(&new_full).unwrap();
    PlaylistCovers::delete(Some(&new_url));

    let bundle = BackupBundle {
        collections_json: Some(format!(
            r#"{{"playlists":[{{"id":10,"name":"P","coverArt":"/home/x/.rustygrade/covers/{new_name}"}}]}}"#
        )),
        playlist_covers: vec![BackupCoverFile {
            name: new_name.clone(),
            bytes: new_bytes,
        }],
        ..Default::default()
    };

    import_backup_bundle(bundle, &[], &BackupImportOptions::default()).unwrap();
    assert!(!old_full.exists(), "replaced old cover deleted");
    assert!(CoverPaths::cover_dir().join(&new_name).exists());
}

#[test]
fn replace_mode_replaces_collections_stats_and_prunes_old_covers() {
    let env = TestEnv::new("backup-replace");
    let albums = vec![mk_album(
        "A",
        vec![mk_track("/m/A/01.flac", "/m/A/01.flac", "A")],
    )];
    let old_img = env.root.join("cover_old.png");
    let new_img = env.root.join("cover_new.png");
    write_png(&old_img, [40, 200, 120]);
    write_png(&new_img, [10, 90, 220]);
    let old_url = PlaylistCovers::save(&old_img).unwrap();
    let old_full = CoverPaths::to_local_path(Some(&old_url)).unwrap();
    assert!(old_full.exists());

    CollectionsStore::save(&CollectionsData {
        liked: vec!["/m/A/01.flac".to_owned()],
        playlists: vec![PlaylistData {
            id: 10,
            name: "Old".to_owned(),
            cover_art: Some(old_url),
            track_ids: vec!["/m/A/01.flac".to_owned()],
            ..Default::default()
        }],
        smart: Some(SmartPlaylists::examples()),
        ..Default::default()
    });
    let mut old_stats = PlayStats::default();
    old_stats.record_at("/m/A/01.flac", 10);
    old_stats.save();

    // Export a new cover via a temporary file then import replace.
    let new_url = PlaylistCovers::save(&new_img).unwrap();
    let new_full = CoverPaths::to_local_path(Some(&new_url)).unwrap();
    let new_name = new_full.file_name().unwrap().to_str().unwrap().to_owned();
    let new_bytes = fs::read(&new_full).unwrap();
    fs::remove_file(&new_full).unwrap();

    let bundle = BackupBundle {
        collections_json: Some(format!(
            r#"{{
                "liked": [],
                "playlists": [{{"id": 20, "name": "New", "coverArt": "/home/user/.rustygrade/covers/{new_name}", "trackIds": ["/m/A/01.flac"]}}],
                "smart": []
            }}"#
        )),
        stats_json: Some(r#"{"Tracks":{}}"#.to_owned()),
        settings_json: None,
        playlist_covers: vec![BackupCoverFile {
            name: new_name.clone(),
            bytes: new_bytes,
        }],
    };

    let report = import_backup_bundle(
        bundle,
        &albums,
        &BackupImportOptions {
            replace: true,
            source_root_override: None,
        },
    )
    .unwrap();
    assert_eq!(report.covers_restored, 1);
    assert!(!old_full.exists(), "orphaned old cover pruned");
    assert!(CoverPaths::cover_dir().join(&new_name).exists());

    let col = CollectionsStore::load();
    assert!(col.liked.is_empty());
    assert_eq!(col.playlists.len(), 1);
    assert_eq!(col.playlists[0].name, "New");
    assert_eq!(
        CoverPaths::to_local_path(col.playlists[0].cover_art.as_deref()),
        Some(CoverPaths::cover_dir().join(&new_name))
    );
    assert!(PlayStats::load().tracks.is_empty());
}

#[test]
fn replace_mode_preserves_content_cover_shared_with_album() {
    let env = TestEnv::new("backup-replace-album-shared");
    let shared_img = env.root.join("shared.png");
    write_png(&shared_img, [15, 160, 225]);
    let shared_path = crate::covers::save_cover_bytes(&fs::read(&shared_img).unwrap()).unwrap();
    let shared_url = CoverPaths::to_file_url(&shared_path);

    let mut album = mk_album("Album Shared", vec![mk_track("/m/s.flac", "/m/s.flac", "Album Shared")]);
    album.cover_thumb_path = Some(shared_path.clone());
    album.cover_full_path = Some(shared_path.clone());
    {
        let _g = LibraryStore::write_lock().lock().unwrap_or_else(|p| p.into_inner());
        let map: crate::library::LibraryMap = serde_json::from_str(&format!(
            r#"{{"Album Shared":{{"name":"Album Shared","artist":"Artist","coverArt":"{shared_url}","coverArtFull":"{shared_url}","tracks":[{{"id":"/m/s.flac","filePath":"/m/s.flac"}}]}}}}"#,
        ))
        .unwrap();
        LibraryStore::save_raw(&map).unwrap();
    }

    CollectionsStore::save(&CollectionsData {
        playlists: vec![PlaylistData {
            id: 1,
            name: "Old".to_owned(),
            cover_art: Some(shared_url),
            ..Default::default()
        }],
        ..Default::default()
    });

    let bundle = BackupBundle {
        collections_json: Some(r#"{"liked": [], "playlists": []}"#.to_owned()),
        ..Default::default()
    };
    import_backup_bundle(
        bundle,
        &[album],
        &BackupImportOptions {
            replace: true,
            source_root_override: None,
        },
    )
    .unwrap();
    assert!(shared_path.exists(), "cover referenced by an album must survive replace import");
}

#[test]
fn post_scan_remap_resolves_unmatched_desktop_ids() {
    let _env = TestEnv::new("backup-post-remap");
    let desktop_bundle = BackupBundle {
        collections_json: Some(
            r#"{
                "liked": ["/home/okarin/Music/flat.flac", "/home/okarin/Music/Alb/01.flac"],
                "playlists": [{
                    "id": 1,
                    "name": "Mix",
                    "trackIds": ["/home/okarin/Music/Alb/01.flac", "/home/okarin/Music/flat.flac"]
                }]
            }"#
            .to_owned(),
        ),
        stats_json: Some(
            r#"{"Tracks":{"/home/okarin/Music/flat.flac":{"Count":8,"Last":200}}}"#.to_owned(),
        ),
        ..Default::default()
    };

    // Import BEFORE scanning: 0 albums -> all IDs stored as raw desktop paths.
    let r1 = import_backup_bundle(
        desktop_bundle,
        &[],
        &BackupImportOptions {
            replace: false,
            source_root_override: Some("/home/okarin/Music".to_owned()),
        },
    )
    .unwrap();
    assert_eq!(r1.tracks_unmatched, 2);

    // Library is later scanned on Android.
    let albums = vec![mk_album(
        "Alb",
        vec![
            mk_track("/saf/primary:Music/flat.flac", "/saf/primary:Music/flat.flac", "Alb"),
            mk_track("/saf/primary:Music/Alb/01.flac", "/saf/primary:Music/Alb/01.flac", "Alb"),
        ],
    )];
    let settings = AppSettings::load();
    let mapper = TrackPathMapper::from_albums(&albums, &saved_backup_source_roots(&settings));

    let mut col = CollectionsStore::load();
    let remapped_col = remap_stored_collections(&mut col, &mapper);
    assert_eq!(remapped_col, 4);
    assert_eq!(
        col.liked,
        vec![
            "/saf/primary:Music/flat.flac".to_owned(),
            "/saf/primary:Music/Alb/01.flac".to_owned(),
        ]
    );
    assert_eq!(
        col.playlists[0].track_ids,
        vec![
            "/saf/primary:Music/Alb/01.flac".to_owned(),
            "/saf/primary:Music/flat.flac".to_owned(),
        ]
    );

    let mut stats = PlayStats::load();
    let remapped_stats = remap_stored_stats(&mut stats, &mapper);
    assert_eq!(remapped_stats, 1);
    assert_eq!(stats.get("/saf/primary:Music/flat.flac").unwrap().count, 8);
    assert!(stats.get("/home/okarin/Music/flat.flac").is_none());
}

#[test]
fn large_stats_merge_is_fast_linear_time() {
    let _env = TestEnv::new("backup-large-stats");
    let mut tracks = serde_json::Map::new();
    for i in 0..5_000 {
        tracks.insert(
            format!("/home/okarin/Music/Album{}/track_{i}.flac", i / 10),
            serde_json::json!({"Count": (i % 20) + 1, "Last": 1000 + i}),
        );
    }
    let stats_json = Value::Object({
        let mut m = serde_json::Map::new();
        m.insert("Tracks".to_owned(), Value::Object(tracks));
        m
    })
    .to_string();

    let bundle = BackupBundle {
        stats_json: Some(stats_json),
        ..Default::default()
    };
    let report = import_backup_bundle(bundle, &[], &BackupImportOptions::default()).unwrap();
    assert_eq!(report.stats_track_count, 5_000);
}

#[test]
fn export_and_validation_rejects_bad_json_empty_and_traversal_covers() {
    let _env = TestEnv::new("backup-validation");

    // Empty bundle rejected.
    assert!(matches!(
        import_backup_bundle(BackupBundle::default(), &[], &BackupImportOptions::default()),
        Err(BackupError::EmptyBackup)
    ));

    // Seed existing data; an invalid import must leave it untouched.
    CollectionsStore::save(&CollectionsData {
        liked: vec!["keep-me".to_owned()],
        ..Default::default()
    });
    let corrupt = BackupBundle {
        collections_json: Some("{not valid json".to_owned()),
        stats_json: Some(r#"{"Tracks":{"x":{"Count":1,"Last":1}}}"#.to_owned()),
        ..Default::default()
    };
    assert!(matches!(
        import_backup_bundle(corrupt, &[], &BackupImportOptions::default()),
        Err(BackupError::InvalidJson {
            file: "native-collections.json",
            ..
        })
    ));
    assert_eq!(CollectionsStore::load().liked, vec!["keep-me".to_owned()]);
    assert!(PlayStats::load().tracks.is_empty());

    // Path traversal cover rejected.
    let traversal = BackupBundle {
        collections_json: Some(r#"{"liked":[]}"#.to_owned()),
        playlist_covers: vec![BackupCoverFile {
            name: "../native_pl_1_1_full.jpg".to_owned(),
            bytes: vec![1, 2, 3],
        }],
        ..Default::default()
    };
    assert!(matches!(
        import_backup_bundle(traversal, &[], &BackupImportOptions::default()),
        Err(BackupError::InvalidCover { .. })
    ));

    // Insane / corrupt image bytes rejected.
    let bad_img = BackupBundle {
        collections_json: Some(r#"{"liked":[]}"#.to_owned()),
        playlist_covers: vec![BackupCoverFile {
            name: "native_pl_1_1_full.jpg".to_owned(),
            bytes: b"not an image".to_vec(),
        }],
        ..Default::default()
    };
    assert!(matches!(
        import_backup_bundle(bad_img, &[], &BackupImportOptions::default()),
        Err(BackupError::InvalidCover { .. })
    ));
}

#[test]
fn export_bundle_captures_json_and_local_playlist_covers() {
    let env = TestEnv::new("backup-export");
    let img = env.root.join("pl.png");
    write_png(&img, [255, 100, 10]);
    let cover_url = PlaylistCovers::save(&img).unwrap();

    let collections = CollectionsData {
        liked: vec!["/m/01.flac".to_owned()],
        playlists: vec![PlaylistData {
            id: 1,
            name: "Test".to_owned(),
            cover_art: Some(cover_url),
            track_ids: vec!["/m/01.flac".to_owned()],
            ..Default::default()
        }],
        smart: Some(SmartPlaylists::examples()),
        ..Default::default()
    };
    let mut stats = PlayStats::default();
    stats.record_at("/m/01.flac", 12345);
    let settings = AppSettings::default();

    let bundle = export_backup_bundle(&collections, &stats, &settings).unwrap();
    assert!(bundle.collections_json.is_some());
    assert!(bundle.stats_json.is_some());
    assert!(bundle.settings_json.is_some());
    assert_eq!(bundle.playlist_covers.len(), 1, "single content-addressed cover exported");
    assert!(CoverPaths::is_content_cover_file(&CoverPaths::cover_dir().join(&bundle.playlist_covers[0].name)));
}

#[test]
fn oversized_json_payloads_and_cover_counts_are_rejected_cleanly() {
    let big = format!("{{\"liked\":[\"{}\"]}}", "a".repeat(MAX_BACKUP_JSON_BYTES));
    let opts = BackupImportOptions::default();
    for bundle in [
        BackupBundle { collections_json: Some(big.clone()), ..Default::default() },
        BackupBundle { stats_json: Some(big.clone()), ..Default::default() },
        BackupBundle { settings_json: Some(big), ..Default::default() },
    ] {
        let err = validate_and_prepare_import(bundle, &[], &opts).err().expect("too large");
        assert!(matches!(err, BackupError::TooLarge { .. }), "{err}");
        assert!(err.to_string().contains("too large"), "{err}");
    }
    let covers = (0..=MAX_BACKUP_COVER_COUNT)
        .map(|i| BackupCoverFile { name: format!("native_pl_{i}_1_full.jpg"), bytes: vec![0] })
        .collect();
    let bundle = BackupBundle {
        collections_json: Some("{}".to_owned()),
        playlist_covers: covers,
        ..Default::default()
    };
    let err = validate_and_prepare_import(bundle, &[], &opts).err().expect("too many covers");
    assert!(matches!(err, BackupError::TooLarge { .. }), "{err}");
    // A payload right under the cap still goes through the normal parsing.
    let ok = BackupBundle { collections_json: Some("{}".to_owned()), ..Default::default() };
    assert!(validate_and_prepare_import(ok, &[], &opts).is_ok());
}

#[test]
fn claimed_ids_never_overflow() {
    let mut used: HashSet<i64> = [i64::MAX, 1].into_iter().collect();
    assert_eq!(claim_id(&mut used, i64::MAX), 2, "wraps past 1, which is taken");
    assert_eq!(claim_id(&mut used, 0), 3, "ids below 1 start at 1");
    assert_eq!(claim_id(&mut used, 50), 50);
    assert_eq!(claim_id(&mut used, 50), 51);
}

#[test]
fn merging_playlists_with_the_largest_id_does_not_panic() {
    let mk = |id: i64, name: &str| PlaylistData { id, name: name.to_owned(), ..Default::default() };
    let mut existing = vec![mk(i64::MAX, "max")];
    let mut replaced = Vec::new();
    merge_playlists(&mut existing, vec![mk(7, "seven"), mk(0, "zero")], &mut replaced);
    let ids: HashSet<i64> = existing.iter().map(|p| p.id).collect();
    assert_eq!(ids.len(), 3);
    let mut smart = vec![SmartPlaylistData { id: i64::MAX, ..Default::default() }];
    merge_smart_playlists(&mut smart, vec![SmartPlaylistData { id: 0, ..Default::default() }]);
    assert_eq!(smart.len(), 2);
}

#[test]
fn remapping_stats_keeps_case_insensitive_lookup_fresh() {
    let mut stats = PlayStats::default();
    stats.record_at("/old/Song.flac", 5);
    assert!(stats.get("/old/song.flac").is_some()); // builds the lookup index
    let albums = vec![Album {
        name: "A".to_owned(),
        artist: "x".to_owned(),
        cover_thumb_path: None,
        cover_full_path: None,
        year: 0,
        added_at: 0,
        tracks: vec![crate::models::Track {
            id: "/new/Song.flac".to_owned(),
            title: "Song".to_owned(),
            artist: "x".to_owned(),
            album: "A".to_owned(),
            duration: 1.0,
            file_path: "/new/Song.flac".to_owned(),
            play_uri: String::new(),
            replay_gain: Default::default(),
            track_number: 0,
            disc_number: 1,
            codec: String::new(),
            sample_rate: 0,
            bits_per_sample: 0,
            channels: 0,
            cover_thumb_path: None,
            cover_full_path: None,
        }],
    }];
    let mapper = TrackPathMapper::from_albums(&albums, &["/old".to_owned()]);
    assert_eq!(remap_stored_stats(&mut stats, &mapper), 1);
    assert!(stats.get("/NEW/song.FLAC").is_some());
    assert!(stats.get("/old/song.flac").is_none());
}

#[test]
fn desktop_content_addressed_covers_are_validated_normalized_and_pruned_safely() {
    let env = TestEnv::new("backup-content-covers");
    let img = env.root.join("cover.png");
    write_png(&img, [30, 140, 210]);
    let cover_bytes = fs::read(&img).unwrap();

    let ref_hash = "0123456789abcdef01234567";
    let orphan_hash = "fedcba9876543210fedcba98";
    let raw_ref_name = format!("c_{ref_hash}.JPG");
    let norm_ref_name = format!("c_{ref_hash}.jpg");

    let bundle = BackupBundle {
        collections_json: Some(format!(
            r#"{{
                "playlists": [{{
                    "id": 42,
                    "name": "Desktop Mix",
                    "coverArt": "file:///home/okarin/.rustygrade/covers/{raw_ref_name}",
                    "trackIds": ["/home/okarin/Music/01.flac"]
                }}]
            }}"#
        )),
        playlist_covers: vec![
            BackupCoverFile {
                name: raw_ref_name,
                bytes: cover_bytes.clone(),
            },
            BackupCoverFile {
                name: format!("c_{orphan_hash}.png"),
                bytes: cover_bytes.clone(),
            },
        ],
        ..Default::default()
    };

    let report = import_backup_bundle(bundle, &[], &BackupImportOptions::default()).unwrap();
    assert_eq!(report.covers_restored, 1);
    assert!(CoverPaths::cover_dir().join(&norm_ref_name).is_file());
    assert!(!CoverPaths::cover_dir().join(format!("c_{orphan_hash}.png")).exists());

    let col = CollectionsStore::load();
    assert_eq!(
        CoverPaths::to_local_path(col.playlists[0].cover_art.as_deref()),
        Some(CoverPaths::cover_dir().join(&norm_ref_name))
    );

    for bad_name in [
        "c_0123456789abcdef0123456.jpg",
        "c_0123456789ABCDEF01234567.jpg",
        "c_0123456789abcdef01234567.gif",
        "../c_0123456789abcdef01234567.jpg",
        "album_thumb.jpg",
    ] {
        let bad = BackupBundle {
            collections_json: Some("{}".to_owned()),
            playlist_covers: vec![BackupCoverFile {
                name: bad_name.to_owned(),
                bytes: cover_bytes.clone(),
            }],
            ..Default::default()
        };
        assert!(
            matches!(
                validate_and_prepare_import(bad, &[], &BackupImportOptions::default()),
                Err(BackupError::InvalidCover { .. })
            ),
            "expected invalid cover for {bad_name}"
        );
    }
}

#[test]
fn export_playlist_backup_bundle_exports_only_that_playlist_and_its_cover() {
    let _env = TestEnv::new("backup-single-pl-export");
    fs::create_dir_all(CoverPaths::cover_dir()).unwrap();
    let cover_name = "c_0123456789abcdef01234567.png";
    let cover_path = CoverPaths::cover_dir().join(cover_name);
    write_png(&cover_path, [90, 200, 110]);

    let pl = PlaylistData {
        id: 55,
        name: "Single Export".to_owned(),
        cover_art: Some(CoverPaths::to_file_url(&cover_path)),
        track_ids: vec!["/m/01.flac".to_owned(), "/m/02.flac".to_owned()],
        ..Default::default()
    };

    let bundle = export_playlist_backup_bundle(&pl).unwrap();
    assert!(bundle.stats_json.is_none());
    assert!(bundle.settings_json.is_none());
    assert_eq!(bundle.playlist_covers.len(), 1);
    assert_eq!(bundle.playlist_covers[0].name, cover_name);

    let parsed: CollectionsData =
        serde_json::from_str(bundle.collections_json.as_deref().unwrap()).unwrap();
    assert!(parsed.liked.is_empty());
    assert!(parsed.smart.is_none());
    assert_eq!(parsed.playlists.len(), 1);
    assert_eq!(parsed.playlists[0].id, 55);
    assert_eq!(parsed.playlists[0].name, "Single Export");
    assert_eq!(parsed.playlists[0].track_ids, vec!["/m/01.flac", "/m/02.flac"]);
}

#[test]
fn merge_playlists_avoids_merging_unrelated_playlists_with_colliding_id() {
    let mk = |id: i64, name: &str, tracks: &[&str]| PlaylistData {
        id,
        name: name.to_owned(),
        track_ids: tracks.iter().map(|s| (*s).to_owned()).collect(),
        ..Default::default()
    };
    let mut existing = vec![
        mk(1, "Gym", &["/a.flac"]),
        mk(2, "   ", &["/d.flac"]),
    ];
    let mut replaced = Vec::new();
    merge_playlists(
        &mut existing,
        vec![
            mk(1, "Chill", &["/b.flac"]),
            mk(1, "gym", &["/c.flac"]),
            mk(2, "Adopted Name", &["/e.flac"]),
        ],
        &mut replaced,
    );

    assert_eq!(existing.len(), 3);
    let gym = existing.iter().find(|p| p.id == 1).unwrap();
    assert_eq!(gym.name, "Gym");
    assert_eq!(gym.track_ids, vec!["/a.flac", "/c.flac"]);

    let chill = existing.iter().find(|p| p.name == "Chill").unwrap();
    assert_ne!(chill.id, 1);
    assert_ne!(chill.id, 2);
    assert_eq!(chill.track_ids, vec!["/b.flac"]);

    let adopted = existing.iter().find(|p| p.id == 2).unwrap();
    assert_eq!(adopted.name, "Adopted Name");
    assert_eq!(adopted.track_ids, vec!["/d.flac", "/e.flac"]);
}

#[test]
fn relative_desktop_cover_path_shared_cover_protection_and_smart_collision_safety() {
    let env = TestEnv::new("backup-rel-cover-shared");
    let img1 = env.root.join("c1.png");
    let img2 = env.root.join("c2.png");
    write_png(&img1, [10, 120, 200]);
    write_png(&img2, [220, 40, 40]);
    let bytes1 = fs::read(&img1).unwrap();
    let bytes2 = fs::read(&img2).unwrap();

    let hash1 = "111111111111111111111111";
    let hash2 = "222222222222222222222222";
    let rel_name1 = format!("c_{hash1}.png");

    // Desktop exports relative `"covers/c_<hash>.png"` in JSON.
    let initial_bundle = BackupBundle {
        collections_json: Some(format!(
            r#"{{
                "playlists": [
                    {{"id": 10, "name": "One", "coverArt": "covers/{rel_name1}", "trackIds": []}},
                    {{"id": 20, "name": "Two", "coverArt": "covers/{rel_name1}", "trackIds": []}}
                ],
                "smart": [
                    {{"id": 5, "name": "Local Smart", "matchAll": true, "rules": [], "sort": "album", "limit": 0}}
                ]
            }}"#
        )),
        playlist_covers: vec![BackupCoverFile {
            name: rel_name1.clone(),
            bytes: bytes1.clone(),
        }],
        ..Default::default()
    };
    import_backup_bundle(initial_bundle, &[], &BackupImportOptions::default()).unwrap();
    let path1 = CoverPaths::cover_dir().join(&rel_name1);
    assert!(path1.is_file());

    // Merge-mode import replaces playlist 10's cover while playlist 20 still shares `rel_name1`,
    // attempts to overwrite existing `rel_name1` with different bytes (must NOT overwrite!),
    // and collides on smart playlist id 5 with a different name (must allocate fresh id).
    let second_bundle = BackupBundle {
        collections_json: Some(format!(
            r#"{{
                "playlists": [
                    {{"id": 10, "name": "One", "coverArt": "covers/c_{hash2}.png", "trackIds": []}},
                    {{"id": 20, "name": "Two", "coverArt": "covers/c_{hash1}.png", "trackIds": []}}
                ],
                "smart": [
                    {{"id": 5, "name": "Foreign Smart", "matchAll": true, "rules": [], "sort": "title", "limit": 10}}
                ]
            }}"#
        )),
        playlist_covers: vec![
            BackupCoverFile {
                name: format!("c_{hash2}.png"),
                bytes: bytes2,
            },
            BackupCoverFile {
                name: format!("c_{hash1}.png"),
                bytes: fs::read(&img2).unwrap(),
            },
        ],
        ..Default::default()
    };
    import_backup_bundle(second_bundle, &[], &BackupImportOptions::default()).unwrap();

    assert!(path1.is_file(), "shared cover still referenced by playlist 20 must remain");
    assert_eq!(fs::read(&path1).unwrap(), bytes1, "existing content cover must not be overwritten");
    assert!(CoverPaths::cover_dir().join(format!("c_{hash2}.png")).is_file());

    let col = CollectionsStore::load();
    let smart = col.smart.unwrap();
    assert!(smart.iter().any(|s| s.id == 5 && s.name == "Local Smart"));
    assert!(smart.iter().any(|s| s.id != 5 && s.name == "Foreign Smart"));
}
