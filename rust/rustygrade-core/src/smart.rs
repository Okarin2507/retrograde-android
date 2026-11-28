use std::collections::HashSet;
use std::path::Path;

use crate::collections::{SmartPlaylistData, SmartRule};
use crate::models::{Album, Track};
use crate::stats::PlayStat;

#[derive(Debug, Clone)]
pub struct SmartField {
    pub key: &'static str,
    pub label: &'static str,
    pub ops: &'static [&'static str],
    pub hint: &'static str,
}

pub struct SmartPlaylists;

impl SmartPlaylists {
    pub const FIELDS: &'static [SmartField] = &[
        SmartField {
            key: "title",
            label: "TITLE",
            ops: &["contains", "not_contains", "is"],
            hint: "TEXT",
        },
        SmartField {
            key: "artist",
            label: "ARTIST",
            ops: &["contains", "not_contains", "is"],
            hint: "TEXT",
        },
        SmartField {
            key: "album",
            label: "ALBUM",
            ops: &["contains", "not_contains", "is"],
            hint: "TEXT",
        },
        SmartField {
            key: "codec",
            label: "FORMAT",
            ops: &["is", "is_not"],
            hint: "FLAC, MP3, AAC...",
        },
        SmartField {
            key: "bits",
            label: "BIT DEPTH",
            ops: &["gte", "lte", "eq"],
            hint: "16, 24...",
        },
        SmartField {
            key: "rate",
            label: "SAMPLE RATE",
            ops: &["gte", "lte", "eq"],
            hint: "KHZ: 44.1, 96...",
        },
        SmartField {
            key: "year",
            label: "YEAR",
            ops: &["gte", "lte", "eq"],
            hint: "1999",
        },
        SmartField {
            key: "duration",
            label: "LENGTH",
            ops: &["gte", "lte"],
            hint: "MINUTES",
        },
        SmartField {
            key: "plays",
            label: "PLAYS",
            ops: &["gte", "lte", "eq"],
            hint: "COUNT",
        },
        SmartField {
            key: "played",
            label: "LAST PLAYED",
            ops: &["within", "not_within", "never"],
            hint: "DAYS",
        },
        SmartField {
            key: "added",
            label: "ADDED",
            ops: &["within", "not_within"],
            hint: "DAYS",
        },
        SmartField {
            key: "liked",
            label: "FAVORITE",
            ops: &["yes", "no"],
            hint: "",
        },
    ];

    pub const SORTS: &'static [(&'static str, &'static str)] = &[
        ("album", "ALBUM ORDER"),
        ("added_desc", "RECENTLY ADDED"),
        ("plays_desc", "MOST PLAYED"),
        ("played_desc", "RECENTLY PLAYED"),
        ("title", "TITLE [A-Z]"),
        ("random", "RANDOM"),
    ];

    pub fn op_label(op: &str) -> &'static str {
        match op {
            "contains" => "CONTAINS",
            "not_contains" => "DOESN'T CONTAIN",
            "is" => "IS",
            "is_not" => "IS NOT",
            "gte" => "AT LEAST",
            "lte" => "AT MOST",
            "eq" => "EXACTLY",
            "within" => "IN THE LAST",
            "not_within" => "NOT IN THE LAST",
            "never" => "NEVER",
            "yes" => "YES",
            "no" => "NO",
            _ => "",
        }
    }

    pub fn needs_value(op: &str) -> bool {
        !matches!(op, "never" | "yes" | "no")
    }

    pub fn examples() -> Vec<SmartPlaylistData> {
        let id = crate::fsutil::now_ms().max(1);
        vec![
            SmartPlaylistData {
                id,
                name: "Hi-Res 24 bit".to_owned(),
                rules: vec![SmartRule {
                    field: "bits".to_owned(),
                    op: "gte".to_owned(),
                    value: "24".to_owned(),
                }],
                ..SmartPlaylistData::default()
            },
            SmartPlaylistData {
                id: id + 1,
                name: "Added in 30 days".to_owned(),
                sort: "added_desc".to_owned(),
                rules: vec![SmartRule {
                    field: "added".to_owned(),
                    op: "within".to_owned(),
                    value: "30".to_owned(),
                }],
                ..SmartPlaylistData::default()
            },
            SmartPlaylistData {
                id: id + 2,
                name: "On Repeat".to_owned(),
                sort: "plays_desc".to_owned(),
                limit: 50,
                rules: vec![SmartRule {
                    field: "plays".to_owned(),
                    op: "gte".to_owned(),
                    value: "5".to_owned(),
                }],
                ..SmartPlaylistData::default()
            },
            SmartPlaylistData {
                id: id + 3,
                name: "Forgotten Favorites".to_owned(),
                sort: "random".to_owned(),
                rules: vec![
                    SmartRule {
                        field: "liked".to_owned(),
                        op: "yes".to_owned(),
                        value: String::new(),
                    },
                    SmartRule {
                        field: "played".to_owned(),
                        op: "not_within".to_owned(),
                        value: "90".to_owned(),
                    },
                ],
                ..SmartPlaylistData::default()
            },
        ]
    }

    /// Evaluates a smart playlist over `(Track, Album)` entries at `now_ms` (Unix milliseconds).
    pub fn evaluate<'a, FStats, FLiked>(
        playlist: &SmartPlaylistData,
        library: impl IntoIterator<Item = (&'a Track, &'a Album)>,
        stats: FStats,
        liked: FLiked,
        now_ms: i64,
    ) -> Vec<Track>
    where
        FStats: Fn(&str) -> Option<&'a PlayStat>,
        FLiked: Fn(&str) -> bool,
    {
        // Parse every rule once (lowercased needle, parsed number, normalized codec): matching then
        // does no per-track rule work or allocation.
        let rules: Vec<CompiledRule> = playlist
            .rules
            .iter()
            .filter(|r| !Self::needs_value(&r.op) || !r.value.trim().is_empty())
            .map(CompiledRule::compile)
            .collect();

        let mut hits: Vec<(&'a Track, &'a Album)> = Vec::new();
        for (track, album) in library {
            if rules.is_empty() {
                hits.push((track, album));
                continue;
            }
            let ok = if playlist.match_all {
                rules
                    .iter()
                    .all(|r| r.matches(track, album, &stats, &liked, now_ms))
            } else {
                rules
                    .iter()
                    .any(|r| r.matches(track, album, &stats, &liked, now_ms))
            };
            if ok {
                hits.push((track, album));
            }
        }

        // Keys are computed once per hit (`sort_by_cached_key`), not per comparison: a stats lookup or
        // a lowercase copy inside the comparator would run O(N log N) times.
        match playlist.sort.as_str() {
            "added_desc" => hits.sort_by_key(|a| std::cmp::Reverse(a.1.added_at)),
            "plays_desc" => hits.sort_by_cached_key(|(t, _)| {
                let s = stats(&t.id);
                (
                    std::cmp::Reverse(s.map(|s| s.count).unwrap_or(0)),
                    std::cmp::Reverse(s.map(|s| s.last).unwrap_or(0)),
                )
            }),
            "played_desc" => hits.sort_by_cached_key(|(t, _)| {
                std::cmp::Reverse(stats(&t.id).map(|s| s.last).unwrap_or(0))
            }),
            "title" => hits.sort_by_cached_key(|(t, _)| t.title.to_lowercase()),
            "random" => {
                let day = now_ms.div_euclid(86_400_000) as u64;
                hits.sort_by_cached_key(|(t, _)| daily_random_key(&t.id, playlist.id as u64, day));
            }
            _ => {}
        }

        let iter = hits.into_iter().map(|(t, _)| t.clone());
        if playlist.limit > 0 {
            iter.take(playlist.limit).collect()
        } else {
            iter.collect()
        }
    }

    pub fn evaluate_albums<'a, FStats>(
        playlist: &SmartPlaylistData,
        albums: &'a [Album],
        stats: FStats,
        liked_ids: &HashSet<String>,
        now_ms: i64,
    ) -> Vec<Track>
    where
        FStats: Fn(&str) -> Option<&'a PlayStat>,
    {
        let entries = albums
            .iter()
            .flat_map(|a| a.tracks.iter().map(move |t| (t, a)));
        Self::evaluate(
            playlist,
            entries,
            stats,
            |id| liked_ids.contains(id),
            now_ms,
        )
    }
}

