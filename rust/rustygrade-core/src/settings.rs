use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::paths::CoverPaths;

static WRITE_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

/// Persistent settings stored in `~/.rustygrade/native-settings.json` (PascalCase for C# parity).
///
/// Only keys the Android app reads are typed. Desktop-only keys (audio device, tray, window
/// position, GPU/display/performance options, MPRIS, Discord, …) round-trip untouched through
/// `extra`, so a desktop backup restored here and exported again keeps them.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct AppSettings {
    pub volume: f64,
    pub library_sort: String,
    pub replay_gain: String,
    pub library_folders: Vec<String>,
    pub crossfade_seconds: i32,
    pub eq_enabled: bool,
    pub eq_preset: String,
    pub eq_gains: Vec<f64>,
    pub scan_on_startup: bool,
    pub auto_replay_gain: bool,
    pub accent_from_cover: bool,
    pub library_view: String,
    pub artwork_itunes: bool,
    pub artwork_deezer: bool,
    pub artwork_musicbrainz: bool,

    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            volume: 0.5,
            library_sort: "added_desc".to_owned(),
            replay_gain: "no".to_owned(),
            library_folders: Vec::new(),
            crossfade_seconds: 0,
            eq_enabled: false,
            eq_preset: "FLAT".to_owned(),
            eq_gains: Vec::new(),
            scan_on_startup: false,
            auto_replay_gain: false,
            accent_from_cover: true,
            library_view: "albums".to_owned(),
            artwork_itunes: true,
            artwork_deezer: true,
            artwork_musicbrainz: true,
            extra: Map::new(),
        }
    }
}

/// Every `LibrarySort` key the C# build writes (`MainViewModel.cs`), in menu order.
/// `library_sort` itself stays an opaque string so any of these round-trips unchanged.
pub const LIBRARY_SORT_KEYS: [&str; 10] = [
    "added_desc",
    "added_asc",
    "name_asc",
    "name_desc",
    "artist_asc",
    "artist_desc",
    "year_desc",
    "year_asc",
    "played_desc",
    "plays_desc",
];

/// `extra` keys the Android app keeps in `native-settings.json` (device-local: not taken from backups).
pub const KEY_SAF_TREES: &str = "AndroidSafTrees";
pub const KEY_USE_MEDIA_STORE: &str = "AndroidUseMediaStore";
pub const KEY_USB_BIT_PERFECT: &str = "AndroidUsbBitPerfect";

/// Equalizer gains are decibels in `-12..=12`; a non-finite value (which serializes as `null` and
/// breaks the next strict parse) becomes 0 dB.
pub fn sanitize_eq_gains(gains: impl IntoIterator<Item = f64>) -> Vec<f64> {
    gains
        .into_iter()
        .map(|g| if g.is_finite() { g.clamp(-12.0, 12.0) } else { 0.0 })
        .collect()
}

/// `"track"` / `"album"`; anything else is `"no"`.
pub fn normalize_replay_gain(mode: &str) -> &'static str {
    match mode {
        "track" => "track",
        "album" => "album",
        _ => "no",
    }
}

/// `"albums"` / `"artists"`; anything else is `"albums"`.
pub fn normalize_library_view(view: &str) -> &'static str {
    if view == "artists" { "artists" } else { "albums" }
}

/// Maps a stored sort key to one of [`LIBRARY_SORT_KEYS`]; anything unknown reads as the default
/// `added_desc`.
pub fn canonical_library_sort(key: &str) -> &'static str {
    LIBRARY_SORT_KEYS
        .iter()
        .copied()
        .find(|k| *k == key)
        .unwrap_or("added_desc")
}

