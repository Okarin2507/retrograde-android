use super::*;

use std::fs::{self, OpenOptions};
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use image::{DynamicImage, ImageReader};

use super::encode::{encode_capped_full, encode_capped_jpeg};
use crate::collections::{CollectionsData, CollectionsStore};
use crate::library::{LibraryMap, LibraryStore};
use crate::paths::CoverPaths;

#[test]
fn dark_cover_with_small_red_accent_extracts_red() {
    let mut pixels = vec![10u8; 48 * 48 * 3];
    // Make ~3% of pixels vivid red.
    for i in 0..70 {
        let base = i * 3;
        pixels[base] = 220;
        pixels[base + 1] = 28;
        pixels[base + 2] = 28;
    }
    let pal = extract_cover_palette_from_rgb(&pixels, 48, 48);
    assert!(pal.vivid);
    assert!(pal.accent[0] > pal.accent[1] + 50);
    assert!(pal.accent[0] > pal.accent[2] + 50);
}

#[test]
fn pure_blue_cover_accent_meets_contrast_floor() {
    let mut pixels = Vec::with_capacity(32 * 32 * 3);
    for _ in 0..(32 * 32) {
        pixels.extend_from_slice(&[0, 0, 255]);
    }
    let pal = extract_cover_palette_from_rgb(&pixels, 32, 32);
    assert!(pal.vivid);
    assert!(
        relative_luminance(pal.accent) >= MIN_ACCENT_LUMINANCE,
        "accent {:?} luminance {}",
        pal.accent,
        relative_luminance(pal.accent)
    );
    // Still recognisably blue.
    assert!(pal.accent[2] > pal.accent[0] && pal.accent[2] > pal.accent[1]);
}

#[test]
fn large_dimensions_do_not_overflow_index_math() {
    // 70_000 * 70_000 overflows u32; a short buffer must simply stop early.
    let pixels = vec![200u8; 3 * 64];
    let pal = extract_cover_palette_from_rgb(&pixels, 70_000, 70_000);
    assert!(relative_luminance(pal.accent) >= MIN_ACCENT_LUMINANCE);
}

#[test]
fn grayscale_cover_falls_back_to_silver() {
    let mut pixels = Vec::with_capacity(24 * 24 * 3);
    for _ in 0..(24 * 24) {
        pixels.extend_from_slice(&[90, 90, 90]);
    }
    let pal = extract_cover_palette_from_rgb(&pixels, 24, 24);
    assert!(!pal.vivid);
    assert!((pal.accent[0] as i16 - pal.accent[1] as i16).abs() <= 2);
    assert!((pal.accent[1] as i16 - pal.accent[2] as i16).abs() <= 2);
    assert!(pal.accent[0] >= 200 && pal.accent[0] <= 230);
}

#[test]
fn thumb_for_handles_non_ascii_paths_and_existing_sibling() {
    let _env = crate::testing::TestEnv::new("covers-thumb-for");
    fs::create_dir_all(CoverPaths::cover_dir()).unwrap();
    // "…/音楽_full.jpg": only switches to _thumb.jpg when the sibling file exists on disk.
    for name in ["音楽/カバー", "あ", "éé", "x音", "ファイル名", "ünï/_ful"] {
        let p = PathBuf::from(format!("/m/{name}"));
        assert_eq!(PlaylistCovers::thumb_for(&p), p, "no suffix: unchanged");
    }
    assert_eq!(
        PlaylistCovers::thumb_for(Path::new("/m/missing_full.jpg")),
        Path::new("/m/missing_full.jpg"),
        "missing sibling keeps full path"
    );
    let full = CoverPaths::cover_dir().join("音楽_FULL.JPG");
    let thumb = CoverPaths::cover_dir().join("音楽_thumb.jpg");
    fs::write(&full, b"x").unwrap();
    fs::write(&thumb, b"y").unwrap();
    assert_eq!(PlaylistCovers::thumb_for(&full), thumb);
    assert_eq!(PlaylistCovers::thumb_for(Path::new("a")), Path::new("a"));
    assert_eq!(PlaylistCovers::thumb_for(Path::new("あいう_full.jp")), Path::new("あいう_full.jp"));
}

