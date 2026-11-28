use super::*;

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use lofty::file::FileType;

use super::cover_extract::save_cover;
use super::discovery::walk_paths;
use super::grouping::merge_split_albums;
use super::metadata::{codec_name, parse_r128_gain, parse_tag_number};
use super::prune::{collect_missing, prune_tracks};
use crate::library::{AlbumDto, LibraryMap, TrackDto};
use crate::paths::CoverPaths;

#[test]
fn replay_gain_tag_values_parse() {
    assert_eq!(parse_tag_number("-6.54 dB"), Some(-6.54));
    assert_eq!(parse_tag_number("+3 dB"), Some(3.0));
    assert_eq!(parse_tag_number("  0.988 "), Some(0.988));
    assert_eq!(parse_tag_number("-6.54dB"), Some(-6.54), "unit glued to the number");
    assert_eq!(parse_tag_number("-6.54 DB"), Some(-6.54));
    assert_eq!(parse_tag_number("nonsense"), None);
    assert_eq!(parse_tag_number("inf"), None);
    // Opus R128: Q7.8 relative to -23 LUFS, RG reference is -18 LUFS (+5 dB).
    assert_eq!(parse_r128_gain("0"), Some(5.0));
    assert_eq!(parse_r128_gain("-1280"), Some(0.0));
    assert_eq!(parse_r128_gain("x"), None);
    assert_eq!(parse_r128_gain("99999"), None, "outside i16"); 
}

#[test]
fn common_artist_splits_separators() {
    let res = common_artist([
        Some("Cö shu Nie & Wuthering Waves"),
        Some("Wuthering Waves, SoYeon"),
    ]);
    assert_eq!(res, "Wuthering Waves");
    assert_eq!(
        common_artist([Some("Alice"), Some("Bob")]),
        "Various Artists"
    );
}

#[test]
fn merges_split_albums_in_same_folder() {
    // `merge_split_albums` deletes cover files under the (global) app dir.
    let _env = crate::testing::TestEnv::new("merge-split");
    let mut lib = LibraryMap::new();
    lib.insert(
        "EP".to_owned(),
        AlbumDto {
            name: Some("EP".to_owned()),
            artist: Some("Artist A".to_owned()),
            album_artist: Some("Artist A".to_owned()),
            tracks: Some(vec![TrackDto {
                file_path: Some("/music/ep/01.flac".to_owned()),
                artist: Some("Artist A".to_owned()),
                ..TrackDto::default()
            }]),
            ..AlbumDto::default()
        },
    );
    lib.insert(
        "EP (Artist B)".to_owned(),
        AlbumDto {
            name: Some("EP (Artist B)".to_owned()),
            artist: Some("Artist B".to_owned()),
            album_artist: Some("Artist B".to_owned()),
            tracks: Some(vec![TrackDto {
                file_path: Some("/music/ep/02.flac".to_owned()),
                artist: Some("Artist B".to_owned()),
                ..TrackDto::default()
            }]),
            ..AlbumDto::default()
        },
    );

    let mut touched = HashSet::new();
    let mut mixed = HashSet::new();
    let mut detached = Vec::new();
    let merged = merge_split_albums(&mut lib, &mut touched, &mut mixed, &mut detached);
    assert_eq!(merged, 1);
    assert!(lib.contains_key("EP"));
    assert!(!lib.contains_key("EP (Artist B)"));
    assert_eq!(lib["EP"].tracks.as_ref().unwrap().len(), 2);
    assert!(mixed.contains("EP"));
}

fn album_with(paths: &[&str]) -> AlbumDto {
    AlbumDto {
        tracks: Some(
            paths
                .iter()
                .map(|p| TrackDto {
                    file_path: Some((*p).to_owned()),
                    ..TrackDto::default()
                })
                .collect(),
        ),
        ..AlbumDto::default()
    }
}

#[test]
fn offline_roots_report_missing_tracks_and_prune_only_on_request() {
    let base = std::env::temp_dir().join(format!("rustygrade-prune-test-{}", std::process::id()));
    let _ = fs::remove_dir_all(&base);
    // An unmounted drive: the mount point exists but is empty.
    let empty_mount = base.join("usb");
    fs::create_dir_all(&empty_mount).unwrap();
    let empty_mount = fs::canonicalize(&empty_mount).unwrap();
    // A root that is gone together with its parent.
    let absent = base.join("gone-parent").join("music");
    // A healthy root with its file present.
    let healthy = base.join("ok");
    fs::create_dir_all(&healthy).unwrap();
    let healthy = fs::canonicalize(&healthy).unwrap();
    let present = healthy.join("01.flac");
    fs::write(&present, b"").unwrap();

    let usb_track = empty_mount.join("Album").join("01.flac");
    let absent_track = absent.join("Album").join("02.flac");
    let elsewhere = base.join("not-scanned").join("03.flac");
    let s = |p: &Path| p.to_string_lossy().into_owned();
    let mut library = LibraryMap::new();
    library.insert("USB".to_owned(), album_with(&[&s(&usb_track)]));
    library.insert("Absent".to_owned(), album_with(&[&s(&absent_track)]));
    library.insert("Ok".to_owned(), album_with(&[&s(&present)]));
    library.insert("Elsewhere".to_owned(), album_with(&[&s(&elsewhere)]));

    let mut by_path = HashMap::new();
    for (key, album) in &library {
        for (idx, t) in album.tracks.iter().flatten().enumerate() {
            by_path.insert(t.file_path.as_ref().unwrap().to_lowercase(), (key.clone(), idx));
        }
    }
    let found = walk_paths(&[empty_mount.clone(), absent.clone(), healthy.clone()]);
    let seen: HashSet<String> = found
        .files
        .iter()
        .map(|f| f.to_string_lossy().to_lowercase())
        .collect();

    let mut gone = collect_missing(&library, &by_path, &found, &seen);
    gone.sort();
    let keys: Vec<&str> = gone.iter().map(|(_, k)| k.as_str()).collect();
    assert_eq!(keys, vec!["Absent", "USB"]);

    let mut touched = HashSet::new();
    assert_eq!(prune_tracks(&mut library, &gone, &mut touched), 2);
    assert!(library["USB"].tracks.as_ref().unwrap().is_empty());
    assert!(library["Absent"].tracks.as_ref().unwrap().is_empty());
    assert_eq!(library["Ok"].tracks.as_ref().unwrap().len(), 1);
    assert_eq!(library["Elsewhere"].tracks.as_ref().unwrap().len(), 1);
    assert!(touched.contains("USB") && touched.contains("Absent"));

    let _ = fs::remove_dir_all(&base);
}

