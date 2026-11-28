//! End-to-end scanner tests on real (tiny) FLAC fixtures.

use std::fs;
use std::sync::atomic::AtomicBool;

use crate::library::LibraryStore;
use crate::paths::CoverPaths;
use crate::scanner::{SafFile, ScanOptions, ScanSources, Scanner, is_saf_path};
use crate::testing::{DirOpener, Fixture, TestEnv, set_replaygain, write_flac};

fn quick() -> ScanOptions {
    ScanOptions { max_threads: Some(2), ..ScanOptions::default() }
}

#[test]
fn directory_scan_writes_library_and_covers() {
    let env = TestEnv::new("dir-scan");
    write_flac(&env.music.join("Alpha/01.flac"), &Fixture::new("One", "Artist X", "Alpha", 1).with_cover());
    write_flac(&env.music.join("Alpha/02.flac"), &Fixture::new("Two", "Artist X", "Alpha", 2));
    write_flac(&env.music.join("Beta/01.flac"), &Fixture::new("Solo", "Artist Y", "Beta", 1));

    let res = Scanner::scan(std::slice::from_ref(&env.music), quick(), None, None).unwrap();
    assert_eq!(res.added_tracks, 3);
    assert_eq!(res.new_albums, 2);
    assert_eq!(res.covers, 1, "only Alpha has artwork");

    assert!(LibraryStore::library_path().exists(), "library.json written");

    let albums = LibraryStore::load().unwrap();
    let alpha = albums.iter().find(|a| a.name == "Alpha").expect("Alpha");
    let beta = albums.iter().find(|a| a.name == "Beta").expect("Beta");
    assert_eq!(alpha.tracks.len(), 2);
    assert_eq!(alpha.tracks[0].title, "One");
    assert_eq!(alpha.tracks[1].track_number, 2);
    assert_eq!(alpha.year, 2021);
    assert_eq!(alpha.tracks[0].sample_rate, 44100);
    assert!(alpha.tracks[0].play_uri.starts_with("file://"));
    let thumb = alpha.cover_thumb_path.as_deref().expect("Alpha thumb cover");
    let full = alpha.cover_full_path.as_deref().expect("Alpha full cover");
    assert_eq!(thumb, full, "single content-addressed cover file");
    assert!(thumb.exists(), "thumb cover exists on disk");
    assert!(
        CoverPaths::is_content_cover_file(thumb),
        "content-addressed cover name: {}",
        thumb.display()
    );
    assert!(beta.cover_thumb_path.is_none());

    // Unchanged files are not re-parsed or re-added.
    let again = Scanner::scan(std::slice::from_ref(&env.music), quick(), None, None).unwrap();
    assert!(!again.changed(), "{again:?}");
}

#[test]
fn explicit_files_with_roots_report_and_prune_missing() {
    let env = TestEnv::new("roots");
    let a = env.music.join("Alpha/01.flac");
    let b = env.music.join("Alpha/02.flac");
    write_flac(&a, &Fixture::new("One", "X", "Alpha", 1));
    write_flac(&b, &Fixture::new("Two", "X", "Alpha", 2));

    let mut sources = ScanSources {
        files: vec![a.clone(), b.clone()],
        roots: vec![env.music.clone()],
        ..ScanSources::default()
    };
    let res = Scanner::scan_sources(&sources, None, quick(), None, None).unwrap();
    assert_eq!(res.added_tracks, 2);

    // `b` disappears from storage and from the MediaStore listing.
    fs::remove_file(&b).unwrap();
    sources.files = vec![a.clone()];
    let res = Scanner::scan_sources(&sources, None, quick(), None, None).unwrap();
    assert_eq!(res.missing_tracks, 1, "reported, not dropped");
    assert_eq!(res.removed_tracks, 0);
    assert_eq!(LibraryStore::load().unwrap()[0].tracks.len(), 2);

    let prune = ScanOptions { prune_missing: true, ..quick() };
    let res = Scanner::scan_sources(&sources, None, prune, None, None).unwrap();
    assert_eq!(res.removed_tracks, 1);
    assert_eq!(LibraryStore::load().unwrap()[0].tracks.len(), 1);
}

