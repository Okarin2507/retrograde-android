use std::io::BufReader;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;

use lofty::config::ParseOptions;
use lofty::file::TaggedFile;
use lofty::file::{AudioFile, FileType, TaggedFileExt};
use lofty::probe::Probe;
use lofty::tag::{Accessor, ItemKey};

use super::{Parsed, ScanCtx};
use crate::error_log;
use crate::library::{REPLAY_GAIN_KEY, SCAN_SCHEMA, SCAN_SCHEMA_KEY};
use crate::models::ReplayGain;

/// Opens `path` (real file or SAF document) and reads its tags. Mirrors `Probe::open`: the type is
/// taken from the extension when known, otherwise guessed from the content.
pub(super) fn read_tagged(path: &Path, ctx: &ScanCtx<'_>, read_cover: bool) -> Result<TaggedFile, ParseFailure> {
    let file = ctx.open(path).ok_or(ParseFailure::Unavailable)?;
    let mut probe = Probe::new(BufReader::new(file))
        .options(ParseOptions::new().read_cover_art(read_cover));
    probe = match FileType::from_path(path) {
        Some(ft) => probe.set_file_type(ft),
        None => probe.guess_file_type().map_err(|_| ParseFailure::Unparseable)?,
    };
    probe.read().map_err(|_| ParseFailure::Unparseable)
}

/// Why a file produced no track.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ParseFailure {
    /// It could not be opened: possibly temporary, so it is retried on the next scan.
    Unavailable,
    /// It opened but is not readable audio (corrupt, wrong format, or the parser panicked).
    Unparseable,
}

/// `parse_file` that survives a panic in the tag parser: the file is skipped and logged.
pub(super) fn parse_guarded(path: &Path, ctx: &ScanCtx<'_>) -> Result<Parsed, ParseFailure> {
    match catch_unwind(AssertUnwindSafe(|| parse_file(path, ctx))) {
        Ok(parsed) => parsed,
        Err(_) => {
            error_log::write("scanner", &format!("panic while parsing {}", path.display()));
            Err(ParseFailure::Unparseable)
        }
    }
}

fn parse_file(path: &Path, ctx: &ScanCtx<'_>) -> Result<Parsed, ParseFailure> {
    let tagged = read_tagged(path, ctx, false)?;
    let props = tagged.properties();
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag());

    let file_path = path.to_string_lossy().into_owned();
    let album_artist = tag
        .and_then(|t| t.get_string(ItemKey::AlbumArtist))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let artist = tag
        .and_then(|t| t.artist())
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .or_else(|| album_artist.clone())
        .unwrap_or_else(|| "Unknown Artist".to_owned());
    let title = tag
        .and_then(|t| t.title())
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            path.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Unknown")
                .to_owned()
        });
    let album = tag
        .and_then(|t| t.album())
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Unknown Album".to_owned());

    let track_number = tag.and_then(|t| t.track()).unwrap_or(0) as i32;
    let disc_number = tag.and_then(|t| t.disk()).filter(|&d| d > 0).unwrap_or(1) as i32;
    let year = tag
        .and_then(|t| t.date().map(|ts| ts.year as i32))
        .unwrap_or(0);

    // Quick cover presence check without reading large blobs for FLAC/MP3/MP4 where supported;
    // Note: with read_cover_art(false), tag.pictures() may be empty, so cover lookup inspects the
    // first tracks of albums lacking cover art in stage 6.
    let has_picture = tag.is_some_and(|t| !t.pictures().is_empty());
    let replay_gain = tag.map(read_replay_gain).unwrap_or_default();

    Ok(Parsed {
        file_path,
        title,
        artist,
        album_artist,
        album,
        duration: props.duration().as_secs_f64(),
        track_number,
        disc_number,
        year,
        codec: codec_name(tagged.file_type(), path, props.bit_depth()),
        sample_rate: props.sample_rate().unwrap_or(0) as i32,
        bits_per_sample: props.bit_depth().unwrap_or(0) as i32,
        channels: props.channels().unwrap_or(0) as i32,
        has_picture,
        replay_gain,
    })
}

/// The `extra` of a freshly scanned track: the schema marker and the ReplayGain tags.
pub(super) fn track_extra(rg: &ReplayGain) -> serde_json::Map<String, serde_json::Value> {
    use serde_json::{Map, Value, json};
    let mut extra = Map::new();
    extra.insert(SCAN_SCHEMA_KEY.to_owned(), Value::from(SCAN_SCHEMA));
    if !rg.is_empty() {
        let mut o = Map::new();
        for (key, v) in [
            ("trackGain", rg.track_gain_db),
            ("trackPeak", rg.track_peak),
            ("albumGain", rg.album_gain_db),
            ("albumPeak", rg.album_peak),
        ] {
            if let Some(v) = v {
                o.insert(key.to_owned(), json!(v));
            }
        }
        extra.insert(REPLAY_GAIN_KEY.to_owned(), Value::Object(o));
    }
    extra
}

/// `"-6.54 dB"`, `"+3 dB"`, `"0.988"` -> the leading number.
pub(super) fn parse_tag_number(raw: &str) -> Option<f32> {
    // The unit is optional and may touch the number ("-6.54dB").
    let number = raw.trim().trim_end_matches(|c: char| c.is_alphabetic()).trim();
    number.parse::<f32>().ok().filter(|v| v.is_finite())
}

/// Opus stores `R128_*_GAIN` as Q7.8 dB relative to -23 LUFS; ReplayGain's reference is -18 LUFS.
pub(super) fn parse_r128_gain(raw: &str) -> Option<f32> {
    // RFC 7845: a signed 16-bit integer.
    let q: i16 = raw.trim().parse().ok()?;
    Some(f32::from(q) / 256.0 + 5.0)
}

fn read_replay_gain(tag: &lofty::tag::Tag) -> ReplayGain {
    let text = |key: ItemKey| tag.get_string(key);
    ReplayGain {
        track_gain_db: text(ItemKey::ReplayGainTrackGain)
            .and_then(parse_tag_number)
            .or_else(|| text(ItemKey::R128TrackGain).and_then(parse_r128_gain)),
        track_peak: text(ItemKey::ReplayGainTrackPeak).and_then(parse_tag_number),
        album_gain_db: text(ItemKey::ReplayGainAlbumGain)
            .and_then(parse_tag_number)
            .or_else(|| text(ItemKey::R128AlbumGain).and_then(parse_r128_gain)),
        album_peak: text(ItemKey::ReplayGainAlbumPeak).and_then(parse_tag_number),
    }
}

/// `bit_depth` is what lofty reports for the stream: in an MP4 container only the lossless codecs
/// (ALAC, FLAC) have one, so it tells an ALAC `.m4a` from an AAC one.
pub(super) fn codec_name(ft: FileType, path: &Path, bit_depth: Option<u8>) -> String {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ft {
        FileType::Flac => "FLAC".to_owned(),
        FileType::Mpeg => "MPEG 1 Layer 3".to_owned(),
        FileType::Opus => "Opus".to_owned(),
        FileType::Vorbis => "Vorbis I".to_owned(),
        FileType::Wav | FileType::Aiff => "PCM".to_owned(),
        FileType::Aac => "AAC".to_owned(),
        FileType::Mp4 => {
            if ext == "alac" || bit_depth.is_some() {
                "ALAC".to_owned()
            } else {
                "AAC".to_owned()
            }
        }
        _ => match ext.as_str() {
            "wma" => "Windows Media Audio".to_owned(),
            "" => "AUDIO".to_owned(),
            other => other.to_ascii_uppercase(),
        },
    }
}
