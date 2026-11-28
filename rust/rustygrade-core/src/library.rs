use std::collections::HashMap;

use indexmap::IndexMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};
use thiserror::Error;

use crate::collections::CollectionsStore;
use crate::covers::delete_unreferenced_covers;
use crate::models::{Album, ReplayGain, Track};
use crate::paths::CoverPaths;

#[derive(Debug, Error)]
pub enum LibraryError {
    #[error("I/O error accessing library.json: {0}")]
    Io(#[from] std::io::Error),
    #[error(
        "library.json is damaged and there is no usable backup. It was copied to library.json.corrupt; remove library.json to start a new library."
    )]
    Corrupt,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AlbumDto {
    pub name: Option<String>,
    pub artist: Option<String>,
    pub cover_art: Option<String>,
    pub cover_art_full: Option<String>,
    #[serde(deserialize_with = "de_i32_lenient")]
    pub year: i32,
    #[serde(deserialize_with = "de_i64_lenient")]
    pub added_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub album_artist: Option<String>,
    pub tracks: Option<Vec<TrackDto>>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TrackDto {
    pub id: Option<String>,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    #[serde(deserialize_with = "de_f64_lenient")]
    pub duration: f64,
    pub file_path: Option<String>,
    pub src: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "de_opt_i32_lenient")]
    pub track_number: Option<i32>,
    #[serde(deserialize_with = "de_i32_lenient")]
    pub disc_number: i32,
    pub codec: Option<String>,
    #[serde(deserialize_with = "de_i32_lenient")]
    pub sample_rate: i32,
    #[serde(deserialize_with = "de_i32_lenient")]
    pub bits_per_sample: i32,
    #[serde(deserialize_with = "de_i32_lenient")]
    pub channels: i32,
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "de_opt_i64_lenient")]
    pub mtime: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "de_opt_i64_lenient")]
    pub size: Option<i64>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Keeps the order of the keys in library.json, like the C# dictionary (ties in "Recently Added" depend on it).
pub type LibraryMap = IndexMap<String, AlbumDto>;

static WRITE_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));
static RESTORED_FROM_BACKUP: LazyLock<Mutex<bool>> = LazyLock::new(|| Mutex::new(false));

pub struct LibraryStore;

impl LibraryStore {
    pub fn library_path() -> PathBuf {
        CoverPaths::app_dir().join("library.json")
    }

    pub fn backup_path() -> PathBuf {
        CoverPaths::app_dir().join("library.json.bak")
    }

    pub fn damaged_path() -> PathBuf {
        CoverPaths::app_dir().join("library.json.corrupt")
    }

    pub fn restored_from_backup() -> bool {
        RESTORED_FROM_BACKUP.lock().map(|g| *g).unwrap_or(false)
    }

