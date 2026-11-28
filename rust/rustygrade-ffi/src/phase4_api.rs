//! Lyrics, tag editing, backdrops and the crossfade setting for the full-screen player.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rustygrade_core::{
    LibraryStore, LyricsLoader, ReplayGain, TagEdit, TagWriter, blurred_backdrop, is_saf_path,
    r128::{R128Analyzer, R128TrackResult, compute_album_replay_gain_iter},
};

use crate::engine::{FdOpener, RustygradeEngine, UriOpener};
use crate::error::EngineError;

#[derive(Debug, Clone, uniffi::Record)]
pub struct LyricLineInfo {
    /// Seconds, or a negative number for unsynced lines.
    pub time_s: f64,
    pub text: String,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct LyricsInfo {
    pub lines: Vec<LyricLineInfo>,
    pub synced: bool,
    /// `LRC` (sidecar file) or `TAG` (embedded).
    pub source: String,
}

/// Fields left `None` are not touched. `0` for year/track/disc removes the value.
#[derive(Debug, Clone, Default, uniffi::Record)]
pub struct TagEditInfo {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub year: Option<u32>,
    pub track: Option<u32>,
    pub disc: Option<u32>,
    /// A jpg/png/webp file to embed as the front cover.
    pub cover_image_path: Option<String>,
    pub remove_cover: bool,
    /// Optional lyrics (synced LRC or plain text) to embed into the audio file's lyrics tag.
    pub lyrics: Option<String>,
}

/// Writes tags into `path`. Android never lets Rust open the music file for writing, so Kotlin
/// edits a temp copy with this and writes the result back through the system.
#[uniffi::export]
pub fn edit_tags_in_file(path: String, edit: TagEditInfo) -> Result<(), EngineError> {
    let edit = TagEdit {
        title: edit.title,
        artist: edit.artist,
        album: edit.album,
        album_artist: edit.album_artist,
        year: edit.year,
        track: edit.track,
        disc: edit.disc,
        cover_image: edit.cover_image_path.map(PathBuf::from),
        remove_cover: edit.remove_cover,
        lyrics: edit.lyrics,
        replay_gain: None,
    };
    TagWriter::write(Path::new(&path), &edit).map_err(|e| EngineError::Library(e.to_string()))
}

/// A blurred, tiny copy of a cover for the player backdrop on Android < 12 (cached in the app's cover cache).
#[uniffi::export]
pub fn blurred_cover(path: String) -> Option<String> {
    blurred_backdrop(Path::new(&path)).map(|p| p.to_string_lossy().into_owned())
}

#[uniffi::export]
impl RustygradeEngine {
    /// Lyrics of a track: cached online-fetched lyrics first, else sidecar `.lrc` next to a real
    /// file, else embedded tag. SAF documents read embedded lyrics through [`UriOpener`].
    pub fn lyrics(&self, track_id: String, opener: Option<Arc<dyn UriOpener>>) -> Result<Option<LyricsInfo>, EngineError> {
        let data = self.library()?;
        let Some(track) = data.lookup().get(&track_id) else {
            return Ok(None);
        };
        let doc = LyricsLoader::load_cached(&track.id).or_else(|| {
            if is_saf_path(&track.file_path) {
                let opener = opener?;
                let file = rustygrade_core::FileOpener::open(&FdOpener::new(opener), &track.play_uri)?;
                LyricsLoader::load_embedded(std::io::BufReader::new(file), Path::new(&track.file_path))
            } else {
                LyricsLoader::load(Path::new(&track.file_path))
            }
        });
        Ok(doc.map(|d| LyricsInfo {
            synced: d.is_synced,
            source: d.source.to_owned(),
            lines: d.lines.into_iter().map(|l| LyricLineInfo { time_s: l.time, text: l.text }).collect(),
        }))
    }

    /// Saves online-fetched lyrics into the persistent app-private lyrics cache for `track_id`
    /// and returns the parsed [`LyricsInfo`] immediately.
    pub fn save_track_lyrics(&self, track_id: String, text: String) -> Result<Option<LyricsInfo>, EngineError> {
        let doc = LyricsLoader::save_cached(&track_id, &text)
            .map_err(|e| EngineError::Library(e.to_string()))?;
        Ok(doc.map(|d| LyricsInfo {
            synced: d.is_synced,
            source: d.source.to_owned(),
            lines: d.lines.into_iter().map(|l| LyricLineInfo { time_s: l.time, text: l.text }).collect(),
        }))
    }

