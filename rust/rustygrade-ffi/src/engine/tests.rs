use super::*;

use std::fs::File;
use std::os::fd::IntoRawFd;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::mpsc;

use rustygrade_core::testing::{Fixture, TestEnv, write_flac};
use rustygrade_core::{CollectionsData, CollectionsStore, CoverPaths, PlayStats, fold};
use serde_json::Value;

use crate::error::EngineError;

#[derive(Default)]
struct Events(Mutex<Vec<(String, u32, u32)>>);

impl ScanProgressListener for Events {
    fn on_progress(&self, e: ScanProgressEvent) {
        self.0.lock().unwrap().push((e.phase, e.scanned, e.total));
    }
}

fn dir_request(env: &TestEnv) -> ScanRequest {
    ScanRequest { dirs: vec![env.music.to_string_lossy().into_owned()], ..ScanRequest::default() }
}

fn seed(env: &TestEnv) {
    write_flac(&env.music.join("Alpha/01.flac"), &Fixture::new("One", "Artist X", "Alpha", 1).with_cover());
    write_flac(&env.music.join("Alpha/02.flac"), &Fixture::new("Two", "Artist X", "Alpha", 2));
    write_flac(&env.music.join("Beta/01.flac"), &Fixture::new("Solo", "Artist Y", "Beta", 1));
}

#[test]
fn scan_then_browse_snapshots() {
    let env = TestEnv::new("engine-browse");
    seed(&env);
    let engine = RustygradeEngine::new(env.app_dir.to_string_lossy().into_owned());

    let events = Arc::new(Events::default());
    let summary = engine.scan_sources(dir_request(&env), Some(events.clone()), None).unwrap();
    assert_eq!((summary.added_tracks, summary.new_albums, summary.covers), (3, 2, 1));
    assert!(!summary.cancelled);
    assert!(!engine.is_scanning());
    assert!(events.0.lock().unwrap().iter().any(|(p, s, t)| p == "scanning" && s == t && *t == 3));

    assert!(env.app_dir.join("library.json").exists());

    let snap = engine.load_library_snapshot("name_asc".into()).unwrap();
    assert_eq!(snap.track_count, 3);
    assert_eq!(snap.albums.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["Alpha", "Beta"]);
    assert_eq!(snap.artists.len(), 2);
    assert_eq!(snap.sort, "name_asc");
    let alpha_thumb = snap.albums[0].thumb_path.as_deref().expect("Alpha thumb");
    assert!(std::path::Path::new(alpha_thumb).exists());
    assert!(CoverPaths::is_content_cover_file(std::path::Path::new(alpha_thumb)));
    assert!(snap.albums[1].thumb_path.is_none());
    assert_eq!(snap.albums[0].quality_label.as_deref(), Some("FLAC / 16BIT / 44.1KHZ"));
    // Unknown sort keys fall back to the default instead of failing.
    assert_eq!(engine.load_library_snapshot("bogus".into()).unwrap().sort, "added_desc");

    let detail = engine.get_album_detail("Alpha".into()).unwrap().unwrap();
    assert_eq!(detail.tracks.len(), 2);
    assert_eq!(detail.tracks[1].title, "Two");
    assert!(detail.tracks[0].play_uri.starts_with("file://"));
    assert!(engine.get_album_detail("Nope".into()).unwrap().is_none());

    let artist = engine.get_artist_detail("artist x".into()).unwrap().unwrap();
    assert_eq!(artist.artist.track_count, 2);
    assert_eq!(artist.albums[0].name, "Alpha");

    let hits = engine.search_library("SOLO".into()).unwrap();
    assert_eq!(hits.tracks.len(), 1);
    assert!(engine.search_library("   ".into()).unwrap().albums.is_empty());
    let by_artist = engine.search_library("artist y".into()).unwrap();
    assert_eq!(by_artist.artists.len(), 1);

    let ids = vec![detail.tracks[1].id.clone(), "missing".into(), detail.tracks[0].id.clone()];
    let got = engine.get_tracks(ids).unwrap();
    assert_eq!(got.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(), ["Two", "One"]);

    assert_eq!(engine.delete_albums(vec!["Beta".into(), "Nope".into()]).unwrap(), 1);
    assert_eq!(engine.load_library_snapshot("name_asc".into()).unwrap().albums.len(), 1);
}