#[test]
fn blurred_backdrop_stays_inside_the_cover_cache_and_is_cached() {
    let env = crate::testing::TestEnv::new("covers-blur");
    // A cover outside the cache (user's folder): the blur must not be written next to it.
    let outside = env.music.join("Album/folder.png");
    fs::create_dir_all(outside.parent().unwrap()).unwrap();
    crate::testing::write_png(&outside, [200, 40, 40]);
    let blur = blurred_backdrop(&outside).expect("blur");
    assert_eq!(blur.parent(), Some(CoverPaths::cover_dir().as_path()));
    assert!(fs::metadata(&blur).unwrap().len() > 0);
    assert_eq!(fs::read_dir(outside.parent().unwrap()).unwrap().count(), 1, "nothing beside the source");
    assert_eq!(blurred_backdrop(&outside), Some(blur.clone()), "second call reuses it");
    // A cover inside the cache keeps `<name>_blur.jpg` beside it.
    fs::create_dir_all(CoverPaths::cover_dir()).unwrap();
    let cached = CoverPaths::cover_dir().join("c_0123456789abcdef01234567.png");
    crate::testing::write_png(&cached, [10, 200, 10]);
    let blur2 = blurred_backdrop(&cached).expect("blur");
    assert_eq!(blur2, CoverPaths::cover_dir().join("c_0123456789abcdef01234567_blur.jpg"));
    // A 0-byte leftover is rebuilt, not trusted.
    fs::write(&blur2, b"").unwrap();
    assert!(fs::metadata(blurred_backdrop(&cached).unwrap()).unwrap().len() > 0);
    // Not an image: nothing is cached.
    let junk = env.music.join("junk.png");
    fs::write(&junk, b"not an image").unwrap();
    assert!(blurred_backdrop(&junk).is_none());
}

#[test]
fn playlist_cover_deduplicates_content_and_delete_removes_file_and_blur() {
    let env = crate::testing::TestEnv::new("covers-write-new");
    let src = env.music.join("pic.png");
    crate::testing::write_png(&src, [1, 2, 3]);
    let first = PlaylistCovers::save(&src).unwrap();
    let second = PlaylistCovers::save(&src).unwrap();
    assert_eq!(first, second, "identical content reuses the same hash path");
    let first_path = CoverPaths::to_local_path(Some(&first)).unwrap();
    assert!(first_path.exists());
    assert!(CoverPaths::is_content_cover_file(&first_path));
    let blur = blurred_backdrop(&first_path).unwrap();
    assert!(blur.exists());
    PlaylistCovers::delete(Some(&first));
    assert!(!first_path.exists() && !blur.exists());
    // Files outside the cache or without a managed playlist/content prefix are never deleted.
    let foreign = env.music.join("native_pl_1_1_full.jpg");
    fs::write(&foreign, b"x").unwrap();
    PlaylistCovers::delete(Some(&CoverPaths::to_file_url(&foreign)));
    assert!(foreign.exists());
    let other = CoverPaths::cover_dir().join("album_full.jpg");
    fs::write(&other, b"x").unwrap();
    PlaylistCovers::delete(Some(&CoverPaths::to_file_url(&other)));
    assert!(other.exists());
    // Rejected inputs create nothing.
    assert!(matches!(PlaylistCovers::save(Path::new("/x/y.gif")), Err(CoverError::UnsupportedFormat)));
    let junk = env.music.join("junk.png");
    fs::write(&junk, b"nope").unwrap();
    assert!(matches!(PlaylistCovers::save(&junk), Err(CoverError::InsaneImage)));
}

#[test]
fn pixel_cap_is_tightened() {
    assert!(!ImageLimits::fits(Some((6_000, 6_000)))); // 36 MP
    assert!(ImageLimits::fits(Some((5_000, 5_000))));
}

#[test]
fn misnamed_png_and_webp_with_jpg_extension_sniff_by_content() {
    let env = crate::testing::TestEnv::new("covers-sniff");
    fs::create_dir_all(CoverPaths::cover_dir()).unwrap();

    let misnamed_png = CoverPaths::cover_dir().join("ajr5jv_full.jpg");
    crate::testing::write_png(&misnamed_png, [30, 180, 220]);
    assert!(ImageLimits::is_sane_path(&misnamed_png));
    let pal = extract_cover_palette(&misnamed_png).expect("palette from PNG bytes in .jpg");
    assert!(pal.vivid);
    let blur = blurred_backdrop(&misnamed_png).expect("blur from PNG bytes in .jpg");
    assert!(fs::metadata(blur).unwrap().len() > 0);
    let saved_from_png_jpg = PlaylistCovers::save(&misnamed_png).expect("playlist cover");
    let saved_png_bytes = fs::read(CoverPaths::to_local_path(Some(&saved_from_png_jpg)).unwrap()).unwrap();
    assert!(ImageReader::new(Cursor::new(&saved_png_bytes)).with_guessed_format().unwrap().format().is_some());

    let mut webp_buf = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(image::RgbImage::from_pixel(4, 4, image::Rgb([220, 60, 40])))
        .write_to(&mut webp_buf, image::ImageFormat::WebP)
        .expect("encode WebP");
    let webp_bytes = webp_buf.into_inner();
    let misnamed_webp = env.music.join("cover.jpg");
    fs::write(&misnamed_webp, &webp_bytes).unwrap();
    assert!(ImageLimits::is_sane_path(&misnamed_webp));
    assert!(extract_cover_palette(&misnamed_webp).is_some());
    let saved_from_webp_jpg = PlaylistCovers::save(&misnamed_webp).expect("webp cover");
    assert!(CoverPaths::to_local_path(Some(&saved_from_webp_jpg)).unwrap().exists());
}

