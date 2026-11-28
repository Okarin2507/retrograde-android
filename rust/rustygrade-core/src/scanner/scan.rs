use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use rayon::prelude::*;

use super::cover_extract::{FolderImageCache, album_cover_missing, find_and_save_cover};
use super::grouping::{
    AlbumFolders, common_artist, merge_split_albums, normalize_artist, resolve_album, same_artist,
};
use super::metadata::{ParseFailure, parse_guarded, track_extra};
use super::prune::{collect_missing, file_stamp, prune_tracks};
use super::{Discovery, Parsed, ScanCtx, ScanOptions, ScanProgress, ScanResult};
use crate::collections::CollectionsStore;
use crate::covers::delete_unreferenced_covers;
use crate::error_log;
use crate::library::{
    LibraryError, LibraryStore, SCAN_SCHEMA, SCAN_SCHEMA_KEY, TrackDto,
    collect_album_cover_candidates,
};
use crate::paths::CoverPaths;

pub(super) fn scan_locked(
    found: Discovery,
    ctx: &ScanCtx<'_>,
    options: ScanOptions,
    cancel: Option<&AtomicBool>,
    mut progress: Option<&mut dyn FnMut(ScanProgress)>,
) -> Result<ScanResult, LibraryError> {
    let mut library = LibraryStore::load_raw_locked()?;
    let mut dirty = false;
    let mut touched: HashSet<String> = HashSet::new();
    let mut mixed: HashSet<String> = HashSet::new();
    let mut detached_covers: Vec<PathBuf> = Vec::new();

    let merged = merge_split_albums(&mut library, &mut touched, &mut mixed, &mut detached_covers);

    // Map lowercase filePath -> (album_key, track_index)
    let mut by_path: HashMap<String, (String, usize)> = HashMap::new();
    for (key, album) in &library {
        if let Some(tracks) = &album.tracks {
            for (idx, t) in tracks.iter().enumerate() {
                if let Some(fp) = &t.file_path {
                    by_path.entry(fp.to_lowercase()).or_insert((key.clone(), idx));
                }
            }
        }
    }

    let mut to_parse: Vec<PathBuf> = Vec::new();
    let mut changed: HashSet<String> = HashSet::new();
    let mut stamps: HashMap<String, (i64, i64)> = HashMap::new();
    let mut in_scan: HashSet<String> = HashSet::new();
    let mut seen_lower: HashSet<String> = HashSet::new();

    for f in &found.files {
        let f_str = f.to_string_lossy().into_owned();
        let f_low = f_str.to_lowercase();
        if !seen_lower.insert(f_low.clone()) {
            // The same file reached through overlapping sources (e.g. nested SAF trees).
            continue;
        }
        let Some(stamp) = file_stamp(f, ctx) else {
            continue;
        };
        stamps.insert(f_low.clone(), stamp);

        if let Some((album_key, track_idx)) = by_path.get(&f_low) {
            in_scan.insert(album_key.clone());
            if let Some(album) = library.get_mut(album_key)
                && let Some(tracks) = &mut album.tracks
                && let Some(track) = tracks.get_mut(*track_idx)
            {
                // Entries written before the scanner read ReplayGain are re-parsed once.
                let stale_schema =
                    track.extra.get(SCAN_SCHEMA_KEY).and_then(|v| v.as_u64()) != Some(SCAN_SCHEMA);
                if track.mtime.is_none() || track.size.is_none() {
                    track.mtime = Some(stamp.0);
                    track.size = Some(stamp.1);
                    dirty = true;
                    if stale_schema {
                        to_parse.push(f.clone());
                        changed.insert(f_low);
                    }
                } else if options.force
                    || stale_schema
                    || track.mtime != Some(stamp.0)
                    || track.size != Some(stamp.1)
                {
                    to_parse.push(f.clone());
                    changed.insert(f_low);
                }
            }
        } else {
            to_parse.push(f.clone());
        }
    }

    let gone = collect_missing(&library, &by_path, &found, &seen_lower);
    let missing = gone.len();
    let removed = if options.prune_missing {
        prune_tracks(&mut library, &gone, &mut touched)
    } else {
        0
    };

    let mut parsed_results: Vec<Parsed> = Vec::new();
    // Files that opened but whose tags cannot be read (lowercased paths).
    let mut unparseable: HashSet<String> = HashSet::new();
    if !to_parse.is_empty() {
        let total = to_parse.len();
        if let Some(cb) = progress.as_deref_mut() {
            cb(ScanProgress {
                phase: "scanning",
                scanned: 0,
                total,
                current_file: String::new(),
            });
        }
        let threads = options
            .max_threads
            .unwrap_or_else(|| std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(4))
            .max(1);
        let done = AtomicUsize::new(0);
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .map_err(std::io::Error::other)?;
        // Parse on a worker thread so this thread can report real progress while it runs.
        let outcomes = std::thread::scope(|scope| {
            let worker = scope.spawn(|| {
                pool.install(|| {
                    to_parse
                        .par_iter()
                        .filter_map(|p| {
                            if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
                                return None;
                            }
                            let res = parse_guarded(p, ctx);
                            done.fetch_add(1, Ordering::Relaxed);
                            Some((p.to_string_lossy().to_lowercase(), res))
                        })
                        .collect::<Vec<(String, Result<Parsed, ParseFailure>)>>()
                })
            });
            let mut last = usize::MAX;
            while !worker.is_finished() {
                std::thread::sleep(std::time::Duration::from_millis(20));
                let n = done.load(Ordering::Relaxed).min(total);
                if n != last {
                    last = n;
                    if let Some(cb) = progress.as_deref_mut() {
                        cb(ScanProgress {
                            phase: "scanning",
                            scanned: n,
                            total,
                            current_file: String::new(),
                        });
                    }
                }
            }
            worker.join().unwrap_or_else(|_| {
                error_log::write("scanner", "tag-parsing worker panicked; scan results are partial");
                Vec::new()
            })
        });
        for (low, outcome) in outcomes {
            match outcome {
                Ok(parsed) => parsed_results.push(parsed),
                Err(ParseFailure::Unparseable) => {
                    unparseable.insert(low);
                }
                // The file could not be opened (SAF provider glitch, no opener, permission blip):
                // it may well parse next time.
                Err(ParseFailure::Unavailable) => {}
            }
        }
        if let Some(cb) = progress.as_deref_mut() {
            cb(ScanProgress {
                phase: "scanning",
                scanned: total,
                total,
                current_file: String::new(),
            });
        }
    }

    if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
        return Ok(ScanResult { cancelled: true, ..ScanResult::default() });
    }

    // A file that was due for a re-parse and opened, but whose tags cannot be read (corrupt), must
    // not be retried on every scan: record the stamp and schema so it waits for the file to change.
    // Files that could not even be opened are left alone and retried.
    if !changed.is_empty() {
        for low in changed.iter().filter(|low| unparseable.contains(*low)) {
            let Some((album_key, _)) = by_path.get(low) else { continue };
            let stamp = stamps.get(low).copied();
            let Some(tracks) = library.get_mut(album_key).and_then(|a| a.tracks.as_mut()) else { continue };
            for t in tracks.iter_mut().filter(|t| {
                t.file_path.as_ref().is_some_and(|fp| fp.to_lowercase() == *low)
            }) {
                t.extra.insert(SCAN_SCHEMA_KEY.to_owned(), serde_json::Value::from(SCAN_SCHEMA));
                if let Some((m, s)) = stamp {
                    t.mtime = Some(m);
                    t.size = Some(s);
                }
                dirty = true;
            }
        }
    }

    parsed_results.sort_by_cached_key(|a| a.file_path.to_lowercase());

    let mut folder_cache: HashMap<String, AlbumFolders> = HashMap::new();
    let mut picture_source: HashMap<String, String> = HashMap::new();
    let mut added = 0usize;
    let mut updated = 0usize;
    let mut new_albums = 0usize;
    let now_ms = crate::fsutil::now_ms();

    // Pull every re-parsed track out of its old album with one `retain` per album (not one per
    // track). The album's folder set is captured first, so tracks re-resolving into it still see
    // the folders of their siblings that are being re-added.
    let mut reparsed_by_album: HashMap<String, HashSet<String>> = HashMap::new();
    for r in &parsed_results {
        let low = r.file_path.to_lowercase();
        if changed.contains(&low)
            && let Some((old_key, _)) = by_path.get(&low)
        {
            reparsed_by_album.entry(old_key.clone()).or_default().insert(low);
        }
    }
    // Lowercased paths pulled out above: re-added below and counted as updates.
    let mut updated_paths: HashSet<String> = HashSet::new();
    for (old_key, lows) in reparsed_by_album {
        let Some(old_album) = library.get_mut(&old_key) else { continue };
        if old_album.tracks.is_none() {
            continue;
        }
        folder_cache.insert(old_key.clone(), AlbumFolders::from_album(old_album));
        if let Some(tracks) = &mut old_album.tracks {
            tracks.retain(|t| t.file_path.as_ref().is_none_or(|fp| !lows.contains(&fp.to_lowercase())));
        }
        touched.insert(old_key);
        updated_paths.extend(lows);
    }
    // Paths added during this pass. Together with `by_path` (every path in the library before the
    // scan, minus `updated_paths`) this is the set of tracks already present: no rebuild needed.
    let mut added_paths: HashSet<String> = HashSet::new();

    for r in parsed_results {
        let low = r.file_path.to_lowercase();
        let is_update = updated_paths.contains(&low);

        let (key, created) = resolve_album(&mut library, &mut folder_cache, &r, now_ms);
        if created {
            new_albums += 1;
        }
        let album = library.get_mut(&key).expect("album just resolved");
        if album.year == 0 && r.year > 0 {
            album.year = r.year;
        }
        if !created
            && let Some(ra) = &r.album_artist
            && !ra.trim().is_empty()
            && let Some(aa) = &album.album_artist
            && !aa.trim().is_empty()
            && !same_artist(aa, ra)
        {
            mixed.insert(key.clone());
        } else if album.album_artist.as_deref().unwrap_or("").is_empty()
            && let Some(ra) = &r.album_artist
            && !ra.is_empty()
            && !mixed.contains(&key)
        {
            album.album_artist = Some(ra.clone());
        }

        if r.has_picture && !picture_source.contains_key(&key) {
            picture_source.insert(key.clone(), r.file_path.clone());
        }
        if (!is_update && by_path.contains_key(&low)) || added_paths.contains(&low) {
            continue;
        }

        let stamp = stamps.get(&low).copied();
        let replay_gain = r.replay_gain;
        let track = TrackDto {
            id: Some(r.file_path.clone()),
            title: Some(r.title),
            artist: Some(r.artist),
            album: Some(key.clone()),
            duration: r.duration,
            file_path: Some(r.file_path.clone()),
            src: Some(match ctx.saf_file(Path::new(&r.file_path)) {
                Some(doc) => doc.uri.clone(),
                None => CoverPaths::to_file_url(Path::new(&r.file_path)),
            }),
            track_number: Some(r.track_number),
            disc_number: if r.disc_number <= 0 { 1 } else { r.disc_number },
            codec: Some(r.codec),
            sample_rate: r.sample_rate,
            bits_per_sample: r.bits_per_sample,
            channels: r.channels,
            mtime: stamp.map(|s| s.0),
            size: stamp.map(|s| s.1),
            extra: track_extra(&replay_gain),
        };
        album.tracks.get_or_insert_with(Vec::new).push(track);
        folder_cache
            .entry(key.clone())
            .or_insert_with(|| AlbumFolders::from_album(album))
            .add(&r.file_path);
        added_paths.insert(low);
        touched.insert(key.clone());
        in_scan.insert(key);
        if is_update {
            updated += 1;
        } else {
            added += 1;
        }
    }

    for key in &touched {
        if let Some(a) = library.get(key)
            && a.album_artist.as_deref().unwrap_or("").trim().is_empty()
            && let Some(tracks) = &a.tracks
            && tracks.len() > 1
        {
            let distinct: HashSet<String> = tracks
                .iter()
                .map(|t| normalize_artist(t.artist.as_deref().unwrap_or("")))
                .collect();
            if distinct.len() > 1 {
                mixed.insert(key.clone());
            }
        }
    }
    for key in &mixed {
        if let Some(album) = library.get_mut(key)
            && let Some(tracks) = &album.tracks
            && !tracks.is_empty()
        {
            album.album_artist = None;
            album.artist = Some(common_artist(tracks.iter().map(|t| t.artist.as_deref())));
        }
    }

    for key in &touched {
        let empty = library
            .get(key)
            .is_some_and(|a| a.tracks.as_ref().is_none_or(|t| t.is_empty()));
        if empty {
            if let Some(removed_album) = library.shift_remove(key) {
                // Cover files only go with a confirmed prune; an orphaned cover is harmless.
                if options.prune_missing {
                    collect_album_cover_candidates(key, &removed_album, &mut detached_covers);
                }
            }
            continue;
        }
        if let Some(album) = library.get_mut(key)
            && let Some(tracks) = &mut album.tracks
        {
            tracks.sort_by(|a, b| {
                let da = if a.disc_number <= 0 { 1 } else { a.disc_number };
                let db = if b.disc_number <= 0 { 1 } else { b.disc_number };
                da.cmp(&db)
                    .then_with(|| a.track_number.unwrap_or(0).cmp(&b.track_number.unwrap_or(0)))
            });
        }
    }

    let needing: Vec<String> = in_scan
        .into_iter()
        .filter(|k| {
            library
                .get(k)
                .is_some_and(|a| options.resync_covers || album_cover_missing(a))
        })
        .collect();
    let mut covers = 0usize;
    let mut cancelled_in_covers = false;
    let mut cover_dedup_cache: HashMap<[u8; 32], String> = HashMap::new();
    let mut folder_images = FolderImageCache::new();
    let total_covers = needing.len();
    for (i, key) in needing.into_iter().enumerate() {
        if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
            cancelled_in_covers = true;
            break;
        }
        if let Some(cb) = progress.as_deref_mut() {
            cb(ScanProgress {
                phase: "covers",
                scanned: i,
                total: total_covers,
                current_file: key.clone(),
            });
        }
        let pic_src = picture_source.get(&key).map(String::as_str);
        if let Some(album) = library.get(&key)
            && let Some((thumb, full)) = find_and_save_cover(album, pic_src, ctx, &mut cover_dedup_cache, &mut folder_images)
            && let Some(album_mut) = library.get_mut(&key)
        {
            let new_thumb_local = CoverPaths::to_local_path(Some(&thumb));
            let new_full_local = CoverPaths::to_local_path(Some(&full));
            if let Some(old) = CoverPaths::to_local_path(album_mut.cover_art.as_deref())
                && new_thumb_local.as_ref() != Some(&old)
            {
                detached_covers.push(old);
            }
            if let Some(old) = CoverPaths::to_local_path(album_mut.cover_art_full.as_deref())
                && new_full_local.as_ref() != Some(&old)
            {
                detached_covers.push(old);
            }
            if album_mut.cover_art.as_deref() != Some(thumb.as_str())
                || album_mut.cover_art_full.as_deref() != Some(full.as_str())
            {
                dirty = true;
            }
            album_mut.cover_art = Some(thumb);
            album_mut.cover_art_full = Some(full);
            covers += 1;
        }
    }

    let result = ScanResult {
        added_tracks: added,
        new_albums,
        covers,
        updated_tracks: updated,
        removed_tracks: removed,
        missing_tracks: if options.prune_missing { 0 } else { missing },
        merged_albums: merged,
        cancelled: cancelled_in_covers,
    };
    if !mixed.is_empty() {
        dirty = true;
    }
    if result.changed() || dirty {
        LibraryStore::save_raw(&library)?;
        if !detached_covers.is_empty() {
            let collections = CollectionsStore::load();
            delete_unreferenced_covers(&detached_covers, &library, &collections);
        }
    }
    Ok(result)
}
