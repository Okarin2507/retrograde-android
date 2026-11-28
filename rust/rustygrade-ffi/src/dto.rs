//! Coarse-grained snapshot records crossing the FFI boundary.
//!
//! Compose never calls into Rust per row: the engine returns whole, already sorted/filtered lists.

use std::path::Path;

use rustygrade_core::{Album, ArtistSummary, ReplayGain, Track};

fn path_str(p: Option<&Path>) -> Option<String> {
    p.map(|p| p.to_string_lossy().into_owned())
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct AlbumSummary {
    /// Stable key (the album name as stored in `library.json`).
    pub key: String,
    pub name: String,
    pub artist: String,
    pub year: i32,
    pub added_at: i64,
    pub track_count: u32,
    pub duration_secs: f64,
    pub thumb_path: Option<String>,
    pub full_path: Option<String>,
    /// e.g. `FLAC / 24BIT / 96KHZ`, `MIXED / ...`.
    pub quality_label: Option<String>,
}

impl From<&Album> for AlbumSummary {
    fn from(a: &Album) -> Self {
        Self {
            key: a.name.clone(),
            name: a.name.clone(),
            artist: a.artist.clone(),
            year: a.year,
            added_at: a.added_at,
            track_count: a.tracks.len() as u32,
            duration_secs: a.tracks.iter().map(|t| t.duration).sum(),
            thumb_path: path_str(a.cover_thumb_path.as_deref()),
            full_path: path_str(a.cover_full_path.as_deref()),
            quality_label: Track::album_quality_label(&a.tracks),
        }
    }
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct ArtistInfo {
    pub name: String,
    pub album_count: u32,
    pub track_count: u32,
    pub newest_added_at: i64,
    pub thumb_path: Option<String>,
}

impl ArtistInfo {
    /// `albums` is the library slice the artist was grouped from.
    pub(crate) fn new(a: &ArtistSummary, albums: &[Album]) -> Self {
        Self {
            name: a.name.clone(),
            album_count: a.album_indices.len() as u32,
            track_count: a.track_count as u32,
            newest_added_at: a.newest_added_at,
            thumb_path: a
                .albums(albums)
                .find_map(|al| path_str(al.cover_thumb_path.as_deref())),
        }
    }
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct TrackInfo {
    /// Stable identity (absolute file path, or `/saf/<document id>` for SAF documents).
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_secs: f64,
    pub file_path: String,
    /// `file://` URL or `content://` URI to hand to ExoPlayer.
    pub play_uri: String,
    pub track_number: i32,
    pub disc_number: i32,
    pub codec: String,
    pub quality_label: String,
    pub sample_rate: i32,
    pub bits_per_sample: i32,
    pub channels: i32,
    pub thumb_path: Option<String>,
    pub full_path: Option<String>,
    /// ReplayGain tags in dB / linear peak; feed them to [`crate::replay_gain_factor`].
    pub replay_gain_track_db: Option<f32>,
    pub replay_gain_track_peak: Option<f32>,
    pub replay_gain_album_db: Option<f32>,
    pub replay_gain_album_peak: Option<f32>,
}

impl TrackInfo {
    pub(crate) fn replay_gain(&self) -> ReplayGain {
        ReplayGain {
            track_gain_db: self.replay_gain_track_db,
            track_peak: self.replay_gain_track_peak,
            album_gain_db: self.replay_gain_album_db,
            album_peak: self.replay_gain_album_peak,
        }
    }
}

impl From<&Track> for TrackInfo {
    fn from(t: &Track) -> Self {
        let codec = t.short_codec();
        let quality_label = t.quality_label_with_codec(&codec);
        Self {
            id: t.id.clone(),
            title: t.title.clone(),
            artist: t.artist.clone(),
            album: t.album.clone(),
            duration_secs: t.duration,
            file_path: t.file_path.clone(),
            play_uri: t.play_uri.clone(),
            track_number: t.track_number,
            disc_number: t.disc_number,
            codec,
            quality_label,
            sample_rate: t.sample_rate,
            bits_per_sample: t.bits_per_sample,
            channels: t.channels,
            thumb_path: path_str(t.cover_thumb_path.as_deref()),
            full_path: path_str(t.cover_full_path.as_deref()),
            replay_gain_track_db: t.replay_gain.track_gain_db,
            replay_gain_track_peak: t.replay_gain.track_peak,
            replay_gain_album_db: t.replay_gain.album_gain_db,
            replay_gain_album_peak: t.replay_gain.album_peak,
        }
    }
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct LibrarySnapshot {
    pub albums: Vec<AlbumSummary>,
    pub artists: Vec<ArtistInfo>,
    pub track_count: u32,
    /// Canonical sort key actually applied (one of the 10 `LIBRARY_SORT_KEYS`).
    pub sort: String,
    pub restored_from_backup: bool,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct AlbumDetail {
    pub album: AlbumSummary,
    pub tracks: Vec<TrackInfo>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct ArtistDetail {
    pub artist: ArtistInfo,
    pub albums: Vec<AlbumSummary>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct SearchResults {
    pub albums: Vec<AlbumSummary>,
    pub artists: Vec<ArtistInfo>,
    pub tracks: Vec<TrackInfo>,
    pub playlists: Vec<PlaylistInfo>,
    pub smart_playlists: Vec<SmartPlaylistInfo>,
}

#[derive(Debug, Clone, Default, uniffi::Record)]
pub struct CoverDedupSummary {
    pub migrated_albums: u32,
    pub migrated_playlists: u32,
    pub removed_files: u32,
}

// ---- scanning -------------------------------------------------------------------------------

/// A document found by walking a SAF tree in Kotlin.
#[derive(Debug, Clone, uniffi::Record)]
pub struct SafDocument {
    /// `DocumentsContract.COLUMN_DOCUMENT_ID`, e.g. `primary:Music/Album/01.flac`.
    pub document_id: String,
    /// `content://` URI used to open and play it.
    pub uri: String,
    /// `COLUMN_LAST_MODIFIED`, milliseconds.
    pub mtime_ms: i64,
    pub size: i64,
}

#[derive(Debug, Clone, Default, uniffi::Record)]
pub struct ScanRequest {
    /// Directories walked with `read_dir`.
    pub dirs: Vec<String>,
    /// Explicit files (e.g. the `MediaStore` `DATA` paths).
    pub files: Vec<String>,
    /// Roots whose tracks count as missing when absent from `files`/`dirs` (e.g. `/storage/emulated/0`).
    pub roots: Vec<String>,
    pub saf_files: Vec<SafDocument>,
    /// Document ids of the tree roots the `saf_files` came from.
    pub saf_roots: Vec<String>,
    /// Drop tracks whose files are gone. Off by default: missing tracks are only reported.
    pub prune_missing: bool,
    /// Re-read tags even when mtime/size are unchanged.
    pub force: bool,
    /// Clear existing covers / force artwork resync across scanned files.
    pub resync_covers: bool,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct ScanProgressEvent {
    /// `discovering`, `scanning` or `covers`.
    pub phase: String,
    pub scanned: u32,
    pub total: u32,
    pub current_file: String,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct ScanSummary {
    pub added_tracks: u32,
    pub new_albums: u32,
    pub covers: u32,
    pub updated_tracks: u32,
    pub removed_tracks: u32,
    pub missing_tracks: u32,
    pub merged_albums: u32,
    pub cancelled: bool,
}

// ---- collections / stats / settings ---------------------------------------------------------

#[derive(Debug, Clone, uniffi::Record)]
pub struct PlaylistInfo {
    pub id: i64,
    pub name: String,
    pub cover_path: Option<String>,
    pub track_ids: Vec<String>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct SmartRuleInfo {
    pub field: String,
    pub op: String,
    pub value: String,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct SmartPlaylistInfo {
    pub id: i64,
    pub name: String,
    pub match_all: bool,
    pub rules: Vec<SmartRuleInfo>,
    pub sort: String,
    pub limit: u32,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct CollectionsSnapshot {
    /// Track ids marked as favorite.
    pub liked: Vec<String>,
    pub playlists: Vec<PlaylistInfo>,
    pub smart: Vec<SmartPlaylistInfo>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct PlayStatInfo {
    pub track_id: String,
    pub count: i32,
    pub last_played_ms: i64,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct StatsSnapshot {
    pub tracks: Vec<PlayStatInfo>,
}

/// Settings the Android app reads and writes. Unknown keys in `native-settings.json` are preserved.
#[derive(Debug, Clone, uniffi::Record)]
pub struct SettingsSnapshot {
    pub volume: f64,
    pub library_sort: String,
    pub library_view: String,
    pub replay_gain: String,
    pub accent_from_cover: bool,
    pub crossfade_seconds: i32,
    pub eq_enabled: bool,
    pub eq_preset: String,
    pub eq_gains: Vec<f64>,
    /// Real filesystem folders (desktop-compatible `LibraryFolders`).
    pub library_folders: Vec<String>,
    /// Persisted SAF tree URIs the user granted (`AndroidSafTrees`).
    pub saf_trees: Vec<String>,
    /// Whether the shared-storage `MediaStore` library is included (`AndroidUseMediaStore`).
    pub use_media_store: bool,
    /// Userspace UAC1/UAC2 Direct USB DAC bit-perfect mode (`AndroidUsbBitPerfect`).
    /// Bypasses EQ and OS mixing; applies ReplayGain through the DAC's hardware Feature Unit when supported.
    pub usb_bit_perfect: bool,
    /// Full library scan on app launch (`ScanOnStartup`).
    pub scan_on_startup: bool,
    /// Automatically scan EBU R128 ReplayGain for newly added/untagged tracks (`AutoReplayGain`).
    pub auto_replay_gain: bool,
    /// Enable Apple iTunes artwork lookup (`ArtworkItunes`).
    pub artwork_itunes: bool,
    /// Enable Deezer artwork lookup (`ArtworkDeezer`).
    pub artwork_deezer: bool,
    /// Enable MusicBrainz / Cover Art Archive lookup (`ArtworkMusicbrainz`).
    pub artwork_musicbrainz: bool,
}