#[test]
fn saf_scan_through_foreign_style_fd_opener() {
    struct Opener(PathBuf);
    impl UriOpener for Opener {
        fn open_fd(&self, uri: String) -> Option<i32> {
            let name = uri.strip_prefix("test://")?;
            File::open(self.0.join(name)).ok().map(|f| f.into_raw_fd())
        }
    }
    let env = TestEnv::new("engine-saf");
    write_flac(&env.music.join("s1.flac"), &Fixture::new("S One", "Z", "Saf", 1).with_cover());
    let engine = RustygradeEngine::new(env.app_dir.to_string_lossy().into_owned());
    let req = ScanRequest {
        saf_files: vec![SafDocument {
            document_id: "primary:Music/Saf/01.flac".into(),
            uri: "test://s1.flac".into(),
            mtime_ms: 1,
            size: 1,
        }],
        saf_roots: vec!["primary:Music".into()],
        ..ScanRequest::default()
    };
    let s = engine.scan_sources(req, None, Some(Arc::new(Opener(env.music.clone())))).unwrap();
    assert_eq!((s.added_tracks, s.covers), (1, 1));
    let t = &engine.get_album_detail("Saf".into()).unwrap().unwrap().tracks[0];
    assert_eq!(t.id, "/saf/primary:Music/Saf/01.flac");
    assert_eq!(t.play_uri, "test://s1.flac");
}

#[test]
fn a_negative_or_missing_fd_from_the_opener_skips_the_file() {
    struct Broken(Option<i32>);
    impl UriOpener for Broken {
        fn open_fd(&self, _uri: String) -> Option<i32> {
            self.0
        }
    }
    let env = TestEnv::new("engine-bad-fd");
    write_flac(&env.music.join("s1.flac"), &Fixture::new("S One", "Z", "Saf", 1));
    let engine = RustygradeEngine::new(env.app_dir.to_string_lossy().into_owned());
    let req = || ScanRequest {
        saf_files: vec![SafDocument {
            document_id: "primary:Music/01.flac".into(),
            uri: "test://s1.flac".into(),
            mtime_ms: 1,
            size: 1,
        }],
        saf_roots: vec!["primary:Music".into()],
        ..ScanRequest::default()
    };
    // Negative fds and the process's own stdin/stdout/stderr are never a document.
    for fd in [Some(-1), Some(0), Some(1), Some(2), None] {
        let s = engine.scan_sources(req(), None, Some(Arc::new(Broken(fd)))).unwrap();
        assert_eq!(s.added_tracks, 0, "fd {fd:?} must be treated as 'could not open'");
    }
}

#[test]
fn second_scan_while_running_is_rejected() {
    struct Gate(Mutex<Option<mpsc::Sender<()>>>, Mutex<mpsc::Receiver<()>>);
    impl ScanProgressListener for Gate {
        fn on_progress(&self, _: ScanProgressEvent) {
            if let Some(tx) = self.0.lock().unwrap().take() {
                tx.send(()).unwrap(); // tell the test the scan is in flight...
                let _ = self.1.lock().unwrap().recv(); // ...and hold it there
            }
        }
    }
    let env = TestEnv::new("engine-busy");
    seed(&env);
    let engine = RustygradeEngine::new(env.app_dir.to_string_lossy().into_owned());
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let gate = Arc::new(Gate(Mutex::new(Some(started_tx)), Mutex::new(release_rx)));

    std::thread::scope(|s| {
        let first = s.spawn(|| engine.scan_sources(dir_request(&env), Some(gate.clone()), None));
        started_rx.recv().unwrap();
        assert!(engine.is_scanning());
        let second = engine.scan_sources(dir_request(&env), None, None);
        assert!(matches!(second, Err(EngineError::ScanInProgress)));
        release_tx.send(()).unwrap();
        assert_eq!(first.join().unwrap().unwrap().added_tracks, 3);
    });
    assert!(!engine.is_scanning());
}