    /// Removes cached online-fetched lyrics for `track_id` so file/tag lyrics take over again.
    pub fn clear_cached_lyrics(&self, track_id: String) {
        LyricsLoader::clear_cached(&track_id);
    }

    /// Reads raw lyrics text (cached -> sidecar `.lrc` -> embedded tag) for the Tag Editor.
    pub fn raw_lyrics(&self, track_id: String, opener: Option<Arc<dyn UriOpener>>) -> Result<Option<String>, EngineError> {
        let data = self.library()?;
        let Some(track) = data.lookup().get(&track_id) else {
            return Ok(None);
        };
        let text = LyricsLoader::load_raw_cached(&track.id).or_else(|| {
            if is_saf_path(&track.file_path) {
                let opener = opener?;
                let file = rustygrade_core::FileOpener::open(&FdOpener::new(opener), &track.play_uri)?;
                LyricsLoader::load_raw_embedded(std::io::BufReader::new(file), Path::new(&track.file_path))
            } else {
                LyricsLoader::load_raw(Path::new(&track.file_path))
            }
        });
        Ok(text)
    }

    /// Applies an online-fetched cover image file to `album_key` directly in the app's cover store
    /// and `library.json`, returning the saved cover path.
    pub fn apply_album_cover(&self, album_key: String, image_path: String) -> Result<Option<String>, EngineError> {
        // Refuse oversized files before reading them fully into memory.
        let len = std::fs::metadata(&image_path).map_err(|e| EngineError::Library(e.to_string()))?.len();
        if len > rustygrade_core::covers::MAX_COVER_FILE_BYTES {
            return Err(EngineError::Library(rustygrade_core::covers::CoverError::InsaneImage.to_string()));
        }
        let bytes = std::fs::read(&image_path).map_err(|e| EngineError::Library(e.to_string()))?;
        let saved = LibraryStore::set_album_cover(&album_key, &bytes)?;
        self.invalidate_cache();
        Ok(saved.map(|p| p.to_string_lossy().into_owned()))
    }

    /// Persists scanned EBU R128 ReplayGain values directly into `library.json`.
    pub fn apply_replay_gain_results(&self, entries: Vec<ReplayGainUpdateDto>) -> Result<u32, EngineError> {
        let pairs: Vec<(String, ReplayGain)> = entries
            .into_iter()
            .map(|e| {
                (
                    e.track_id,
                    ReplayGain {
                        track_gain_db: e.track_gain_db,
                        track_peak: e.track_peak,
                        album_gain_db: e.album_gain_db,
                        album_peak: e.album_peak,
                    },
                )
            })
            .collect();
        let n = LibraryStore::set_tracks_replay_gain(&pairs)?;
        if n > 0 {
            self.invalidate_cache();
        }
        Ok(n as u32)
    }

    /// Forgets the cached covers of these albums; the next scan of any of their files extracts them
    /// again (used after a cover was changed in the tags).
    pub fn clear_album_covers(&self, keys: Vec<String>) -> Result<(), EngineError> {
        LibraryStore::clear_covers(&keys)?;
        self.invalidate_cache();
        Ok(())
    }

