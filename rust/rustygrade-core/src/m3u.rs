use std::collections::{HashMap, HashSet};

use thiserror::Error;

use crate::backup::{MapOutcome, TrackPathMapper};
use crate::models::{Album, Track};

pub const MAX_M3U_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Error)]
pub enum M3uError {
    #[error("Playlist file is too large to import (limit {limit}).")]
    TooLarge { limit: &'static str },
    #[error("Playlist file contains no track entries.")]
    EmptyPlaylist,
    #[error("None of the {total_entries} tracks in this playlist matched your local library.")]
    NoTracksMatched { total_entries: usize },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct M3uEntry {
    pub path: String,
    pub duration_secs: Option<i64>,
    pub artist: Option<String>,
    pub title: Option<String>,
    pub album: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedM3u {
    pub name: String,
    pub entries: Vec<M3uEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct M3uMatchResult {
    pub playlist_name: String,
    pub track_ids: Vec<String>,
    pub matched_exact: usize,
    pub matched_remapped: usize,
    pub matched_metadata: usize,
    pub unmatched: usize,
}

/// Sanitizes a playlist title into a cross-platform filename `<stem>.<ext>`.
pub fn sanitize_export_filename(name: &str, ext: &str) -> String {
    let mut cleaned = String::with_capacity(name.len());
    for ch in name.chars() {
        let forbidden = matches!(ch, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
            || (ch as u32) < 0x20
            || ch == '\u{007f}';
        cleaned.push(if forbidden { '_' } else { ch });
    }
    let trimmed = cleaned.trim().trim_matches(|c: char| c == '.' || c == '_' || c.is_whitespace());
    let truncated: String = trimmed.chars().take(80).collect();
    let stem = truncated.trim().trim_matches(|c: char| c == '.' || c == '_' || c.is_whitespace());
    let final_stem = if stem.is_empty() { "playlist" } else { stem };
    let clean_ext = ext.trim().trim_start_matches('.');
    if clean_ext.is_empty() {
        final_stem.to_owned()
    } else {
        format!("{final_stem}.{clean_ext}")
    }
}

/// Appends `s` trimmed, with any interior CR/LF turned into spaces, so a field can never break the
/// line-based format. (Trimming first means no CR/LF is left at either end.)
fn push_clean(out: &mut String, s: &str) {
    for c in s.trim().chars() {
        out.push(if matches!(c, '\r' | '\n') { ' ' } else { c });
    }
}

/// Exports a playlist as Extended M3U8 (UTF-8, LF line endings) compatible with Desktop Rustygrade.
pub fn export_m3u8<'a>(playlist_name: &str, tracks: impl IntoIterator<Item = &'a Track>) -> String {
    use std::fmt::Write as _;

    let mut out = String::from("#EXTM3U\n");
    if !playlist_name.trim().is_empty() {
        out.push_str("#PLAYLIST:");
        push_clean(&mut out, playlist_name);
        out.push('\n');
    }

    for track in tracks {
        let raw_path = if !track.file_path.is_empty() {
            &track.file_path
        } else {
            &track.id
        };
        if raw_path.trim().is_empty() {
            continue;
        }
        let secs = if track.duration.is_finite() && track.duration > 0.0 {
            track.duration.clamp(0.0, i64::MAX as f64).round() as i64
        } else {
            -1
        };
        let has_artist = !track.artist.trim().is_empty();
        let has_title = !track.title.trim().is_empty();

        let _ = write!(out, "#EXTINF:{secs},");
        if has_artist && has_title {
            push_clean(&mut out, &track.artist);
            out.push_str(" - ");
            push_clean(&mut out, &track.title);
        } else if has_title {
            push_clean(&mut out, &track.title);
        } else {
            push_clean(&mut out, &track.artist);
        }
        out.push('\n');

        if !track.album.trim().is_empty() {
            out.push_str("#EXTALB:");
            push_clean(&mut out, &track.album);
            out.push('\n');
        }
        push_clean(&mut out, raw_path);
        out.push('\n');
    }

    out
}

fn strip_ascii_prefix<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let bytes = s.as_bytes();
    let p_bytes = prefix.as_bytes();
    if bytes.len() >= p_bytes.len() && bytes[..p_bytes.len()].eq_ignore_ascii_case(p_bytes) {
        Some(&s[p_bytes.len()..])
    } else {
        None
    }
}

fn strip_ascii_suffix<'a>(s: &'a str, suffix: &str) -> Option<&'a str> {
    let bytes = s.as_bytes();
    let s_bytes = suffix.as_bytes();
    if bytes.len() >= s_bytes.len()
        && bytes[bytes.len() - s_bytes.len()..].eq_ignore_ascii_case(s_bytes)
    {
        Some(&s[..bytes.len() - s_bytes.len()])
    } else {
        None
    }
}

fn fallback_name_from_hint(hint: Option<&str>) -> Option<String> {
    let raw = hint?.trim();
    if raw.is_empty() {
        return None;
    }
    let base = raw.rsplit(['/', '\\']).next()?.trim();
    let stem = strip_ascii_suffix(base, ".m3u8")
        .or_else(|| strip_ascii_suffix(base, ".m3u"))
        .unwrap_or(base)
        .trim();
    if stem.is_empty() {
        None
    } else {
        Some(stem.to_owned())
    }
}

/// Parses `.m3u` / `.m3u8` content, stripping an optional UTF-8 BOM and enforcing [`MAX_M3U_BYTES`].
pub fn parse_m3u(content: &str, filename_hint: Option<&str>) -> Result<ParsedM3u, M3uError> {
    if content.len() > MAX_M3U_BYTES {
        return Err(M3uError::TooLarge { limit: "16 MiB" });
    }
    let body = content.strip_prefix('\u{FEFF}').unwrap_or(content);

    let mut playlist_name: Option<String> = None;
    let mut entries = Vec::new();
    let mut pending_duration: Option<i64> = None;
    let mut pending_artist: Option<String> = None;
    let mut pending_title: Option<String> = None;
    let mut pending_album: Option<String> = None;

    for raw_line in body
        .split('\n')
        .flat_map(|s| s.strip_suffix('\r').unwrap_or(s).split('\r'))
    {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = strip_ascii_prefix(line, "#PLAYLIST:") {
            let trimmed = rest.trim();
            if playlist_name.is_none() && !trimmed.is_empty() {
                playlist_name = Some(trimmed.to_owned());
            }
            continue;
        }
        if let Some(rest) = strip_ascii_prefix(line, "#EXTINF:") {
            pending_duration = None;
            pending_artist = None;
            pending_title = None;
            pending_album = None;

            let (dur_part, display_part) = match rest.split_once(',') {
                Some((left, right)) => (left.trim(), right.trim()),
                None => (rest.trim(), ""),
            };
            if let Some(tok) = dur_part.split_whitespace().next()
                && let Ok(secs) = tok.parse::<f64>()
                && secs.is_finite()
                && secs >= 0.0
            {
                pending_duration = Some(secs.clamp(0.0, i64::MAX as f64).round() as i64);
            }
            if !display_part.is_empty() {
                if let Some((artist, title)) = display_part.split_once(" - ") {
                    let a = artist.trim();
                    let t = title.trim();
                    if !a.is_empty() && !t.is_empty() {
                        pending_artist = Some(a.to_owned());
                        pending_title = Some(t.to_owned());
                    } else if !t.is_empty() {
                        pending_title = Some(t.to_owned());
                    } else if !a.is_empty() {
                        pending_title = Some(a.to_owned());
                    }
                } else {
                    pending_title = Some(display_part.to_owned());
                }
            }
            continue;
        }
        if let Some(rest) = strip_ascii_prefix(line, "#EXTALB:") {
            let trimmed = rest.trim();
            if !trimmed.is_empty() {
                pending_album = Some(trimmed.to_owned());
            }
            continue;
        }
        if line.starts_with('#') {
            continue;
        }

        entries.push(M3uEntry {
            path: line.to_owned(),
            duration_secs: pending_duration.take(),
            artist: pending_artist.take(),
            title: pending_title.take(),
            album: pending_album.take(),
        });
    }

    let name = playlist_name
        .or_else(|| fallback_name_from_hint(filename_hint))
        .unwrap_or_else(|| "Imported Playlist".to_owned());

    Ok(ParsedM3u { name, entries })
}

fn unwrap_m3u_path(raw: &str) -> String {
    let trimmed = raw.trim();
    let Some(rest) = strip_ascii_prefix(trimmed, "file://") else {
        return trimmed.to_owned();
    };
    let no_qf = match (rest.find('?'), rest.find('#')) {
        (Some(q), Some(h)) => &rest[..q.min(h)],
        (Some(q), None) => &rest[..q],
        (None, Some(h)) => &rest[..h],
        (None, None) => rest,
    };
    let mut p = if let Some(after_host) = strip_ascii_prefix(no_qf, "localhost/") {
        // Re-anchor to the `/` before `after_host`: `no_qf` starts with `"localhost/"`.
        &no_qf[no_qf.len() - after_host.len() - 1..]
    } else {
        no_qf
    };
    let bytes = p.as_bytes();
    if bytes.len() > 2 && bytes[0] == b'/' && bytes[1].is_ascii_alphabetic() && bytes[2] == b':' {
        p = &p[1..];
    }
    urlencoding::decode(p)
        .map(|decoded| decoded.into_owned())
        .unwrap_or_else(|_| p.to_owned())
}

fn is_http_url(raw: &str) -> bool {
    let trimmed = raw.trim();
    strip_ascii_prefix(trimmed, "http://").is_some()
        || strip_ascii_prefix(trimmed, "https://").is_some()
}

fn normalize_meta(s: &str) -> String {
    let mut out = String::new();
    for word in s.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&word.to_lowercase());
    }
    out
}

