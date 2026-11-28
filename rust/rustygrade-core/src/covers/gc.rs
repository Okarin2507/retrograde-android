use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use image::ImageReader;

use super::encode::MAX_FULL_COVER_EDGE;
use super::store::{PlaylistCovers, blur_path_for, save_cover_bytes};
use crate::collections::{CollectionsData, CollectionsStore};
use crate::library::{LibraryError, LibraryMap, LibraryStore, try_delete};
use crate::paths::CoverPaths;

/// Collects every playlist cover path (and any legacy `_thumb.jpg` sibling) from `collections`.
pub fn collect_playlist_cover_paths(collections: &CollectionsData, out: &mut HashSet<PathBuf>) {
    for pl in &collections.playlists {
        if let Some(p) = CoverPaths::to_local_path(pl.cover_art.as_deref()) {
            let thumb = PlaylistCovers::thumb_for(&p);
            if thumb != p {
                out.insert(thumb);
            }
            out.insert(p);
        }
    }
}

/// Every cover file path referenced by albums in `library` or playlists in `collections`.
pub fn referenced_cover_paths(
    library: &LibraryMap,
    collections: &CollectionsData,
) -> HashSet<PathBuf> {
    let mut set = HashSet::new();
    for album in library.values() {
        if let Some(p) = CoverPaths::to_local_path(album.cover_art.as_deref()) {
            set.insert(p);
        }
        if let Some(p) = CoverPaths::to_local_path(album.cover_art_full.as_deref()) {
            set.insert(p);
        }
    }
    collect_playlist_cover_paths(collections, &mut set);
    set
}

/// Deletes any managed cover file (and its unreferenced blur cache) in `candidates` that is no
/// longer referenced in `library`, `collections`, or `extra_refs`.
pub fn delete_unreferenced_covers_with_extra(
    candidates: &[PathBuf],
    library: &LibraryMap,
    collections: &CollectionsData,
    extra_refs: &HashSet<PathBuf>,
) {
    if candidates.is_empty() {
        return;
    }
    let mut referenced = referenced_cover_paths(library, collections);
    referenced.extend(extra_refs.iter().cloned());
    let referenced_blurs: HashSet<PathBuf> = referenced
        .iter()
        .filter_map(|p| blur_path_for(p))
        .collect();
    for p in candidates {
        if CoverPaths::is_managed_cover_file(p) && !referenced.contains(p) {
            try_delete(p);
            if let Some(blur) = blur_path_for(p)
                && !referenced_blurs.contains(&blur)
            {
                try_delete(&blur);
            }
        }
    }
}

/// Deletes any managed cover file in `candidates` that is no longer referenced in `library` or `collections`.
pub fn delete_unreferenced_covers(
    candidates: &[PathBuf],
    library: &LibraryMap,
    collections: &CollectionsData,
) {
    delete_unreferenced_covers_with_extra(candidates, library, collections, &HashSet::new());
}

/// Migrates legacy playlist covers stored under `CoverPaths::cover_dir()` to single-file
/// content-addressed covers (`c_<hash>.<ext>`). Returns `true` if `collections` changed.
pub fn migrate_playlist_covers(collections: &mut CollectionsData) -> bool {
    let cover_dir = CoverPaths::cover_dir();
    let mut changed = false;
    for pl in &mut collections.playlists {
        let Some(local) = CoverPaths::to_local_path(pl.cover_art.as_deref()) else {
            continue;
        };
        if local.parent() != Some(cover_dir.as_path()) {
            continue;
        }
        if CoverPaths::is_content_cover_file(&local) && local.is_file() {
            continue;
        }
        if !local.is_file() {
            continue;
        }
        if let Ok(bytes) = fs::read(&local)
            && let Ok(new_path) = save_cover_bytes(&bytes)
        {
            let url = CoverPaths::to_file_url(&new_path);
            if pl.cover_art.as_deref() != Some(url.as_str()) {
                pl.cover_art = Some(url);
                changed = true;
            }
        }
    }
    changed
}

#[derive(Debug, Clone, Default)]
pub struct CoverDedupResult {
    pub migrated_albums: usize,
    pub removed_files: usize,
    pub changed_paths: Vec<PathBuf>,
}