    /// Crossfade length in seconds (0 = off, clamped to 0..=12).
    pub fn set_crossfade_seconds(&self, seconds: i32) {
        self.update_settings_field(|s| s.crossfade_seconds = seconds.clamp(0, 12));
    }
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct ReplayGainUpdateDto {
    pub track_id: String,
    pub track_gain_db: Option<f32>,
    pub track_peak: Option<f32>,
    pub album_gain_db: Option<f32>,
    pub album_peak: Option<f32>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct R128TrackSummaryDto {
    pub track_id: String,
    pub gain_db: f32,
    pub peak: f32,
    pub integrated_lufs: f32,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct R128AlbumSummaryDto {
    pub album_gain_db: f32,
    pub album_peak: f32,
    pub tracks: Vec<R128TrackSummaryDto>,
}

struct R128BatchState {
    active_track_id: Option<String>,
    active: Option<R128Analyzer>,
    finished: Vec<(String, R128TrackResult)>,
}

/// Streaming EBU R128 / ITU-R BS.1770-4 loudness analyzer for one album (or loose track).
/// Kotlin decodes PCM with Android's `MediaExtractor` + `MediaCodec` and streams chunks here.
#[derive(uniffi::Object)]
pub struct R128BatchScanner {
    inner: Mutex<R128BatchState>,
}

#[uniffi::export]
impl R128BatchScanner {
    #[uniffi::constructor]
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(R128BatchState {
                active_track_id: None,
                active: None,
                finished: Vec::new(),
            }),
        }
    }

    pub fn begin_track(&self, track_id: String, sample_rate: u32, channels: u8) {
        let mut g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        g.active_track_id = Some(track_id);
        g.active = Some(R128Analyzer::new(sample_rate, channels));
    }

    pub fn feed_s16le(&self, pcm: Vec<u8>) {
        let mut g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(a) = g.active.as_mut() {
            a.feed_s16le(&pcm);
        }
    }

    /// Feeds interleaved signed 24-bit little-endian packed PCM (3 bytes/sample).
    pub fn feed_s24le(&self, pcm: Vec<u8>) {
        let mut g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(a) = g.active.as_mut() {
            a.feed_s24le(&pcm);
        }
    }

    /// Feeds interleaved signed 32-bit little-endian PCM.
    pub fn feed_s32le(&self, pcm: Vec<u8>) {
        let mut g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(a) = g.active.as_mut() {
            a.feed_s32le(&pcm);
        }
    }

    pub fn feed_f32le(&self, pcm: Vec<u8>) {
        let mut g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(a) = g.active.as_mut() {
            a.feed_f32le(&pcm);
        }
    }

    pub fn finish_track(&self) -> Option<R128TrackSummaryDto> {
        let mut g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let id = g.active_track_id.take()?;
        let analyzer = g.active.take()?;
        let res = analyzer.finish();
        let summary = R128TrackSummaryDto {
            track_id: id.clone(),
            gain_db: res.gain_db,
            peak: res.peak,
            integrated_lufs: res.integrated_lufs,
        };
        g.finished.push((id, res));
        Some(summary)
    }

    pub fn finish_album(&self) -> R128AlbumSummaryDto {
        let mut g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let finished = std::mem::take(&mut g.finished);
        let (album_gain_db, album_peak) = compute_album_replay_gain_iter(finished.iter().map(|(_, r)| r));
        let tracks = finished
            .into_iter()
            .map(|(track_id, r)| R128TrackSummaryDto {
                track_id,
                gain_db: r.gain_db,
                peak: r.peak,
                integrated_lufs: r.integrated_lufs,
            })
            .collect();
        R128AlbumSummaryDto {
            album_gain_db,
            album_peak,
            tracks,
        }
    }
}

impl Default for R128BatchScanner {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::fd::IntoRawFd;

    use rustygrade_core::testing::{Fixture, TestEnv, set_lyrics, write_flac, write_png};
    use rustygrade_core::CoverPaths;

    use super::*;
    use crate::dto::{SafDocument, ScanRequest};

    fn scan_dir(lib: &RustygradeEngine, env: &TestEnv) {
        let req = ScanRequest { dirs: vec![env.music.to_string_lossy().into_owned()], ..ScanRequest::default() };
        lib.scan_sources(req, None, None).unwrap();
    }

    /// The Android write path: edit a temp copy, write it back over the original, rescan just that file.
    fn edit_like_android(lib: &RustygradeEngine, env: &TestEnv, file: &Path, edit: TagEditInfo, stale_albums: Vec<String>) {
        let temp = env.root.join("temp-copy.flac");
        fs::copy(file, &temp).unwrap();
        edit_tags_in_file(temp.to_string_lossy().into_owned(), edit).unwrap();
        fs::copy(&temp, file).unwrap();
        fs::remove_file(&temp).unwrap();
        lib.clear_album_covers(stale_albums).unwrap();
        let req = ScanRequest { files: vec![file.to_string_lossy().into_owned()], force: true, ..ScanRequest::default() };
        lib.scan_sources(req, None, None).unwrap();
    }

