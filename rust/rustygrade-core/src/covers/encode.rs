use std::borrow::Cow;
use std::fs;
use std::io::Cursor;
use std::path::Path;

use fast_image_resize::images::Image as FirImage;
use fast_image_resize::{FilterType, IntoImageView, ResizeAlg, ResizeOptions, Resizer};
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::{ColorType, DynamicImage, ImageEncoder, ImageReader, RgbImage, RgbaImage};

use super::CoverError;

pub const MAX_COVER_FILE_BYTES: u64 = 30 * 1024 * 1024;
/// ~25 MP: decoding to RGBA8 peaks at about 100 MB, which a low-memory phone survives.
pub const MAX_PIXELS: u64 = 25_000_000;
pub const MAX_DIMENSION: u32 = 16_384;

pub struct ImageLimits;

impl ImageLimits {
    pub fn is_sane_bytes(data: &[u8]) -> bool {
        if data.len() as u64 > MAX_COVER_FILE_BYTES {
            return false;
        }
        let Ok(reader) = ImageReader::new(Cursor::new(data)).with_guessed_format() else {
            return false;
        };
        Self::fits(reader.into_dimensions().ok())
    }

    pub fn is_sane_path(path: &Path) -> bool {
        let Ok(meta) = fs::metadata(path) else {
            return false;
        };
        if meta.len() > MAX_COVER_FILE_BYTES {
            return false;
        }
        let Ok(reader) = ImageReader::open(path).and_then(|r| r.with_guessed_format()) else {
            return false;
        };
        Self::fits(reader.into_dimensions().ok())
    }

    pub(super) fn fits(dims: Option<(u32, u32)>) -> bool {
        let Some((w, h)) = dims else {
            return false;
        };
        w > 0
            && h > 0
            && w <= MAX_DIMENSION
            && h <= MAX_DIMENSION
            && (w as u64) * (h as u64) <= MAX_PIXELS
    }
}

/// Longest edge allowed for stored full-size covers. 720 px preserves crisp full-screen artwork on
/// mobile while cutting cache size dramatically and keeping desktop compatibility.
pub const MAX_FULL_COVER_EDGE: u32 = 720;

/// Size of `w` x `h` fitted within `max_edge` x `max_edge`, preserving aspect ratio.
fn capped_dims(w: u32, h: u32, max_edge: u32) -> (u32, u32) {
    if max_edge == 0 || w.max(h) <= max_edge {
        return (w, h);
    }
    let scale = |side: u32, long: u32| -> u32 {
        (((side as u64) * (max_edge as u64) + (long as u64) / 2) / (long as u64)).max(1) as u32
    };
    if h > w {
        (scale(w, h), max_edge)
    } else {
        (max_edge, scale(h, w))
    }
}

/// Encodes a full-size cover fitted within `max_edge`. Opaque images become JPEG; images with
/// an alpha channel become PNG so transparency survives. Returns `(bytes, extension)`.
pub(crate) fn encode_capped_full_with_ext(
    img: &DynamicImage,
    max_edge: u32,
    quality: u8,
) -> Result<(Vec<u8>, &'static str), CoverError> {
    if img.color().has_alpha() {
        Ok((encode_capped_png(img, max_edge)?, "png"))
    } else {
        Ok((encode_capped_jpeg(img, max_edge, quality)?, "jpg"))
    }
}

#[cfg(test)]
pub(crate) fn encode_capped_full(
    img: &DynamicImage,
    max_edge: u32,
    quality: u8,
) -> Result<Vec<u8>, CoverError> {
    encode_capped_full_with_ext(img, max_edge, quality).map(|(bytes, _)| bytes)
}

/// Encode `img` as JPEG fitted within `max_edge` x `max_edge`, preserving aspect ratio.
/// Images already within the cap are encoded without resizing.
pub(crate) fn encode_capped_jpeg(
    img: &DynamicImage,
    max_edge: u32,
    quality: u8,
) -> Result<Vec<u8>, CoverError> {
    let (dw, dh) = capped_dims(img.width(), img.height(), max_edge);
    encode_to_size(img, dw, dh, quality)
}

fn encode_capped_png(img: &DynamicImage, max_edge: u32) -> Result<Vec<u8>, CoverError> {
    // Borrow the pixels when the image already is RGBA8; convert only when the layout differs.
    let rgba: Cow<'_, RgbaImage> = match img.as_rgba8() {
        Some(buf) => Cow::Borrowed(buf),
        None => Cow::Owned(img.to_rgba8()),
    };
    let rgba: &RgbaImage = &rgba;
    let (dw, dh) = capped_dims(rgba.width(), rgba.height(), max_edge);
    let pixels: Cow<'_, [u8]> = if (dw, dh) != (rgba.width(), rgba.height()) {
        let pixel_type = rgba
            .pixel_type()
            .ok_or_else(|| CoverError::Image("Unsupported pixel type".to_owned()))?;
        let mut dst = FirImage::new(dw, dh, pixel_type);
        let opts = ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FilterType::Mitchell));
        Resizer::new()
            .resize(rgba, &mut dst, &opts)
            .map_err(|e| CoverError::Image(e.to_string()))?;
        Cow::Owned(dst.into_vec())
    } else {
        Cow::Borrowed(rgba.as_raw())
    };
    let mut out = Vec::new();
    PngEncoder::new(&mut out)
        .write_image(&pixels, dw, dh, ColorType::Rgba8.into())
        .map_err(|e| CoverError::Image(e.to_string()))?;
    Ok(out)
}

fn encode_to_size(
    img: &DynamicImage,
    dst_w: u32,
    dst_h: u32,
    quality: u8,
) -> Result<Vec<u8>, CoverError> {
    // Borrow the pixels when the image already is RGB8; convert only when the layout differs.
    let rgb: Cow<'_, RgbImage> = match img.as_rgb8() {
        Some(buf) => Cow::Borrowed(buf),
        None => Cow::Owned(img.to_rgb8()),
    };
    let rgb: &RgbImage = &rgb;
    let pixels: Cow<'_, [u8]> = if (dst_w, dst_h) != (rgb.width(), rgb.height()) {
        let pixel_type = rgb
            .pixel_type()
            .ok_or_else(|| CoverError::Image("Unsupported pixel type".to_owned()))?;
        let mut dst = FirImage::new(dst_w, dst_h, pixel_type);
        let mut resizer = Resizer::new();
        let opts = ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FilterType::Mitchell));
        resizer
            .resize(rgb, &mut dst, &opts)
            .map_err(|e| CoverError::Image(e.to_string()))?;
        Cow::Owned(dst.into_vec())
    } else {
        Cow::Borrowed(rgb.as_raw())
    };

    let mut out = Vec::new();
    let mut enc = JpegEncoder::new_with_quality(&mut out, quality);
    enc.encode(&pixels, dst_w, dst_h, ColorType::Rgb8.into())
        .map_err(|e| CoverError::Image(e.to_string()))?;
    Ok(out)
}