    pub fn write_lock() -> &'static Mutex<()> {
        &WRITE_LOCK
    }

    pub fn load() -> Result<Vec<Album>, LibraryError> {
        let raw = Self::load_raw()?;
        Ok(raw.into_values().map(map_album).collect())
    }

    /// Reads `library.json`. A damaged file is recovered from the backup under the write lock; do not
    /// call this while holding [`Self::write_lock`] (use [`Self::load_raw_locked`] there).
    pub fn load_raw() -> Result<LibraryMap, LibraryError> {
        let path = Self::library_path();
        if !path.exists() {
            return Ok(LibraryMap::new());
        }
        if let Some(parsed) = Self::read_library(&path) {
            return Ok(parsed);
        }
        // Recovery rewrites `library.json`: serialise it with the writers (and re-read under the lock).
        let _guard = WRITE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        Self::load_raw_locked()
    }

    /// [`Self::load_raw`] for callers that already hold [`Self::write_lock`].
    pub(crate) fn load_raw_locked() -> Result<LibraryMap, LibraryError> {
        let path = Self::library_path();
        if !path.exists() {
            return Ok(LibraryMap::new());
        }
        if let Some(parsed) = Self::read_library(&path) {
            return Ok(parsed);
        }

        let _ = fs::copy(&path, Self::damaged_path());
        let backup = Self::backup_path();
        if let Some(parsed) = Self::read_library(&backup) {
            let _ = fs::copy(&backup, &path);
            if let Ok(mut flag) = RESTORED_FROM_BACKUP.lock() {
                *flag = true;
            }
            return Ok(parsed);
        }
        Err(LibraryError::Corrupt)
    }

    fn read_library(path: &Path) -> Option<LibraryMap> {
        let bytes = fs::read(path).ok()?;
        serde_json::from_slice::<LibraryMap>(&bytes).ok()
    }

    /// Writes `library.json` atomically (keeping the previous one as `.bak`). The caller holds
    /// [`Self::write_lock`]: a load-modify-save cycle must not interleave with another writer.
    pub(crate) fn save_raw(library: &LibraryMap) -> Result<(), LibraryError> {
        fs::create_dir_all(CoverPaths::app_dir())?;
        let bytes = serde_json::to_vec(library).map_err(std::io::Error::other)?;
        crate::fsutil::save_with_backup(&Self::library_path(), &Self::backup_path(), &bytes)?;
        Ok(())
    }

    pub fn delete_albums(names: &[String]) -> Result<usize, LibraryError> {
        let _guard = WRITE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let mut library = Self::load_raw_locked()?;
        let mut removed = 0;
        let mut candidates = Vec::new();
        for name in names {
            if let Some(album) = library.shift_remove(name) {
                removed += 1;
                collect_album_cover_candidates(name, &album, &mut candidates);
            }
        }
        if removed > 0 {
            Self::save_raw(&library)?;
            if !candidates.is_empty() {
                let collections = CollectionsStore::load();
                delete_unreferenced_covers(&candidates, &library, &collections);
            }
        }
        Ok(removed)
    }

    pub fn clear_covers(names: &[String]) -> Result<(), LibraryError> {
        let _guard = WRITE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let mut library = Self::load_raw_locked()?;
        let mut changed = false;
        let mut candidates = Vec::new();
        for name in names {
            if let Some(album) = library.get_mut(name) {
                collect_album_cover_candidates(name, album, &mut candidates);
                album.cover_art = None;
                album.cover_art_full = None;
                changed = true;
            }
        }
        if changed {
            Self::save_raw(&library)?;
            if !candidates.is_empty() {
                let collections = CollectionsStore::load();
                delete_unreferenced_covers(&candidates, &library, &collections);
            }
        }
        Ok(())
    }

    /// Saves `image_bytes` into the content-addressed cover store and assigns it as the cover of
    /// `album_name` in `library.json`, sweeping any unreferenced old cover files.
    pub fn set_album_cover(album_name: &str, image_bytes: &[u8]) -> Result<Option<PathBuf>, LibraryError> {
        let saved = crate::covers::save_cover_bytes(image_bytes)
            .map_err(|e| LibraryError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())))?;
        let url = CoverPaths::to_file_url(&saved);
        let _guard = WRITE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let mut library = Self::load_raw_locked()?;
        let mut candidates = Vec::new();
        let Some(album) = library.get_mut(album_name) else {
            return Ok(None);
        };
        collect_album_cover_candidates(album_name, album, &mut candidates);
        album.cover_art = Some(url.clone());
        album.cover_art_full = Some(url);
        Self::save_raw(&library)?;
        if !candidates.is_empty() {
            let collections = CollectionsStore::load();
            delete_unreferenced_covers(&candidates, &library, &collections);
        }
        Ok(Some(saved))
    }

    /// Persists scanned ReplayGain values into `library.json` for every matching track id.
    /// Returns the number of tracks updated.
    pub fn set_tracks_replay_gain(updates: &[(String, ReplayGain)]) -> Result<usize, LibraryError> {
        if updates.is_empty() {
            return Ok(0);
        }
        let map: HashMap<&str, ReplayGain> = updates.iter().map(|(id, rg)| (id.as_str(), *rg)).collect();
        let _guard = WRITE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let mut library = Self::load_raw_locked()?;
        let mut updated = 0usize;
        for album in library.values_mut() {
            let Some(tracks) = album.tracks.as_mut() else {
                continue;
            };
            for t in tracks {
                let Some(id) = t.id.as_deref().or(t.file_path.as_deref()) else {
                    continue;
                };
                if let Some(&rg) = map.get(id) {
                    if rg.is_empty() {
                        t.extra.remove(REPLAY_GAIN_KEY);
                    } else {
                        let mut obj = Map::new();
                        if let Some(v) = rg.track_gain_db.filter(|x| x.is_finite()) {
                            obj.insert("trackGain".to_owned(), serde_json::json!(v));
                        }
                        if let Some(v) = rg.track_peak.filter(|x| x.is_finite() && *x >= 0.0) {
                            obj.insert("trackPeak".to_owned(), serde_json::json!(v));
                        }
                        if let Some(v) = rg.album_gain_db.filter(|x| x.is_finite()) {
                            obj.insert("albumGain".to_owned(), serde_json::json!(v));
                        }
                        if let Some(v) = rg.album_peak.filter(|x| x.is_finite() && *x >= 0.0) {
                            obj.insert("albumPeak".to_owned(), serde_json::json!(v));
                        }
                        t.extra.insert(REPLAY_GAIN_KEY.to_owned(), Value::Object(obj));
                    }
                    updated += 1;
                }
            }
        }
        if updated > 0 {
            Self::save_raw(&library)?;
        }
        Ok(updated)
    }
}

