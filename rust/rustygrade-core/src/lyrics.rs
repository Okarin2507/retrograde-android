use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use lofty::config::ParseOptions;
use lofty::file::TaggedFileExt;
use lofty::probe::Probe;
use lofty::tag::ItemKey;
use regex::Regex;

use crate::covers::sha256_hex_prefix;
use crate::paths::CoverPaths;

#[derive(Debug, Clone, PartialEq)]
pub struct LyricLine {
    /// Time in seconds, or `-1.0` for plain/unsynced lines.
    pub time: f64,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct LyricsDoc {
    pub lines: Vec<LyricLine>,
    pub is_synced: bool,
    /// `"LRC"` (sidecar `.lrc`) or `"TAG"` (embedded tag).
    pub source: &'static str,
}

static STAMP_RX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[(\d{1,3}):(\d{1,2}(?:[.:]\d{1,3})?)\]").unwrap());
static META_RX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\[(ar|ti|al|au|by|re|ve|length|id|#|offset)\s*:([^\]]*)\]\s*$").unwrap()
});
static INLINE_WORD_RX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<\d{1,3}:\d{1,2}(?:[.:]\d{1,3})?>").unwrap());

/// A sidecar `.lrc` bigger than this is not lyrics (the biggest real ones are a few KiB).
const MAX_LRC_BYTES: u64 = 2 * 1024 * 1024;
/// `[offset:...]` beyond ten minutes is a typo; also keeps the shifted times sane.
const MAX_OFFSET_MS: f64 = 600_000.0;

pub struct LyricsLoader;

impl LyricsLoader {
    /// App-private cache path for online-fetched lyrics of `track_id`.
    pub fn cache_path(track_id: &str) -> PathBuf {
        let hash = sha256_hex_prefix(track_id.as_bytes());
        CoverPaths::app_dir().join("lyrics_cache").join(format!("{hash}.lrc"))
    }

    /// Loads previously fetched & cached lyrics for `track_id`, if present.
    pub fn load_cached(track_id: &str) -> Option<LyricsDoc> {
        let text = Self::load_raw_cached(track_id)?;
        Self::parse(&text, "CACHE")
    }

    /// Raw text from the app-private lyrics cache (preserves original LRC timestamps).
    pub fn load_raw_cached(track_id: &str) -> Option<String> {
        let path = Self::cache_path(track_id);
        let meta = fs::metadata(&path).ok()?;
        if !meta.is_file() || meta.len() > MAX_LRC_BYTES {
            return None;
        }
        let bytes = fs::read(&path).ok()?;
        let text = decode_text(&bytes);
        if text.trim().is_empty() { None } else { Some(text) }
    }

