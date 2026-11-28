use std::fs;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};

use crate::paths::CoverPaths;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PlaylistData {
    pub id: i64,
    pub name: String,
    pub cover_art: Option<String>,
    pub track_ids: Vec<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SmartRule {
    pub field: String,
    pub op: String,
    #[serde(deserialize_with = "de_string_lenient")]
    pub value: String,
}

impl Default for SmartRule {
    fn default() -> Self {
        Self {
            field: "plays".to_owned(),
            op: "gte".to_owned(),
            value: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SmartPlaylistData {
    pub id: i64,
    pub name: String,
    pub match_all: bool,
    pub rules: Vec<SmartRule>,
    pub sort: String,
    #[serde(deserialize_with = "de_usize_lenient")]
    pub limit: usize,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for SmartPlaylistData {
    fn default() -> Self {
        Self {
            id: 0,
            name: String::new(),
            match_all: true,
            rules: Vec::new(),
            sort: "album".to_owned(),
            limit: 0,
            extra: Map::new(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CollectionsData {
    pub liked: Vec<String>,
    pub playlists: Vec<PlaylistData>,
    pub smart: Option<Vec<SmartPlaylistData>>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

static WRITE_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

pub struct CollectionsStore;

impl CollectionsStore {
    pub fn file_path() -> PathBuf {
        CoverPaths::app_dir().join("native-collections.json")
    }

    pub fn backup_path() -> PathBuf {
        CoverPaths::app_dir().join("native-collections.json.bak")
    }

    pub fn load() -> CollectionsData {
        Self::load_existing().unwrap_or_default()
    }

    /// The saved file, else its `.bak` when the file is unreadable; `None` when neither reads.
    fn load_existing() -> Option<CollectionsData> {
        let path = Self::file_path();
        if !path.exists() {
            return None;
        }
        if let Some(data) = Self::try_read(&path) {
            return Some(data);
        }
        // Damaged: recovery writes files, so take the write lock (as `try_save` does) and look again.
        let _guard = WRITE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(data) = Self::try_read(&path) {
            return Some(data);
        }
        let corrupt = CoverPaths::app_dir().join("native-collections.json.corrupt");
        let _ = fs::copy(&path, corrupt);
        Self::try_read(&Self::backup_path())
    }

    fn try_read(path: &std::path::Path) -> Option<CollectionsData> {
        let text = fs::read_to_string(path).ok()?;
        serde_json::from_str::<CollectionsData>(&text).ok()
    }

    pub fn save(data: &CollectionsData) {
        if let Err(e) = Self::try_save(data) {
            crate::error_log::write("collections", &format!("could not save native-collections.json: {e}"));
        }
    }

    pub fn try_save(data: &CollectionsData) -> std::io::Result<()> {
        let _guard = WRITE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        fs::create_dir_all(CoverPaths::app_dir())?;
        let json = serde_json::to_string_pretty(data)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        crate::fsutil::save_with_backup(&Self::file_path(), &Self::backup_path(), json.as_bytes())
    }
}

fn de_string_lenient<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    let v = Option::<Value>::deserialize(d)?;
    Ok(match v {
        Some(Value::String(s)) => s,
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        _ => String::new(),
    })
}

fn de_usize_lenient<'de, D: Deserializer<'de>>(d: D) -> Result<usize, D::Error> {
    let v = Option::<Value>::deserialize(d)?;
    Ok(match v {
        // `try_from`: on a 32-bit target a huge JSON number must saturate, not wrap to a small limit.
        Some(Value::Number(n)) => n
            .as_u64()
            .map(|x| usize::try_from(x).unwrap_or(usize::MAX))
            .or_else(|| n.as_i64().filter(|&x| x > 0).map(|x| usize::try_from(x).unwrap_or(usize::MAX)))
            .unwrap_or(0),
        Some(Value::String(s)) => s
            .trim()
            .parse::<i64>()
            .ok()
            .filter(|&x| x > 0)
            .map(|x| usize::try_from(x).unwrap_or(usize::MAX))
            .unwrap_or(0),
        _ => 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_lenient_smart_rule_value_and_negative_limit() {
        let json = r#"{
            "smart": [{
                "id": 1,
                "name": "Test",
                "limit": -1,
                "rules": [{"field": "year", "op": "gte", "value": 2020}]
            }]
        }"#;
        let data: CollectionsData = serde_json::from_str(json).unwrap();
        let sp = &data.smart.unwrap()[0];
        assert_eq!(sp.limit, 0);
        assert_eq!(sp.rules[0].value, "2020");
    }

    #[test]
    fn huge_limits_saturate_instead_of_wrapping() {
        let data: CollectionsData =
            serde_json::from_str(r#"{"smart":[{"id":1,"limit":18446744073709551615},{"id":2,"limit":"9000000000"},{"id":3,"limit":"-4"}]}"#)
                .unwrap();
        let smart = data.smart.unwrap();
        assert_eq!(smart[0].limit, usize::try_from(u64::MAX).unwrap_or(usize::MAX));
        assert_eq!(smart[1].limit, usize::try_from(9_000_000_000u64).unwrap_or(usize::MAX));
        assert_eq!(smart[2].limit, 0);
    }

    #[test]
    fn damaged_collections_come_back_from_the_backup() {
        let _env = crate::testing::TestEnv::new("collections-recover");
        let mut d = CollectionsData { liked: vec!["/a".to_owned()], ..Default::default() };
        CollectionsStore::try_save(&d).unwrap();
        d.liked.push("/b".to_owned());
        CollectionsStore::try_save(&d).unwrap();
        fs::write(CollectionsStore::file_path(), "][").unwrap();
        let back = CollectionsStore::load();
        assert_eq!(back.liked, vec!["/a".to_owned()]);
        // Nothing readable at all: empty data, no panic.
        fs::remove_file(CollectionsStore::backup_path()).unwrap();
        fs::write(CollectionsStore::file_path(), "][").unwrap();
        assert!(CollectionsStore::load().liked.is_empty());
    }
}
