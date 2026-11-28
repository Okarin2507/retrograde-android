//! Small filesystem helpers shared by the stores: a wall clock in ms and crash-safe writes.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Unix time in milliseconds (0 if the clock is before the epoch).
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// A fresh sibling path of `dest` for staging a write: unique per process, thread and call, and it
/// keeps `dest`'s extension (format sniffers such as lofty look at it). Same directory means the
/// final `rename` never crosses a filesystem.
pub fn unique_sibling(dest: &Path) -> PathBuf {
    let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let name = dest.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let tmp_name = match dest.extension().and_then(|e| e.to_str()) {
        Some(ext) => format!(".{name}.{}-{seq}.tmp.{ext}", std::process::id()),
        None => format!(".{name}.{}-{seq}.tmp", std::process::id()),
    };
    dest.with_file_name(tmp_name)
}

/// Writes `bytes` to a sibling temp file and renames it over `dest`, so readers see either the old
/// or the new file (never a truncated one) even if the process dies or two writers race.
pub fn write_atomic(dest: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = unique_sibling(dest);
    if let Err(e) = fs::write(&tmp, bytes) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    if let Err(e) = fs::rename(&tmp, dest) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

/// Atomic JSON persistence with a one-generation backup, the shape every store in this crate uses:
/// `bytes` go to a temp file, the current file (if any) is copied to `backup`, then the temp file
/// replaces it. Callers hold their own write lock.
pub fn save_with_backup(path: &Path, backup: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = unique_sibling(path);
    if let Err(e) = fs::write(&tmp, bytes) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    if path.exists() {
        let _ = fs::copy(path, backup);
    }
    if let Err(e) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique_sibling_keeps_extension_and_differs_per_call() {
        let a = unique_sibling(Path::new("/x/y/song.flac"));
        let b = unique_sibling(Path::new("/x/y/song.flac"));
        assert_ne!(a, b);
        assert_eq!(a.parent(), Some(Path::new("/x/y")));
        assert_eq!(a.extension().and_then(|e| e.to_str()), Some("flac"));
        let plain = unique_sibling(Path::new("/x/y/noext"));
        assert!(plain.to_string_lossy().ends_with(".tmp"));
    }

    #[test]
    fn write_atomic_replaces_and_leaves_no_temp_files() {
        let dir = std::env::temp_dir().join(format!("rusty-fsutil-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("a.json");
        write_atomic(&dest, b"one").unwrap();
        write_atomic(&dest, b"two").unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"two");
        save_with_backup(&dest, &dir.join("a.json.bak"), b"three").unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"three");
        assert_eq!(fs::read(dir.join("a.json.bak")).unwrap(), b"two");
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 2, "only the two real files remain");
        // A write into a missing directory fails and cleans up after itself.
        assert!(write_atomic(&dir.join("missing/x"), b"z").is_err());
        let _ = fs::remove_dir_all(&dir);
    }
}
