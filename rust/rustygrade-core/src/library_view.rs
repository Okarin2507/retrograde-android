use std::cmp::Reverse;
use std::collections::HashMap;

use crate::{
    Album, ArtistSummary, CollectionsData, LIBRARY_SORT_KEYS, PlayStats, SmartPlaylists,
    canonical_library_sort,
};

/// The library sort of the original (`MainViewModel.SortOptions`): the key it stores and the menu label.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LibrarySort {
    #[default]
    AddedDesc,
    AddedAsc,
    NameAsc,
    NameDesc,
    ArtistAsc,
    ArtistDesc,
    YearDesc,
    YearAsc,
    PlayedDesc,
    PlaysDesc,
}

impl LibrarySort {
    /// Menu order, as the C# build lists them.
    pub const ALL: [Self; 10] = [
        Self::AddedDesc,
        Self::AddedAsc,
        Self::NameAsc,
        Self::NameDesc,
        Self::ArtistAsc,
        Self::ArtistDesc,
        Self::YearDesc,
        Self::YearAsc,
        Self::PlayedDesc,
        Self::PlaysDesc,
    ];

    pub fn as_str(self) -> &'static str {
        LIBRARY_SORT_KEYS[Self::ALL.iter().position(|s| *s == self).unwrap_or(0)]
    }

    /// Reads a stored key (old Rust keys and unknown values included, see `canonical_library_sort`).
    pub fn from_key(s: &str) -> Self {
        let key = canonical_library_sort(s);
        Self::ALL
            .iter()
            .copied()
            .find(|v| v.as_str() == key)
            .unwrap_or_default()
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::AddedDesc => "RECENTLY ADDED",
            Self::AddedAsc => "OLDEST ADDED",
            Self::NameAsc => "NAME [A-Z]",
            Self::NameDesc => "NAME [Z-A]",
            Self::ArtistAsc => "ARTIST [A-Z]",
            Self::ArtistDesc => "ARTIST [Z-A]",
            Self::YearDesc => "NEWEST",
            Self::YearAsc => "OLDEST",
            Self::PlayedDesc => "RECENTLY PLAYED",
            Self::PlaysDesc => "MOST PLAYED",
        }
    }

    pub fn uses_stats(self) -> bool {
        matches!(self, Self::PlayedDesc | Self::PlaysDesc)
    }
}

/// Case-insensitive text key (Unicode lowercase), used for every comparison and match in the library.
pub fn fold(s: &str) -> String {
    s.to_lowercase()
}

/// Whether `haystack` contains `folded_needle` (already [`fold`]ed), ignoring case. ASCII text — the
/// common case — is matched in place without allocating; anything else falls back to [`fold`].
pub fn contains_folded(haystack: &str, folded_needle: &str) -> bool {
    if folded_needle.is_empty() {
        return true;
    }
    if haystack.is_ascii() && folded_needle.is_ascii() {
        let (h, n) = (haystack.as_bytes(), folded_needle.as_bytes());
        return h.len() >= n.len() && h.windows(n.len()).any(|w| w.eq_ignore_ascii_case(n));
    }
    fold(haystack).contains(folded_needle)
}

/// The artist key an artist page is opened with.
#[cfg(test)]
pub fn artist_key(name: &str) -> String {
    let trimmed = name.trim();
    fold(if trimmed.is_empty() { "Unknown Artist" } else { trimmed })
}

/// Last play time and total plays of an album's tracks.
fn album_plays(album: &Album, stats: &PlayStats) -> (i64, i64) {
    let mut last = 0;
    let mut count = 0;
    for t in &album.tracks {
        if let Some(s) = stats.get(&t.id) {
            last = last.max(s.last);
            count += i64::from(s.count);
        }
    }
    (last, count)
}

/// `(last played, plays)` of every album, in album order; only built for the stats-based sorts.
fn plays_table(albums: &[Album], stats: &PlayStats) -> Vec<(i64, i64)> {
    albums.iter().map(|a| album_plays(a, stats)).collect()
}

/// The library grid order: every album, sorted like the original's `RebuildLibrary`. The sort is stable, so
/// albums that tie keep the order they were loaded in.
#[cfg(test)]
pub fn sort_albums(albums: &[Album], sort: LibrarySort, stats: &PlayStats) -> Vec<usize> {
    let plays = sort.uses_stats().then(|| plays_table(albums, stats));
    sort_albums_by(albums, sort, plays.as_deref())
}