fn dims_of(bytes: &[u8]) -> (u32, u32) {
    let img = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .unwrap()
        .decode()
        .unwrap();
    (img.width(), img.height())
}

fn blank(w: u32, h: u32) -> DynamicImage {
    DynamicImage::ImageRgb8(image::RgbImage::new(w, h))
}

#[test]
fn capped_full_keeps_alpha_as_png_and_opaque_as_jpeg() {
    let mut rgba = image::RgbaImage::new(2048, 1024);
    rgba.put_pixel(0, 0, image::Rgba([255, 0, 0, 0]));
    let out = encode_capped_full(&DynamicImage::ImageRgba8(rgba), MAX_FULL_COVER_EDGE, 90).unwrap();
    assert!(out.starts_with(&[0x89, b'P', b'N', b'G']));
    let img = ImageReader::new(Cursor::new(&out)).with_guessed_format().unwrap().decode().unwrap();
    assert_eq!((img.width(), img.height()), (720, 360));
    assert!(img.color().has_alpha());

    let jpg = encode_capped_full(&blank(3000, 3000), MAX_FULL_COVER_EDGE, 90).unwrap();
    assert!(jpg.starts_with(&[0xFF, 0xD8]));
    assert_eq!(dims_of(&jpg), (720, 720));
}

#[test]
fn sha256_matches_nist_vectors() {
    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }
    assert_eq!(hex(&sha256(b"")), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    assert_eq!(hex(&sha256(b"abc")), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
}

#[test]
fn save_cover_bytes_handles_exif_rotation_dedup_and_refcounting() {
    let _env = crate::testing::TestEnv::new("covers-exif-refcount");
    let raw_jpg = encode_capped_jpeg(&blank(200, 100), u32::MAX, 88).unwrap();
    let app1_payload: Vec<u8> = vec![
        b'E', b'x', b'i', b'f', 0, 0,
        b'I', b'I', 0x2A, 0x00, 0x08, 0x00, 0x00, 0x00,
        0x01, 0x00,
        0x12, 0x01, 0x03, 0x00, 0x01, 0x00, 0x00, 0x00, 0x06, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00,
    ];
    let seg_len = (app1_payload.len() + 2) as u16;
    let mut exif_jpg = Vec::with_capacity(raw_jpg.len() + app1_payload.len() + 4);
    exif_jpg.extend_from_slice(&[0xFF, 0xD8, 0xFF, 0xE1]);
    exif_jpg.extend_from_slice(&seg_len.to_be_bytes());
    exif_jpg.extend_from_slice(&app1_payload);
    exif_jpg.extend_from_slice(&raw_jpg[2..]);

    let p1 = save_cover_bytes(&exif_jpg).unwrap();
    let p2 = save_cover_bytes(&exif_jpg).unwrap();
    assert_eq!(p1, p2);
    assert_eq!(dims_of(&fs::read(&p1).unwrap()), (100, 200));

    let shared_url = CoverPaths::to_file_url(&p1);
    let blur = blurred_backdrop(&p1).unwrap();
    assert!(blur.exists());

    let mut lib = LibraryMap::new();
    lib.insert("A".to_owned(), crate::library::AlbumDto {
        cover_art: Some(shared_url.clone()),
        cover_art_full: Some(shared_url.clone()),
        ..Default::default()
    });
    let mut cols = CollectionsData::default();
    cols.playlists.push(crate::collections::PlaylistData {
        id: 1, name: "P".to_owned(), cover_art: Some(shared_url), ..Default::default()
    });
    lib.shift_remove("A");
    delete_unreferenced_covers(std::slice::from_ref(&p1), &lib, &cols);
    assert!(p1.exists() && blur.exists(), "playlist ref keeps cover and blur");
    cols.playlists.clear();
    delete_unreferenced_covers(std::slice::from_ref(&p1), &lib, &cols);
    assert!(!p1.exists() && !blur.exists(), "deleting last ref cleans both cover and blur");
}

#[test]
fn deduplicate_and_sweep_migrates_legacy_pairs_and_keeps_playlists() {
    let _env = crate::testing::TestEnv::new("covers-dedup-sweep");
    let cover_dir = CoverPaths::cover_dir();
    fs::create_dir_all(&cover_dir).unwrap();

    let shared_img = encode_capped_jpeg(&blank(1000, 1000), u32::MAX, 90).unwrap();
    let legacy_a_thumb = CoverPaths::thumb_path("Album A");
    let legacy_a_full = CoverPaths::full_path("Album A");
    let legacy_b_thumb = CoverPaths::thumb_path("Album B");
    let legacy_b_full = CoverPaths::full_path("Album B");
    fs::write(&legacy_a_thumb, &shared_img).unwrap();
    fs::write(&legacy_a_full, &shared_img).unwrap();
    fs::write(&legacy_b_thumb, &shared_img).unwrap();
    fs::write(&legacy_b_full, &shared_img).unwrap();

    let pl_img = encode_capped_jpeg(&DynamicImage::ImageRgb8(image::RgbImage::from_pixel(300, 300, image::Rgb([10, 200, 30]))), u32::MAX, 90).unwrap();
    let pl_legacy_full = cover_dir.join("native_pl_1_999_full.jpg");
    let pl_legacy_thumb = cover_dir.join("native_pl_1_999_thumb.jpg");
    fs::write(&pl_legacy_full, &pl_img).unwrap();
    fs::write(&pl_legacy_thumb, &pl_img).unwrap();

    let orphan = cover_dir.join("deadbeef_full.jpg");
    fs::write(&orphan, &shared_img).unwrap();
    OpenOptions::new().write(true).open(&orphan).unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(60)).unwrap();
    let fresh_orphan = cover_dir.join("freshbeef_full.jpg");
    fs::write(&fresh_orphan, &shared_img).unwrap();

    let mut lib = LibraryMap::new();
    lib.insert("Album A".to_owned(), crate::library::AlbumDto {
        name: Some("Album A".to_owned()),
        cover_art: Some(CoverPaths::to_file_url(&legacy_a_thumb)),
        cover_art_full: Some(CoverPaths::to_file_url(&legacy_a_full)),
        ..Default::default()
    });
    lib.insert("Album B".to_owned(), crate::library::AlbumDto {
        name: Some("Album B".to_owned()),
        cover_art: Some(CoverPaths::to_file_url(&legacy_b_thumb)),
        cover_art_full: Some(CoverPaths::to_file_url(&legacy_b_full)),
        ..Default::default()
    });
    LibraryStore::save_raw(&lib).unwrap();

    let mut cols = CollectionsData::default();
    cols.playlists.push(crate::collections::PlaylistData {
        id: 1, name: "PL".to_owned(), cover_art: Some(CoverPaths::to_file_url(&pl_legacy_full)), ..Default::default()
    });
    assert!(migrate_playlist_covers(&mut cols));
    CollectionsStore::save(&cols);

    let res = deduplicate_and_sweep_covers(Some(&cols), None).unwrap();
    assert_eq!(res.migrated_albums, 2);
    assert!(!legacy_a_thumb.exists() && !legacy_a_full.exists());
    assert!(!legacy_b_thumb.exists() && !legacy_b_full.exists());
    assert!(!orphan.exists(), "60s-old orphan swept");
    assert!(fresh_orphan.exists(), "<5s orphan kept during grace window");

    let updated_lib = LibraryStore::load_raw().unwrap();
    assert_eq!(updated_lib["Album A"].cover_art, updated_lib["Album B"].cover_art);
    let dedup_path = CoverPaths::to_local_path(updated_lib["Album A"].cover_art.as_deref()).unwrap();
    assert_eq!(dims_of(&fs::read(&dedup_path).unwrap()), (720, 720));
}

