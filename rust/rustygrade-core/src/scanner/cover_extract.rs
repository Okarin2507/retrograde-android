use std::collections::HashMap;
use std::fs;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};

use lofty::file::TaggedFileExt;
use lofty::picture::PictureType;

use super::grouping::album_folder;
use super::metadata::read_tagged;
use super::{ScanCtx, is_saf_path};
use crate::covers::{ImageLimits, MAX_COVER_FILE_BYTES, PlaylistCovers, save_cover_bytes, sha256};
use crate::error_log;
use crate::library::AlbumDto;
use crate::paths::CoverPaths;

const FOLDER_COVER_NAMES: &[&str] = &["cover", "folder", "front", "album", "albumart"];

pub(super) fn album_cover_missing(album: &AlbumDto) -> bool {
    let Some(thumb) = CoverPaths::to_local_path(album.cover_art.as_deref()) else {
        return true;
    };
    if !thumb.is_file() {
        return true;
    }
    if let Some(full) = CoverPaths::to_local_path(album.cover_art_full.as_deref())
        && !full.is_file()
    {
        return true;
    }
    false
}

/// Image files found in a folder during one scan pass (lowercased stem, path), so albums sharing a
/// folder (or a parent of disc folders) list it once.
pub(super) type FolderImageCache = HashMap<PathBuf, Vec<(String, PathBuf)>>;

pub(super) fn find_and_save_cover(
    album: &AlbumDto,
    picture_source: Option<&str>,
    ctx: &ScanCtx<'_>,
    cache: &mut HashMap<[u8; 32], String>,
    folder_images: &mut FolderImageCache,
) -> Option<(String, String)> {
    let mut data: Option<Vec<u8>> =
        picture_source.and_then(|p| read_embedded_picture(Path::new(p), ctx));
    let tracks: Vec<&str> = album
        .tracks
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .filter_map(|t| t.file_path.as_deref())
        .collect();

    if data.is_none() {
        for &p in tracks.iter().take(3) {
            if picture_source.is_some_and(|ps| ps.eq_ignore_ascii_case(p)) {
                continue;
            }
            if let Some(bytes) = read_embedded_picture(Path::new(p), ctx) {
                data = Some(bytes);
                break;
            }
        }
    }

    if data.is_none() {
        let mut folders: Vec<String> = Vec::new();
        for &p in tracks.iter().filter(|p| !is_saf_path(p)) {
            if let Some(dir) = Path::new(p).parent() {
                let d = dir.to_string_lossy().into_owned();
                if !d.is_empty() && !folders.iter().any(|x| x.eq_ignore_ascii_case(&d)) {
                    folders.push(d);
                }
            }
            let af = album_folder(p);
            if !af.is_empty() && !folders.iter().any(|x| x.eq_ignore_ascii_case(&af)) {
                folders.push(af);
            }
            if folders.len() >= 8 {
                break;
            }
        }
        for folder in folders.into_iter().take(8) {
            if let Some(bytes) = read_folder_image(Path::new(&folder), folder_images) {
                data = Some(bytes);
                break;
            }
        }
    }

    save_cover(&data?, cache)
}

fn read_embedded_picture(source_file: &Path, ctx: &ScanCtx<'_>) -> Option<Vec<u8>> {
    // The same parser as `parse_guarded`, so the same panic guard: a bad file has no cover.
    catch_unwind(AssertUnwindSafe(|| read_embedded_picture_unguarded(source_file, ctx))).unwrap_or_else(|_| {
        error_log::write("scanner", &format!("panic while reading the cover of {}", source_file.display()));
        None
    })
}

fn read_embedded_picture_unguarded(source_file: &Path, ctx: &ScanCtx<'_>) -> Option<Vec<u8>> {
    let tagged = read_tagged(source_file, ctx, true).ok()?;
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag())?;
    let pic = tag
        .get_picture_type(PictureType::CoverFront)
        .or_else(|| tag.pictures().first())?;
    if pic.data().is_empty() {
        None
    } else {
        Some(pic.data().to_vec())
    }
}

fn list_folder_images(folder: &Path) -> Vec<(String, PathBuf)> {
    let Ok(entries) = fs::read_dir(folder) else {
        return Vec::new();
    };
    entries
        .flatten()
        // `file_type` comes from the directory entry itself (no extra `stat` on most filesystems).
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .map(|e| e.path())
        .filter(|p| PlaylistCovers::is_supported_ext(p))
        .filter_map(|p| {
            let stem = p.file_stem()?.to_str()?.to_ascii_lowercase();
            Some((stem, p))
        })
        .collect()
}

fn read_folder_image(folder: &Path, folder_images: &mut FolderImageCache) -> Option<Vec<u8>> {
    let images = folder_images
        .entry(folder.to_path_buf())
        .or_insert_with(|| list_folder_images(folder));
    for &target_name in FOLDER_COVER_NAMES {
        if let Some((_, match_path)) = images.iter().find(|(stem, _)| stem == target_name)
            && let Ok(meta) = fs::metadata(match_path)
            && meta.len() <= MAX_COVER_FILE_BYTES
        {
            return fs::read(match_path).ok();
        }
    }
    None
}

pub(super) fn save_cover(data: &[u8], cache: &mut HashMap<[u8; 32], String>) -> Option<(String, String)> {
    if !ImageLimits::is_sane_bytes(data) {
        return None;
    }
    let raw_hash = sha256(data);
    if let Some(url) = cache.get(&raw_hash) {
        return Some((url.clone(), url.clone()));
    }
    let path = save_cover_bytes(data).ok()?;
    let url = CoverPaths::to_file_url(&path);
    cache.insert(raw_hash, url.clone());
    Some((url.clone(), url))
}