pub(crate) fn collect_album_cover_candidates(
    album_name: &str,
    album: &AlbumDto,
    out: &mut Vec<PathBuf>,
) {
    if let Some(p) = CoverPaths::to_local_path(album.cover_art.as_deref()) {
        out.push(p);
    }
    if let Some(p) = CoverPaths::to_local_path(album.cover_art_full.as_deref()) {
        out.push(p);
    }
    out.push(CoverPaths::thumb_path(album_name));
    out.push(CoverPaths::full_path(album_name));
}

pub(crate) fn try_delete(path: &Path) {
    if path.exists() {
        let _ = fs::remove_file(path);
    }
}

/// Key under which scanned ReplayGain tags live in a track's `extra` (kept by the desktop app too).
pub const REPLAY_GAIN_KEY: &str = "replayGain";
/// Bumped when the scanner starts reading more tags: older library entries are re-parsed once.
pub const SCAN_SCHEMA_KEY: &str = "scanSchema";
pub const SCAN_SCHEMA: u64 = 1;

fn replay_gain_from_extra(extra: &Map<String, Value>) -> ReplayGain {
    let Some(rg) = extra.get(REPLAY_GAIN_KEY).and_then(Value::as_object) else {
        return ReplayGain::default();
    };
    let get = |k: &str| rg.get(k).and_then(Value::as_f64).map(|v| v as f32);
    ReplayGain {
        track_gain_db: get("trackGain"),
        track_peak: get("trackPeak"),
        album_gain_db: get("albumGain"),
        album_peak: get("albumPeak"),
    }
}

pub fn map_album(a: AlbumDto) -> Album {
    let name = a.name.clone().unwrap_or_else(|| "Unknown Album".to_owned());
    let album_artist = a
        .artist
        .clone()
        .unwrap_or_else(|| "Unknown Artist".to_owned());
    let thumb = CoverPaths::to_local_path(a.cover_art.as_deref());
    let full = CoverPaths::to_local_path(a.cover_art_full.as_deref()).or_else(|| thumb.clone());

    let tracks = a
        .tracks
        .unwrap_or_default()
        .into_iter()
        .filter_map(|t| {
            let file_path = t.file_path?;
            if file_path.is_empty() {
                return None;
            }
            let title = match t.title {
                Some(s) if !s.is_empty() => s,
                _ => Path::new(&file_path)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("Unknown")
                    .to_owned(),
            };
            let id = t.id.unwrap_or_else(|| file_path.clone());
            let replay_gain = replay_gain_from_extra(&t.extra);
            let play_uri = t
                .src
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| CoverPaths::to_file_url(Path::new(&file_path)));
            Some(Track {
                id,
                title,
                artist: t.artist.unwrap_or_else(|| album_artist.clone()),
                album: name.clone(),
                duration: t.duration,
                file_path,
                play_uri,
                replay_gain,
                track_number: t.track_number.unwrap_or(0),
                disc_number: if t.disc_number <= 0 { 1 } else { t.disc_number },
                codec: t.codec.unwrap_or_default(),
                sample_rate: t.sample_rate,
                bits_per_sample: t.bits_per_sample,
                channels: t.channels,
                cover_thumb_path: thumb.clone(),
                cover_full_path: full.clone(),
            })
        })
        .collect();

    Album {
        name,
        artist: album_artist,
        cover_thumb_path: thumb,
        cover_full_path: full,
        year: a.year,
        added_at: a.added_at,
        tracks,
    }
}

