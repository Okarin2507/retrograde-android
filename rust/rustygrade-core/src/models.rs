use std::path::{Path, PathBuf};

/// ReplayGain tags of a track (dB gain, linear sample peak). `None` = the tag is absent.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ReplayGain {
    pub track_gain_db: Option<f32>,
    pub track_peak: Option<f32>,
    pub album_gain_db: Option<f32>,
    pub album_peak: Option<f32>,
}

impl ReplayGain {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Linear factor to apply for `mode` (`"track"`, `"album"`, anything else = off), like mpv's
    /// `--replaygain`: album mode falls back to the track tags, a track without tags is left
    /// unchanged (fallback 0 dB), and a boost is limited by the peak so it cannot clip.
    pub fn factor(&self, mode: &str) -> f32 {
        let (gain, peak) = match mode {
            "track" => (self.track_gain_db, self.track_peak),
            "album" => match self.album_gain_db {
                Some(g) => (Some(g), self.album_peak),
                None => (self.track_gain_db, self.track_peak),
            },
            _ => return 1.0,
        };
        let Some(gain) = gain.filter(|g| g.is_finite()) else {
            return 1.0;
        };
        // A corrupt tag must not turn into an absurd (or infinite) boost.
        let mut factor = 10f32.powf(gain.clamp(-40.0, 24.0) / 20.0);
        if let Some(peak) = peak.filter(|p| p.is_finite() && *p > 0.0)
            && factor * peak > 1.0
        {
            factor = 1.0 / peak;
        }
        factor
    }

    /// Peak-limited gain in dB to apply for `mode` (`"track"`, `"album"`, anything else = 0 dB).
    /// Matches [`Self::factor`] (used for bit-perfect hardware DAC volume ReplayGain).
    pub fn gain_db(&self, mode: &str) -> f32 {
        let f = self.factor(mode);
        if f <= 0.0 || !f.is_finite() || (f - 1.0).abs() < 1e-6 {
            return 0.0;
        }
        (20.0 * f.log10()).clamp(-40.0, 24.0)
    }
}

/// The short display name of an upper-case codec description or file extension
/// (`"FLAC"`, `"ALAC"`, `"AAC"`, `"OPUS"`, `"OGG"`, `"MP3"`, `"WAV"`, `"AIFF"`, `"WMA"`), or `None`
/// when it is not one we know. Shared by [`Track::short_codec`] and `smart::Codecs::short`.
pub(crate) fn canonical_codec(upper: &str) -> Option<&'static str> {
    Some(if upper.contains("FLAC") {
        "FLAC"
    } else if upper.contains("ALAC") || upper.contains("APPLE LOSSLESS") {
        "ALAC"
    } else if upper.contains("AAC") || upper == "M4A" {
        "AAC"
    } else if upper.contains("OPUS") {
        "OPUS"
    } else if upper.contains("VORBIS") || upper == "OGG" {
        "OGG"
    } else if upper.contains("MP3") || upper.contains("MPEG") {
        "MP3"
    } else if upper.contains("WAV") || upper.contains("PCM") {
        "WAV"
    } else if upper.contains("AIFF") {
        "AIFF"
    } else if upper.contains("WINDOWS MEDIA") {
        "WMA"
    } else {
        return None;
    })
}

#[derive(Debug, Clone)]
pub struct Track {
    /// Track identity is the absolute file path (shared with Electron & C# builds).
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration: f64,
    pub file_path: String,
    /// What a player opens: a `file://` URL, or the `content://` URI of a SAF document.
    pub play_uri: String,
    pub replay_gain: ReplayGain,
    pub track_number: i32,
    pub disc_number: i32,
    pub codec: String,
    pub sample_rate: i32,
    pub bits_per_sample: i32,
    pub channels: i32,
    pub cover_thumb_path: Option<PathBuf>,
    pub cover_full_path: Option<PathBuf>,
}

impl Track {
    pub fn short_codec(&self) -> String {
        let raw = if self.codec.is_empty() {
            Path::new(&self.file_path)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
        } else {
            self.codec.as_str()
        };
        let u = raw.trim().to_ascii_uppercase();
        match canonical_codec(&u) {
            Some(name) => name.to_owned(),
            None => u,
        }
    }

    pub fn quality_label(&self) -> String {
        self.quality_label_with_codec(&self.short_codec())
    }

    /// [`Self::quality_label`] for a caller that already computed [`Self::short_codec`].
    pub fn quality_label_with_codec(&self, short_codec: &str) -> String {
        let mut label = short_codec.to_owned();
        if self.bits_per_sample > 0 {
            label.push_str(&format!(" / {}BIT", self.bits_per_sample));
        }
        if self.sample_rate > 0 {
            let sr = self.sample_rate;
            if sr >= 1000 {
                if sr % 1000 == 0 {
                    label.push_str(&format!(" / {}KHZ", sr / 1000));
                } else {
                    label.push_str(&format!(" / {:.1}KHZ", sr as f64 / 1000.0));
                }
            } else {
                label.push_str(&format!(" / {sr}HZ"));
            }
        }
        label
    }