/// `plays` is the [`plays_table`] when `sort.uses_stats()`.
fn sort_albums_by(albums: &[Album], sort: LibrarySort, plays: Option<&[(i64, i64)]>) -> Vec<usize> {
    let played = |i: usize| plays.and_then(|p| p.get(i).copied()).unwrap_or((0, 0));
    let mut idx: Vec<usize> = (0..albums.len()).collect();
    match sort {
        LibrarySort::AddedDesc => idx.sort_by_key(|&i| Reverse(albums[i].added_at)),
        LibrarySort::AddedAsc => idx.sort_by_key(|&i| albums[i].added_at),
        LibrarySort::NameAsc => idx.sort_by_cached_key(|&i| fold(&albums[i].name)),
        LibrarySort::NameDesc => idx.sort_by_cached_key(|&i| Reverse(fold(&albums[i].name))),
        LibrarySort::ArtistAsc => idx.sort_by_cached_key(|&i| fold(&albums[i].artist)),
        LibrarySort::ArtistDesc => idx.sort_by_cached_key(|&i| Reverse(fold(&albums[i].artist))),
        LibrarySort::YearDesc => idx.sort_by_key(|&i| Reverse(albums[i].year)),
        LibrarySort::YearAsc => idx.sort_by_key(|&i| albums[i].year),
        // Albums never played keep their added order at the end
        LibrarySort::PlayedDesc => idx.sort_by_cached_key(|&i| {
            let (last, _) = played(i);
            (Reverse(last), Reverse(albums[i].added_at))
        }),
        LibrarySort::PlaysDesc => idx.sort_by_cached_key(|&i| {
            let (last, count) = played(i);
            (Reverse(count), Reverse(last), Reverse(albums[i].added_at))
        }),
    }
    idx
}

/// The artists in the library sort (`SortedArtists` of the original): names alphabetically, dates by the
/// artist's newest album. `albums` is the slice the artists were grouped from.
#[cfg(test)]
pub fn sort_artists(artists: &[ArtistSummary], albums: &[Album], sort: LibrarySort, stats: &PlayStats) -> Vec<usize> {
    let plays = sort.uses_stats().then(|| plays_table(albums, stats));
    sort_artists_by(artists, albums, sort, plays.as_deref())
}

fn sort_artists_by(
    artists: &[ArtistSummary],
    albums: &[Album],
    sort: LibrarySort,
    plays: Option<&[(i64, i64)]>,
) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..artists.len()).collect();
    let plays_of = |a: &ArtistSummary| {
        a.album_indices
            .iter()
            .map(|&ai| plays.and_then(|p| p.get(ai).copied()).unwrap_or((0, 0)))
            .fold((0i64, 0i64), |(l, c), (al, ac)| (l.max(al), c + ac))
    };
    match sort {
        LibrarySort::NameAsc | LibrarySort::ArtistAsc => idx.sort_by_cached_key(|&i| fold(&artists[i].name)),
        LibrarySort::NameDesc | LibrarySort::ArtistDesc => {
            idx.sort_by_cached_key(|&i| Reverse(fold(&artists[i].name)))
        }
        // Keys walk each artist's albums: compute them once per artist, not once per comparison.
        LibrarySort::AddedAsc => idx.sort_by_cached_key(|&i| {
            artists[i].albums(albums).map(|a| a.added_at).min().unwrap_or(0)
        }),
        LibrarySort::YearDesc => {
            idx.sort_by_cached_key(|&i| Reverse(artists[i].albums(albums).map(|a| a.year).max().unwrap_or(0)))
        }
        LibrarySort::YearAsc => idx.sort_by_cached_key(|&i| {
            artists[i].albums(albums).map(|a| a.year).filter(|y| *y > 0).min().unwrap_or(i32::MAX)
        }),
        LibrarySort::PlayedDesc => {
            idx.sort_by_cached_key(|&i| (Reverse(plays_of(&artists[i]).0), fold(&artists[i].name)))
        }
        LibrarySort::PlaysDesc => {
            idx.sort_by_cached_key(|&i| (Reverse(plays_of(&artists[i]).1), fold(&artists[i].name)))
        }
        LibrarySort::AddedDesc => idx.sort_by_key(|&i| Reverse(artists[i].newest_added_at)),
    }
    idx
}