fn daily_random_key(track_id: &str, playlist_id: u64, day: u64) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325 ^ playlist_id ^ day.rotate_left(17);
    for b in track_id.to_lowercase().bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    // SplitMix64 finalizer so consecutive paths spread evenly
    h ^= h >> 30;
    h = h.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    h ^= h >> 27;
    h = h.wrapping_mul(0x94d0_49bb_1331_11eb);
    h ^ (h >> 31)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TextOp {
    Contains,
    NotContains,
    Is,
}

#[derive(Debug, Clone, Copy)]
enum NumOp {
    Gte,
    Lte,
    Eq,
}

#[derive(Debug, Clone, Copy)]
enum NumField {
    Bits,
    RateKhz,
    Year,
    DurationMin,
    Plays,
}

/// A [`SmartRule`] parsed once per evaluation.
#[derive(Debug, Clone)]
enum CompiledRule {
    Title(TextOp, String),
    Artist(TextOp, String),
    Album(TextOp, String),
    /// `true` = "is not"; the value is the normalized short codec name.
    Codec { negate: bool, short: String },
    Number { field: NumField, op: NumOp, n: f64, tolerance: f64 },
    PlayedNever,
    /// `inside` = "within" (else "not within"), over the last `days` days.
    Played { inside: bool, days: f64 },
    Added { inside: bool, days: f64 },
    Liked(bool),
    /// Unknown field/op or unparseable value: matches nothing.
    Never,
}