/// Migrates legacy dual-file (`_thumb.jpg` / `_full.jpg`) or oversized album covers to deduplicated
/// single-file content-addressed covers, then deletes unreferenced managed files in `cover_dir`.
pub fn deduplicate_and_sweep_covers(
    extra_collections: Option<&CollectionsData>,
    extra_refs: Option<&Mutex<HashSet<PathBuf>>>,
) -> Result<CoverDedupResult, LibraryError> {
    let _guard = LibraryStore::write_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let sweep_started = SystemTime::now();
    let pre_collections = CollectionsStore::load();
    // The write lock is already held: the recovering `load_raw` would re-lock it and deadlock.
    let mut library = LibraryStore::load_raw_locked()?;
    let mut explicitly_replaced: HashSet<PathBuf> = HashSet::new();
    let mut checked_ok: HashSet<PathBuf> = HashSet::new();
    let mut result = CoverDedupResult::default();
    let mut changed_seen: HashSet<PathBuf> = HashSet::new();

    for album in library.values_mut() {
        let thumb = CoverPaths::to_local_path(album.cover_art.as_deref());
        let full = CoverPaths::to_local_path(album.cover_art_full.as_deref());

        if thumb.is_some()
            && thumb == full
            && let Some(p) = thumb.as_deref()
        {
            if checked_ok.contains(p) {
                continue;
            }
            if CoverPaths::is_content_cover_file(p) && p.is_file() {
                let within_cap = ImageReader::open(p)
                    .and_then(|r| r.with_guessed_format())
                    .ok()
                    .and_then(|r| r.into_dimensions().ok())
                    .is_some_and(|(w, h)| w.max(h) <= MAX_FULL_COVER_EDGE);
                if within_cap {
                    checked_ok.insert(p.to_path_buf());
                    continue;
                }
            }
        }

        let src = full
            .as_deref()
            .filter(|p| p.is_file())
            .or_else(|| thumb.as_deref().filter(|p| p.is_file()));
        if let Some(src_path) = src {
            if let Ok(bytes) = fs::read(src_path)
                && let Ok(new_path) = save_cover_bytes(&bytes)
            {
                checked_ok.insert(new_path.clone());
                let url = CoverPaths::to_file_url(&new_path);
                for old in [thumb.as_ref(), full.as_ref()].into_iter().flatten() {
                    if old != &new_path {
                        explicitly_replaced.insert(old.clone());
                        if let Some(old_blur) = blur_path_for(old) {
                            explicitly_replaced.insert(old_blur);
                        }
                        if changed_seen.insert(old.clone()) {
                            result.changed_paths.push(old.clone());
                        }
                    }
                }
                if changed_seen.insert(new_path.clone()) {
                    result.changed_paths.push(new_path.clone());
                }
                album.cover_art = Some(url.clone());
                album.cover_art_full = Some(url);
                result.migrated_albums += 1;
            }
        } else if album.cover_art.is_some() || album.cover_art_full.is_some() {
            album.cover_art = None;
            album.cover_art_full = None;
            result.migrated_albums += 1;
        }
    }

    if result.migrated_albums > 0 {
        LibraryStore::save_raw(&library)?;
    }

    let post_collections = CollectionsStore::load();
    let mut referenced = referenced_cover_paths(&library, &pre_collections);
    collect_playlist_cover_paths(&post_collections, &mut referenced);
    if let Some(c) = extra_collections {
        collect_playlist_cover_paths(c, &mut referenced);
    }
    if let Some(refs) = extra_refs
        && let Ok(guard) = refs.lock()
    {
        referenced.extend(guard.iter().cloned());
    }
    let referenced_blurs: HashSet<PathBuf> = referenced
        .iter()
        .filter_map(|p| blur_path_for(p))
        .collect();

    let cutoff = sweep_started.checked_sub(Duration::from_secs(5));
    if let Ok(entries) = fs::read_dir(CoverPaths::cover_dir()) {
        for entry in entries.flatten() {
            let p = entry.path();
            let sweep_cover = CoverPaths::is_managed_cover_file(&p) && !referenced.contains(&p);
            let sweep_blur = CoverPaths::is_blur_cache_file(&p) && !referenced_blurs.contains(&p);
            if (sweep_cover || sweep_blur) && entry.file_type().is_ok_and(|t| t.is_file()) {
                let old_enough = entry
                    .metadata()
                    .and_then(|m| m.modified())
                    .ok()
                    .is_none_or(|mtime| cutoff.is_none_or(|c| mtime < c));
                if (explicitly_replaced.contains(&p) || old_enough) && fs::remove_file(&p).is_ok() {
                    result.removed_files += 1;
                }
            }
        }
    }

    Ok(result)
}
