use std::path::{Path, PathBuf};
use std::sync::{LazyLock, RwLock};

static APP_DIR_OVERRIDE: LazyLock<RwLock<Option<PathBuf>>> = LazyLock::new(|| RwLock::new(None));

/// Cover cache naming and app directory resolution shared with the Electron & C# builds
/// (`{app_dir}/covers/{hash}_thumb.jpg`).
pub struct CoverPaths;

impl CoverPaths {
    /// Sets the app data directory (on Android, pass `Context.filesDir` at startup).
    pub fn set_app_dir(path: PathBuf) {
        *APP_DIR_OVERRIDE.write().unwrap_or_else(|p| p.into_inner()) = Some(path);
    }

    /// Configured Android/desktop app dir, else `RUSTYGRADE_APPDIR`, else `~/.rustygrade`.
    pub fn app_dir() -> PathBuf {
        if let Some(dir) = &*APP_DIR_OVERRIDE.read().unwrap_or_else(|p| p.into_inner()) {
            return dir.clone();
        }
        if let Ok(dir) = std::env::var("RUSTYGRADE_APPDIR")
            && !dir.is_empty()
        {
            return PathBuf::from(dir);
        }
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        home.join(".rustygrade")
    }

    pub fn cover_dir() -> PathBuf {
        Self::app_dir().join("covers")
    }

    /// Bit-exact port of the JS `hashName()` / C# `CoverPaths.HashName()`:
    /// iterates UTF-16 code units (`encode_utf16`), updating
    /// `hash = ((hash << 5) - hash + unit)` in wrapping 32-bit signed arithmetic,
    /// then formats `abs(hash as i64)` in lowercase base-36.
    pub fn hash_name(s: &str) -> String {
        let mut hash: i32 = 0;
        for unit in s.encode_utf16() {
            hash = hash
                .wrapping_shl(5)
                .wrapping_sub(hash)
                .wrapping_add(unit as i32);
        }
        let abs = (hash as i64).abs();
        to_base36(abs)
    }

    pub fn thumb_path(album_name: &str) -> PathBuf {
        Self::cover_dir().join(format!("{}_thumb.jpg", Self::hash_name(album_name)))
    }

    pub fn full_path(album_name: &str) -> PathBuf {
        Self::cover_dir().join(format!("{}_full.jpg", Self::hash_name(album_name)))
    }

    pub fn content_cover_path(hash_hex: &str, ext: &str) -> PathBuf {
        Self::cover_dir().join(format!("c_{hash_hex}.{ext}"))
    }

    pub fn is_blur_cache_file(path: &Path) -> bool {
        if path.parent() != Some(Self::cover_dir().as_path()) {
            return false;
        }
        path.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|name| {
                if name.contains(".tmp") {
                    return false;
                }
                let lower = name.to_ascii_lowercase();
                lower.ends_with("_blur.jpg")
                    || (lower.starts_with("blur_") && lower.ends_with(".jpg"))
            })
    }

    pub fn is_content_cover_file(path: &Path) -> bool {
        if path.parent() != Some(Self::cover_dir().as_path()) {
            return false;
        }
        path.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|name| {
                name.starts_with("c_")
                    && !name.contains(".tmp")
                    && !name.ends_with(".resync-old")
                    && !name.ends_with(".shrink-tmp")
                    && !name.to_ascii_lowercase().ends_with("_blur.jpg")
            })
    }

    pub fn is_managed_cover_file(path: &Path) -> bool {
        if path.parent() != Some(Self::cover_dir().as_path()) {
            return false;
        }
        if Self::is_blur_cache_file(path) {
            return false;
        }
        path.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|name| {
                if name.contains(".tmp")
                    || name.ends_with(".resync-old")
                    || name.ends_with(".shrink-tmp")
                {
                    return false;
                }
                let lower = name.to_ascii_lowercase();
                name.starts_with("c_")
                    || name.starts_with("native_pl_")
                    || lower.ends_with("_thumb.jpg")
                    || lower.ends_with("_full.jpg")
            })
    }

    /// Turns `"file:///path/to/x.jpg?t=..."` or a plain path into a local `PathBuf`.
    pub fn to_local_path(url_or_path: Option<&str>) -> Option<PathBuf> {
        let raw = url_or_path?.trim();
        if raw.is_empty() {
            return None;
        }
        // Compare bytes: slicing the `str` at 7 would panic inside a multi-byte character.
        if raw.len() >= 7 && raw.as_bytes()[..7].eq_ignore_ascii_case(b"file://") {
            let stripped = match raw.find('?') {
                Some(idx) => &raw[..idx],
                None => raw,
            };
            let mut p = &stripped[7..];
            // Legacy Windows file:///C:/... safety just in case a shared library.json contains it.
            let bytes = p.as_bytes();
            if bytes.len() > 2 && bytes[0] == b'/' && bytes[2] == b':' {
                p = &p[1..];
            }
            let decoded = urlencoding::decode(p)
                .map(|c| c.into_owned())
                .unwrap_or_else(|_| p.to_owned());
            return Some(PathBuf::from(decoded));
        }
        Some(PathBuf::from(raw))
    }

    pub fn to_file_url(path: &Path) -> String {
        let normalized = path.to_string_lossy().replace('\\', "/");
        let encoded = normalized
            .split('/')
            .map(|seg| urlencoding::encode(seg).into_owned())
            .collect::<Vec<_>>()
            .join("/");
        format!("file://{encoded}")
    }
}