/// Both library orders (albums, artists) from one pass over the play stats: the stats-based sorts need
/// every album's plays, which are computed once and shared.
pub fn sort_library(
    albums: &[Album],
    artists: &[ArtistSummary],
    sort: LibrarySort,
    stats: &PlayStats,
) -> (Vec<usize>, Vec<usize>) {
    let plays = sort.uses_stats().then(|| plays_table(albums, stats));
    (
        sort_albums_by(albums, sort, plays.as_deref()),
        sort_artists_by(artists, albums, sort, plays.as_deref()),
    )
}

/// Groups the albums by album artist, in the order the artists first appear in the library. Each artist's
/// albums (as indices into `albums`: nothing is copied) are newest first (year, then added).
pub fn group_artists(albums: &[Album]) -> Vec<ArtistSummary> {
    let mut by_key: HashMap<String, usize> = HashMap::new();
    let mut artists: Vec<ArtistSummary> = Vec::new();
    for (ai, album) in albums.iter().enumerate() {
        let display_name = if album.artist.trim().is_empty() {
            "Unknown Artist".to_owned()
        } else {
            album.artist.trim().to_owned()
        };
        let i = *by_key.entry(fold(&display_name)).or_insert_with(|| {
            artists.push(ArtistSummary {
                name: display_name,
                album_indices: Vec::new(),
                track_count: 0,
                newest_added_at: 0,
            });
            artists.len() - 1
        });
        let entry = &mut artists[i];
        entry.track_count += album.tracks.len();
        entry.newest_added_at = entry.newest_added_at.max(album.added_at);
        entry.album_indices.push(ai);
    }
    for artist in &mut artists {
        artist
            .album_indices
            .sort_by_key(|&ai| (Reverse(albums[ai].year), Reverse(albums[ai].added_at)));
    }
    artists
}

/// SEARCH, ALBUMS: albums whose name or artist contains the query (already folded).
pub fn search_albums(albums: &[Album], q: &str) -> Vec<usize> {
    albums
        .iter()
        .enumerate()
        .filter(|(_, a)| contains_folded(&a.name, q) || contains_folded(&a.artist, q))
        .map(|(i, _)| i)
        .collect()
}

/// SEARCH, ARTISTS: artists whose name contains the query (already folded), by name.
pub fn search_artists(artists: &[ArtistSummary], q: &str) -> Vec<usize> {
    let mut idx: Vec<usize> = artists
        .iter()
        .enumerate()
        .filter(|(_, a)| contains_folded(&a.name, q))
        .map(|(i, _)| i)
        .collect();
    idx.sort_by_cached_key(|&i| fold(&artists[i].name));
    idx
}

