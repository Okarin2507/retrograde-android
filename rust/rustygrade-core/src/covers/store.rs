use std::borrow::Cow;
use std::fs::{self, OpenOptions};
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use image::codecs::jpeg::JpegEncoder;
use image::metadata::Orientation;
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader};

use super::CoverError;
use super::encode::{ImageLimits, MAX_COVER_FILE_BYTES, MAX_FULL_COVER_EDGE, encode_capped_full_with_ext};
use super::hash::sha256_hex_prefix;
use crate::library::try_delete;
use crate::paths::CoverPaths;

static COVER_TMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// Normalized, hashed cover image ready to be persisted into `CoverPaths::cover_dir()`.
#[derive(Debug, Clone)]
pub struct PreparedCover {
    bytes: Vec<u8>,
    hash: String,
    ext: &'static str,
}

/// Result of writing a content-addressed cover file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedCover {
    pub path: PathBuf,
    /// `true` when this call newly created `path` on disk; `false` when identical content already existed.
    pub created: bool,
}

fn prepare_cover_cow(data: &[u8]) -> Result<(Cow<'_, [u8]>, String, &'static str), CoverError> {
    if data.len() as u64 > MAX_COVER_FILE_BYTES {
        return Err(CoverError::InsaneImage);
    }
    // One parse: sniff the format and read the header dimensions, then validate before decoding.
    let reader = ImageReader::new(Cursor::new(data))
        .with_guessed_format()
        .map_err(|_| CoverError::InsaneImage)?;
    let fmt = reader.format();
    let mut decoder = reader.into_decoder().map_err(|_| CoverError::InsaneImage)?;
    let (w, h) = decoder.dimensions();
    if !ImageLimits::fits(Some((w, h))) {
        return Err(CoverError::InsaneImage);
    }
    let orient = decoder.orientation().unwrap_or(Orientation::NoTransforms);

    let (final_bytes, ext): (Cow<'_, [u8]>, &'static str) = if w.max(h) <= MAX_FULL_COVER_EDGE
        && orient == Orientation::NoTransforms
        && matches!(fmt, Some(ImageFormat::Jpeg | ImageFormat::Png | ImageFormat::WebP))
    {
        let ext = match fmt {
            Some(ImageFormat::Png) => "png",
            Some(ImageFormat::WebP) => "webp",
            _ => "jpg",
        };
        (Cow::Borrowed(data), ext)
    } else {
        let mut img =
            DynamicImage::from_decoder(decoder).map_err(|e| CoverError::Image(e.to_string()))?;
        img.apply_orientation(orient);
        let (encoded, ext) = encode_capped_full_with_ext(&img, MAX_FULL_COVER_EDGE, 90)?;
        (Cow::Owned(encoded), ext)
    };

    let hash = sha256_hex_prefix(&final_bytes);
    Ok((final_bytes, hash, ext))
}

fn write_cover_bytes(
    final_bytes: &[u8],
    hash: &str,
    ext: &'static str,
) -> Result<SavedCover, CoverError> {
    let dest = CoverPaths::content_cover_path(hash, ext);
    let cover_dir = CoverPaths::cover_dir();
    fs::create_dir_all(&cover_dir)?;
    let mut created = false;
    if !dest.exists() {
        let seq = COVER_TMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let tmp = cover_dir.join(format!("c_{hash}.{ext}.tmp.{}.{seq}", std::process::id()));
        if let Err(e) = fs::write(&tmp, final_bytes) {
            let _ = fs::remove_file(&tmp);
            return Err(e.into());
        }
        if let Err(e) = fs::rename(&tmp, &dest) {
            let _ = fs::remove_file(&tmp);
            if !dest.exists() {
                return Err(e.into());
            }
        } else {
            created = true;
        }
    } else if let Ok(file) = OpenOptions::new().write(true).open(&dest) {
        // Keep mtime current so a concurrent GC grace window treats reused covers as active.
        let _ = file.set_modified(SystemTime::now());
    }
    Ok(SavedCover {
        path: dest,
        created,
    })
}

/// Decodes, EXIF-orients, downscales if needed, and hashes cover image bytes in memory without
/// holding any store lock or writing to disk.
pub fn prepare_cover_bytes(data: &[u8]) -> Result<PreparedCover, CoverError> {
    let (bytes, hash, ext) = prepare_cover_cow(data)?;
    Ok(PreparedCover {
        bytes: bytes.into_owned(),
        hash,
        ext,
    })
}

/// Writes a previously prepared cover into `CoverPaths::cover_dir()`, returning whether a new
/// file was created on disk.
pub fn write_prepared_cover(prepared: &PreparedCover) -> Result<SavedCover, CoverError> {
    write_cover_bytes(&prepared.bytes, &prepared.hash, prepared.ext)
}

/// Normalizes cover bytes to <= `MAX_FULL_COVER_EDGE` (applying EXIF orientation) and writes a
/// single content-addressed file `c_<hash>.<ext>` under `CoverPaths::cover_dir()`, skipping the write
/// if identical bytes already exist on disk.
pub fn save_cover_bytes(data: &[u8]) -> Result<PathBuf, CoverError> {
    let (final_bytes, hash, ext) = prepare_cover_cow(data)?;
    Ok(write_cover_bytes(&final_bytes, &hash, ext)?.path)
}