fn to_base36(mut value: i64) -> String {
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if value == 0 {
        return "0".to_owned();
    }
    let mut buf = [0u8; 16];
    let mut i = buf.len();
    while value > 0 {
        i -= 1;
        buf[i] = DIGITS[(value % 36) as usize];
        value /= 36;
    }
    String::from_utf8(buf[i..].to_vec()).expect("base36 digits are ASCII")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_name_matches_real_library_and_utf16_surrogates() {
        // Verified directly against ~/.rustygrade/library.json:
        assert_eq!(
            CoverPaths::hash_name("Ado 1st Live Kigeki (Live At Zepp DiverCity, 2022)"),
            "oy1a16"
        );
        assert_eq!(CoverPaths::hash_name(""), "0");
        // Emoji requires UTF-16 surrogate pair handling rather than Unicode scalar codepoints.
        assert_eq!(CoverPaths::hash_name("🎵"), "120ah");
        // Japanese / CJK title
        assert_eq!(CoverPaths::hash_name("残夢"), "is53");
    }

    #[test]
    fn to_local_path_handles_file_urls_and_queries() {
        let p = CoverPaths::to_local_path(Some("file:///home/okarin/.rustygrade/covers/a%20b.jpg?t=123"))
            .unwrap();
        assert_eq!(p, PathBuf::from("/home/okarin/.rustygrade/covers/a b.jpg"));
        // Plain Linux filenames with `?` must not be truncated.
        assert_eq!(
            CoverPaths::to_local_path(Some("/music/What?/01.flac")),
            Some(PathBuf::from("/music/What?/01.flac"))
        );
        let url = CoverPaths::to_file_url(Path::new("/home/okarin/My Covers/a #1?.jpg"));
        assert_eq!(
            url,
            "file:///home/okarin/My%20Covers/a%20%231%3F.jpg"
        );
        assert_eq!(
            CoverPaths::to_local_path(Some(&url)),
            Some(PathBuf::from("/home/okarin/My Covers/a #1?.jpg"))
        );
        assert_eq!(CoverPaths::to_local_path(Some("   ")), None);
        assert_eq!(CoverPaths::to_local_path(None), None);
    }

    #[test]
    fn to_local_path_never_panics_on_non_ascii_input() {
        // Byte 7 falls inside a multi-byte character in each of these.
        for raw in ["音楽/カバー.jpg", "あいう", "é音楽音楽", "日本語のパス/トラック.flac", "ファイル"] {
            assert_eq!(CoverPaths::to_local_path(Some(raw)), Some(PathBuf::from(raw)));
        }
        assert_eq!(
            CoverPaths::to_local_path(Some("FILE:///music/%E9%9F%B3%E6%A5%BD.flac")),
            Some(PathBuf::from("/music/音楽.flac"))
        );
        assert_eq!(
            CoverPaths::to_local_path(Some("file:///音楽/カバー.jpg?t=1")),
            Some(PathBuf::from("/音楽/カバー.jpg"))
        );
    }

    #[test]
    fn set_app_dir_overrides_resolution() {
        // The app dir is process-global: share the test sandbox lock with the scanner tests.
        let _env = crate::testing::TestEnv::new("paths-override");
        let prev = APP_DIR_OVERRIDE.read().ok().and_then(|g| g.clone());
        let custom = PathBuf::from("/data/user/0/dev.okarin.rustygrade/files");
        CoverPaths::set_app_dir(custom.clone());
        assert_eq!(CoverPaths::app_dir(), custom);
        assert_eq!(CoverPaths::cover_dir(), custom.join("covers"));
        if let Ok(mut guard) = APP_DIR_OVERRIDE.write() {
            *guard = prev;
        }
    }
}