#[test]
fn settings_round_trip_keeps_unknown_keys_and_android_fields() {
    let env = TestEnv::new("engine-settings");
    std::fs::write(
        env.app_dir.join("native-settings.json"),
        r#"{"Volume":0.8,"LibrarySort":"name_asc","AudioDevice":"alsa/hw:0","FutureKey":{"keep":true}}"#,
    )
    .unwrap();
    let engine = RustygradeEngine::new(env.app_dir.to_string_lossy().into_owned());
    let mut s = engine.load_settings();
    assert_eq!(s.library_sort, "name_asc");
    assert!(s.use_media_store && s.saf_trees.is_empty());
    s.saf_trees = vec!["content://tree/primary%3AMusic".into()];
    s.use_media_store = false;
    s.crossfade_seconds = 99;
    engine.save_settings(s);

    let again = engine.load_settings();
    assert_eq!(again.saf_trees, vec!["content://tree/primary%3AMusic"]);
    assert!(!again.use_media_store);
    assert_eq!(again.crossfade_seconds, 12, "clamped to the 0..=12 range");
    assert!((again.volume - 0.8).abs() < 1e-9);
    let raw = std::fs::read_to_string(env.app_dir.join("native-settings.json")).unwrap();
    assert!(raw.contains("FutureKey"), "unknown keys survive a save: {raw}");
    assert!(raw.contains("alsa/hw:0"), "desktop-only keys survive a save: {raw}");
}

#[test]
fn field_setters_touch_only_their_fields() {
    let env = TestEnv::new("engine-setters");
    let engine = RustygradeEngine::new(env.app_dir.to_string_lossy().into_owned());
    engine.add_saf_tree("content://tree/1".into());
    engine.set_eq(true, "BASS".into(), vec![6.0, 5.0]);
    engine.set_replay_gain("album".into());
    engine.set_usb_bit_perfect(true);
    let s = engine.load_settings();
    assert!(s.eq_enabled && s.usb_bit_perfect);
    assert_eq!((s.eq_preset.as_str(), s.eq_gains.len(), s.replay_gain.as_str()), ("BASS", 2, "album"));
    assert_eq!(s.saf_trees, vec!["content://tree/1"], "other fields survive");
    engine.set_replay_gain("loudest".into());
    assert_eq!(engine.load_settings().replay_gain, "no");
}

#[test]
fn saf_tree_add_remove_is_atomic_and_idempotent() {
    let env = TestEnv::new("engine-trees");
    let engine = RustygradeEngine::new(env.app_dir.to_string_lossy().into_owned());
    std::thread::scope(|s| {
        for i in 0..8 {
            let e = &engine;
            s.spawn(move || e.add_saf_tree(format!("content://tree/{i}")));
        }
    });
    engine.add_saf_tree("content://tree/3".into());
    let mut trees = engine.load_settings().saf_trees;
    trees.sort();
    assert_eq!(trees.len(), 8, "no lost updates, no duplicates: {trees:?}");
    engine.remove_saf_tree("content://tree/3".into());
    assert_eq!(engine.load_settings().saf_trees.len(), 7);
}