#[test]
fn explicit_files_without_roots_never_report_missing() {
    let env = TestEnv::new("no-roots");
    let a = env.music.join("01.flac");
    write_flac(&a, &Fixture::new("One", "X", "Alpha", 1));
    let sources = ScanSources { files: vec![a.clone()], ..ScanSources::default() };
    Scanner::scan_sources(&sources, None, quick(), None, None).unwrap();
    fs::remove_file(&a).unwrap();
    let res = Scanner::scan_sources(&ScanSources::default(), None, quick(), None, None).unwrap();
    assert_eq!(res.missing_tracks, 0);
}

fn saf(doc_id: &str, name: &str, env: &TestEnv) -> SafFile {
    let meta = fs::metadata(env.music.join(name)).unwrap();
    SafFile {
        document_id: doc_id.to_owned(),
        uri: format!("test://{name}"),
        mtime_ms: 1_700_000_000_000,
        size: meta.len() as i64,
    }
}

#[test]
fn saf_documents_scan_through_opener_with_logical_ids() {
    let env = TestEnv::new("saf");
    write_flac(&env.music.join("s1.flac"), &Fixture::new("S One", "Z", "Saf Album", 1).with_cover());
    write_flac(&env.music.join("s2.flac"), &Fixture::new("S Two", "Z", "Saf Album", 2));
    let opener = DirOpener(env.music.clone());

    let mut sources = ScanSources {
        saf_files: vec![
            saf("primary:Music/Saf Album/01.flac", "s1.flac", &env),
            saf("primary:Music/Saf Album/02.flac", "s2.flac", &env),
        ],
        saf_roots: vec!["primary:Music".to_owned()],
        ..ScanSources::default()
    };
    let res = Scanner::scan_sources(&sources, Some(&opener), quick(), None, None).unwrap();
    assert_eq!(res.added_tracks, 2);
    assert_eq!(res.new_albums, 1);
    assert_eq!(res.covers, 1, "embedded art read through the opener");

    let albums = LibraryStore::load().unwrap();
    assert!(
        albums[0]
            .cover_full_path
            .as_deref()
            .is_some_and(|p| std::path::Path::new(p).exists())
    );
    let t = &albums[0].tracks[0];
    assert!(is_saf_path(&t.file_path), "{}", t.file_path);
    assert_eq!(t.file_path, "/saf/primary:Music/Saf Album/01.flac");
    assert_eq!(t.play_uri, "test://s1.flac", "playback uses the content URI");

    // Same mtime/size from SAF => untouched on rescan.
    let again = Scanner::scan_sources(&sources, Some(&opener), quick(), None, None).unwrap();
    assert!(!again.changed(), "{again:?}");

    // A document that left the tree is reported missing (its logical path is under the root).
    sources.saf_files.pop();
    let res = Scanner::scan_sources(&sources, Some(&opener), quick(), None, None).unwrap();
    assert_eq!(res.missing_tracks, 1);
}

#[test]
fn saf_without_opener_skips_files_instead_of_failing() {
    let env = TestEnv::new("saf-no-opener");
    write_flac(&env.music.join("s1.flac"), &Fixture::new("S One", "Z", "Saf Album", 1));
    let sources = ScanSources {
        saf_files: vec![saf("primary:Music/01.flac", "s1.flac", &env)],
        ..ScanSources::default()
    };
    let res = Scanner::scan_sources(&sources, None, quick(), None, None).unwrap();
    assert_eq!(res.added_tracks, 0);
}

#[test]
fn progress_reports_counts() {
    let env = TestEnv::new("progress");
    for i in 1..=4 {
        write_flac(&env.music.join(format!("A/{i:02}.flac")), &Fixture::new(&format!("T{i}"), "X", "A", i));
    }
    let mut events = Vec::new();
    let mut cb = |p: crate::scanner::ScanProgress| events.push((p.phase, p.scanned, p.total));
    Scanner::scan(std::slice::from_ref(&env.music), quick(), None, Some(&mut cb)).unwrap();
    assert!(events.contains(&("scanning", 4, 4)), "{events:?}");
}