/// Where the blurred backdrop of `cover` lives. Always inside the cover cache: a cover already in
/// the cache keeps `<name>_blur.jpg` beside it, any other file gets `blur_<hash of its path>.jpg`
/// there, so nothing is ever written next to user files.
pub(super) fn blur_path_for(cover: &Path) -> Option<PathBuf> {
    let cover_dir = CoverPaths::cover_dir();
    if cover.parent() == Some(cover_dir.as_path()) {
        let stem = cover.file_stem()?.to_str()?;
        return Some(cover_dir.join(format!("{stem}_blur.jpg")));
    }
    // FNV-1a over the path bytes: stable across runs, cheap, 64 bits so collisions are not a concern.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in cover.as_os_str().as_encoded_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    Some(cover_dir.join(format!("blur_{h:016x}.jpg")))
}

/// A small, heavily blurred copy of a cover (`<name>_blur.jpg` in the cover cache), used as the
/// full-screen player backdrop on Android versions without a GPU blur. Cached: an existing file
/// is reused.
pub fn blurred_backdrop(cover: &Path) -> Option<PathBuf> {
    let out = blur_path_for(cover)?;
    if out.is_file() {
        let meta = fs::metadata(&out).ok();
        let blur_time = meta.as_ref().and_then(|m| m.modified().ok());
        let cover_time = fs::metadata(cover).and_then(|m| m.modified()).ok();
        let stale = matches!((blur_time, cover_time), (Some(b), Some(c)) if c > b);
        if !stale && meta.is_some_and(|m| m.len() > 0) {
            return Some(out);
        }
    }
    if !ImageLimits::is_sane_path(cover) {
        return None;
    }
    let reader = ImageReader::open(cover).ok()?.with_guessed_format().ok()?;
    let mut decoder = reader.into_decoder().ok()?;
    let orient = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let mut img = DynamicImage::from_decoder(decoder).ok()?;
    img.apply_orientation(orient);
    // Blur a tiny version: cheap, and the UI scales it up (it is a blur anyway).
    let small = img.thumbnail(64, 64).to_rgb8();
    let blurred = image::imageops::blur(&small, 3.0);
    let mut bytes = Vec::new();
    JpegEncoder::new_with_quality(&mut bytes, 80)
        .encode_image(&DynamicImage::ImageRgb8(blurred))
        .ok()?;
    fs::create_dir_all(out.parent()?).ok()?;
    crate::fsutil::write_atomic(&out, &bytes).ok()?;
    Some(out)
}

pub struct PlaylistCovers;

impl PlaylistCovers {
    pub const EXTENSIONS: &'static [&'static str] = &["jpg", "jpeg", "png", "webp"];
    const PREFIX: &'static str = "native_pl_";

    pub fn is_supported_ext(path: &Path) -> bool {
        path.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| Self::EXTENSIONS.iter().any(|x| x.eq_ignore_ascii_case(e)))
    }

    pub fn prepare_from_path(source_path: &Path) -> Result<PreparedCover, CoverError> {
        if !Self::is_supported_ext(source_path) {
            return Err(CoverError::UnsupportedFormat);
        }
        if !ImageLimits::is_sane_path(source_path) {
            return Err(CoverError::InsaneImage);
        }
        let bytes = fs::read(source_path)?;
        prepare_cover_bytes(&bytes)
    }

    pub fn save_prepared(prepared: &PreparedCover) -> Result<(String, bool), CoverError> {
        let saved = write_prepared_cover(prepared)?;
        Ok((CoverPaths::to_file_url(&saved.path), saved.created))
    }

    /// Writes a content-addressed playlist cover (`c_<hash>.<ext>`), returning its `file://` URL.
    #[cfg(test)]
    pub fn save(source_path: &Path) -> Result<String, CoverError> {
        let prepared = Self::prepare_from_path(source_path)?;
        Ok(Self::save_prepared(&prepared)?.0)
    }

    /// Deletes a playlist cover (and any legacy `_thumb` / `_blur` siblings). Callers that support
    /// shared covers should prefer [`delete_unreferenced_covers`](super::delete_unreferenced_covers).
    pub fn delete(cover_art: Option<&str>) {
        let Some(full) = CoverPaths::to_local_path(cover_art) else {
            return;
        };
        let Some(name) = full.file_name().and_then(|n| n.to_str()) else {
            return;
        };
        if !(name.starts_with(Self::PREFIX) || CoverPaths::is_content_cover_file(&full))
            || full.parent() != Some(CoverPaths::cover_dir().as_path())
        {
            return;
        }
        let thumb = Self::thumb_for(&full);
        for file in [&full, &thumb] {
            try_delete(file);
            if let Some(blur) = blur_path_for(file) {
                try_delete(&blur);
            }
        }
    }

    pub fn thumb_for(full_path: &Path) -> PathBuf {
        const SUFFIX: &[u8] = b"_full.jpg";
        let s = full_path.to_string_lossy();
        let bytes = s.as_bytes();
        // Compare bytes: slicing the str at `len - 9` can fall inside a multi-byte character. When the
        // suffix matches (it is ASCII), the cut is on a character boundary.
        if bytes.len() >= SUFFIX.len() && bytes[bytes.len() - SUFFIX.len()..].eq_ignore_ascii_case(SUFFIX) {
            let sibling = PathBuf::from(format!("{}_thumb.jpg", &s[..s.len() - SUFFIX.len()]));
            if sibling.exists() {
                return sibling;
            }
        }
        full_path.to_path_buf()
    }
}