    pub fn album_quality_label(tracks: &[Track]) -> Option<String> {
        let first = tracks.first()?;
        let first_codec = first.short_codec();
        let mixed = tracks.iter().skip(1).any(|t| t.short_codec() != first_codec);
        let mut parts = vec![if mixed { "MIXED".to_owned() } else { first_codec }];
        if first.bits_per_sample > 0 {
            parts.push(format!("{}BIT", first.bits_per_sample));
        }
        if first.sample_rate > 0 {
            if first.sample_rate >= 1000 {
                let sr = first.sample_rate;
                parts.push(if sr % 1000 == 0 {
                    format!("{}KHZ", sr / 1000)
                } else {
                    format!("{:.1}KHZ", sr as f64 / 1000.0)
                });
            } else {
                parts.push(format!("{}HZ", first.sample_rate));
            }
        }
        Some(parts.join(" / "))
    }
}

#[derive(Debug, Clone)]
pub struct Album {
    pub name: String,
    pub artist: String,
    pub cover_thumb_path: Option<PathBuf>,
    pub cover_full_path: Option<PathBuf>,
    pub year: i32,
    pub added_at: i64,
    pub tracks: Vec<Track>,
}

#[derive(Debug, Clone)]
pub struct ArtistSummary {
    pub name: String,
    /// Indices into the album slice the artists were grouped from, newest album first.
    pub album_indices: Vec<usize>,
    pub track_count: usize,
    pub newest_added_at: i64,
}

impl ArtistSummary {
    /// This artist's albums, newest first; `albums` is the slice they were grouped from.
    pub fn albums<'a>(&'a self, albums: &'a [Album]) -> impl Iterator<Item = &'a Album> + 'a {
        self.album_indices.iter().filter_map(|&i| albums.get(i))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codec_track(codec: &str, file_path: &str) -> Track {
        Track {
            id: file_path.to_owned(),
            title: String::new(),
            artist: String::new(),
            album: String::new(),
            duration: 0.0,
            file_path: file_path.to_owned(),
            play_uri: String::new(),
            replay_gain: ReplayGain::default(),
            track_number: 0,
            disc_number: 1,
            codec: codec.to_owned(),
            sample_rate: 0,
            bits_per_sample: 0,
            channels: 0,
            cover_thumb_path: None,
            cover_full_path: None,
        }
    }

    #[test]
    fn short_codec_and_smart_codecs_share_one_mapping() {
        for (codec, path) in [
            ("FLAC", "/a.flac"),
            ("Apple Lossless", "/a.m4a"),
            ("MPEG-4 AAC", "/a.m4a"),
            ("Vorbis", "/a.ogg"),
            ("MPEG Layer 3", "/a.mp3"),
            ("PCM", "/a.wav"),
            ("Opus", "/a.opus"),
        ] {
            let t = codec_track(codec, path);
            assert_eq!(t.short_codec(), crate::Codecs::short(codec, path), "{codec}");
        }
        assert_eq!(codec_track("", "/x/y.m4a").short_codec(), "AAC");
        assert_eq!(codec_track("", "/x/y.ogg").short_codec(), "OGG");
        assert_eq!(codec_track("Windows Media", "/x.wma").short_codec(), "WMA");
    }

    fn rg(track: Option<f32>, tpeak: Option<f32>, album: Option<f32>, apeak: Option<f32>) -> ReplayGain {
        ReplayGain { track_gain_db: track, track_peak: tpeak, album_gain_db: album, album_peak: apeak }
    }

    #[test]
    fn replay_gain_factor_follows_mode_fallbacks_and_peak() {
        let tags = rg(Some(-6.0), Some(0.9), Some(-3.0), Some(0.95));
        let near = |a: f32, b: f32| (a - b).abs() < 1e-3;
        assert!(near(tags.factor("track"), 10f32.powf(-6.0 / 20.0)));
        assert!(near(tags.factor("album"), 10f32.powf(-3.0 / 20.0)));
        assert_eq!(tags.factor("no"), 1.0);
        assert_eq!(tags.factor("anything-else"), 1.0);
        // Album mode without album tags uses the track tags; no tags at all is unity.
        assert!(near(rg(Some(-6.0), None, None, None).factor("album"), 10f32.powf(-6.0 / 20.0)));
        assert_eq!(rg(None, Some(0.5), None, None).factor("track"), 1.0);
        // A boost that would clip is limited to 1 / peak.
        assert!(near(rg(Some(6.0), Some(0.8), None, None).factor("track"), 1.25));
        // Without a peak the boost is applied as is.
        assert!(near(rg(Some(6.0), None, None, None).factor("track"), 10f32.powf(6.0 / 20.0)));
        // Garbage never poisons the output: huge gains are clamped, not infinite.
        assert!(rg(Some(1000.0), None, None, None).factor("track").is_finite());
        assert!(near(rg(Some(1000.0), None, None, None).factor("track"), 10f32.powf(24.0 / 20.0)));
        assert_eq!(rg(Some(f32::NAN), None, None, None).factor("track"), 1.0);
    }
}