impl AppSettings {
    /// Parses settings JSON. A field whose value does not fit (an unknown enum value written by
    /// another version, a wrong type) falls back to its default instead of failing the whole file.
    /// Returns `None` only when the text is not a JSON object.
    pub fn parse_lenient(text: &str) -> Option<Self> {
        if let Ok(parsed) = serde_json::from_str::<Self>(text) {
            return Some(parsed);
        }
        let Ok(Value::Object(input)) = serde_json::from_str::<Value>(text) else {
            return None;
        };
        let Ok(Value::Object(mut merged)) = serde_json::to_value(Self::default()) else {
            return None;
        };
        // Every field is independent and unknown keys land in `extra`, so a key is judged on its own:
        // put it alone into a probe object and see whether `Self` still deserializes (the container
        // `default` fills in the rest). O(K) overall, and no value is cloned.
        let mut probe = Value::Object(Map::new());
        for (key, val) in input {
            let Value::Object(slot) = &mut probe else {
                return None;
            };
            slot.insert(key.clone(), val);
            let fits = Self::deserialize(&probe).is_ok();
            let Value::Object(slot) = &mut probe else {
                return None;
            };
            let Some(val) = slot.remove(&key) else {
                continue;
            };
            if fits {
                merged.insert(key, val);
            }
        }
        serde_json::from_value(Value::Object(merged)).ok()
    }

    pub fn file_path() -> PathBuf {
        CoverPaths::app_dir().join("native-settings.json")
    }

    pub fn backup_path() -> PathBuf {
        CoverPaths::app_dir().join("native-settings.json.bak")
    }

    pub fn corrupt_path() -> PathBuf {
        CoverPaths::app_dir().join("native-settings.json.corrupt")
    }

    pub fn load() -> Self {
        let path = Self::file_path();
        if !path.exists() {
            return Self::default();
        }
        if let Some(parsed) = Self::read_lenient(&path) {
            return parsed;
        }
        // Damaged: recovery rewrites the file, so serialise it with `try_save` and look again.
        let _guard = WRITE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(parsed) = Self::read_lenient(&path) {
            return parsed;
        }
        let _ = fs::copy(&path, Self::corrupt_path());
        let backup = Self::backup_path();
        if let Some(parsed) = Self::read_lenient(&backup) {
            let _ = fs::copy(&backup, &path);
            return parsed;
        }
        Self::default()
    }

    fn read_lenient(path: &Path) -> Option<Self> {
        let text = fs::read_to_string(path).ok()?;
        Self::parse_lenient(&text)
    }

    pub fn save(&self) {
        if let Err(e) = self.try_save() {
            crate::error_log::write("settings", &format!("could not save native-settings.json: {e}"));
        }
    }

    pub fn try_save(&self) -> std::io::Result<()> {
        let _guard = WRITE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        fs::create_dir_all(CoverPaths::app_dir())?;
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        crate::fsutil::save_with_backup(&Self::file_path(), &Self::backup_path(), json.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_bad_field_falls_back_without_resetting_the_rest() {
        let json = r#"{
            "Volume": 0.3,
            "GpuPreference": "quantum",
            "PerformanceMode": 7,
            "CrossfadeSeconds": "six",
            "LibraryFolders": ["/music"],
            "LibrarySort": "played_desc",
            "SomethingNew": {"a": 1}
        }"#;
        let s = AppSettings::parse_lenient(json).expect("lenient parse");
        assert_eq!(s.volume, 0.3);
        // Desktop-only keys are untyped here: kept verbatim, whatever their value.
        assert_eq!(s.extra.get("GpuPreference"), Some(&Value::from("quantum")));
        assert_eq!(s.extra.get("PerformanceMode"), Some(&Value::from(7)));
        assert_eq!(s.crossfade_seconds, 0);
        assert_eq!(s.library_folders, vec!["/music".to_owned()]);
        assert_eq!(s.library_sort, "played_desc");
        assert!(s.extra.contains_key("SomethingNew"));
        assert!(AppSettings::parse_lenient("not json").is_none());
        assert!(AppSettings::parse_lenient("[1, 2]").is_none());
    }

    #[test]
    fn csharp_sort_keys_roundtrip_unchanged() {
        // Fixture shaped like a native-settings.json written by the C# build.
        let template = r#"{
  "Volume": 0.5,
  "AudioDevice": "auto",
  "LibrarySort": "KEY",
  "ReplayGain": "no",
  "LibraryFolders": [],
  "MinimizeToTray": null,
  "SidebarVisible": true,
  "CrossfadeSeconds": 0,
  "EqEnabled": false,
  "EqPreset": "FLAT",
  "EqGains": [],
  "WatchFolders": true,
  "AccentFromCover": true,
  "MiniX": null,
  "MiniY": null,
  "LibraryView": "albums"
}"#;
        for key in LIBRARY_SORT_KEYS {
            let s = AppSettings::parse_lenient(&template.replace("KEY", key)).unwrap();
            assert_eq!(s.library_sort, key);
            let back = AppSettings::parse_lenient(&serde_json::to_string(&s).unwrap()).unwrap();
            assert_eq!(back.library_sort, key);
            assert_eq!(canonical_library_sort(key), key);
            // The desktop keys of the fixture survive the round trip through `extra`.
            for desktop_key in ["AudioDevice", "MinimizeToTray", "SidebarVisible", "WatchFolders", "MiniX", "MiniY"] {
                assert_eq!(back.extra.get(desktop_key), s.extra.get(desktop_key), "{desktop_key}");
                assert!(back.extra.contains_key(desktop_key), "{desktop_key}");
            }
        }
        assert_eq!(canonical_library_sort("bogus"), "added_desc");
        assert_eq!(canonical_library_sort("title_asc"), "added_desc");
    }