impl CompiledRule {
    fn compile(r: &SmartRule) -> Self {
        let v = r.value.trim();
        let text = |make: fn(TextOp, String) -> Self| match r.op.as_str() {
            "contains" => make(TextOp::Contains, crate::library_view::fold(v)),
            "not_contains" => make(TextOp::NotContains, crate::library_view::fold(v)),
            "is" => make(TextOp::Is, crate::library_view::fold(v)),
            _ => Self::Never,
        };
        let number = |field: NumField, tolerance: f64| {
            let op = match r.op.as_str() {
                "gte" => NumOp::Gte,
                "lte" => NumOp::Lte,
                "eq" => NumOp::Eq,
                _ => return Self::Never,
            };
            match parse_decimal(v) {
                Some(n) => Self::Number { field, op, n, tolerance },
                None => Self::Never,
            }
        };
        let window = |make: fn(bool, f64) -> Self| match parse_decimal(v) {
            Some(days) if days.is_finite() => make(r.op == "within", days),
            _ => Self::Never,
        };
        match r.field.as_str() {
            "title" => text(Self::Title),
            "artist" => text(Self::Artist),
            "album" => text(Self::Album),
            "codec" => Self::Codec {
                negate: r.op == "is_not",
                short: Codecs::short(v, ""),
            },
            "bits" => number(NumField::Bits, 0.001),
            "rate" => number(NumField::RateKhz, 0.05),
            "year" => number(NumField::Year, 0.001),
            "duration" => number(NumField::DurationMin, 0.001),
            "plays" => number(NumField::Plays, 0.001),
            "played" if r.op == "never" => Self::PlayedNever,
            "played" => window(|inside, days| Self::Played { inside, days }),
            "added" => window(|inside, days| Self::Added { inside, days }),
            "liked" => Self::Liked(r.op == "yes"),
            _ => Self::Never,
        }
    }

    fn matches<'a, FStats, FLiked>(&self, t: &Track, a: &Album, stats: &FStats, liked: &FLiked, now_ms: i64) -> bool
    where
        FStats: Fn(&str) -> Option<&'a PlayStat>,
        FLiked: Fn(&str) -> bool,
    {
        match self {
            Self::Title(op, needle) => cmp_text(&t.title, *op, needle),
            Self::Artist(op, needle) => {
                if *op == TextOp::NotContains {
                    cmp_text(&t.artist, *op, needle) && cmp_text(&a.artist, *op, needle)
                } else {
                    cmp_text(&t.artist, *op, needle) || cmp_text(&a.artist, *op, needle)
                }
            }
            Self::Album(op, needle) => cmp_text(&a.name, *op, needle),
            Self::Codec { negate, short } => {
                let same = Codecs::short(&t.codec, &t.file_path).eq_ignore_ascii_case(short);
                same != *negate
            }
            Self::Number { field, op, n, tolerance } => {
                let value = match field {
                    NumField::Bits => t.bits_per_sample as f64,
                    NumField::RateKhz => t.sample_rate as f64 / 1000.0,
                    NumField::Year => a.year as f64,
                    NumField::DurationMin => t.duration / 60.0,
                    NumField::Plays => stats(&t.id).map(|s| s.count).unwrap_or(0) as f64,
                };
                match op {
                    NumOp::Gte => value >= n - tolerance,
                    NumOp::Lte => value <= n + tolerance,
                    NumOp::Eq => (value - n).abs() <= *tolerance,
                }
            }
            Self::PlayedNever => stats(&t.id).map(|s| s.last).unwrap_or(0) == 0,
            Self::Played { inside, days } => {
                within(stats(&t.id).map(|s| s.last).unwrap_or(0), now_ms, *days) == *inside
            }
            Self::Added { inside, days } => within(a.added_at, now_ms, *days) == *inside,
            Self::Liked(want) => liked(&t.id) == *want,
            Self::Never => false,
        }
    }
}