#[test]
fn cancelled_scan_saves_nothing() {
    let env = TestEnv::new("cancel");
    write_flac(&env.music.join("A/01.flac"), &Fixture::new("T", "X", "A", 1));
    let cancel = AtomicBool::new(true);
    let res = Scanner::scan(std::slice::from_ref(&env.music), quick(), Some(&cancel), None).unwrap();
    assert!(res.cancelled);
    assert!(!res.changed());
    assert!(!LibraryStore::library_path().exists(), "cancelled scan saves nothing");
}

#[test]
fn poisoned_library_lock_does_not_block_later_scans() {
    let env = TestEnv::new("poison");
    write_flac(&env.music.join("01.flac"), &Fixture::new("One", "X", "Alpha", 1));
    let _ = std::thread::spawn(|| {
        let _g = LibraryStore::write_lock().lock().unwrap_or_else(|p| p.into_inner());
        panic!("simulated tag-parser panic");
    })
    .join();
    assert!(LibraryStore::write_lock().is_poisoned());
    let res = Scanner::scan(std::slice::from_ref(&env.music), quick(), None, None).unwrap();
    assert_eq!(res.added_tracks, 1);
    // Recovery verified: un-poison the process-global lock so later tests see a clean mutex.
    LibraryStore::write_lock().clear_poison();
}

#[test]
fn saf_volume_root_tree_detects_missing_tracks() {
    // Picking a volume root gives the tree id "primary:", whose children are "primary:Music/..".
    let env = TestEnv::new("saf-volume-root");
    write_flac(&env.music.join("s1.flac"), &Fixture::new("S One", "Z", "Saf", 1));
    write_flac(&env.music.join("s2.flac"), &Fixture::new("S Two", "Z", "Saf", 2));
    let opener = DirOpener(env.music.clone());
    let mut sources = ScanSources {
        saf_files: vec![
            saf("primary:Music/Saf/01.flac", "s1.flac", &env),
            saf("primary:Music/Saf/02.flac", "s2.flac", &env),
        ],
        saf_roots: vec!["primary:".to_owned()],
        ..ScanSources::default()
    };
    Scanner::scan_sources(&sources, Some(&opener), quick(), None, None).unwrap();
    sources.saf_files.pop();
    let res = Scanner::scan_sources(&sources, Some(&opener), quick(), None, None).unwrap();
    assert_eq!(res.missing_tracks, 1);
}

#[test]
fn overlapping_sources_parse_each_file_once() {
    let env = TestEnv::new("overlap");
    write_flac(&env.music.join("s1.flac"), &Fixture::new("S One", "Z", "Saf", 1));
    let a = env.music.join("Alpha/01.flac");
    write_flac(&a, &Fixture::new("One", "X", "Alpha", 1));
    let opener = DirOpener(env.music.clone());
    let doc = saf("primary:Music/Saf/01.flac", "s1.flac", &env);
    // The same document reached through two nested trees, and a real file given twice.
    let sources = ScanSources {
        dirs: vec![env.music.join("Alpha")],
        files: vec![a.clone()],
        saf_files: vec![doc.clone(), doc],
        saf_roots: vec!["primary:Music".to_owned(), "primary:Music/Saf".to_owned()],
        ..ScanSources::default()
    };
    let mut totals = Vec::new();
    let mut cb = |p: crate::scanner::ScanProgress| {
        if p.phase == "scanning" {
            totals.push(p.total);
        }
    };
    let res = Scanner::scan_sources(&sources, Some(&opener), quick(), None, Some(&mut cb)).unwrap();
    assert_eq!(res.added_tracks, 2);
    assert!(totals.iter().all(|&t| t == 2), "{totals:?}");
    let total: usize = LibraryStore::load().unwrap().iter().map(|al| al.tracks.len()).sum();
    assert_eq!(total, 2);
}

