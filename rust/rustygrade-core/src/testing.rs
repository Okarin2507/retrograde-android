//! Test helpers: tiny tagged FLAC files, a real PNG cover, and a sandboxed app dir.
//!
//! `CoverPaths::app_dir` is process-global, so every test that touches the library must hold a
//! [`TestEnv`] (it serialises tests through one mutex and points the app dir at a fresh temp dir).

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use crate::paths::CoverPaths;
use crate::scanner::FileOpener;
use crate::tag_writer::{TagEdit, TagWriter};

static ENV_LOCK: Mutex<()> = Mutex::new(());

pub struct TestEnv {
    _guard: MutexGuard<'static, ()>,
    pub root: PathBuf,
    pub app_dir: PathBuf,
    pub music: PathBuf,
}

impl TestEnv {
    pub fn new(name: &str) -> Self {
        let guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let root = std::env::temp_dir().join(format!("rustygrade-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let app_dir = root.join("app");
        let music = root.join("music");
        fs::create_dir_all(&app_dir).expect("mkdir app");
        fs::create_dir_all(&music).expect("mkdir music");
        let root = fs::canonicalize(&root).expect("canonicalize");
        let app_dir = root.join("app");
        let music = root.join("music");
        CoverPaths::set_app_dir(app_dir.clone());
        Self { _guard: guard, root, app_dir, music }
    }
}

impl Drop for TestEnv {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

pub fn minimal_flac_bytes() -> Vec<u8> {
    minimal_flac_bytes_secs(1)
}

/// The same tiny FLAC, but its STREAMINFO claims `secs` seconds (44.1 kHz): only the header matters
/// to the tag reader, so this is how tests get long tracks without audio data.
pub fn minimal_flac_bytes_secs(secs: u32) -> Vec<u8> {
    let samples = 44_100u64 * u64::from(secs.max(1));
    let mut v = Vec::with_capacity(42);
    v.extend_from_slice(b"fLaC");
    v.extend_from_slice(&[0x80, 0x00, 0x00, 0x22]); // last block = STREAMINFO (34 bytes)
    v.extend_from_slice(&[0x10, 0x00, 0x10, 0x00]); // min/max block 4096
    v.extend_from_slice(&[0x00; 6]); // min/max frame size unknown
    // 44100 Hz, 2 ch, 16 bps, then the 36-bit total sample count.
    v.extend_from_slice(&[0x0A, 0xC4, 0x42, 0xF0 | ((samples >> 32) & 0xF) as u8]);
    v.extend_from_slice(&(samples as u32).to_be_bytes());
    v.extend_from_slice(&[0x00; 16]); // MD5
    v
}

/// A small but real PNG, so the cover passes `ImageLimits::is_sane_bytes`.
pub fn write_png(path: &Path, rgb: [u8; 3]) {
    let img = image::RgbImage::from_pixel(32, 32, image::Rgb(rgb));
    img.save(path).expect("write png");
}

#[derive(Clone, Debug)]
pub struct Fixture<'a> {
    pub title: &'a str,
    pub artist: &'a str,
    pub album: &'a str,
    pub album_artist: Option<&'a str>,
    pub track: u32,
    pub year: u32,
    pub cover: bool,
    pub seconds: u32,
}

impl<'a> Fixture<'a> {
    pub fn new(title: &'a str, artist: &'a str, album: &'a str, track: u32) -> Self {
        Self { title, artist, album, album_artist: None, track, year: 2021, cover: false, seconds: 1 }
    }

    pub fn with_cover(mut self) -> Self {
        self.cover = true;
        self
    }

    pub fn with_seconds(mut self, seconds: u32) -> Self {
        self.seconds = seconds;
        self
    }
}

/// Writes a tagged FLAC at `path` (parent dirs are created).
pub fn write_flac(path: &Path, fx: &Fixture<'_>) {
    fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    fs::write(path, minimal_flac_bytes_secs(fx.seconds)).expect("write flac");
    let cover_png = path.with_extension("cover.png");
    if fx.cover {
        write_png(&cover_png, [200, 80, 40]);
    }
    let edit = TagEdit {
        title: Some(fx.title.to_owned()),
        artist: Some(fx.artist.to_owned()),
        album: Some(fx.album.to_owned()),
        album_artist: fx.album_artist.map(str::to_owned),
        year: Some(fx.year),
        track: Some(fx.track),
        cover_image: fx.cover.then(|| cover_png.clone()),
        ..TagEdit::default()
    };
    TagWriter::write(path, &edit).expect("tag flac");
    let _ = fs::remove_file(cover_png);
}

/// Fake SAF opener: `test://<name>` opens `<dir>/<name>`.
pub struct DirOpener(pub PathBuf);

impl FileOpener for DirOpener {
    fn open(&self, uri: &str) -> Option<File> {
        let name = uri.strip_prefix("test://")?;
        File::open(self.0.join(name)).ok()
    }
}

/// Adds ReplayGain tags to an already tagged file (values as written by common taggers).
pub fn set_replaygain(path: &Path, track: (&str, &str), album: Option<(&str, &str)>) {
    use lofty::config::WriteOptions;
    use lofty::file::TaggedFileExt;
    use lofty::probe::Probe;
    use lofty::tag::{ItemKey, TagExt};

    let mut tagged = Probe::open(path).expect("open").read().expect("read");
    let tag = tagged.primary_tag_mut().expect("tag");
    tag.insert_text(ItemKey::ReplayGainTrackGain, track.0.to_owned());
    tag.insert_text(ItemKey::ReplayGainTrackPeak, track.1.to_owned());
    if let Some((gain, peak)) = album {
        tag.insert_text(ItemKey::ReplayGainAlbumGain, gain.to_owned());
        tag.insert_text(ItemKey::ReplayGainAlbumPeak, peak.to_owned());
    }
    tag.save_to_path(path, WriteOptions::default()).expect("save");
}

/// Writes embedded lyrics (ID3/Vorbis `LYRICS`) into an already tagged file.
pub fn set_lyrics(path: &Path, text: &str) {
    use lofty::config::WriteOptions;
    use lofty::file::TaggedFileExt;
    use lofty::probe::Probe;
    use lofty::tag::{ItemKey, TagExt};

    let mut tagged = Probe::open(path).expect("open").read().expect("read");
    let tag = tagged.primary_tag_mut().expect("tag");
    tag.insert_text(ItemKey::Lyrics, text.to_owned());
    tag.save_to_path(path, WriteOptions::default()).expect("save");
}