/// `"1,5"` and `"1.5"` both parse (decimal comma from some keyboards).
fn parse_decimal(v: &str) -> Option<f64> {
    if v.contains(',') {
        v.replace(',', ".").parse().ok()
    } else {
        v.parse().ok()
    }
}

/// `needle` is already folded (lowercase).
fn cmp_text(field: &str, op: TextOp, needle: &str) -> bool {
    match op {
        TextOp::Contains => crate::library_view::contains_folded(field, needle),
        TextOp::NotContains => !crate::library_view::contains_folded(field, needle),
        TextOp::Is => {
            let f = field.trim();
            if f.is_ascii() && needle.is_ascii() {
                f.eq_ignore_ascii_case(needle)
            } else {
                crate::library_view::fold(f) == needle
            }
        }
    }
}

/// Whether `when_ms` falls within the last `days` days before `now_ms` (never, when unset).
fn within(when_ms: i64, now_ms: i64, days: f64) -> bool {
    // `saturating_sub`: a bogus `when_ms` (i64::MIN) must not overflow.
    when_ms > 0 && (now_ms.saturating_sub(when_ms) as f64) <= days * 86_400_000.0
}

/// Short format names (`FLAC`, `MP3`, `AAC`...) from codec descriptions or file extensions.
pub struct Codecs;

