use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use super::{Discovery, SAF_PREFIX, ScanCtx};
use crate::library::LibraryMap;

/// Library tracks under the scanned roots whose files are gone, as `(lowercase path, album key)`.
/// Tracks under unreadable folders are skipped; an empty or absent root counts as missing.
pub(super) fn collect_missing(
    library: &LibraryMap,
    by_path: &HashMap<String, (String, usize)>,
    found: &Discovery,
    seen_lower: &HashSet<String>,
) -> Vec<(String, String)> {
    if found.walked_dirs.is_empty() && found.gone_dirs.is_empty() {
        return Vec::new();
    }
    // Lowercase + trim every root once, not once per candidate track.
    let unreadable = LoweredDirs::new(&found.unreadable_dirs);
    let walked = LoweredDirs::new(&found.walked_dirs);
    let gone = LoweredDirs::new(&found.gone_dirs);
    by_path
        .iter()
        .filter_map(|(low_path, (album_key, idx))| {
            if seen_lower.contains(low_path) {
                return None;
            }
            let fp = library
                .get(album_key)
                .and_then(|a| a.tracks.as_ref())
                .and_then(|ts| ts.get(*idx))
                .and_then(|t| t.file_path.as_deref())?;
            // `low_path` is `fp` lowercased (the `by_path` key).
            if unreadable.contains(low_path) {
                return None;
            }
            if !walked.contains(low_path) && !gone.contains(low_path) {
                return None;
            }
            // SAF logical paths are not real files: not being listed by the provider is the signal.
            if !low_path.starts_with(SAF_PREFIX) && Path::new(fp).exists() {
                return None;
            }
            Some((low_path.clone(), album_key.clone()))
        })
        .collect()
}

/// Drops the given missing tracks from the library; returns how many were removed.
pub(super) fn prune_tracks(
    library: &mut LibraryMap,
    gone: &[(String, String)],
    touched: &mut HashSet<String>,
) -> usize {
    let gone_paths: HashSet<&str> = gone.iter().map(|(p, _)| p.as_str()).collect();
    // Each affected album once: `gone` lists one entry per track, and the retain below walks the
    // whole album every time.
    let albums: HashSet<&str> = gone.iter().map(|(_, k)| k.as_str()).collect();
    let mut removed = 0usize;
    for album_key in albums {
        if let Some(album) = library.get_mut(album_key)
            && let Some(tracks) = &mut album.tracks
        {
            let before = tracks.len();
            tracks.retain(|t| {
                t.file_path
                    .as_ref()
                    .is_none_or(|fp| !gone_paths.contains(fp.to_lowercase().as_str()))
            });
            removed += before - tracks.len();
            touched.insert(album_key.to_owned());
        }
    }
    removed
}

pub(super) fn file_stamp(path: &Path, ctx: &ScanCtx<'_>) -> Option<(i64, i64)> {
    if let Some(doc) = ctx.saf_file(path) {
        return Some((doc.mtime_ms, doc.size));
    }
    let meta = fs::metadata(path).ok()?;
    let modified = meta.modified().ok()?;
    let ms = modified
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    Some((ms, meta.len() as i64))
}

/// Root directories lowercased with trailing separators trimmed, prepared once per scan.
struct LoweredDirs(Vec<String>);

impl LoweredDirs {
    fn new(dirs: &[PathBuf]) -> Self {
        Self(
            dirs.iter()
                .map(|d| d.to_string_lossy().trim_end_matches(['/', '\\']).to_lowercase())
                .collect(),
        )
    }

    /// Whether the already-lowercased `low_path` lies strictly inside one of the directories.
    fn contains(&self, low_path: &str) -> bool {
        self.0.iter().any(|trimmed| {
            low_path.len() > trimmed.len() + 1
                && low_path.starts_with(trimmed.as_str())
                && (matches!(low_path.as_bytes().get(trimmed.len()), Some(b'/') | Some(b'\\'))
                    // A SAF volume root ("/saf/primary:") has its children glued on: "/saf/primary:Music/..".
                    || (trimmed.starts_with(SAF_PREFIX) && trimmed.ends_with(':')))
        })
    }
}