#[test]
fn replaygain_tags_are_scanned_and_old_libraries_are_reparsed_once() {
    let env = TestEnv::new("replaygain");
    let a = env.music.join("Alpha/01.flac");
    write_flac(&a, &Fixture::new("One", "X", "Alpha", 1));
    set_replaygain(&a, ("-6.54 dB", "0.987654"), Some(("-8.10 dB", "1.000000")));
    write_flac(&env.music.join("Alpha/02.flac"), &Fixture::new("Two", "X", "Alpha", 2));

    let res = Scanner::scan(std::slice::from_ref(&env.music), quick(), None, None).unwrap();
    assert_eq!(res.added_tracks, 2);
    let albums = LibraryStore::load().unwrap();
    let t1 = &albums[0].tracks[0];
    assert_eq!(t1.replay_gain.track_gain_db, Some(-6.54));
    assert_eq!(t1.replay_gain.track_peak, Some(0.987654));
    assert_eq!(t1.replay_gain.album_gain_db, Some(-8.10));
    assert!(albums[0].tracks[1].replay_gain.is_empty(), "untagged track has no gain");

    // A library written before the scanner read ReplayGain: no schema marker, no gain.
    let path = LibraryStore::library_path();
    let mut json: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    for album in json.as_object_mut().unwrap().values_mut() {
        for t in album["tracks"].as_array_mut().unwrap() {
            let o = t.as_object_mut().unwrap();
            o.remove("scanSchema");
            o.remove("replayGain");
        }
    }
    fs::write(&path, serde_json::to_vec(&json).unwrap()).unwrap();
    assert!(LibraryStore::load().unwrap()[0].tracks[0].replay_gain.is_empty());

    let res = Scanner::scan(std::slice::from_ref(&env.music), quick(), None, None).unwrap();
    assert_eq!(res.updated_tracks, 2, "both tracks re-parsed once: {res:?}");
    assert_eq!(res.added_tracks, 0);
    let albums = LibraryStore::load().unwrap();
    assert_eq!(albums[0].tracks.len(), 2, "no duplicates after the re-parse");
    assert_eq!(albums[0].tracks[0].replay_gain.track_gain_db, Some(-6.54));

    let again = Scanner::scan(std::slice::from_ref(&env.music), quick(), None, None).unwrap();
    assert!(!again.changed(), "the marker stops further re-parsing: {again:?}");
}

#[test]
fn unparseable_files_are_not_retried_until_they_change() {
    let env = TestEnv::new("unparseable");
    let a = env.music.join("Alpha/01.flac");
    write_flac(&a, &Fixture::new("One", "X", "Alpha", 1));
    Scanner::scan(std::slice::from_ref(&env.music), quick(), None, None).unwrap();

    // The file is overwritten with junk (new size/mtime), so a re-parse is due but cannot work.
    fs::write(&a, b"definitely not audio").unwrap();
    let phases = |env: &TestEnv| {
        let mut seen = Vec::new();
        let mut cb = |p: crate::scanner::ScanProgress| seen.push(p.phase);
        Scanner::scan(std::slice::from_ref(&env.music), quick(), None, Some(&mut cb)).unwrap();
        seen
    };
    assert!(phases(&env).contains(&"scanning"), "the first scan tries to parse it");
    assert!(!phases(&env).contains(&"scanning"), "the second scan leaves it alone");
    assert_eq!(LibraryStore::load().unwrap()[0].tracks.len(), 1, "its old entry is kept");

    // Fixing the file makes it eligible again.
    write_flac(&a, &Fixture::new("One (fixed)", "X", "Alpha", 1));
    assert!(phases(&env).contains(&"scanning"));
    assert_eq!(LibraryStore::load().unwrap()[0].tracks[0].title, "One (fixed)");
}