impl Codecs {
    pub fn short(codec: &str, file_path: &str) -> String {
        if codec.is_empty() {
            let ext = Path::new(file_path)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("");
            return if ext.is_empty() {
                "AUDIO".to_owned()
            } else {
                ext.to_ascii_uppercase()
            };
        }
        let c = codec.to_ascii_uppercase();
        if let Some(name) = crate::models::canonical_codec(&c) {
            return name.to_owned();
        }
        let ext = Path::new(file_path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");
        if !ext.is_empty() {
            return ext.to_ascii_uppercase();
        }
        // Cut at a character boundary: `c[..8]` panics inside a multi-byte character.
        if c.chars().nth(8).is_some() {
            c.chars().take(8).collect::<String>().trim().to_owned()
        } else {
            c
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_track(id: &str, bits: i32, rate: i32, codec: &str) -> (Track, Album) {
        let track = Track {
            id: id.to_owned(),
            title: format!("Track {id}"),
            artist: "Ado".to_owned(),
            album: "Kyogen".to_owned(),
            duration: 210.0,
            file_path: format!("/music/{id}.flac"),
            play_uri: String::new(),
            replay_gain: Default::default(),
            track_number: 1,
            disc_number: 1,
            codec: codec.to_owned(),
            sample_rate: rate,
            bits_per_sample: bits,
            channels: 2,
            cover_thumb_path: None,
            cover_full_path: None,
        };
        let album = Album {
            name: "Kyogen".to_owned(),
            artist: "Ado".to_owned(),
            cover_thumb_path: None,
            cover_full_path: None,
            year: 2022,
            added_at: 1_700_000_000_000,
            tracks: vec![track.clone()],
        };
        (track, album)
    }

    #[test]
    fn evaluates_hires_and_codec_rules() {
        let (t1, a1) = sample_track("1", 16, 44100, "FLAC");
        let (t2, a2) = sample_track("2", 24, 96000, "FLAC");
        let pl = SmartPlaylistData {
            id: 42,
            name: "Hi-Res".to_owned(),
            match_all: true,
            rules: vec![
                SmartRule {
                    field: "bits".to_owned(),
                    op: "gte".to_owned(),
                    value: "24".to_owned(),
                },
                SmartRule {
                    field: "rate".to_owned(),
                    op: "gte".to_owned(),
                    value: "96".to_owned(),
                },
                SmartRule {
                    field: "codec".to_owned(),
                    op: "is".to_owned(),
                    value: "flac".to_owned(),
                },
            ],
            ..SmartPlaylistData::default()
        };
        let hits = SmartPlaylists::evaluate(
            &pl,
            [(&t1, &a1), (&t2, &a2)],
            |_| None,
            |_| false,
            1_700_100_000_000,
        );
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "2");
    }

    fn rule(field: &str, op: &str, value: &str) -> SmartRule {
        SmartRule { field: field.to_owned(), op: op.to_owned(), value: value.to_owned() }
    }

    fn lib_of(n: usize) -> (Vec<Album>, Vec<Track>) {
        let mut albums = Vec::new();
        for i in 0..n {
            let (mut t, mut a) = sample_track(&format!("{i}"), 16, 44100, "FLAC");
            t.title = format!("T{:02}", (n - i) % 7);
            a.tracks = vec![t];
            a.added_at = 1_700_000_000_000 + i as i64;
            albums.push(a);
        }
        let tracks = albums.iter().map(|a| a.tracks[0].clone()).collect();
        (albums, tracks)
    }

    fn run(pl: &SmartPlaylistData, albums: &[Album], stats: &std::collections::HashMap<String, PlayStat>, now: i64) -> Vec<String> {
        SmartPlaylists::evaluate_albums(pl, albums, |id| stats.get(id), &HashSet::new(), now)
            .into_iter()
            .map(|t| t.id)
            .collect()
    }

    #[test]
    fn every_sort_mode_orders_and_limits() {
        let (albums, _) = lib_of(6);
        let mut stats = std::collections::HashMap::new();
        stats.insert("3".to_owned(), PlayStat { count: 9, last: 500 });
        stats.insert("1".to_owned(), PlayStat { count: 9, last: 900 });
        stats.insert("4".to_owned(), PlayStat { count: 2, last: 1_000 });
        let now = 1_700_100_000_000;
        let mk = |sort: &str, limit: usize| SmartPlaylistData {
            id: 7,
            sort: sort.to_owned(),
            limit,
            ..SmartPlaylistData::default()
        };
        // Most played: count desc, ties by most recent; unplayed keep library order after them.
        assert_eq!(run(&mk("plays_desc", 0), &albums, &stats, now), ["1", "3", "4", "0", "2", "5"]);
        assert_eq!(run(&mk("played_desc", 0), &albums, &stats, now), ["4", "1", "3", "0", "2", "5"]);
        assert_eq!(run(&mk("added_desc", 3), &albums, &stats, now), ["5", "4", "3"]);
        // Titles are T00..T06 in a scrambled order; sorted case-insensitively and stable for ties.
        let by_title = run(&mk("title", 0), &albums, &stats, now);
        let titles: Vec<String> = by_title
            .iter()
            .map(|id| albums.iter().find(|a| a.tracks[0].id == *id).unwrap().tracks[0].title.to_lowercase())
            .collect();
        let mut sorted = titles.clone();
        sorted.sort();
        assert_eq!(titles, sorted);
        // Random is deterministic per day + playlist and keeps every hit.
        let r1 = run(&mk("random", 0), &albums, &stats, now);
        let r2 = run(&mk("random", 0), &albums, &stats, now);
        assert_eq!(r1, r2);
        let mut all = r1.clone();
        all.sort();
        assert_eq!(all, ["0", "1", "2", "3", "4", "5"]);
        // Album order (and unknown sort keys) leave the library order alone.
        assert_eq!(run(&mk("album", 0), &albums, &stats, now), ["0", "1", "2", "3", "4", "5"]);
        assert_eq!(run(&mk("bogus", 2), &albums, &stats, now), ["0", "1"]);
    }

    #[test]
    fn sorting_a_big_library_with_stats_is_fast_enough() {
        let (albums, _) = lib_of(3_000);
        let mut stats = std::collections::HashMap::new();
        for i in (0..3_000).step_by(2) {
            stats.insert(format!("{i}"), PlayStat { count: i % 17, last: i64::from(i) });
        }
        let pl = SmartPlaylistData { sort: "plays_desc".to_owned(), ..SmartPlaylistData::default() };
        let out = run(&pl, &albums, &stats, 1_700_100_000_000);
        assert_eq!(out.len(), 3_000);
        let first = &stats[&out[0]];
        assert_eq!(first.count, 16);
    }

    #[test]
    fn text_number_date_and_play_rules_match() {
        let (t, mut a) = sample_track("9", 24, 96000, "FLAC");
        a.added_at = 1_700_000_000_000;
        let now = a.added_at + 10 * 86_400_000;
        let stat = PlayStat { count: 5, last: now - 40 * 86_400_000 };
        let eval = |rules: Vec<SmartRule>, match_all: bool| {
            let pl = SmartPlaylistData { match_all, rules, ..SmartPlaylistData::default() };
            !SmartPlaylists::evaluate(&pl, [(&t, &a)], |_| Some(&stat), |_| true, now).is_empty()
        };
        assert!(eval(vec![rule("title", "contains", "TRACK")], true));
        assert!(eval(vec![rule("artist", "is", "ado")], true));
        assert!(!eval(vec![rule("album", "not_contains", "kyo")], true));
        assert!(eval(vec![rule("codec", "is_not", "mp3")], true));
        assert!(eval(vec![rule("plays", "gte", "5"), rule("plays", "lte", "5")], true));
        assert!(eval(vec![rule("year", "eq", "2022")], true));
        assert!(eval(vec![rule("duration", "gte", "3")], true));
        assert!(eval(vec![rule("added", "within", "30")], true));
        assert!(eval(vec![rule("played", "not_within", "30")], true));
        assert!(!eval(vec![rule("played", "within", "30")], true));
        assert!(!eval(vec![rule("played", "never", "")], true));
        assert!(eval(vec![rule("liked", "yes", "")], true));
        // Any-match mode: one failing rule is enough to keep the track when another passes.
        assert!(eval(vec![rule("title", "is", "nope"), rule("year", "eq", "2022")], false));
        assert!(!eval(vec![rule("title", "is", "nope"), rule("year", "eq", "2022")], true));
    }

    #[test]
    fn within_survives_non_finite_days_and_extreme_timestamps() {
        // Non-finite day counts compile to a rule that matches nothing, "within" or not.
        assert!(matches!(CompiledRule::compile(&rule("added", "within", "NaN")), CompiledRule::Never));
        assert!(matches!(CompiledRule::compile(&rule("played", "not_within", "inf")), CompiledRule::Never));
        assert!(matches!(
            CompiledRule::compile(&rule("added", "within", "1,5")),
            CompiledRule::Added { inside: true, days } if days == 1.5
        ));
        // i64::MIN as a stored timestamp is "never happened", and must not overflow.
        assert!(!within(i64::MIN, i64::MAX, 30.0));
        assert!(within(i64::MAX, i64::MIN, 30.0));
        assert!(within(1_000, 2_000, 1.0));
    }

    #[test]
    fn short_codec_never_panics_on_non_ascii_descriptions() {
        assert_eq!(Codecs::short("あいうえおかきくけこ", ""), "あいうえおかきく");
        assert_eq!(Codecs::short("音楽", ""), "音楽");
        assert_eq!(Codecs::short("Ünïcödé Cödëc Nämé", ""), "ÜNïCöDé");
        assert_eq!(Codecs::short("あいう", "/x/y.xyz"), "XYZ");
        assert_eq!(Codecs::short("", "/x/y.flac"), "FLAC");
        assert_eq!(Codecs::short("", "/x/noext"), "AUDIO");
        assert_eq!(Codecs::short("Apple Lossless", ""), "ALAC");
        assert_eq!(Codecs::short("Windows Media Audio", ""), "WMA");
        // A smart rule with a non-ASCII value goes through the same path.
        let (t, a) = sample_track("1", 16, 44100, "FLAC");
        let pl = SmartPlaylistData {
            rules: vec![rule("codec", "is", "あいうえおかきくけこ")],
            ..SmartPlaylistData::default()
        };
        assert!(SmartPlaylists::evaluate(&pl, [(&t, &a)], |_| None, |_| false, 0).is_empty());
    }
}
