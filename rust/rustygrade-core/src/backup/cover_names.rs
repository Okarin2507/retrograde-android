use std::collections::HashSet;
use std::path::Path;

use super::{BackupError, PLAYLIST_COVER_PREFIX};
use crate::covers::PlaylistCovers;
use crate::paths::CoverPaths;

fn is_content_addressed_cover(name: &str) -> bool {
    let Some((stem, ext)) = name.rsplit_once('.') else {
        return false;
    };
    let Some(hex) = stem.strip_prefix("c_") else {
        return false;
    };
    hex.len() == 24
        && hex
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        && PlaylistCovers::EXTENSIONS
            .iter()
            .any(|e| e.eq_ignore_ascii_case(ext))
}

fn is_legacy_playlist_cover(name: &str) -> bool {
    let Some((stem, ext)) = name.rsplit_once('.') else {
        return false;
    };
    let Some(rest) = stem.strip_prefix(PLAYLIST_COVER_PREFIX) else {
        return false;
    };
    (1..=120).contains(&rest.len())
        && rest
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
        && PlaylistCovers::EXTENSIONS
            .iter()
            .any(|e| e.eq_ignore_ascii_case(ext))
}

pub(super) fn stored_playlist_cover_name(raw_name: &str) -> String {
    if is_content_addressed_cover(raw_name)
        && let Some((stem, ext)) = raw_name.rsplit_once('.')
    {
        return format!("{stem}.{}", ext.to_ascii_lowercase());
    }
    raw_name.to_owned()
}

pub(super) fn is_normalized_content_cover(name: &str) -> bool {
    is_content_addressed_cover(name)
}

pub(super) fn validate_cover_filename(name: &str) -> Result<(), BackupError> {
    if name.is_empty()
        || name.contains('/')
        || name.contains('\\')
        || name.contains('\0')
        || name.contains("..")
    {
        return Err(BackupError::InvalidCover {
            name: name.to_owned(),
            reason: "illegal path characters",
        });
    }
    if !PlaylistCovers::is_supported_ext(Path::new(name)) {
        return Err(BackupError::InvalidCover {
            name: name.to_owned(),
            reason: "unsupported image extension",
        });
    }
    if !is_legacy_playlist_cover(name) && !is_content_addressed_cover(name) {
        return Err(BackupError::InvalidCover {
            name: name.to_owned(),
            reason: "unexpected cover prefix",
        });
    }
    Ok(())
}

pub fn extract_cover_basename(cover_art: Option<&str>) -> Option<String> {
    let local = CoverPaths::to_local_path(cover_art)?;
    let raw = local.to_string_lossy();
    let base = raw.rsplit(['/', '\\']).next()?.trim();
    if base.is_empty() {
        None
    } else {
        Some(base.to_owned())
    }
}

pub(super) fn dedupe_preserving_order(iter: impl Iterator<Item = String>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for item in iter {
        if seen.insert(item.clone()) {
            out.push(item);
        }
    }
    out
}
