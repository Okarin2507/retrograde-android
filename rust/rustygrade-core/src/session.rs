use std::fs;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};

use serde::{Deserialize, Serialize};

use crate::paths::CoverPaths;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct SessionState {
    pub current_id: Option<String>,
    pub index: i32,
    pub position: f64,
    pub shuffle: bool,
    pub repeat_mode: i32,
}

impl Default for SessionState {
    fn default() -> Self {
        Self {
            current_id: None,
            index: -1,
            position: 0.0,
            shuffle: false,
            repeat_mode: 0,
        }
    }
}

static GATE: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

pub struct SessionStore;

impl SessionStore {
    pub fn state_path() -> PathBuf {
        CoverPaths::app_dir().join("native-session.json")
    }

    pub fn queue_path() -> PathBuf {
        CoverPaths::app_dir().join("native-queue.json")
    }

    pub fn original_queue_path() -> PathBuf {
        CoverPaths::app_dir().join("native-queue-original.json")
    }

    pub fn load() -> Option<(SessionState, Vec<String>)> {
        let state_text = fs::read_to_string(Self::state_path()).ok()?;
        let queue_text = fs::read_to_string(Self::queue_path()).ok()?;
        let state = serde_json::from_str::<SessionState>(&state_text).ok()?;
        let queue = serde_json::from_str::<Vec<String>>(&queue_text).ok()?;
        Some((state, queue))
    }

    pub fn load_original_queue() -> Option<Vec<String>> {
        let text = fs::read_to_string(Self::original_queue_path()).ok()?;
        serde_json::from_str::<Vec<String>>(&text).ok()
    }

    pub fn save_state(state: &SessionState) {
        match serde_json::to_string(state) {
            Ok(json) => write_atomic(&Self::state_path(), &json),
            Err(e) => crate::error_log::write("session", &format!("could not encode the session state: {e}")),
        }
    }

    pub fn save_queue(ids: &[String]) {
        match serde_json::to_string(ids) {
            Ok(json) => write_atomic(&Self::queue_path(), &json),
            Err(e) => crate::error_log::write("session", &format!("could not encode the queue: {e}")),
        }
    }

    pub fn save_original_queue(ids: &[String]) {
        match serde_json::to_string(ids) {
            Ok(json) => write_atomic(&Self::original_queue_path(), &json),
            Err(e) => crate::error_log::write("session", &format!("could not encode the original queue: {e}")),
        }
    }
}

/// Atomic write under the session gate. A failure (storage full, read-only dir) is logged: the
/// session is best-effort state, but a silent loss is impossible to diagnose.
fn write_atomic(path: &std::path::Path, json: &str) {
    let _guard = GATE.lock().unwrap_or_else(|p| p.into_inner());
    if let Err(e) = fs::create_dir_all(CoverPaths::app_dir()) {
        crate::error_log::write("session", &format!("could not create the app dir: {e}"));
        return;
    }
    if let Err(e) = crate::fsutil::write_atomic(path, json.as_bytes()) {
        crate::error_log::write("session", &format!("could not write {}: {e}", path.display()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TestEnv;

    #[test]
    fn nothing_saved_loads_as_none() {
        let _env = TestEnv::new("session-empty");
        assert!(SessionStore::load().is_none());
        assert!(SessionStore::load_original_queue().is_none());
    }

    #[test]
    fn state_and_queues_round_trip() {
        let _env = TestEnv::new("session-roundtrip");
        let state = SessionState {
            current_id: Some("/m/音楽.flac".to_owned()),
            index: 1,
            position: 12.5,
            shuffle: true,
            repeat_mode: 2,
        };
        let ids = vec!["/m/a.flac".to_owned(), "/m/音楽.flac".to_owned()];
        SessionStore::save_state(&state);
        SessionStore::save_queue(&ids);
        SessionStore::save_original_queue(&["/m/z.flac".to_owned()]);
        let (loaded, queue) = SessionStore::load().expect("saved");
        assert_eq!(loaded.current_id.as_deref(), Some("/m/音楽.flac"));
        assert_eq!((loaded.index, loaded.position, loaded.shuffle, loaded.repeat_mode), (1, 12.5, true, 2));
        assert_eq!(queue, ids);
        assert_eq!(SessionStore::load_original_queue().unwrap(), vec!["/m/z.flac".to_owned()]);
        // No temp files are left behind.
        let leftovers = fs::read_dir(CoverPaths::app_dir())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp"))
            .count();
        assert_eq!(leftovers, 0);
    }

    #[test]
    fn a_damaged_session_file_loads_as_none() {
        let _env = TestEnv::new("session-damaged");
        SessionStore::save_state(&SessionState::default());
        SessionStore::save_queue(&["/a".to_owned()]);
        fs::write(SessionStore::queue_path(), "{oops").unwrap();
        assert!(SessionStore::load().is_none());
        fs::write(SessionStore::original_queue_path(), "nope").unwrap();
        assert!(SessionStore::load_original_queue().is_none());
    }

    #[test]
    fn a_poisoned_gate_does_not_stop_later_saves() {
        let _env = TestEnv::new("session-poison");
        let _ = std::thread::spawn(|| {
            let _g = GATE.lock().unwrap();
            panic!("poison the session gate");
        })
        .join();
        assert!(GATE.is_poisoned());
        SessionStore::save_queue(&["/a".to_owned()]);
        assert_eq!(fs::read_to_string(SessionStore::queue_path()).unwrap(), r#"["/a"]"#);
    }

    #[test]
    fn a_write_into_an_unusable_app_dir_is_skipped_without_panicking() {
        let env = TestEnv::new("session-fail");
        // Make the "app dir" a file so nothing can be created in it.
        let blocked = env.root.join("blocked");
        fs::write(&blocked, b"x").unwrap();
        let log_dir = env.app_dir.clone();
        CoverPaths::set_app_dir(blocked.clone());
        SessionStore::save_queue(&["/a".to_owned()]);
        assert!(!blocked.join("native-queue.json").exists());
        CoverPaths::set_app_dir(log_dir);
    }
}