pub fn ensure_default_smart_playlists(collections: &mut CollectionsData) -> bool {
    if collections.smart.is_none() {
        collections.smart = Some(SmartPlaylists::examples());
        true
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Track;

    fn mk_album(name: &str, artist: &str, year: i32, added_at: i64, track_title: &str) -> Album {
        Album {
            name: name.to_owned(),
            artist: artist.to_owned(),
            cover_thumb_path: None,
            cover_full_path: None,
            year,
            added_at,
            tracks: vec![Track {
                id: format!("/m/{name}/01.flac"),
                title: track_title.to_owned(),
                artist: artist.to_owned(),
                album: name.to_owned(),
                duration: 180.0,
                file_path: format!("/m/{name}/01.flac"),
                play_uri: String::new(),
                replay_gain: Default::default(),
                track_number: 1,
                disc_number: 1,
                codec: "FLAC".to_owned(),
                sample_rate: 44100,
                bits_per_sample: 16,
                channels: 2,
                cover_thumb_path: None,
                cover_full_path: None,
            }],
        }
    }

    #[test]
    fn sorts_searches_and_groups_artists() {
        let albums = vec![
            mk_album("Zebra", "Ado", 2024, 10, "Show"),
            mk_album("alpha", "ado", 2022, 30, "Usseewa"),
            mk_album("Jazz", "Miles", 1959, 20, "So What"),
        ];
        let stats = PlayStats::default();

        assert_eq!(sort_albums(&albums, LibrarySort::AddedDesc, &stats), vec![1, 2, 0]);
        assert_eq!(sort_albums(&albums, LibrarySort::NameAsc, &stats), vec![1, 2, 0]);
        assert_eq!(sort_albums(&albums, LibrarySort::NameDesc, &stats), vec![0, 2, 1]);
        assert_eq!(search_albums(&albums, "jaz"), vec![2]);

        let artists = group_artists(&albums);
        assert_eq!(artists.len(), 2);
        assert_eq!(artists[0].name, "Ado");
        assert_eq!(artists[0].track_count, 2);
        assert_eq!(artists[0].newest_added_at, 30);
        // Newest album first (year, then added): Zebra 2024 before alpha 2022.
        assert_eq!(artists[0].album_indices, vec![0, 1]);
        assert_eq!(search_artists(&artists, "miles"), vec![1]);
        assert_eq!(sort_artists(&artists, &albums, LibrarySort::NameDesc, &stats), vec![1, 0]);
    }

    #[test]
    fn ties_keep_load_order() {
        let albums = vec![
            mk_album("C", "X", 2000, 5, "a"),
            mk_album("A", "Y", 2000, 5, "b"),
            mk_album("B", "Z", 2000, 5, "c"),
        ];
        let stats = PlayStats::default();
        for sort in LibrarySort::ALL {
            if matches!(sort, LibrarySort::NameAsc | LibrarySort::NameDesc | LibrarySort::ArtistAsc | LibrarySort::ArtistDesc) {
                continue;
            }
            assert_eq!(sort_albums(&albums, sort, &stats), vec![0, 1, 2], "{sort:?}");
        }
    }

    #[test]
    fn unicode_case_folds_and_sort_keys_roundtrip() {
        let albums = vec![mk_album("Élan", "Äther", 2001, 1, "x"), mk_album("b", "äther", 2002, 2, "y")];
        assert_eq!(group_artists(&albums).len(), 1);
        assert_eq!(search_albums(&albums, &fold("ÉLAN")), vec![0]);
        for s in LibrarySort::ALL {
            assert_eq!(LibrarySort::from_key(s.as_str()), s);
        }
        assert_eq!(LibrarySort::from_key("title_asc"), LibrarySort::AddedDesc, "no legacy alias");
        assert_eq!(LibrarySort::from_key("bogus"), LibrarySort::AddedDesc);
    }

    #[test]
    fn initializes_default_smart_playlists_only_once() {
        let mut c = CollectionsData::default();
        assert!(ensure_default_smart_playlists(&mut c));
        assert_eq!(c.smart.as_ref().unwrap().len(), 4);
        assert!(!ensure_default_smart_playlists(&mut c));
    }

    #[test]
    fn stats_sorts_share_one_pass_and_agree_with_the_single_sorts() {
        let albums = vec![
            mk_album("One", "A1", 2000, 1, "t1"),
            mk_album("Two", "A2", 2001, 2, "t2"),
            mk_album("Three", "A1", 2002, 3, "t3"),
        ];
        let mut stats = PlayStats::default();
        stats.record_at("/m/Two/01.flac", 500);
        stats.record_at("/m/Two/01.flac", 600);
        stats.record_at("/m/Three/01.flac", 900);
        let artists = group_artists(&albums);
        for sort in LibrarySort::ALL {
            let (al, ar) = sort_library(&albums, &artists, sort, &stats);
            assert_eq!(al, sort_albums(&albums, sort, &stats), "{sort:?}");
            assert_eq!(ar, sort_artists(&artists, &albums, sort, &stats), "{sort:?}");
        }
        let (al, ar) = sort_library(&albums, &artists, LibrarySort::PlaysDesc, &stats);
        assert_eq!(al, vec![1, 2, 0]);
        assert_eq!(ar, vec![1, 0], "A2: 2 plays; A1: 1 play");
        let (al, _) = sort_library(&albums, &artists, LibrarySort::PlayedDesc, &stats);
        assert_eq!(al, vec![2, 1, 0]);
    }

    #[test]
    fn grouping_a_huge_album_copies_no_tracks() {
        let mut big = mk_album("Big", "VA", 2020, 1, "t");
        let first = big.tracks[0].clone();
        big.tracks = (0..5_000).map(|i| Track { id: format!("/b/{i}"), ..first.clone() }).collect();
        let albums = vec![big];
        let artists = group_artists(&albums);
        assert_eq!(artists[0].track_count, 5_000);
        assert_eq!(artists[0].albums(&albums).count(), 1);
        assert_eq!(artists[0].albums(&albums).next().unwrap().tracks.len(), 5_000);
    }

    #[test]
    fn blank_artists_group_under_unknown() {
        let albums = vec![mk_album("X", "  ", 0, 1, "t"), mk_album("Y", "", 0, 2, "t")];
        let artists = group_artists(&albums);
        assert_eq!(artists.len(), 1);
        assert_eq!(artists[0].name, "Unknown Artist");
        assert_eq!(artist_key("  "), "unknown artist");
    }
}