    /// Saves fetched lyrics text into the app-private lyrics cache (atomic write).
    pub fn save_cached(track_id: &str, text: &str) -> std::io::Result<Option<LyricsDoc>> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            Self::clear_cached(track_id);
            return Ok(None);
        }
        let path = Self::cache_path(track_id);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        crate::fsutil::write_atomic(&path, trimmed.as_bytes())?;
        Ok(Self::parse(trimmed, "CACHE"))
    }

    /// Removes any cached fetched lyrics for `track_id`.
    pub fn clear_cached(track_id: &str) {
        let path = Self::cache_path(track_id);
        if path.exists() {
            let _ = fs::remove_file(path);
        }
    }

    pub fn load(audio_path: &Path) -> Option<LyricsDoc> {
        let lrc_path = audio_path.with_extension("lrc");
        if fs::metadata(&lrc_path).is_ok_and(|m| m.is_file() && m.len() <= MAX_LRC_BYTES)
            && let Ok(bytes) = fs::read(&lrc_path)
            && let Some(doc) = Self::parse(&decode_text(&bytes), "LRC")
        {
            return Some(doc);
        }

        let file = fs::File::open(audio_path).ok()?;
        Self::load_embedded(std::io::BufReader::new(file), audio_path)
    }

    /// Raw text from sidecar `.lrc` or embedded tag (for the Tag Editor).
    pub fn load_raw(audio_path: &Path) -> Option<String> {
        let lrc_path = audio_path.with_extension("lrc");
        if fs::metadata(&lrc_path).is_ok_and(|m| m.is_file() && m.len() <= MAX_LRC_BYTES)
            && let Ok(bytes) = fs::read(&lrc_path)
        {
            let text = decode_text(&bytes);
            if !text.trim().is_empty() {
                return Some(text);
            }
        }
        let file = fs::File::open(audio_path).ok()?;
        Self::load_raw_embedded(std::io::BufReader::new(file), audio_path)
    }

    /// Raw embedded lyrics text from any seekable reader.
    pub fn load_raw_embedded<R: std::io::Read + std::io::Seek>(reader: R, hint: &Path) -> Option<String> {
        let text = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut probe = Probe::new(reader)
                .options(ParseOptions::new().read_properties(false).read_cover_art(false));
            probe = match lofty::file::FileType::from_path(hint) {
                Some(ft) => probe.set_file_type(ft),
                None => probe.guess_file_type().ok()?,
            };
            let tagged = probe.read().ok()?;
            let tag = tagged.primary_tag().or_else(|| tagged.first_tag())?;
            tag.get_string(ItemKey::Lyrics).map(str::to_owned)
        }))
        .ok()??;
        if text.trim().is_empty() { None } else { Some(text) }
    }

    /// Lyrics stored in the audio file's tag, read from any seekable source (a SAF document opened
    /// through a file descriptor). `hint` is the file name, used to pick the tag format.
    pub fn load_embedded<R: std::io::Read + std::io::Seek>(reader: R, hint: &Path) -> Option<LyricsDoc> {
        let text = Self::load_raw_embedded(reader, hint)?;
        Self::parse(&text, "TAG")
    }

    pub fn parse(text: &str, source: &'static str) -> Option<LyricsDoc> {
        let mut synced = Vec::new();
        let mut plain = Vec::new();
        let mut offset_secs = 0.0_f64;

        let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
        for raw in normalized.split('\n') {
            let line = raw.trim().trim_start_matches('\u{feff}').trim_start();
            if let Some(meta) = META_RX.captures(line) {
                if meta[1].eq_ignore_ascii_case("offset")
                    && let Ok(ms) = meta[2].trim().parse::<f64>()
                    && ms.is_finite()
                {
                    offset_secs = ms.clamp(-MAX_OFFSET_MS, MAX_OFFSET_MS) / 1000.0;
                }
                continue;
            }

            // Only the run of timestamps at the very start of the line stamps it; one in the middle
            // of the text is part of the text.
            let mut times = Vec::new();
            let mut rest = line;
            while let Some(cap) = STAMP_RX.captures(rest) {
                let whole = cap.get(0).unwrap();
                if whole.start() != 0 {
                    break;
                }
                let sec_str = cap[2].replace(':', ".");
                if let (Ok(min), Ok(sec)) = (cap[1].parse::<i64>(), sec_str.parse::<f64>()) {
                    times.push((min as f64) * 60.0 + sec);
                }
                rest = rest[whole.end()..].trim_start();
            }
            if rest.len() == line.len() {
                plain.push(LyricLine {
                    time: -1.0,
                    text: line.to_owned(),
                });
                continue;
            }

            let body = INLINE_WORD_RX.replace_all(rest, "").trim().to_owned();
            for time in times {
                synced.push(LyricLine { time, text: body.clone() });
            }
        }

        if !synced.is_empty() {
            for l in &mut synced {
                l.time = (l.time - offset_secs).max(0.0);
            }
            synced.sort_by(|a, b| a.time.total_cmp(&b.time));
            return Some(LyricsDoc {
                lines: synced,
                is_synced: true,
                source,
            });
        }

        // Drop leading blank lines in one O(n) drain, not one O(n) shift per line.
        let leading_blank = plain.iter().take_while(|l| l.text.is_empty()).count();
        plain.drain(..leading_blank);
        while plain.last().is_some_and(|l| l.text.is_empty()) {
            plain.pop();
        }
        if plain.is_empty() {
            None
        } else {
            Some(LyricsDoc {
                lines: plain,
                is_synced: false,
                source,
            })
        }
    }
}