fn pick_unique_candidate(
    candidates: &[usize],
    tracks: &[&Track],
    target_duration: Option<i64>,
) -> Option<String> {
    match candidates.len() {
        0 => None,
        1 => Some(tracks[candidates[0]].id.clone()),
        _ => {
            let dur = target_duration.filter(|&d| d >= 0)?;
            let mut matched: Option<&str> = None;
            for &idx in candidates {
                let t = tracks[idx];
                if t.duration.is_finite()
                    && t.duration > 0.0
                    && (t.duration - dur as f64).abs() <= 3.0
                {
                    if matched.is_some() {
                        return None;
                    }
                    matched = Some(&t.id);
                }
            }
            matched.map(str::to_owned)
        }
    }
}

/// Library tracks keyed by normalized (artist, title, album) and (artist, title), as indices into
/// the `local_tracks` slice it was built from.
struct MetaIndex {
    by_artist_title_album: HashMap<(String, String, String), Vec<usize>>,
    by_artist_title: HashMap<(String, String), Vec<usize>>,
}

impl MetaIndex {
    fn build(tracks: &[&Track], album_names: &[&str]) -> Self {
        let mut by_artist_title_album: HashMap<(String, String, String), Vec<usize>> = HashMap::new();
        let mut by_artist_title: HashMap<(String, String), Vec<usize>> = HashMap::new();
        for (idx, (track, album_name)) in tracks.iter().zip(album_names).enumerate() {
            let norm_title = normalize_meta(&track.title);
            if norm_title.is_empty() {
                continue;
            }
            let norm_artist = normalize_meta(&track.artist);
            let norm_album = normalize_meta(if track.album.trim().is_empty() {
                album_name
            } else {
                &track.album
            });
            if !norm_album.is_empty() {
                by_artist_title_album
                    .entry((norm_artist.clone(), norm_title.clone(), norm_album))
                    .or_default()
                    .push(idx);
            }
            by_artist_title
                .entry((norm_artist, norm_title))
                .or_default()
                .push(idx);
        }
        Self {
            by_artist_title_album,
            by_artist_title,
        }
    }