/// Track id -> `(album index, track index)` into the album slice it was built from. Positions, not
/// clones: an album owns all its tracks, so cloning `(Track, Album)` per track was O(K²) per album.
pub type TrackIndex = HashMap<String, (usize, usize)>;

pub fn build_track_index(albums: &[Album]) -> TrackIndex {
    let mut map = HashMap::with_capacity(albums.iter().map(|a| a.tracks.len()).sum());
    for (ai, album) in albums.iter().enumerate() {
        for (ti, track) in album.tracks.iter().enumerate() {
            map.insert(track.id.clone(), (ai, ti));
        }
    }
    map
}

/// A [`TrackIndex`] bound to the albums it indexes.
#[derive(Clone, Copy)]
pub struct TrackLookup<'a> {
    albums: &'a [Album],
    index: &'a TrackIndex,
}

impl<'a> TrackLookup<'a> {
    pub fn new(albums: &'a [Album], index: &'a TrackIndex) -> Self {
        Self { albums, index }
    }

    pub fn get(&self, id: &str) -> Option<&'a Track> {
        let &(ai, ti) = self.index.get(id)?;
        self.albums.get(ai)?.tracks.get(ti)
    }

    pub fn contains(&self, id: &str) -> bool {
        self.index.contains_key(id)
    }
}

fn de_i32_lenient<'de, D: Deserializer<'de>>(d: D) -> Result<i32, D::Error> {
    Ok(de_opt_i32_lenient(d)?.unwrap_or(0))
}

fn de_opt_i32_lenient<'de, D: Deserializer<'de>>(d: D) -> Result<Option<i32>, D::Error> {
    let v = Option::<Value>::deserialize(d)?;
    Ok(match v {
        Some(Value::Number(n)) => n.as_i64().map(|x| x as i32).or_else(|| n.as_f64().map(|x| x as i32)),
        Some(Value::String(s)) => s.trim().parse::<i32>().ok(),
        _ => None,
    })
}

fn de_i64_lenient<'de, D: Deserializer<'de>>(d: D) -> Result<i64, D::Error> {
    Ok(de_opt_i64_lenient(d)?.unwrap_or(0))
}

fn de_opt_i64_lenient<'de, D: Deserializer<'de>>(d: D) -> Result<Option<i64>, D::Error> {
    let v = Option::<Value>::deserialize(d)?;
    Ok(match v {
        Some(Value::Number(n)) => n.as_i64().or_else(|| n.as_f64().map(|x| x as i64)),
        Some(Value::String(s)) => s.trim().parse::<i64>().ok(),
        _ => None,
    })
}