#[test]
fn cancelled_scan_is_reported_and_non_finite_volume_is_sanitised() {
    let env = TestEnv::new("engine-cancel");
    seed(&env);
    struct CancelOnFirst(Arc<RustygradeEngine>);
    impl ScanProgressListener for CancelOnFirst {
        fn on_progress(&self, _: ScanProgressEvent) {
            self.0.cancel_scan();
        }
    }
    let engine = RustygradeEngine::new(env.app_dir.to_string_lossy().into_owned());
    let s = engine
        .scan_sources(dir_request(&env), Some(Arc::new(CancelOnFirst(engine.clone()))), None)
        .unwrap();
    assert!(s.cancelled);
    assert_eq!(s.added_tracks, 0);
    assert!(!env.app_dir.join("library.json").exists());
    // A later scan starts with a fresh cancel flag.
    assert_eq!(engine.scan_sources(dir_request(&env), None, None).unwrap().added_tracks, 3);

    let mut st = engine.load_settings();
    st.volume = f64::NAN;
    engine.save_settings(st);
    assert!((engine.load_settings().volume - 0.5).abs() < 1e-9);
}

#[test]
fn collections_get_default_smart_playlists_and_stats_load() {
    let env = TestEnv::new("engine-collections");
    let engine = RustygradeEngine::new(env.app_dir.to_string_lossy().into_owned());
    let c = engine.load_collections();
    assert!(!c.smart.is_empty(), "default smart playlists are created");
    assert!(c.liked.is_empty() && c.playlists.is_empty());
    assert!(engine.load_stats().tracks.is_empty());
    let empty = engine.load_library_snapshot("added_desc".into()).unwrap();
    assert_eq!(empty.track_count, 0);
}

#[test]
fn eq_gains_and_enum_settings_are_sanitised_on_save() {
    let env = TestEnv::new("engine-sanitise");
    let engine = RustygradeEngine::new(env.app_dir.to_string_lossy().into_owned());
    engine.set_eq(true, "CUSTOM".into(), vec![f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -50.0, 3.5, 50.0]);
    let s = engine.load_settings();
    assert_eq!(s.eq_gains, vec![0.0, 0.0, 0.0, -12.0, 3.5, 12.0]);
    // The stored file stays strictly parseable (a NaN would have been written as `null`).
    let raw = std::fs::read_to_string(env.app_dir.join("native-settings.json")).unwrap();
    let json: Value = serde_json::from_str(&raw).unwrap();
    assert!(json["EqGains"].as_array().unwrap().iter().all(Value::is_number), "{raw}");
    assert!(serde_json::from_str::<rustygrade_core::AppSettings>(&raw).is_ok());

    let mut snapshot = engine.load_settings();
    snapshot.eq_gains = vec![f64::NAN, 99.0];
    snapshot.replay_gain = "loudest".into();
    snapshot.library_view = "grid".into();
    engine.save_settings(snapshot);
    let s = engine.load_settings();
    assert_eq!(s.eq_gains, vec![0.0, 12.0]);
    assert_eq!((s.replay_gain.as_str(), s.library_view.as_str()), ("no", "albums"));
    let mut snapshot = engine.load_settings();
    snapshot.replay_gain = "album".into();
    snapshot.library_view = "artists".into();
    engine.save_settings(snapshot);
    let s = engine.load_settings();
    assert_eq!((s.replay_gain.as_str(), s.library_view.as_str()), ("album", "artists"));
}

#[test]
fn contains_folded_matches_fold_for_ascii_and_unicode() {
    for (hay, needle) in [
        ("Hello World", "lo wo"),
        ("Hello World", "xyz"),
        ("ABC", "abc"),
        ("", "a"),
        ("abc", ""),
        ("Äther Ünïcode", "ünï"),
        ("音楽カバー", "カバ"),
        ("İstanbul", "i̇s"),
        ("straße", "strasse"),
        ("Ado", "ado"),
    ] {
        assert_eq!(contains_folded(hay, &fold(needle)), fold(hay).contains(&fold(needle)), "{hay:?} / {needle:?}");
    }
}

