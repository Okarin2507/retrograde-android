use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use crate::library::{LibraryError, LibraryStore};
use crate::models::ReplayGain;

mod cover_extract;
mod discovery;
mod grouping;
mod metadata;
mod prune;
mod scan;
#[cfg(test)]
mod tests;

pub use grouping::common_artist;

use discovery::discover_sources;
#[cfg(test)]
use discovery::walk_paths;
use scan::scan_locked;

pub const AUDIO_EXTENSIONS: &[&str] = &[
    "flac", "mp3", "wav", "aac", "ogg", "m4a", "wma", "aiff", "alac", "opus",
];

#[derive(Debug, Clone)]
pub struct ScanProgress {
    pub phase: &'static str,
    pub scanned: usize,
    pub total: usize,
    pub current_file: String,
}

#[derive(Debug, Clone, Default)]
pub struct ScanResult {
    pub added_tracks: usize,
    pub new_albums: usize,
    pub covers: usize,
    pub updated_tracks: usize,
    pub removed_tracks: usize,
    pub missing_tracks: usize,
    pub merged_albums: usize,
    /// The scan was cancelled. Cancelled before the tracks were applied nothing is saved; cancelled
    /// while fetching covers, the tracks already parsed are kept and the missing covers retry next scan.
    pub cancelled: bool,
}

impl ScanResult {
    pub fn changed(&self) -> bool {
        self.added_tracks > 0
            || self.covers > 0
            || self.updated_tracks > 0
            || self.removed_tracks > 0
            || self.merged_albums > 0
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ScanOptions {
    pub prune_missing: bool,
    pub max_threads: Option<usize>,
    pub force: bool,
    pub resync_covers: bool,
}

/// Prefix of the path-shaped identity given to tracks that come from the Storage Access Framework.
/// The real `content://` URI is kept in [`SafFile::uri`] (and in the track's `src`).
pub const SAF_PREFIX: &str = "/saf/";

/// Logical, path-shaped identity for a SAF document: `primary:Music/Album/01.flac` ->
/// `/saf/primary:Music/Album/01.flac`. Folder grouping, ids and "missing" detection all work on it.
pub fn saf_logical_path(document_id: &str) -> String {
    format!("{SAF_PREFIX}{}", document_id.trim_start_matches('/'))
}

pub fn is_saf_path(file_path: &str) -> bool {
    file_path.starts_with(SAF_PREFIX)
}

/// Opens a SAF document (`content://` URI) as an owned read-only file. On Android this is backed
/// by `ContentResolver.openFileDescriptor(..).detachFd()`; Rust takes ownership and closes it.
pub trait FileOpener: Send + Sync {
    fn open(&self, uri: &str) -> Option<File>;
}

/// A document found by walking a SAF tree on the Kotlin side.
#[derive(Debug, Clone)]
pub struct SafFile {
    pub document_id: String,
    pub uri: String,
    /// `COLUMN_LAST_MODIFIED` (milliseconds since epoch).
    pub mtime_ms: i64,
    pub size: i64,
}

/// Everything one scan looks at.
///
/// - `dirs`: walked recursively with `read_dir` (desktop-style / readable shared storage).
/// - `files`: explicit files, e.g. enumerated from `MediaStore`.
/// - `roots`: directories whose tracks count as *missing* when absent from `files`/`dirs`. Needed
///   with an explicit file list: otherwise nothing is "walked" and deletions are never noticed.
/// - `saf_files` / `saf_roots`: SAF documents and the document ids of their tree roots.
#[derive(Debug, Clone, Default)]
pub struct ScanSources {
    pub dirs: Vec<PathBuf>,
    pub files: Vec<PathBuf>,
    pub roots: Vec<PathBuf>,
    pub saf_files: Vec<SafFile>,
    pub saf_roots: Vec<String>,
}

/// Per-scan I/O context: resolves SAF logical paths to URIs and opens them.
#[derive(Default)]
struct ScanCtx<'a> {
    /// Lowercased logical path -> document.
    saf: HashMap<String, SafFile>,
    opener: Option<&'a dyn FileOpener>,
}

impl ScanCtx<'_> {
    fn saf_file(&self, path: &Path) -> Option<&SafFile> {
        if self.saf.is_empty() {
            return None;
        }
        self.saf.get(&path.to_string_lossy().to_lowercase())
    }

    /// Opens a real file by path, or a SAF document through the opener.
    fn open(&self, path: &Path) -> Option<File> {
        match self.saf_file(path) {
            Some(doc) => self.opener?.open(&doc.uri),
            None => File::open(path).ok(),
        }
    }
}

#[derive(Default)]
struct Discovery {
    files: Vec<PathBuf>,
    walked_dirs: Vec<PathBuf>,
    unreadable_dirs: Vec<PathBuf>,
    gone_dirs: Vec<PathBuf>,
    saf: HashMap<String, SafFile>,
}

#[derive(Debug, Clone)]
struct Parsed {
    file_path: String,
    title: String,
    artist: String,
    album_artist: Option<String>,
    album: String,
    duration: f64,
    track_number: i32,
    disc_number: i32,
    year: i32,
    codec: String,
    sample_rate: i32,
    bits_per_sample: i32,
    channels: i32,
    has_picture: bool,
    replay_gain: ReplayGain,
}

pub struct Scanner;

impl Scanner {
    pub fn is_audio_file(path: &Path) -> bool {
        path.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| AUDIO_EXTENSIONS.iter().any(|x| x.eq_ignore_ascii_case(e)))
    }

    #[cfg(test)]
    pub fn discover(paths: &[PathBuf]) -> Vec<PathBuf> {
        walk_paths(paths).files
    }

    #[cfg(test)]
    pub fn scan(
        input_paths: &[PathBuf],
        options: ScanOptions,
        cancel: Option<&AtomicBool>,
        mut progress: Option<&mut dyn FnMut(ScanProgress)>,
    ) -> Result<ScanResult, LibraryError> {
        if let Some(cb) = progress.as_deref_mut() {
            cb(ScanProgress {
                phase: "discovering",
                scanned: 0,
                total: 0,
                current_file: "Discovering files...".to_owned(),
            });
        }

        let found = walk_paths(input_paths);
        let _guard = lock_library();
        let ctx = ScanCtx::default();
        scan_locked(found, &ctx, options, cancel, progress)
    }

    /// Scan with explicit files, authoritative roots and/or SAF documents (Android).
    pub fn scan_sources(
        sources: &ScanSources,
        opener: Option<&dyn FileOpener>,
        options: ScanOptions,
        cancel: Option<&AtomicBool>,
        mut progress: Option<&mut dyn FnMut(ScanProgress)>,
    ) -> Result<ScanResult, LibraryError> {
        if let Some(cb) = progress.as_deref_mut() {
            cb(ScanProgress {
                phase: "discovering",
                scanned: 0,
                total: 0,
                current_file: "Discovering files...".to_owned(),
            });
        }
        let mut found = discover_sources(sources);
        let ctx = ScanCtx {
            saf: std::mem::take(&mut found.saf),
            opener,
        };
        let _guard = lock_library();
        scan_locked(found, &ctx, options, cancel, progress)
    }
}

/// A panic while parsing a user file must not wedge every later scan: recover the poisoned lock.
fn lock_library() -> std::sync::MutexGuard<'static, ()> {
    LibraryStore::write_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
