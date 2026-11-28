use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use super::{Discovery, ScanSources, Scanner, saf_logical_path};

pub(super) fn discover_sources(sources: &ScanSources) -> Discovery {
    let mut found = walk_paths(&sources.dirs);
    // Explicit files (MediaStore) are already absolute, canonical paths: one `stat` each, no
    // `is_dir` probe or `canonicalize`.
    for p in &sources.files {
        if Scanner::is_audio_file(p) && p.is_file() {
            found.files.push(p.clone());
        } else if p.is_absolute() && !p.exists() {
            // A vanished file counts its track as missing (reported, pruned only on confirmation).
            found.gone_dirs.push(p.clone());
        }
    }

    // Roots are authoritative for "missing" detection but are not walked: their files come in
    // through `files` (e.g. MediaStore). A root that is gone entirely reports its tracks missing.
    for root in &sources.roots {
        if root.is_dir() {
            let dir = fs::canonicalize(root).unwrap_or_else(|_| root.clone());
            if !found.walked_dirs.contains(&dir) {
                found.walked_dirs.push(dir);
            }
        } else if root.is_absolute() && !found.gone_dirs.contains(root) {
            found.gone_dirs.push(root.clone());
        }
    }

    for doc in &sources.saf_files {
        let logical = saf_logical_path(&doc.document_id);
        if !Scanner::is_audio_file(Path::new(&logical)) {
            continue;
        }
        found.saf.insert(logical.to_lowercase(), doc.clone());
        found.files.push(PathBuf::from(logical));
    }
    for root in &sources.saf_roots {
        let logical = PathBuf::from(saf_logical_path(root));
        if !found.walked_dirs.contains(&logical) {
            found.walked_dirs.push(logical);
        }
    }
    found
}

fn dir_identity(path: &Path) -> Option<(u64, u64)> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        fs::metadata(path).ok().map(|m| (m.dev(), m.ino()))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

pub(super) fn walk_paths(paths: &[PathBuf]) -> Discovery {
    let mut found = Discovery::default();
    let mut stack = Vec::new();
    let mut visited_dirs: HashSet<(u64, u64)> = HashSet::new();

    for p in paths {
        if p.is_dir() {
            let dir = fs::canonicalize(p).unwrap_or_else(|_| p.clone());
            if dir_identity(&dir).is_none_or(|id| visited_dirs.insert(id)) {
                stack.push(dir.clone());
                found.walked_dirs.push(dir);
            }
        } else if p.is_file() {
            if Scanner::is_audio_file(p) {
                found
                    .files
                    .push(fs::canonicalize(p).unwrap_or_else(|_| p.clone()));
            }
        } else if p.is_absolute() {
            // An absent root (moved folder, unmounted drive) counts its tracks as missing. They are
            // only reported; dropping them needs `prune_missing`, which the user has to confirm.
            found.gone_dirs.push(p.clone());
        }
    }

    while let Some(dir) = stack.pop() {
        match fs::read_dir(&dir) {
            Ok(entries) => {
                for entry in entries.flatten() {
                    let path = entry.path();
                    let Ok(ft) = entry.file_type() else {
                        continue;
                    };
                    if ft.is_symlink() {
                        continue;
                    }
                    if ft.is_dir() {
                        if dir_identity(&path).is_none_or(|id| visited_dirs.insert(id)) {
                            stack.push(path);
                        }
                    } else if ft.is_file() && Scanner::is_audio_file(&path) {
                        found.files.push(path);
                    }
                }
            }
            Err(_) => {
                found.unreadable_dirs.push(dir);
            }
        }
    }
    found
}