#[test]
fn search_and_lookup_handle_non_ascii_libraries() {
    let env = TestEnv::new("engine-unicode");
    write_flac(&env.music.join("jp/01.flac"), &Fixture::new("残夢", "Ado あど", "狂言", 1));
    write_flac(&env.music.join("fr/01.flac"), &Fixture::new("Élan Vital", "Café Tacvba", "Éléphant", 1));
    write_flac(&env.music.join("en/01.flac"), &Fixture::new("Plain Song", "Plain Artist", "Plain", 1));
    let engine = RustygradeEngine::new(env.app_dir.to_string_lossy().into_owned());
    engine.scan_sources(dir_request(&env), None, None).unwrap();
    assert_eq!(engine.search_library("残".into()).unwrap().tracks.len(), 1);
    assert_eq!(engine.search_library("ÉLAN".into()).unwrap().tracks.len(), 1);
    assert_eq!(engine.search_library("café".into()).unwrap().tracks.len(), 1);
    assert_eq!(engine.search_library("plain SONG".into()).unwrap().tracks.len(), 1);
    assert_eq!(engine.search_library("あど".into()).unwrap().artists.len(), 1);
    // Smart playlist and codec filters with non-ASCII text must not panic either.
    let ids: Vec<String> = engine.search_library("a".into()).unwrap().tracks.into_iter().map(|t| t.id).collect();
    assert!(!ids.is_empty());
    assert_eq!(engine.get_tracks(ids.iter().rev().cloned().collect()).unwrap().len(), ids.len());
}

#[test]
fn get_tracks_on_a_big_single_album_library_is_fast_and_ordered() {
    // One album with many tracks used to blow up the track index (every track cloned its album).
    let env = TestEnv::new("engine-big-album");
    for i in 1..=60 {
        write_flac(&env.music.join(format!("big/{i:02}.flac")), &Fixture::new(&format!("t{i}"), "VA", "Big", i));
    }
    let engine = RustygradeEngine::new(env.app_dir.to_string_lossy().into_owned());
    engine.scan_sources(dir_request(&env), None, None).unwrap();
    let detail = engine.get_album_detail("Big".into()).unwrap().unwrap();
    let mut ids: Vec<String> = detail.tracks.iter().map(|t| t.id.clone()).collect();
    ids.reverse();
    ids.push("missing".into());
    let got = engine.get_tracks(ids).unwrap();
    assert_eq!(got.len(), 60);
    assert_eq!(got[0].title, "t60");
    let artist = engine.get_artist_detail("va".into()).unwrap().unwrap();
    assert_eq!((artist.artist.track_count, artist.albums.len()), (60, 1));
}

#[test]
fn library_snapshot_sorts_by_plays_with_one_stats_pass() {
    let env = TestEnv::new("engine-plays-sort");
    seed(&env);
    let engine = RustygradeEngine::new(env.app_dir.to_string_lossy().into_owned());
    engine.scan_sources(dir_request(&env), None, None).unwrap();
    let beta = engine.get_album_detail("Beta".into()).unwrap().unwrap();
    PlayStats::update(|s| {
        s.record_at(&beta.tracks[0].id.to_uppercase(), 100);
        s.record_at(&beta.tracks[0].id, 200);
    });
    let snap = engine.load_library_snapshot("plays_desc".into()).unwrap();
    assert_eq!(snap.albums[0].name, "Beta");
    assert_eq!(snap.artists[0].name, "Artist Y");
    let snap = engine.load_library_snapshot("played_desc".into()).unwrap();
    assert_eq!((snap.albums[0].name.as_str(), snap.artists[0].name.as_str()), ("Beta", "Artist Y"));
}

#[test]
fn search_library_returns_matching_playlists_and_smart_playlists() {
    let env = TestEnv::new("engine-search-playlists");
    seed(&env);
    let engine = RustygradeEngine::new(env.app_dir.to_string_lossy().into_owned());
    engine.scan_sources(dir_request(&env), None, None).unwrap();
    engine.create_playlist("Night Drive Mix".into());
    engine.create_playlist("Workout".into());

    let mix = engine.search_library("drive".into()).unwrap();
    assert_eq!(mix.playlists.len(), 1);
    assert_eq!(mix.playlists[0].name, "Night Drive Mix");

    let smart = engine.search_library("favorite".into()).unwrap();
    assert!(smart.smart_playlists.iter().any(|s| s.name == "Forgotten Favorites"));
}

