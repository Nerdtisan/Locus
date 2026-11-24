// SPDX-License-Identifier: AGPL-3.0-or-later

//! PNG and JPEG reference images (#390).
//!
//! The file is decoded once here to prove it is whole and to learn its
//! size, with the decoder's own limits set, so a hostile header can't ask
//! for gigabytes. The document keeps the original bytes
//! (`locus_doc::image`); the app decodes them again, with [`decode`], to
//! draw them. Decoding is `image` with only its pure-Rust PNG (`png`) and
//! JPEG (`zune-jpeg`) codecs (MIT / Apache-2.0 / Zlib, SPEC §2).

use std::io::Cursor;
use std::sync::Arc;

use locus_doc::image::{
    DEFAULT_MM_PER_PX, ImageFormat, ImagePlacement, MAX_IMAGE_BYTES, MAX_IMAGE_PIXELS,
    MAX_IMAGE_SIDE, RefImage,
};
use locus_geom::Vec2;

use crate::ImportError;

/// A decoded image: straight (not premultiplied) RGBA8, row 0 at the top.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rgba {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

/// Decode a PNG or JPEG within the document's limits. Never panics on
/// malformed data: anything wrong is [`ImportError::BadImage`].
///
/// # Errors
/// [`ImportError::TooLarge`] past the byte limit, [`ImportError::BadImage`]
/// for anything the decoder refuses or a size past the pixel limits.
pub fn decode(bytes: &[u8]) -> Result<Rgba, ImportError> {
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(ImportError::TooLarge);
    }
    let format = match ImageFormat::sniff(bytes).ok_or(ImportError::UnknownFormat)? {
        ImageFormat::Png => image::ImageFormat::Png,
        ImageFormat::Jpeg => image::ImageFormat::Jpeg,
    };
    let mut reader = image::ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_SIDE);
    limits.max_image_height = Some(MAX_IMAGE_SIDE);
    // The RGBA buffer plus the decoder's own working memory.
    limits.max_alloc = Some(MAX_IMAGE_PIXELS * 4 * 2);
    reader.limits(limits);
    let (w, h) = reader
        .into_dimensions()
        .map_err(|_| ImportError::BadImage)?;
    if u64::from(w) * u64::from(h) > MAX_IMAGE_PIXELS || w == 0 || h == 0 {
        return Err(ImportError::BadImage);
    }
    let mut reader = image::ImageReader::with_format(Cursor::new(bytes), format);
    reader.limits(limits_for(w, h));
    let img = reader.decode().map_err(|_| ImportError::BadImage)?;
    let rgba = img.into_rgba8();
    let (width, height) = rgba.dimensions();
    Ok(Rgba {
        width,
        height,
        pixels: rgba.into_raw(),
    })
}

/// As [`decode`], shrunk to at most `max_side` pixels a side (the GPU's
/// texture limit), keeping the aspect ratio.
///
/// # Errors
/// As [`decode`].
pub fn decode_fit(bytes: &[u8], max_side: u32) -> Result<Rgba, ImportError> {
    let d = decode(bytes)?;
    let max_side = max_side.max(1);
    if d.width <= max_side && d.height <= max_side {
        return Ok(d);
    }
    let scale = f64::from(max_side) / f64::from(d.width.max(d.height));
    let w = ((f64::from(d.width) * scale).round() as u32).clamp(1, max_side);
    let h = ((f64::from(d.height) * scale).round() as u32).clamp(1, max_side);
    let img =
        image::RgbaImage::from_raw(d.width, d.height, d.pixels).ok_or(ImportError::BadImage)?;
    let small = image::imageops::resize(&img, w, h, image::imageops::FilterType::Triangle);
    Ok(Rgba {
        width: w,
        height: h,
        pixels: small.into_raw(),
    })
}

fn limits_for(w: u32, h: u32) -> image::Limits {
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(w.min(MAX_IMAGE_SIDE));
    limits.max_image_height = Some(h.min(MAX_IMAGE_SIDE));
    limits.max_alloc = Some(MAX_IMAGE_PIXELS * 4 * 2);
    limits
}

/// The reference image for `bytes`, centred on `centre` at 96 dpi (an
/// image's own resolution is often wrong; Calibrate sets the real scale,
/// #391), at half opacity so the sketch reads over it.
///
/// # Errors
/// As [`decode`].
pub fn reference_image(bytes: &[u8], centre: Vec2) -> Result<RefImage, ImportError> {
    let decoded = decode(bytes)?;
    let format = ImageFormat::sniff(bytes).ok_or(ImportError::UnknownFormat)?;
    let size = Vec2::new(f64::from(decoded.width), f64::from(decoded.height)) * DEFAULT_MM_PER_PX;
    let centre = if centre.x.is_finite() && centre.y.is_finite() {
        centre
    } else {
        Vec2::ZERO
    };
    let image = RefImage {
        format,
        width_px: decoded.width,
        height_px: decoded.height,
        data: Arc::from(bytes),
        placement: ImagePlacement {
            origin: centre - size * 0.5,
            mm_per_px: DEFAULT_MM_PER_PX,
            opacity: 0.5,
            rotation: 0.0,
        },
    };
    image
        .validate()
        .map_err(|e| ImportError::Document(e.to_string()))?;
    Ok(image)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // fixtures built in the test
mod tests {
    use super::*;

    /// A `w`×`h` PNG, encoded here so the test needs no binary fixture.
    pub(crate) fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba([10, 20, 30, 255]));
        let mut out = Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    #[test]
    fn decodes_a_png_and_places_it_centred() {
        let bytes = png(96, 48);
        let d = decode(&bytes).unwrap();
        assert_eq!((d.width, d.height), (96, 48));
        assert_eq!(&d.pixels[..4], &[10, 20, 30, 255]);
        let r = reference_image(&bytes, Vec2::new(100.0, 50.0)).unwrap();
        let (lo, hi) = r.rect();
        assert!(
            (hi.x - lo.x - 25.4).abs() < 1e-9,
            "96 px at 96 dpi is an inch"
        );
        assert!(((lo + hi) * 0.5 - Vec2::new(100.0, 50.0)).length() < 1e-9);
    }

    #[test]
    fn decode_fit_shrinks_to_the_texture_limit() {
        let d = decode_fit(&png(300, 100), 150).unwrap();
        assert_eq!((d.width, d.height), (150, 50));
        assert_eq!(d.pixels.len(), 150 * 50 * 4);
        let same = decode_fit(&png(30, 10), 150).unwrap();
        assert_eq!((same.width, same.height), (30, 10));
    }

    /// Malformed files are refused without panicking (CLAUDE.md).
    #[test]
    fn malformed_images_are_refused() {
        let good = png(8, 8);
        for bad in [
            &good[..good.len() / 2],
            &good[..12],
            b"\x89PNG\r\n\x1a\ngarbage".as_slice(),
            &[0xFF, 0xD8, 0xFF, 0xE0, 0, 0, 0],
            b"not an image",
        ] {
            assert!(decode(bad).is_err());
        }
        // A header claiming a huge image is refused before allocating.
        let mut huge = good.clone();
        // IHDR width and height sit at bytes 16..24.
        huge[16..20].copy_from_slice(&100_000u32.to_be_bytes());
        huge[20..24].copy_from_slice(&100_000u32.to_be_bytes());
        assert!(decode(&huge).is_err());
    }
}