    #[test]
    fn a_tag_edit_moves_the_track_to_its_new_album_and_replaces_the_cover() {
        let env = TestEnv::new("p4-tags");
        let one = env.music.join("A/01.flac");
        write_flac(&one, &Fixture::new("One", "Artist", "Old Album", 1).with_cover());
        write_flac(&env.music.join("A/02.flac"), &Fixture::new("Two", "Artist", "Old Album", 2));
        let lib = RustygradeEngine::new(env.app_dir.to_string_lossy().into_owned());
        scan_dir(&lib, &env);
        let old_thumb_path = lib
            .get_album_detail("Old Album".into())
            .unwrap()
            .unwrap()
            .album
            .thumb_path
            .unwrap();
        let old_thumb = fs::read(&old_thumb_path).unwrap();

        let new_cover = env.root.join("new.png");
        write_png(&new_cover, [10, 200, 30]);
        edit_like_android(
            &lib,
            &env,
            &one,
            TagEditInfo {
                title: Some("One (edited)".into()),
                album: Some("New Album".into()),
                year: Some(1999),
                cover_image_path: Some(new_cover.to_string_lossy().into_owned()),
                ..TagEditInfo::default()
            },
            vec!["Old Album".into()],
        );

        let new = lib.get_album_detail("New Album".into()).unwrap().expect("the track moved");
        assert_eq!((new.tracks.len(), new.tracks[0].title.as_str(), new.album.year), (1, "One (edited)", 1999));
        let old = lib.get_album_detail("Old Album".into()).unwrap().expect("the other track stays");
        assert_eq!(old.tracks.len(), 1);
        let new_thumb_path = new.album.thumb_path.as_deref().expect("the new cover was extracted");
        let new_thumb = fs::read(new_thumb_path).expect("the new cover file exists");
        assert_ne!(new_thumb, old_thumb, "it is the replaced picture, not the old one");

        // Editing only text keeps working, and a missing file is an error rather than a crash.
        edit_like_android(&lib, &env, &one, TagEditInfo { artist: Some("Someone Else".into()), ..TagEditInfo::default() }, vec![]);
        assert_eq!(lib.get_album_detail("New Album".into()).unwrap().unwrap().tracks[0].artist, "Someone Else");
        assert!(edit_tags_in_file("/nope/missing.flac".into(), TagEditInfo::default()).is_err());
    }

    #[test]
    fn removing_the_cover_clears_it_from_the_cache() {
        let env = TestEnv::new("p4-tags-nocover");
        let one = env.music.join("A/01.flac");
        write_flac(&one, &Fixture::new("One", "Artist", "Alb", 1).with_cover());
        let lib = RustygradeEngine::new(env.app_dir.to_string_lossy().into_owned());
        scan_dir(&lib, &env);
        let alb_thumb = lib
            .get_album_detail("Alb".into())
            .unwrap()
            .unwrap()
            .album
            .thumb_path
            .expect("Alb cover extracted");
        assert!(Path::new(&alb_thumb).exists());
        let blur = blurred_cover(alb_thumb.clone()).expect("blur");
        assert!(Path::new(&blur).exists());
        edit_like_android(&lib, &env, &one, TagEditInfo { remove_cover: true, ..TagEditInfo::default() }, vec!["Alb".into()]);
        assert!(!Path::new(&alb_thumb).exists(), "no embedded art left to extract");
        assert!(!Path::new(&blur).exists(), "the blurred backdrop goes with the cover");
        assert!(lib.get_album_detail("Alb".into()).unwrap().unwrap().album.thumb_path.is_none());
    }