    /// The single library track matching an entry's normalized tags (`norm_title` non-empty).
    fn lookup(
        &self,
        norm_artist: String,
        norm_title: String,
        norm_album: String,
        tracks: &[&Track],
        duration_secs: Option<i64>,
    ) -> Option<(String, Tier)> {
        let candidates = if !norm_album.is_empty() {
            self.by_artist_title_album.get(&(norm_artist, norm_title, norm_album))
        } else if !norm_artist.is_empty() {
            self.by_artist_title.get(&(norm_artist, norm_title))
        } else {
            None
        }?;
        pick_unique_candidate(candidates, tracks, duration_secs).map(|id| (id, Tier::Metadata))
    }
}

enum Tier {
    Exact,
    Remapped,
    Metadata,
}

/// Parses an `.m3u` / `.m3u8` playlist and resolves its entries against `albums` using
/// exact paths, `TrackPathMapper` cross-OS path normalization, and unambiguous metadata fallback.
pub fn match_m3u_playlist(
    content: &str,
    filename_hint: Option<&str>,
    albums: &[Album],
    source_roots: &[String],
) -> Result<M3uMatchResult, M3uError> {
    let parsed = parse_m3u(content, filename_hint)?;
    if parsed.entries.is_empty() {
        return Err(M3uError::EmptyPlaylist);
    }

    let mapper = TrackPathMapper::from_albums(albums, source_roots);

    // Every distinct library track with its album's name (for tracks without an album tag).
    let mut local_tracks: Vec<&Track> = Vec::new();
    let mut local_album_names: Vec<&str> = Vec::new();
    let mut seen_local = HashSet::new();
    for album in albums {
        for track in &album.tracks {
            if seen_local.insert(track.id.as_str()) {
                local_tracks.push(track);
                local_album_names.push(&album.name);
            }
        }
    }
    // The metadata index normalizes every track's tags: only build it once an entry actually
    // fails path matching (a playlist exported from this library never needs it).
    let mut meta_index: Option<MetaIndex> = None;

    let mut track_ids = Vec::new();
    let mut seen_ids = HashSet::new();
    let mut matched_exact = 0usize;
    let mut matched_remapped = 0usize;
    let mut matched_metadata = 0usize;
    let mut unmatched = 0usize;

    let total_entries = parsed.entries.len();
    for entry in parsed.entries {
        let outcome = if is_http_url(&entry.path) {
            MapOutcome::Unmatched(entry.path.clone())
        } else {
            let candidate_path = unwrap_m3u_path(&entry.path);
            mapper.map_id(&candidate_path)
        };
        let resolved: Option<(String, Tier)> = match outcome {
            MapOutcome::Exact(id) => Some((id, Tier::Exact)),
            MapOutcome::Remapped(id) => Some((id, Tier::Remapped)),
            MapOutcome::Unmatched(_) => {
                let norm_title = entry.title.as_deref().map(normalize_meta).unwrap_or_default();
                let norm_artist = entry.artist.as_deref().map(normalize_meta).unwrap_or_default();
                let norm_album = entry.album.as_deref().map(normalize_meta).unwrap_or_default();
                if norm_title.is_empty() {
                    None
                } else {
                    let index = meta_index
                        .get_or_insert_with(|| MetaIndex::build(&local_tracks, &local_album_names));
                    index.lookup(norm_artist, norm_title, norm_album, &local_tracks, entry.duration_secs)
                }
            }
        };

        match resolved {
            Some((id, tier)) => {
                if seen_ids.insert(id.clone()) {
                    track_ids.push(id);
                    match tier {
                        Tier::Exact => matched_exact += 1,
                        Tier::Remapped => matched_remapped += 1,
                        Tier::Metadata => matched_metadata += 1,
                    }
                }
            }
            None => unmatched += 1,
        }
    }

    if track_ids.is_empty() {
        return Err(M3uError::NoTracksMatched { total_entries });
    }

    Ok(M3uMatchResult {
        playlist_name: parsed.name,
        track_ids,
        matched_exact,
        matched_remapped,
        matched_metadata,
        unmatched,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ReplayGain;

    fn mk_track(
        id: &str,
        file_path: &str,
        title: &str,
        artist: &str,
        album: &str,
        duration: f64,
    ) -> Track {
        Track {
            id: id.to_owned(),
            title: title.to_owned(),
            artist: artist.to_owned(),
            album: album.to_owned(),
            duration,
            file_path: file_path.to_owned(),
            play_uri: format!("file://{file_path}"),
            replay_gain: ReplayGain::default(),
            track_number: 1,
            disc_number: 1,
            codec: "FLAC".to_owned(),
            sample_rate: 44100,
            bits_per_sample: 16,
            channels: 2,
            cover_thumb_path: None,
            cover_full_path: None,
        }
    }

    fn mk_album(name: &str, tracks: Vec<Track>) -> Album {
        Album {
            name: name.to_owned(),
            artist: "Artist".to_owned(),
            cover_thumb_path: None,
            cover_full_path: None,
            year: 2024,
            added_at: 1,
            tracks,
        }
    }

    #[test]
    fn export_m3u8_formats_headers_extinf_extalb_and_fallbacks() {
        let t1 = mk_track(
            "/saf/primary:Music/Ado/Zanmu/01.flac",
            "",
            "Show\nLive",
            "Ado\r\n",
            "Zanmu\r",
            213.6,
        );
        let t2 = mk_track(
            "id2",
            "/storage/0/Music/Solo.mp3",
            "Instrumental",
            "",
            "",
            0.0,
        );
        let text = export_m3u8("Road\nTrip\r", [&t1, &t2]);
        assert!(!text.contains('\r'));
        let expected = "\
#EXTM3U
#PLAYLIST:Road Trip
#EXTINF:214,Ado - Show Live
#EXTALB:Zanmu
/saf/primary:Music/Ado/Zanmu/01.flac
#EXTINF:-1,Instrumental
/storage/0/Music/Solo.mp3
";
        assert_eq!(text, expected);
    }

    #[test]
    fn sanitize_export_filename_strips_unsafe_chars_and_falls_back() {
        assert_eq!(
            sanitize_export_filename("  My / Best: Mix? *2025*  ", "m3u8"),
            "My _ Best_ Mix_ _2025.m3u8"
        );
        assert_eq!(sanitize_export_filename("...___...", "zip"), "playlist.zip");
        assert_eq!(sanitize_export_filename(&"音".repeat(100), "m3u8").chars().count(), 80 + 5);
    }

    #[test]
    fn parse_m3u_handles_bom_crlf_comments_non_ascii_and_name_fallbacks() {
        let raw = "\u{FEFF}#EXTM3U\r\n# 音楽コメント\r\n\r\n#EXTINF:180.4,Artist - Title\r\n#EXTALB:Album\r\n/music/01.flac\r\n#EXTINF:-1,Solo\r\n/music/02.flac\r\n";
        let parsed = parse_m3u(raw, Some("/tmp/Road Trip.M3U8")).unwrap();
        assert_eq!(parsed.name, "Road Trip");
        assert_eq!(parsed.entries.len(), 2);
        assert_eq!(
            parsed.entries[0],
            M3uEntry {
                path: "/music/01.flac".to_owned(),
                duration_secs: Some(180),
                artist: Some("Artist".to_owned()),
                title: Some("Title".to_owned()),
                album: Some("Album".to_owned()),
            }
        );
        assert_eq!(
            parsed.entries[1],
            M3uEntry {
                path: "/music/02.flac".to_owned(),
                duration_secs: None,
                artist: None,
                title: Some("Solo".to_owned()),
                album: None,
            }
        );

        let fallback = parse_m3u("/music/01.flac\n", None).unwrap();
        assert_eq!(fallback.name, "Imported Playlist");

        let oversized = "a".repeat(MAX_M3U_BYTES + 1);
        assert!(matches!(
            parse_m3u(&oversized, None),
            Err(M3uError::TooLarge { .. })
        ));
    }

    #[test]
    fn match_m3u_resolves_exact_file_url_remapped_disc_and_metadata() {
        let albums = vec![mk_album(
            "Alpha",
            vec![
                mk_track(
                    "/storage/emulated/0/Music/Rock/Alpha/01 Track.flac",
                    "/storage/emulated/0/Music/Rock/Alpha/01 Track.flac",
                    "Opening",
                    "Band",
                    "Alpha",
                    200.0,
                ),
                mk_track(
                    "/saf/primary:Music/Box/CD1/02.flac",
                    "/saf/primary:Music/Box/CD1/02.flac",
                    "Encore",
                    "Band",
                    "Box",
                    150.0,
                ),
                mk_track(
                    "/storage/emulated/0/Music/Elsewhere/Renamed.flac",
                    "/storage/emulated/0/Music/Elsewhere/Renamed.flac",
                    "Hidden Gem",
                    "Indie Artist",
                    "Debut",
                    242.0,
                ),
                // Two tracks with same artist + title in different albums / durations
                mk_track(
                    "/storage/emulated/0/Music/V1/dup.flac",
                    "/storage/emulated/0/Music/V1/dup.flac",
                    "Echo",
                    "Synth",
                    "Single",
                    120.0,
                ),
                mk_track(
                    "/storage/emulated/0/Music/V2/dup.flac",
                    "/storage/emulated/0/Music/V2/dup.flac",
                    "Echo",
                    "Synth",
                    "Live",
                    310.0,
                ),
            ],
        )];

        let m3u = "\
#EXTM3U
#PLAYLIST:Cross Platform Mix
#EXTINF:200,Band - Opening
file://localhost/storage/emulated/0/music/rock/alpha/01%20Track.flac
#EXTINF:150,Band - Encore
D:\\DesktopMusic\\Box\\CD1\\02.flac
#EXTINF:241,  indie   artist  -  hidden gem 
#EXTALB:Debut
/completely/different/path/unknown.mp3
#EXTINF:311,Synth - Echo
/missing/echo_live.flac
#EXTINF:200,Band - Opening
/storage/emulated/0/Music/Rock/Alpha/01 Track.flac
#EXTINF:150,Other Band - Encore
/other/WrongAlbum/CD1/02.flac
#EXTINF:200,Synth - Echo
/missing/ambiguous_no_match.flac
#EXTINF:241,Indie Artist - Hidden Gem
#EXTALB:Wrong Album
/missing/wrong_album.flac
";

        let res = match_m3u_playlist(m3u, Some("fallback.m3u8"), &albums, &[]).unwrap();
        assert_eq!(res.playlist_name, "Cross Platform Mix");
        assert_eq!(
            res.track_ids,
            vec![
                "/storage/emulated/0/Music/Rock/Alpha/01 Track.flac",
                "/saf/primary:Music/Box/CD1/02.flac",
                "/storage/emulated/0/Music/Elsewhere/Renamed.flac",
                "/storage/emulated/0/Music/V2/dup.flac",
            ]
        );
        assert_eq!(res.matched_exact, 1);
        assert_eq!(res.matched_remapped, 1);
        assert_eq!(res.matched_metadata, 2);
        // Duplicate exact entry deduplicated without counting as unmatched; 3 genuinely unmatched:
        // WrongAlbum/CD1/02.flac, Echo @ 200s (ambiguous), and Hidden Gem with Wrong Album.
        assert_eq!(res.unmatched, 3);
    }

    #[test]
    fn match_m3u_rejects_empty_and_zero_matches() {
        let albums = vec![mk_album(
            "A",
            vec![mk_track("/m/01.flac", "/m/01.flac", "One", "Art", "A", 100.0)],
        )];

        assert!(matches!(
            match_m3u_playlist("#EXTM3U\n#PLAYLIST:Empty\n", None, &albums, &[]),
            Err(M3uError::EmptyPlaylist)
        ));

        assert!(matches!(
            match_m3u_playlist("#EXTM3U\n#EXTINF:90,Unknown - Nope\n/nope/missing.flac\n", None, &albums, &[]),
            Err(M3uError::NoTracksMatched { total_entries: 1 })
        ));
    }

    #[test]
    fn match_m3u_handles_artistless_tracks_album_fallback_http_urls_and_file_fragments() {
        let albums = vec![mk_album(
            "Fallback Album",
            vec![
                mk_track(
                    "/storage/0/Music/Solo.flac",
                    "/storage/0/Music/Solo.flac",
                    "Solo Instrumental",
                    "",
                    "",
                    180.0,
                ),
                mk_track(
                    "/storage/0/Music/Artist/Song #1.flac",
                    "/storage/0/Music/Artist/Song #1.flac",
                    "Song #1",
                    "Some Band",
                    "",
                    205.0,
                ),
            ],
        )];

        // Title-only entry without `#EXTALB` must NOT match even an artist-less track,
        // while adding `#EXTALB:<album>` disambiguates and matches the artist-less track.
        // Track with empty `track.album` matches `#EXTALB:<album.name>`.
        // HTTP(S) URL does not get remapped via trailing segments (`Artist/Song #1.flac`).
        // `file://` URL strips `#fragment` while decoding `%23`.
        let m3u = "\
#EXTM3U
#EXTINF:180,Solo Instrumental
/other/path/solo_no_album.mp3
#EXTINF:180,Solo Instrumental
#EXTALB:Fallback Album
/other/path/solo_with_album.mp3
#EXTINF:180,Song #1
/other/path/no_artist_for_band_track.mp3
#EXTINF:205,Some Band - Song #1
#EXTALB:Fallback Album
/other/path/by_album_fallback.mp3
https://stream.example.com/Artist/Song%20%231.flac
file:///storage/0/Music/Artist/Song%20%231.flac#t=10
";
        let res = match_m3u_playlist(m3u, None, &albums, &[]).unwrap();
        assert_eq!(
            res.track_ids,
            vec![
                "/storage/0/Music/Solo.flac",
                "/storage/0/Music/Artist/Song #1.flac",
            ]
        );
        assert_eq!(res.matched_metadata, 2);
        // `file://...Song%20%231.flac#t=10` resolves Exact (deduped into track_ids),
        // while `Solo Instrumental` without album/artist, `Song #1` without artist,
        // and the `https://` URL stay unmatched.
        assert_eq!(res.matched_exact, 0);
        assert_eq!(res.unmatched, 3);
    }

    #[test]
    fn parse_and_match_m3u_handles_lone_cr_line_endings_and_empty_artist() {
        let content = concat!(
            "#EXTM3U\r",
            "#PLAYLIST:CR Mix\r",
            "#EXTINF:180,Intro\r",
            "/missing/intro.flac\r",
            "#EXTINF:205,Some Band - Song #1\r",
            "/missing/song1.flac\r",
        );
        let parsed = parse_m3u(content, None).unwrap();
        assert_eq!(parsed.name, "CR Mix");
        assert_eq!(parsed.entries.len(), 2);
        assert_eq!(parsed.entries[0].title.as_deref(), Some("Intro"));
        assert_eq!(parsed.entries[0].artist, None);
        assert_eq!(parsed.entries[1].artist.as_deref(), Some("Some Band"));
        assert_eq!(parsed.entries[1].title.as_deref(), Some("Song #1"));

        let albums = vec![mk_album(
            "Album",
            vec![
                mk_track(
                    "/m/intro.flac",
                    "/m/intro.flac",
                    "Intro",
                    "",
                    "Other",
                    180.0,
                ),
                mk_track(
                    "/m/song1.flac",
                    "/m/song1.flac",
                    "Song #1",
                    "Some Band",
                    "Album",
                    205.0,
                ),
            ],
        )];
        let res = match_m3u_playlist(content, None, &albums, &[]).unwrap();
        assert_eq!(res.track_ids, vec!["/m/song1.flac"]);
        assert_eq!(res.matched_metadata, 1);
        assert_eq!(res.unmatched, 1);
    }
}