    #[test]
    fn lenient_parse_with_thousands_of_keys_is_linear() {
        // One wrongly typed known field + 20_000 unknown keys: the old O(K²) merge cloned the whole
        // map for every key.
        let mut obj = serde_json::Map::new();
        for i in 0..20_000 {
            obj.insert(format!("Unknown{i}"), serde_json::json!({"n": i, "pad": "x".repeat(20)}));
        }
        obj.insert("Volume".to_owned(), serde_json::json!("loud"));
        obj.insert("LibrarySort".to_owned(), serde_json::json!("plays_desc"));
        obj.insert("EqGains".to_owned(), serde_json::json!([1.5, "bad"]));
        let text = serde_json::to_string(&obj).unwrap();
        let started = std::time::Instant::now();
        let s = AppSettings::parse_lenient(&text).expect("lenient");
        assert!(started.elapsed() < std::time::Duration::from_secs(5), "{:?}", started.elapsed());
        assert_eq!(s.volume, 0.5, "the bad field falls back to its default");
        assert_eq!(s.library_sort, "plays_desc");
        assert!(s.eq_gains.is_empty(), "a field with a bad element is dropped as a whole");
        assert_eq!(s.extra.len(), 20_000);
    }

    #[test]
    fn lenient_parse_keeps_valid_fields_in_any_key_order() {
        let a = AppSettings::parse_lenient(r#"{"CrossfadeSeconds":"x","Volume":0.9,"EqEnabled":true,"MiniX":"no"}"#).unwrap();
        let b = AppSettings::parse_lenient(r#"{"MiniX":"no","EqEnabled":true,"Volume":0.9,"CrossfadeSeconds":"x"}"#).unwrap();
        for s in [a, b] {
            assert_eq!((s.volume, s.eq_enabled, s.crossfade_seconds), (0.9, true, 0));
            assert_eq!(s.extra.get("MiniX"), Some(&Value::from("no")));
        }
    }

    #[test]
    fn settings_sanitizers() {
        assert_eq!(sanitize_eq_gains([f64::NAN, f64::INFINITY, -99.0, 5.5, 99.0]), vec![0.0, 0.0, -12.0, 5.5, 12.0]);
        assert_eq!(normalize_replay_gain("track"), "track");
        assert_eq!(normalize_replay_gain("album"), "album");
        assert_eq!(normalize_replay_gain("TRACK"), "no");
        assert_eq!(normalize_library_view("artists"), "artists");
        assert_eq!(normalize_library_view("grid"), "albums");
    }

    #[test]
    fn damaged_settings_are_recovered_from_backup_and_save_keeps_a_backup() {
        let _env = crate::testing::TestEnv::new("settings-recover");
        let mut s = AppSettings { volume: 0.2, ..Default::default() };
        s.try_save().unwrap();
        s.volume = 0.7;
        s.try_save().unwrap();
        fs::write(AppSettings::file_path(), "{ broken").unwrap();
        assert_eq!(AppSettings::load().volume, 0.2, "the backup is the previous save");
        assert!(AppSettings::corrupt_path().exists());
        assert_eq!(AppSettings::load().volume, 0.2, "and it was put back as the live file");
        fs::remove_file(AppSettings::backup_path()).unwrap();
        fs::write(AppSettings::file_path(), "[]").unwrap();
        assert_eq!(AppSettings::load().volume, 0.5, "nothing usable: defaults");
    }
}
