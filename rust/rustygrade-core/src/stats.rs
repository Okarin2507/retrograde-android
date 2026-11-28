use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, OnceLock};

use serde::{Deserialize, Serialize};

use crate::paths::CoverPaths;

static WRITE_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));
static RMW_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct PlayStat {
    pub count: i32,
    pub last: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct PlayStats {
    pub tracks: HashMap<String, PlayStat>,
    /// ASCII-lowercase id -> stored key, built on first miss so a lookup of a never-played track is
    /// O(1) instead of a scan of every entry. Paired with `tracks.len()` to notice direct edits of
    /// `tracks` that changed the number of entries; code that swaps keys in place calls
    /// [`Self::invalidate_index`].
    #[serde(skip)]
    ci_index: OnceLock<(usize, HashMap<String, String>)>,
}

impl PlayStats {
    pub fn file_path() -> PathBuf {
        CoverPaths::app_dir().join("native-stats.json")
    }

    pub fn backup_path() -> PathBuf {
        CoverPaths::app_dir().join("native-stats.json.bak")
    }

    pub fn corrupt_path() -> PathBuf {
        CoverPaths::app_dir().join("native-stats.json.corrupt")
    }

    pub fn load() -> Self {
        let path = Self::file_path();
        if !path.exists() {
            return Self::default();
        }
        if let Some(stats) = Self::read_valid(&path) {
            return stats;
        }
        // The file is damaged: recovery rewrites it, so take the write lock (as `try_save` does) and
        // look again, in case a writer replaced the file while we were deciding.
        let _guard = WRITE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(stats) = Self::read_valid(&path) {
            return stats;
        }
        let _ = fs::copy(&path, Self::corrupt_path());
        let backup = Self::backup_path();
        if let Some(stats) = Self::read_valid(&backup) {
            let _ = fs::copy(&backup, &path);
            return stats;
        }
        Self::default()
    }

    fn read_valid(path: &Path) -> Option<Self> {
        let text = fs::read_to_string(path).ok()?;
        serde_json::from_str::<Self>(&text).ok()
    }

    /// Forgets the case-insensitive index; call after editing `tracks` in a way that replaces keys.
    pub fn invalidate_index(&mut self) {
        self.ci_index.take();
    }

    fn ci_lookup(&self, id: &str) -> Option<&String> {
        let len = self.tracks.len();
        let (built_for, index) = self.ci_index.get_or_init(|| (len, build_ci_index(&self.tracks)));
        if *built_for != len {
            // `tracks` was edited directly since the index was built: scan until `invalidate_index()`
            // (stats are reloaded per call, so this is a rare, short-lived state).
            return self.tracks.keys().find(|k| k.eq_ignore_ascii_case(id));
        }
        index.get(&id.to_ascii_lowercase())
    }

    pub fn get(&self, id: &str) -> Option<&PlayStat> {
        if let Some(stat) = self.tracks.get(id) {
            return Some(stat);
        }
        if self.tracks.is_empty() {
            return None;
        }
        self.ci_lookup(id).and_then(|key| self.tracks.get(key))
    }

    pub fn record_now(&mut self, id: &str) {
        self.record_at(id, crate::fsutil::now_ms());
    }

    pub fn record_at(&mut self, id: &str, when_ms: i64) {
        let key = if self.tracks.is_empty() || self.tracks.contains_key(id) {
            id.to_owned()
        } else {
            self.ci_lookup(id).cloned().unwrap_or_else(|| id.to_owned())
        };
        if !self.tracks.contains_key(&key) {
            // A new key: keep the index in step instead of rebuilding it on the next miss.
            let len_before = self.tracks.len();
            if let Some((built_for, index)) = self.ci_index.get_mut()
                && *built_for == len_before
            {
                index.entry(key.to_ascii_lowercase()).or_insert_with(|| key.clone());
                *built_for = len_before + 1;
            }
        }
        let entry = self.tracks.entry(key).or_default();
        entry.count = entry.count.saturating_add(1);
        entry.last = when_ms;
    }

    /// Seconds a track must be heard before it counts as played
    /// (C# `Math.Clamp(d * 0.5, Math.Min(d, 15), 240)`, 30 s when the length is unknown).
    pub fn threshold(duration_secs: f64) -> f64 {
        if !duration_secs.is_finite() || duration_secs <= 0.0 {
            30.0
        } else {
            (duration_secs * 0.5).clamp(duration_secs.min(15.0), 240.0)
        }
    }

    pub fn save(&self) {
        if let Err(e) = self.try_save() {
            crate::error_log::write("stats", &format!("could not save native-stats.json: {e}"));
        }
    }

    pub fn try_save(&self) -> std::io::Result<()> {
        let _guard = WRITE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let dir = CoverPaths::app_dir();
        fs::create_dir_all(&dir)?;
        let json = serde_json::to_string(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        crate::fsutil::save_with_backup(&Self::file_path(), &Self::backup_path(), json.as_bytes())
    }

    /// Runs a serialized read-modify-write cycle on `native-stats.json`.
    pub fn update<R>(f: impl FnOnce(&mut Self) -> R) -> R {
        Self::with_lock(|stats| {
            let out = f(stats);
            stats.save();
            out
        })
    }

    /// Runs `f` with the current `PlayStats` loaded while holding the read-modify-write lock.
    /// Call `stats.save()` inside `f` when modifications should be persisted.
    pub fn with_lock<R>(f: impl FnOnce(&mut Self) -> R) -> R {
        let _guard = RMW_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let mut stats = Self::load();
        f(&mut stats)
    }

    /// Loads `native-stats.json` serialized against a concurrent [`Self::update`].
    pub fn load_locked() -> Self {
        let _guard = RMW_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        Self::load()
    }
}

fn build_ci_index(tracks: &HashMap<String, PlayStat>) -> HashMap<String, String> {
    let mut index = HashMap::with_capacity(tracks.len());
    for k in tracks.keys() {
        index.entry(k.to_ascii_lowercase()).or_insert_with(|| k.clone());
    }
    index
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn threshold_handles_short_and_long_durations() {
        assert_eq!(PlayStats::threshold(0.0), 30.0);
        assert_eq!(PlayStats::threshold(-1.0), 30.0);
        // Short tracks must be heard in full, up to 15 s (C# formula).
        assert_eq!(PlayStats::threshold(2.0), 2.0);
        assert_eq!(PlayStats::threshold(10.0), 10.0);
        assert_eq!(PlayStats::threshold(14.0), 14.0);
        assert_eq!(PlayStats::threshold(20.0), 15.0);
        assert_eq!(PlayStats::threshold(30.0), 15.0);
        assert_eq!(PlayStats::threshold(180.0), 90.0);
        assert_eq!(PlayStats::threshold(480.0), 240.0);
        assert_eq!(PlayStats::threshold(600.0), 240.0);
    }

    #[test]
    fn threshold_ignores_non_finite_durations() {
        assert_eq!(PlayStats::threshold(f64::NAN), 30.0);
        assert_eq!(PlayStats::threshold(f64::INFINITY), 30.0);
        assert_eq!(PlayStats::threshold(f64::NEG_INFINITY), 30.0);
    }

    #[test]
    fn get_and_record_match_ids_case_insensitively_without_scanning() {
        let mut stats = PlayStats::default();
        stats.record_at("/Music/Song.flac", 10);
        // Exact hit, case-insensitive hit and a miss.
        assert_eq!(stats.get("/Music/Song.flac").unwrap().count, 1);
        assert_eq!(stats.get("/music/song.FLAC").unwrap().count, 1);
        assert!(stats.get("/music/other.flac").is_none());
        // Recording under another case bumps the existing entry instead of adding a duplicate.
        stats.record_at("/MUSIC/SONG.FLAC", 20);
        assert_eq!(stats.tracks.len(), 1);
        let e = stats.get("/Music/Song.flac").unwrap();
        assert_eq!((e.count, e.last), (2, 20));
        // A brand-new id is findable straight away (the index is kept in step).
        stats.record_at("/Music/New.flac", 30);
        assert_eq!(stats.get("/music/new.flac").unwrap().last, 30);
        assert_eq!(stats.tracks.len(), 2);
    }

    #[test]
    fn index_survives_direct_edits_of_tracks() {
        let mut stats = PlayStats::default();
        stats.record_at("/a/One.flac", 1);
        assert!(stats.get("/a/one.flac").is_some()); // builds the index
        stats.tracks.insert("/a/Two.flac".to_owned(), PlayStat { count: 5, last: 2 });
        assert_eq!(stats.get("/a/two.flac").unwrap().count, 5, "a changed length is noticed");
        stats.tracks.remove("/a/One.flac");
        stats.tracks.insert("/a/Three.flac".to_owned(), PlayStat { count: 1, last: 3 });
        stats.invalidate_index();
        assert!(stats.get("/a/one.flac").is_none());
        assert!(stats.get("/a/three.flac").is_some());
    }

    #[test]
    fn many_misses_stay_cheap_and_correct() {
        let mut stats = PlayStats::default();
        for i in 0..5_000 {
            stats.record_at(&format!("/m/Track{i}.flac"), i);
        }
        let mut misses = 0;
        for i in 0..20_000 {
            if stats.get(&format!("/m/never{i}.flac")).is_none() {
                misses += 1;
            }
        }
        assert_eq!(misses, 20_000);
        assert_eq!(stats.get("/M/TRACK4999.FLAC").unwrap().last, 4999);
    }

    #[test]
    fn count_saturates_instead_of_overflowing() {
        let mut stats = PlayStats::default();
        stats.tracks.insert("x".to_owned(), PlayStat { count: i32::MAX, last: 0 });
        stats.record_at("x", 5);
        assert_eq!(stats.tracks["x"].count, i32::MAX);
    }

    #[test]
    fn index_is_not_serialized_and_cloning_keeps_working() {
        let mut stats = PlayStats::default();
        stats.record_at("/a/B.flac", 1);
        let _ = stats.get("/a/b.flac");
        let json = serde_json::to_string(&stats).unwrap();
        assert!(!json.contains("ci_index"), "{json}");
        let back: PlayStats = serde_json::from_str(&json).unwrap();
        assert_eq!(back.get("/A/b.FLAC").unwrap().count, 1);
        assert_eq!(stats.clone().get("/a/b.flac").unwrap().count, 1);
    }

    #[test]
    fn damaged_stats_file_is_recovered_from_the_backup() {
        let _env = crate::testing::TestEnv::new("stats-recover");
        let mut stats = PlayStats::default();
        stats.record_at("/a/1.flac", 1);
        stats.try_save().unwrap();
        stats.record_at("/a/1.flac", 2);
        stats.try_save().unwrap(); // the first save is now the backup
        fs::write(PlayStats::file_path(), "{ not json").unwrap();
        let loaded = PlayStats::load();
        assert_eq!(loaded.tracks["/a/1.flac"].count, 1, "the backup is used");
        assert!(PlayStats::corrupt_path().exists(), "the damaged file is kept aside");
        assert_eq!(PlayStats::load().tracks["/a/1.flac"].count, 1, "and the live file was restored");
        // No backup and nothing parseable: start empty rather than failing.
        fs::remove_file(PlayStats::backup_path()).unwrap();
        fs::write(PlayStats::file_path(), "garbage").unwrap();
        assert!(PlayStats::load().tracks.is_empty());
    }
}