#[cfg(unix)]
#[test]
fn discover_skips_symlink_cycles() {
    let root = std::env::temp_dir().join(format!("rustygrade-symlink-test-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let sub = root.join("album");
    fs::create_dir_all(&sub).unwrap();
    let flac = sub.join("track.flac");
    fs::write(&flac, b"").unwrap();
    std::os::unix::fs::symlink(&root, sub.join("loop")).unwrap();

    let files = Scanner::discover(std::slice::from_ref(&root));
    assert_eq!(files.len(), 1);
    assert!(files[0].ends_with("track.flac"));

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn alac_in_an_m4a_is_told_apart_from_aac() {
    let m4a = Path::new("/m/song.m4a");
    assert_eq!(codec_name(FileType::Mp4, m4a, Some(16)), "ALAC");
    assert_eq!(codec_name(FileType::Mp4, m4a, Some(24)), "ALAC");
    assert_eq!(codec_name(FileType::Mp4, m4a, None), "AAC");
    assert_eq!(codec_name(FileType::Mp4, Path::new("/m/song.alac"), None), "ALAC");
    assert_eq!(codec_name(FileType::Flac, m4a, Some(16)), "FLAC");
    assert_eq!(codec_name(FileType::Aac, Path::new("/m/a.aac"), None), "AAC");
    assert_eq!(crate::Codecs::short("ALAC", "/m/song.m4a"), "ALAC");
}

#[test]
fn prune_removes_each_gone_track_once_per_album() {
    let mut library: LibraryMap = serde_json::from_str(
        r#"{"A":{"name":"A","tracks":[{"filePath":"/m/1.flac"},{"filePath":"/m/2.flac"},{"filePath":"/m/3.flac"}]},
            "B":{"name":"B","tracks":[{"filePath":"/m/9.flac"}]}}"#,
    )
    .unwrap();
    let gone = vec![
        ("/m/1.flac".to_owned(), "A".to_owned()),
        ("/m/3.flac".to_owned(), "A".to_owned()),
        ("/m/nope.flac".to_owned(), "Missing".to_owned()),
    ];
    let mut touched = HashSet::new();
    assert_eq!(prune_tracks(&mut library, &gone, &mut touched), 2);
    assert_eq!(library["A"].tracks.as_ref().unwrap().len(), 1);
    assert_eq!(library["B"].tracks.as_ref().unwrap().len(), 1);
    assert_eq!(touched, HashSet::from(["A".to_owned()]));
}

#[test]
fn save_cover_deduplicates_and_rejects_garbage() {
    let _env = crate::testing::TestEnv::new("scanner-save-cover");
    let png_path = CoverPaths::cover_dir().join("src.png");
    fs::create_dir_all(CoverPaths::cover_dir()).unwrap();
    crate::testing::write_png(&png_path, [9, 99, 199]);
    let data = fs::read(&png_path).unwrap();
    fs::remove_file(&png_path).unwrap();

    let mut cache = HashMap::new();
    let (thumb_url, full_url) = save_cover(&data, &mut cache).expect("saved");
    assert_eq!(thumb_url, full_url);
    let saved_path = CoverPaths::to_local_path(Some(&thumb_url)).unwrap();
    assert!(CoverPaths::is_content_cover_file(&saved_path));
    assert!(fs::metadata(&saved_path).unwrap().len() > 0);
    let (thumb2, full2) = save_cover(&data, &mut cache).expect("cached");
    assert_eq!(thumb2, thumb_url);
    assert_eq!(full2, full_url);

    let names: Vec<String> = fs::read_dir(CoverPaths::cover_dir())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names.len(), 1);
    assert!(names.iter().all(|n| !n.contains(".tmp")), "{names:?}");
    // Not an image: nothing is written.
    assert!(save_cover(b"garbage", &mut cache).is_none());
}

#[test]
fn save_cover_keeps_small_png_and_webp_in_their_own_format() {
    let _env = crate::testing::TestEnv::new("scanner-cover-formats");
    fs::create_dir_all(CoverPaths::cover_dir()).unwrap();
    let mut cache = HashMap::new();

    let png_path = CoverPaths::cover_dir().join("tmp.png");
    crate::testing::write_png(&png_path, [12, 34, 56]);
    let png_bytes = fs::read(&png_path).unwrap();
    fs::remove_file(&png_path).unwrap();
    let (url_png, _) = save_cover(&png_bytes, &mut cache).expect("saved PNG");
    assert!(url_png.ends_with(".png"), "{url_png}");

    let mut webp_buf = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(4, 4, image::Rgb([90, 120, 200])))
        .write_to(&mut webp_buf, image::ImageFormat::WebP)
        .expect("encode WebP");
    let (url_webp, _) = save_cover(&webp_buf.into_inner(), &mut cache).expect("saved WebP");
    assert!(url_webp.ends_with(".webp"), "{url_webp}");
}
