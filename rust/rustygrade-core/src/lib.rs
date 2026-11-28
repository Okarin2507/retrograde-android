pub mod backup;
pub mod collections;
pub mod covers;
pub mod error_log;
pub mod fsutil;
pub mod library;
pub mod library_view;
pub mod lyrics;
pub mod m3u;
pub mod models;
pub mod paths;
pub mod playback;
pub mod r128;
pub mod scanner;
pub mod session;
pub mod settings;
pub mod smart;
pub mod stats;
pub mod tag_writer;

#[cfg(any(test, feature = "test-fixtures"))]
pub mod testing;
#[cfg(test)]
mod scan_tests;

pub use backup::{
    BackupBundle, BackupCoverFile, BackupError, BackupImportOptions, BackupImportReport,
    MapOutcome, TrackPathMapper, export_backup_bundle, export_playlist_backup_bundle,
    extract_cover_basename, normalize_path_segments, remap_stored_collections, remap_stored_stats,
    saved_backup_source_roots, validate_and_prepare_import,
};
pub use collections::{
    CollectionsData, CollectionsStore, PlaylistData, SmartPlaylistData, SmartRule,
};
pub use covers::{
    CoverDedupResult, CoverPalette, ImageLimits, MIN_ACCENT_LUMINANCE, PlaylistCovers,
    PreparedCover, SavedCover, blurred_backdrop, collect_playlist_cover_paths,
    deduplicate_and_sweep_covers, delete_unreferenced_covers, delete_unreferenced_covers_with_extra,
    extract_cover_palette, extract_cover_palette_from_rgb, migrate_playlist_covers,
    prepare_cover_bytes, referenced_cover_paths, relative_luminance, save_cover_bytes,
    write_prepared_cover,
};
pub use fsutil::now_ms;
pub use library::{AlbumDto, LibraryStore, TrackDto, TrackIndex, TrackLookup, build_track_index};
pub use library_view::{
    LibrarySort, contains_folded, ensure_default_smart_playlists, fold, group_artists, search_albums,
    search_artists, sort_library,
};
pub use lyrics::{LyricLine, LyricsDoc, LyricsLoader};
pub use m3u::{
    MAX_M3U_BYTES, M3uEntry, M3uError, M3uMatchResult, ParsedM3u, export_m3u8, match_m3u_playlist,
    parse_m3u, sanitize_export_filename,
};
pub use models::{Album, ArtistSummary, ReplayGain, Track};
pub use paths::CoverPaths;
pub use playback::{CrossfadeMath, PlayTracker, PlaybackQueue, PlayerAdvance, RepeatMode};
pub use scanner::{
    FileOpener, SAF_PREFIX, SafFile, ScanOptions, ScanProgress, ScanResult, ScanSources, Scanner,
    is_saf_path, saf_logical_path,
};
pub use session::{SessionState, SessionStore};
pub use settings::{
    AppSettings, LIBRARY_SORT_KEYS, canonical_library_sort,
};
pub use smart::{Codecs, SmartField, SmartPlaylists};
pub use stats::{PlayStat, PlayStats};
pub use tag_writer::{TagEdit, TagWriter};