#[test]
fn deduplicate_and_sweep_recovers_damaged_library_without_deadlocking() {
    let _env = crate::testing::TestEnv::new("covers-dedup-damaged");
    fs::create_dir_all(CoverPaths::cover_dir()).unwrap();

    let mut lib = LibraryMap::new();
    lib.insert("Album A".to_owned(), crate::library::AlbumDto {
        name: Some("Album A".to_owned()),
        ..Default::default()
    });
    // Two saves so `library.json.bak` holds a good copy, then damage `library.json`.
    LibraryStore::save_raw(&lib).unwrap();
    LibraryStore::save_raw(&lib).unwrap();
    assert!(LibraryStore::backup_path().is_file());
    fs::write(LibraryStore::library_path(), b"{ not json").unwrap();

    // Run on a worker so a re-entrant lock shows up as a timeout instead of hanging the suite.
    let run = || {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(deduplicate_and_sweep_covers(None, None));
        });
        rx.recv_timeout(Duration::from_secs(10))
            .expect("deduplicate_and_sweep_covers deadlocked on a damaged library.json")
    };

    let res = run().expect("recovered from .bak");
    assert_eq!(res.migrated_albums, 0);
    assert!(LibraryStore::load_raw().unwrap().contains_key("Album A"), "library.json restored from .bak");

    // Damaged with no usable backup: reports corruption promptly instead of blocking.
    fs::write(LibraryStore::library_path(), b"{ not json").unwrap();
    fs::write(LibraryStore::backup_path(), b"also broken").unwrap();
    assert!(run().is_err());
}