fn de_f64_lenient<'de, D: Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
    let v = Option::<Value>::deserialize(d)?;
    Ok(match v {
        Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0),
        Some(Value::String(s)) => s.trim().parse::<f64>().unwrap_or(0.0),
        _ => 0.0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TestEnv;

    fn track(id: &str, album: &str) -> Track {
        Track {
            id: id.to_owned(),
            title: id.to_owned(),
            artist: "A".to_owned(),
            album: album.to_owned(),
            duration: 1.0,
            file_path: id.to_owned(),
            play_uri: String::new(),
            replay_gain: ReplayGain::default(),
            track_number: 0,
            disc_number: 1,
            codec: String::new(),
            sample_rate: 0,
            bits_per_sample: 0,
            channels: 0,
            cover_thumb_path: None,
            cover_full_path: None,
        }
    }

    fn album(name: &str, ids: &[String]) -> Album {
        Album {
            name: name.to_owned(),
            artist: "A".to_owned(),
            cover_thumb_path: None,
            cover_full_path: None,
            year: 0,
            added_at: 0,
            tracks: ids.iter().map(|i| track(i, name)).collect(),
        }
    }

    #[test]
    fn track_index_maps_ids_to_positions() {
        let albums = vec![
            album("One", &["/a/1".to_owned(), "/a/2".to_owned()]),
            album("Two", &["/b/1".to_owned()]),
        ];
        let index = build_track_index(&albums);
        assert_eq!(index.len(), 3);
        assert_eq!(index["/b/1"], (1, 0));
        let lookup = TrackLookup::new(&albums, &index);
        assert_eq!(lookup.get("/a/2").unwrap().album, "One");
        assert!(lookup.get("/nope").is_none());
        assert!(lookup.contains("/a/1") && !lookup.contains("/zzz"));
    }

    #[test]
    fn track_index_scales_linearly_with_a_huge_album() {
        // 20_000 tracks in one album: the old (Track, Album) index would clone 4e8 tracks.
        let ids: Vec<String> = (0..20_000).map(|i| format!("/big/{i}.flac")).collect();
        let albums = vec![album("Big", &ids)];
        let index = build_track_index(&albums);
        assert_eq!(index.len(), 20_000);
        let lookup = TrackLookup::new(&albums, &index);
        assert_eq!(lookup.get("/big/19999.flac").unwrap().title, "/big/19999.flac");
    }

    fn library_json(name: &str) -> String {
        format!(r#"{{"{name}":{{"name":"{name}","artist":"X","tracks":[{{"id":"/m/{name}.flac","filePath":"/m/{name}.flac"}}]}}}}"#)
    }

    #[test]
    fn damaged_library_is_restored_from_the_backup_and_flagged() {
        let _env = TestEnv::new("library-recover");
        let dir = CoverPaths::app_dir();
        fs::write(LibraryStore::backup_path(), library_json("Saved")).unwrap();
        fs::write(LibraryStore::library_path(), "{ truncated").unwrap();
        let albums = LibraryStore::load().unwrap();
        assert_eq!(albums.len(), 1);
        assert_eq!(albums[0].name, "Saved");
        assert!(LibraryStore::damaged_path().exists());
        assert!(LibraryStore::restored_from_backup());
        // The live file is valid again.
        assert!(serde_json::from_slice::<LibraryMap>(&fs::read(dir.join("library.json")).unwrap()).is_ok());
    }

    #[test]
    fn damaged_library_without_a_backup_is_an_error_and_is_kept_aside() {
        let _env = TestEnv::new("library-corrupt");
        fs::write(LibraryStore::library_path(), "not json").unwrap();
        assert!(matches!(LibraryStore::load_raw(), Err(LibraryError::Corrupt)));
        assert_eq!(fs::read_to_string(LibraryStore::damaged_path()).unwrap(), "not json");
        // A missing library is just empty.
        fs::remove_file(LibraryStore::library_path()).unwrap();
        assert!(LibraryStore::load_raw().unwrap().is_empty());
    }

    #[test]
    fn save_raw_keeps_the_previous_version_as_backup() {
        let _env = TestEnv::new("library-save");
        let _g = LibraryStore::write_lock().lock().unwrap_or_else(|p| p.into_inner());
        let first: LibraryMap = serde_json::from_str(&library_json("First")).unwrap();
        let second: LibraryMap = serde_json::from_str(&library_json("Second")).unwrap();
        LibraryStore::save_raw(&first).unwrap();
        LibraryStore::save_raw(&second).unwrap();
        assert!(LibraryStore::load_raw_locked().unwrap().contains_key("Second"));
        let backup: LibraryMap = serde_json::from_slice(&fs::read(LibraryStore::backup_path()).unwrap()).unwrap();
        assert!(backup.contains_key("First"));
    }

    #[test]
    fn delete_albums_takes_the_lock_once_and_removes_entries() {
        let _env = TestEnv::new("library-delete");
        {
            let _g = LibraryStore::write_lock().lock().unwrap_or_else(|p| p.into_inner());
            let lib: LibraryMap = serde_json::from_str(&library_json("Gone")).unwrap();
            LibraryStore::save_raw(&lib).unwrap();
        }
        assert_eq!(LibraryStore::delete_albums(&["Gone".to_owned(), "Nope".to_owned()]).unwrap(), 1);
        assert!(LibraryStore::load().unwrap().is_empty());
    }
}