#[test]
fn deduplicate_and_sweep_covers_migrates_legacy_files_and_cleans_orphans() {
    let env = TestEnv::new("engine-dedup-sweep");
    seed(&env);
    let engine = RustygradeEngine::new(env.app_dir.to_string_lossy().into_owned());
    engine.scan_sources(dir_request(&env), None, None).unwrap();

    std::fs::create_dir_all(CoverPaths::cover_dir()).unwrap();
    let legacy_pl = CoverPaths::cover_dir().join("native_pl_9_1_full.jpg");
    let orphan = CoverPaths::cover_dir().join("c_000000000000000000000000.jpg");
    rustygrade_core::testing::write_png(&legacy_pl, [120, 40, 200]);
    rustygrade_core::testing::write_png(&orphan, [10, 10, 10]);
    std::fs::OpenOptions::new()
        .write(true)
        .open(&orphan)
        .unwrap()
        .set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(60))
        .unwrap();

    let p = engine.create_playlist("Legacy".into());
    CollectionsStore::save(&CollectionsData {
        playlists: vec![rustygrade_core::PlaylistData {
            id: p.id,
            name: p.name,
            cover_art: Some(CoverPaths::to_file_url(&legacy_pl)),
            ..Default::default()
        }],
        ..Default::default()
    });

    let summary = engine.deduplicate_and_sweep_covers().unwrap();
    assert_eq!(summary.migrated_playlists, 1);
    assert!(summary.removed_files >= 2);
    assert!(!orphan.exists());
    assert!(!legacy_pl.exists());
    let updated = engine.playlist(p.id).unwrap();
    assert!(
        updated
            .cover_path
            .as_deref()
            .is_some_and(|path| CoverPaths::is_content_cover_file(std::path::Path::new(path)))
    );
}

#[test]
fn all_tracks_and_album_lookup_match_per_album_details() {
    let env = TestEnv::new("engine-all-tracks");
    seed(&env);
    let engine = RustygradeEngine::new(env.app_dir.to_string_lossy().into_owned());
    engine.scan_sources(dir_request(&env), None, None).unwrap();

    let all = engine.all_tracks().unwrap();
    assert_eq!(all.len(), 3);
    // One call returns exactly what the per-album detail loop used to collect.
    let snapshot = engine.load_library_snapshot("name_asc".into()).unwrap();
    let mut from_details: Vec<String> = Vec::new();
    for album in &snapshot.albums {
        let detail = engine.get_album_detail(album.key.clone()).unwrap().expect("album by key");
        assert_eq!(detail.album.key, album.key);
        from_details.extend(detail.tracks.into_iter().map(|t| t.id));
    }
    let mut ids: Vec<String> = all.into_iter().map(|t| t.id).collect();
    ids.sort();
    from_details.sort();
    assert_eq!(ids, from_details);
    assert!(engine.get_album_detail("No Such Album".into()).unwrap().is_none());
}

#[test]
fn concurrent_cache_misses_share_one_library_load() {
    let env = TestEnv::new("engine-load-lock");
    seed(&env);
    let engine = RustygradeEngine::new(env.app_dir.to_string_lossy().into_owned());
    engine.scan_sources(dir_request(&env), None, None).unwrap();
    engine.invalidate_cache();
    let loaded: Vec<_> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..8).map(|_| scope.spawn(|| engine.library().unwrap())).collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    // Every caller got the same cached allocation: the file was parsed once.
    assert!(loaded.windows(2).all(|w| Arc::ptr_eq(&w[0], &w[1])));
}