fn decode_text(bytes: &[u8]) -> String {
    if bytes.len() >= 2 && bytes[0] == 0xFF && bytes[1] == 0xFE {
        let u16s: Vec<u16> = bytes[2..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .collect();
        return String::from_utf16_lossy(&u16s);
    }
    if bytes.len() >= 2 && bytes[0] == 0xFE && bytes[1] == 0xFF {
        let u16s: Vec<u16> = bytes[2..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_be_bytes(*c))
            .collect();
        return String::from_utf16_lossy(&u16s);
    }
    if bytes.len() >= 3 && bytes[0] == 0xEF && bytes[1] == 0xBB && bytes[2] == 0xBF {
        return String::from_utf8_lossy(&bytes[3..]).into_owned();
    }
    String::from_utf8_lossy(bytes).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_multi_stamp_offset_and_enhanced_lrc() {
        let lrc = "\
[ti:Test]
[offset:500]
[00:01.00][00:10.50]<00:01.10>Hello <00:01.50>world
[00:05.00]Second line
";
        let doc = LyricsLoader::parse(lrc, "LRC").unwrap();
        assert!(doc.is_synced);
        assert_eq!(doc.lines.len(), 3);
        assert!((doc.lines[0].time - 0.5).abs() < 1e-6);
        assert_eq!(doc.lines[0].text, "Hello world");
        assert!((doc.lines[1].time - 4.5).abs() < 1e-6);
        assert_eq!(doc.lines[1].text, "Second line");
        assert!((doc.lines[2].time - 10.0).abs() < 1e-6);
    }

    #[test]
    fn sidecar_wins_over_the_tag_and_a_reader_gets_the_tag() {
        use crate::testing::{Fixture, TestEnv, set_lyrics, write_flac};
        let env = TestEnv::new("lyrics-load");
        let song = env.music.join("a.flac");
        write_flac(&song, &Fixture::new("A", "X", "Alb", 1));
        set_lyrics(&song, "[00:01.00]from the tag\n[00:02.00]second");

        let doc = LyricsLoader::load(&song).expect("tag lyrics");
        assert_eq!((doc.source, doc.is_synced, doc.lines.len()), ("TAG", true, 2));

        // A reader (what a SAF document gives us) reads the same embedded lyrics.
        let bytes = std::fs::read(&song).unwrap();
        let from_reader = LyricsLoader::load_embedded(std::io::Cursor::new(bytes), Path::new("x.flac")).unwrap();
        assert_eq!(from_reader.lines, doc.lines);

        std::fs::write(song.with_extension("lrc"), "[00:05.00]from the sidecar").unwrap();
        let doc = LyricsLoader::load(&song).unwrap();
        assert_eq!((doc.source, doc.lines[0].text.as_str()), ("LRC", "from the sidecar"));
    }

    #[test]
    fn non_finite_and_huge_offsets_are_ignored_or_clamped() {
        for bad in ["NaN", "inf", "-inf", "Infinity"] {
            let doc = LyricsLoader::parse(&format!("[offset:{bad}]\n[00:10.00]line"), "LRC").unwrap();
            assert!((doc.lines[0].time - 10.0).abs() < 1e-9, "{bad}: {}", doc.lines[0].time);
        }
        let doc = LyricsLoader::parse("[offset:99999999]\n[05:00.00]line", "LRC").unwrap();
        assert_eq!(doc.lines[0].time, 0.0, "clamped to 10 minutes, then floored at 0");
        let doc = LyricsLoader::parse("[offset:-99999999]\n[00:10.00]line", "LRC").unwrap();
        assert!((doc.lines[0].time - 610.0).abs() < 1e-9, "{}", doc.lines[0].time);
        assert!(doc.lines[0].time.is_finite());
    }

    #[test]
    fn only_leading_timestamps_stamp_a_line() {
        let doc = LyricsLoader::parse("[00:01.00]\n[00:02.00] [00:03.00]sung twice\n[00:04.00]before [00:05.00] after", "LRC").unwrap();
        let got: Vec<(i64, &str)> = doc.lines.iter().map(|l| (l.time as i64, l.text.as_str())).collect();
        assert_eq!(got, vec![(1, ""), (2, "sung twice"), (3, "sung twice"), (4, "before [00:05.00] after")]);
        // Text before a mid-line stamp is never dropped: without leading stamps the line is plain.
        let plain = LyricsLoader::parse("Hello [00:05.00] world", "TAG").unwrap();
        assert!(!plain.is_synced);
        assert_eq!(plain.lines[0].text, "Hello [00:05.00] world");
    }

    #[test]
    fn byte_order_marks_are_stripped_in_files_and_tags() {
        let lrc = "[00:01.00]hello\n[00:02.00]wörld";
        let mut utf8 = vec![0xEF, 0xBB, 0xBF];
        utf8.extend_from_slice(lrc.as_bytes());
        let mut le = vec![0xFF, 0xFE];
        le.extend(lrc.encode_utf16().flat_map(u16::to_le_bytes));
        let mut be = vec![0xFE, 0xFF];
        be.extend(lrc.encode_utf16().flat_map(u16::to_be_bytes));
        for bytes in [&utf8, &le, &be] {
            let doc = LyricsLoader::parse(&decode_text(bytes), "LRC").unwrap();
            assert_eq!(doc.lines.len(), 2);
            assert_eq!(doc.lines[1].text, "wörld");
        }
        // A BOM that survived into tag text must not hide the first timestamp.
        let doc = LyricsLoader::parse("\u{feff}[00:01.00]first", "TAG").unwrap();
        assert!(doc.is_synced);
        // An odd trailing byte in UTF-16 is dropped, not a panic.
        let mut odd = vec![0xFF, 0xFE];
        odd.extend("[00:01.00]x".encode_utf16().flat_map(u16::to_le_bytes));
        odd.push(0x41);
        assert!(LyricsLoader::parse(&decode_text(&odd), "LRC").is_some());
    }

    #[test]
    fn an_oversized_sidecar_is_ignored() {
        use crate::testing::{Fixture, TestEnv, write_flac};
        let env = TestEnv::new("lyrics-big-lrc");
        let song = env.music.join("a.flac");
        write_flac(&song, &Fixture::new("A", "X", "Alb", 1));
        let mut big = String::from("[00:01.00]line\n");
        big.push_str(&"x".repeat(MAX_LRC_BYTES as usize));
        std::fs::write(song.with_extension("lrc"), big).unwrap();
        assert!(LyricsLoader::load(&song).is_none(), "neither a sidecar nor a tag: no lyrics");
    }

    #[test]
    fn garbage_audio_reads_as_no_lyrics() {
        assert!(LyricsLoader::load_embedded(std::io::Cursor::new(vec![0u8; 64]), Path::new("x.flac")).is_none());
        assert!(LyricsLoader::load_embedded(std::io::Cursor::new(b"fLaC\xff\xff\xff".to_vec()), Path::new("x.flac")).is_none());
        assert!(LyricsLoader::load_embedded(std::io::Cursor::new(Vec::new()), Path::new("noext")).is_none());
    }
}