    #[test]
    fn lyrics_come_from_a_sidecar_the_tag_or_a_saf_document() {
        struct Opener(std::path::PathBuf);
        impl UriOpener for Opener {
            fn open_fd(&self, uri: String) -> Option<i32> {
                fs::File::open(self.0.join(uri.strip_prefix("test://")?)).ok().map(|f| f.into_raw_fd())
            }
        }
        let env = TestEnv::new("p4-lyrics");
        let plain = env.music.join("A/01.flac");
        let tagged = env.music.join("A/02.flac");
        let none = env.music.join("A/03.flac");
        write_flac(&plain, &Fixture::new("Sidecar", "X", "Alb", 1));
        write_flac(&tagged, &Fixture::new("Tagged", "X", "Alb", 2));
        write_flac(&none, &Fixture::new("Silent", "X", "Alb", 3));
        fs::write(plain.with_extension("lrc"), "[00:01.00]hello\n[00:03.50]world").unwrap();
        set_lyrics(&tagged, "[00:02.00]in the tag");
        let lib = RustygradeEngine::new(env.app_dir.to_string_lossy().into_owned());
        scan_dir(&lib, &env);

        let id_of = |t: &str| lib.search_library(t.into()).unwrap().tracks.into_iter().find(|x| x.title == t).unwrap().id;
        let l = lib.lyrics(id_of("Sidecar"), None).unwrap().unwrap();
        assert_eq!((l.source.as_str(), l.synced, l.lines.len()), ("LRC", true, 2));
        assert_eq!(l.lines[1].time_s, 3.5);
        assert_eq!(lib.lyrics(id_of("Tagged"), None).unwrap().unwrap().source, "TAG");
        assert!(lib.lyrics(id_of("Silent"), None).unwrap().is_none());
        assert!(lib.lyrics("unknown id".into(), None).unwrap().is_none());

        // A SAF document: the embedded lyrics are read through the opener; without one there are none.
        let req = ScanRequest {
            saf_files: vec![SafDocument {
                document_id: "primary:Music/Saf/01.flac".into(),
                uri: "test://A/02.flac".into(),
                mtime_ms: 1,
                size: 1,
            }],
            saf_roots: vec!["primary:Music".into()],
            ..ScanRequest::default()
        };
        lib.scan_sources(req, None, Some(Arc::new(Opener(env.music.clone())))).unwrap();
        let saf_id = "/saf/primary:Music/Saf/01.flac".to_string();
        assert!(lib.lyrics(saf_id.clone(), None).unwrap().is_none());
        let l = lib.lyrics(saf_id, Some(Arc::new(Opener(env.music.clone())))).unwrap().unwrap();
        assert_eq!((l.source.as_str(), l.lines[0].text.as_str()), ("TAG", "in the tag"));
    }

    #[test]
    fn crossfade_setting_is_clamped_and_blur_is_cached() {
        let env = TestEnv::new("p4-misc");
        let lib = RustygradeEngine::new(env.app_dir.to_string_lossy().into_owned());
        lib.set_crossfade_seconds(99);
        assert_eq!(lib.load_settings().crossfade_seconds, 12);
        lib.set_crossfade_seconds(-3);
        assert_eq!(lib.load_settings().crossfade_seconds, 0);

        let cover = env.root.join("cover.png");
        write_png(&cover, [200, 30, 30]);
        let first = blurred_cover(cover.to_string_lossy().into_owned()).expect("blurred copy");
        // The blur lives in the app's cover cache, never next to the source image.
        assert!(Path::new(&first).starts_with(CoverPaths::cover_dir()) && Path::new(&first).exists(), "{first}");
        assert!(!env.root.join("cover_blur.jpg").exists());
        let stamp = fs::metadata(&first).unwrap().modified().unwrap();
        assert_eq!(blurred_cover(cover.to_string_lossy().into_owned()).unwrap(), first);
        assert_eq!(fs::metadata(&first).unwrap().modified().unwrap(), stamp, "the second call reuses the file");

        // A replaced cover (newer than the blur made from the old one) gets a fresh blur.
        std::thread::sleep(std::time::Duration::from_millis(30));
        write_png(&cover, [10, 10, 220]);
        let again = blurred_cover(cover.to_string_lossy().into_owned()).unwrap();
        assert!(fs::metadata(&again).unwrap().modified().unwrap() > stamp, "rebuilt after the cover changed");
        assert!(blurred_cover("/nope.png".into()).is_none());

        // A cover already in the cache keeps `<name>_blur.jpg` beside it.
        fs::create_dir_all(CoverPaths::cover_dir()).unwrap();
        let cached = CoverPaths::cover_dir().join("abc_thumb.jpg");
        write_png(&cached, [1, 2, 3]);
        let blur = blurred_cover(cached.to_string_lossy().into_owned()).unwrap();
        assert!(blur.ends_with("abc_thumb_blur.jpg"), "{blur}");
    }
}
