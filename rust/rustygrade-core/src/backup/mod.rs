use thiserror::Error;

use crate::settings::{KEY_SAF_TREES, KEY_USB_BIT_PERFECT, KEY_USE_MEDIA_STORE};

mod cover_names;
mod export;
mod import;
mod merge;
mod path_mapper;
mod remap;
#[cfg(test)]
mod tests;

pub use cover_names::extract_cover_basename;
pub use export::{export_backup_bundle, export_playlist_backup_bundle};
#[cfg(test)]
pub use import::import_backup_bundle;
pub use import::{PreparedImport, validate_and_prepare_import};
pub use path_mapper::{MapOutcome, TrackPathMapper, normalize_path_segments};
pub use remap::{remap_stored_collections, remap_stored_stats, saved_backup_source_roots};

pub const MAX_BACKUP_COVER_BYTES: usize = 10 * 1024 * 1024;
pub const MAX_BACKUP_COVERS_TOTAL_BYTES: usize = 50 * 1024 * 1024;
/// Largest accepted `native-*.json` payload of a backup. A real file with 100k tracks is a few MiB.
pub const MAX_BACKUP_JSON_BYTES: usize = 16 * 1024 * 1024;
/// Most playlist cover files in one backup (a full + thumb pair per playlist).
pub const MAX_BACKUP_COVER_COUNT: usize = 500;
pub const KEY_BACKUP_SOURCE_ROOTS: &str = "AndroidBackupSourceRoots";
const MAX_SAVED_SOURCE_ROOTS: usize = 32;

const PLAYLIST_COVER_PREFIX: &str = "native_pl_";
/// Device-local settings keys: a backup from another device never overwrites them.
pub const ANDROID_EXTRA_KEYS: [&str; 4] = [
    KEY_SAF_TREES,
    KEY_USE_MEDIA_STORE,
    KEY_USB_BIT_PERFECT,
    KEY_BACKUP_SOURCE_ROOTS,
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupCoverFile {
    pub name: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, Default)]
pub struct BackupBundle {
    pub collections_json: Option<String>,
    pub stats_json: Option<String>,
    pub settings_json: Option<String>,
    pub playlist_covers: Vec<BackupCoverFile>,
}

#[derive(Debug, Clone, Default)]
pub struct BackupImportOptions {
    pub replace: bool,
    pub source_root_override: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BackupImportReport {
    pub collections_imported: bool,
    pub stats_imported: bool,
    pub settings_imported: bool,
    pub liked_count: usize,
    pub playlist_count: usize,
    pub smart_playlist_count: usize,
    pub stats_track_count: usize,
    /// Distinct imported track IDs resolved exactly.
    pub tracks_exact: usize,
    /// Distinct imported track IDs remapped from another OS / root / SAF path.
    pub tracks_remapped: usize,
    /// Distinct imported track IDs that could not be matched to the local library (kept as-is).
    pub tracks_unmatched: usize,
    pub covers_restored: usize,
}

#[derive(Debug, Error)]
pub enum BackupError {
    #[error("Backup archive contains no Rustygrade data files.")]
    EmptyBackup,
    #[error("Invalid JSON in {file}: {message}")]
    InvalidJson { file: &'static str, message: String },
    #[error("{what} is too large to import (limit {limit}).")]
    TooLarge { what: &'static str, limit: String },
    #[error("Invalid playlist cover '{name}': {reason}")]
    InvalidCover { name: String, reason: &'static str },
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}