#[test]
fn files_that_could_not_be_opened_are_retried_but_corrupt_ones_are_not() {
    let env = TestEnv::new("unavailable");
    write_flac(&env.music.join("s1.flac"), &Fixture::new("S One", "Z", "Saf", 1));
    let opener = DirOpener(env.music.clone());
    let sources = ScanSources {
        saf_files: vec![saf("primary:Music/01.flac", "s1.flac", &env)],
        saf_roots: vec!["primary:Music".to_owned()],
        ..ScanSources::default()
    };
    Scanner::scan_sources(&sources, Some(&opener), quick(), None, None).unwrap();

    // An old library entry (no schema marker) is due for a re-parse, but the provider fails to open
    // the document this time: nothing may be recorded, so the next scan tries again.
    let path = LibraryStore::library_path();
    let mut json: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    for album in json.as_object_mut().unwrap().values_mut() {
        for t in album["tracks"].as_array_mut().unwrap() {
            t.as_object_mut().unwrap().remove("scanSchema");
        }
    }
    fs::write(&path, serde_json::to_vec(&json).unwrap()).unwrap();

    let res = Scanner::scan_sources(&sources, None, quick(), None, None).unwrap();
    assert_eq!(res.updated_tracks, 0);
    let res = Scanner::scan_sources(&sources, Some(&opener), quick(), None, None).unwrap();
    assert_eq!(res.updated_tracks, 1, "the retry succeeds once the document opens: {res:?}");
}

#[test]
fn progress_reports_intermediate_counts_while_files_are_parsed() {
    struct SlowOpener(DirOpener);
    impl crate::scanner::FileOpener for SlowOpener {
        fn open(&self, uri: &str) -> Option<fs::File> {
            std::thread::sleep(std::time::Duration::from_millis(60));
            self.0.open(uri)
        }
    }
    let env = TestEnv::new("progress-slow");
    let mut docs = Vec::new();
    for i in 1..=6 {
        let name = format!("s{i}.flac");
        write_flac(&env.music.join(&name), &Fixture::new(&format!("T{i}"), "X", "Saf", i));
        docs.push(saf(&format!("primary:Music/{name}"), &name, &env));
    }
    let sources = ScanSources { saf_files: docs, saf_roots: vec!["primary:Music".to_owned()], ..ScanSources::default() };
    let opener = SlowOpener(DirOpener(env.music.clone()));
    let mut counts = Vec::new();
    let mut cb = |p: crate::scanner::ScanProgress| {
        if p.phase == "scanning" {
            counts.push(p.scanned);
        }
    };
    let one_thread = ScanOptions { max_threads: Some(1), ..ScanOptions::default() };
    Scanner::scan_sources(&sources, Some(&opener), one_thread, None, Some(&mut cb)).unwrap();
    assert!(counts.iter().any(|&n| n > 0 && n < 6), "no intermediate count was reported: {counts:?}");
    assert_eq!(counts.last(), Some(&6));
    assert!(counts.windows(2).all(|w| w[0] <= w[1]), "progress never goes backwards: {counts:?}");
}

#[test]
fn resync_covers_clears_cache_and_reextracts_without_pruning() {
    let env = TestEnv::new("resync-covers");
    write_flac(&env.music.join("Alpha/01.flac"), &Fixture::new("One", "Artist X", "Alpha", 1).with_cover());
    Scanner::scan(std::slice::from_ref(&env.music), quick(), None, None).unwrap();

    let albums = LibraryStore::load().unwrap();
    let old_cover = albums[0].cover_full_path.as_deref().unwrap().to_owned();
    assert!(std::path::Path::new(&old_cover).is_file());

    // Simulate missing/cleared cached cover file on disk; resync_covers forces re-extraction.
    fs::remove_file(&old_cover).unwrap();
    assert!(!std::path::Path::new(&old_cover).exists());

    let res = Scanner::scan(
        std::slice::from_ref(&env.music),
        ScanOptions { resync_covers: true, ..quick() },
        None,
        None,
    )
    .unwrap();
    assert_eq!(res.covers, 1);
    assert!(std::path::Path::new(&old_cover).is_file());
}
