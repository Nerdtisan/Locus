// SPDX-License-Identifier: AGPL-3.0-or-later

//! PNG raster writer (#332, SPEC §8 Formats).
//!
//! Draws the same prepared [`ExportModel`] the SVG writer writes, with the
//! same paint per shape (SPEC §7, #284): a Draw object's own style, a
//! machine layer's colour and hairline. The image covers the model's bounds
//! (its `frame` when it is one layer of a per-layer export, so the files
//! line up), at a chosen resolution, on a transparent or white background.
//!
//! Strokes thinner than one pixel are drawn one pixel wide, so a profile's
//! hairline stays visible. A fixed margin of [`MARGIN_MM`] on every side
//! keeps strokes on the edge whole; it is the same for every file of a
//! per-layer export, so they still line up. Rasterised with `tiny-skia` (BSD-3-Clause), pure
//! Rust without SIMD, so the crate still builds for wasm32 (SPEC §3).
//! The image size is bounded: a huge sketch at a high resolution is refused
//! with a reason instead of allocating gigabytes (untrusted input).

use crate::exporter::{ExportError, Exported, Exporter};
use crate::pipeline::{ExportModel, Shape};
use locus_geom::Vec2;
use tiny_skia::{
    Color, FillRule, LineCap, LineJoin, Paint, PathBuilder, Pixmap, Stroke, Transform,
};

/// Longest side, in pixels.
pub const MAX_SIDE: u32 = 20_000;
/// Most pixels in one image (100 MP, 400 MB of RGBA).
pub const MAX_PIXELS: u64 = 100_000_000;
/// Default resolution: what laser and print software expect.
pub const DEFAULT_DPI: f64 = 300.0;
const MM_PER_INCH: f64 = 25.4;
/// Blank border around the design on every side, in mm.
pub const MARGIN_MM: f64 = 1.0;
/// Canvas for a model with nothing in it, in mm.
const EMPTY_SIZE_MM: f64 = 1.0;

/// How fine the image is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Resolution {
    /// Dots per inch of the design's true size.
    Dpi(f64),
    /// The image's width in pixels; the height follows the design.
    Width(u32),
}

/// What lies behind the drawing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Background {
    #[default]
    Transparent,
    White,
}

/// The PNG export settings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PngOptions {
    pub resolution: Resolution,
    pub background: Background,
}

impl Default for PngOptions {
    fn default() -> Self {
        Self {
            resolution: Resolution::Dpi(DEFAULT_DPI),
            background: Background::Transparent,
        }
    }
}

impl PngOptions {
    /// Pixels per millimetre for a design `width_mm` wide.
    fn px_per_mm(&self, width_mm: f64) -> Result<f64, ExportError> {
        let k = match self.resolution {
            Resolution::Dpi(dpi) => dpi / MM_PER_INCH,
            Resolution::Width(px) => f64::from(px) / width_mm,
        };
        if k.is_finite() && k > 0.0 {
            Ok(k)
        } else {
            Err(ExportError::Unsupported(
                "The PNG resolution must be a positive number.".to_owned(),
            ))
        }
    }

    /// The image size for a design of `w` × `h` mm, or why it is too big.
    pub fn size(&self, w: f64, h: f64) -> Result<(u32, u32), ExportError> {
        let k = self.px_per_mm(w)?;
        let side = |mm: f64| (mm * k).ceil().max(1.0);
        let (pw, ph) = (side(w), side(h));
        let max = f64::from(MAX_SIDE);
        if pw > max || ph > max || pw * ph > MAX_PIXELS as f64 {
            return Err(ExportError::Unsupported(format!(
                "A {pw:.0} × {ph:.0} px PNG is too large (at most {MAX_SIDE} px a side and {} megapixels). Lower the resolution.",
                MAX_PIXELS / 1_000_000
            )));
        }
        // In range by the checks above.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        Ok((pw as u32, ph as u32))
    }
}

/// The PNG exporter, with its settings.
#[derive(Debug, Clone, Copy, Default)]
pub struct PngExporter {
    pub options: PngOptions,
}

impl Exporter for PngExporter {
    fn id(&self) -> &str {
        "png"
    }
    fn name(&self) -> &str {
        "PNG image"
    }
    fn extension(&self) -> &str {
        "png"
    }
    fn mime(&self) -> &str {
        "image/png"
    }
    fn export(&self, model: &ExportModel) -> Result<Exported, ExportError> {
        Ok(Exported {
            bytes: to_png_model(model, &self.options)?,
            extension: "png".to_owned(),
            mime: "image/png".to_owned(),
        })
    }
}

/// Rasterise a prepared model.
///
/// # Errors
/// [`ExportError::Unsupported`] when the image would be too large or the
/// resolution isn't positive; [`ExportError::Failed`] if encoding fails.
pub fn to_png_model(model: &ExportModel, opts: &PngOptions) -> Result<Vec<u8>, ExportError> {
    // The page: half of every stroke included (#450). A raster has no
    // coordinates to keep, so the anchor doesn't matter here.
    let (min, w, h) = match model.page() {
        Some((_, b)) => (b.min, b.width(), b.height()),
        None => (Vec2::ZERO, EMPTY_SIZE_MM, EMPTY_SIZE_MM),
    };
    // A lone horizontal or vertical line has no height; keep a pixel.
    let (w, h) = (
        w.max(EMPTY_SIZE_MM * 1e-3) + 2.0 * MARGIN_MM,
        h.max(EMPTY_SIZE_MM * 1e-3) + 2.0 * MARGIN_MM,
    );
    let min = min - Vec2::new(MARGIN_MM, MARGIN_MM);
    let (pw, ph) = opts.size(w, h)?;
    let k = opts.px_per_mm(w)?;
    let mut pixmap = Pixmap::new(pw, ph)
        .ok_or_else(|| ExportError::Failed("Could not allocate the image.".to_owned()))?;
    if opts.background == Background::White {
        pixmap.fill(Color::WHITE);
    }
    let top = min.y + h;
    // Document mm (Y up) → image pixels (Y down, origin at the top-left).
    #[allow(clippy::cast_possible_truncation)]
    let map = |p: Vec2| ((p.x - min.x) * k, (top - p.y) * k);
    let to32 = |v: f64| {
        #[allow(clippy::cast_possible_truncation)]
        let f = v as f32;
        f
    };
    for layer in &model.layers {
        for path in &layer.paths {
            let Some(skpath) = build(path, &map, k, &to32) else {
                continue;
            };
            let p = layer.paint(path);
            let alpha = (p.opacity.clamp(0.0, 1.0) * 255.0).round();
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let a = alpha as u8;
            if let Some([r, g, b]) = p.fill
                && path.closed
            {
                let mut paint = Paint::default();
                paint.set_color_rgba8(r, g, b, a);
                paint.anti_alias = true;
                pixmap.fill_path(
                    &skpath,
                    &paint,
                    FillRule::EvenOdd,
                    Transform::identity(),
                    None,
                );
            }
            let [r, g, b] = p.stroke;
            let mut paint = Paint::default();
            paint.set_color_rgba8(r, g, b, a);
            paint.anti_alias = true;
            let stroke = Stroke {
                // Thinner than a pixel would vanish: hairlines show as one pixel.
                width: to32((p.width * k).max(1.0)),
                line_cap: LineCap::Round,
                line_join: LineJoin::Round,
                ..Stroke::default()
            };
            pixmap.stroke_path(&skpath, &paint, &stroke, Transform::identity(), None);
        }
    }
    pixmap
        .encode_png()
        .map_err(|e| ExportError::Failed(format!("Could not encode the PNG: {e}")))
}

/// One export path as a tiny-skia path in pixels; `None` if empty.
fn build(
    path: &crate::pipeline::ExportPath,
    map: &impl Fn(Vec2) -> (f64, f64),
    k: f64,
    to32: &impl Fn(f64) -> f32,
) -> Option<tiny_skia::Path> {
    let mut pb = PathBuilder::new();
    let pt = |p: Vec2| {
        let (x, y) = map(p);
        (to32(x), to32(y))
    };
    if let [seg] = path.segs.as_slice()
        && let Shape::Circle(c) = seg.shape
    {
        let (x, y) = pt(c.center);
        pb.push_circle(x, y, to32(c.radius * k));
        return pb.finish();
    }
    let (x, y) = pt(path.start());
    pb.move_to(x, y);
    for s in &path.segs {
        match s.shape {
            Shape::Line(_) => {
                let (x, y) = pt(s.end());
                pb.line_to(x, y);
            }
            Shape::Arc(a) => {
                // Flattened finely enough that no pixel shows a facet: the
                // chord error stays under a tenth of a pixel.
                let r = a.radius();
                let sweep = a.sweep();
                let tol_mm = 0.1 / k;
                let step = if r > tol_mm {
                    2.0 * (1.0 - tol_mm / r).clamp(-1.0, 1.0).acos()
                } else {
                    sweep
                };
                let n = (sweep / step.max(1e-3)).ceil().clamp(1.0, 4096.0);
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let n = n as usize;
                let a0 = a.start_angle();
                for i in 1..=n {
                    // In travel order: a reversed segment runs end to start.
                    #[allow(clippy::cast_precision_loss)]
                    let t = i as f64 / n as f64;
                    let t = if s.reversed { 1.0 - t } else { t };
                    let p = a.center + Vec2::from_angle(a0 + sweep * t) * r;
                    let (x, y) = pt(p);
                    pb.line_to(x, y);
                }
            }
            Shape::Cubic(c) => {
                let (c1, c2) = if s.reversed {
                    (c.c1, c.c0)
                } else {
                    (c.c0, c.c1)
                };
                let (x1, y1) = pt(c1);
                let (x2, y2) = pt(c2);
                let (x, y) = pt(s.end());
                pb.cubic_to(x1, y1, x2, y2, x, y);
            }
            Shape::Circle(c) => {
                let (x, y) = pt(c.center);
                pb.push_circle(x, y, to32(c.radius * k));
            }
        }
    }
    if path.closed {
        pb.close();
    }
    pb.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::testutil::{layer, rect};
    use locus_doc::{EntityId, OperationKind};

    /// A 10 × 10 mm cut square at the origin, black hairline.
    fn square() -> ExportModel {
        ExportModel {
            layers: vec![layer(OperationKind::Cut, rect(1, 0.0, 0.0, 10.0, 10.0))],
            ..Default::default()
        }
    }

    fn decode(bytes: &[u8]) -> Pixmap {
        Pixmap::decode_png(bytes).unwrap()
    }

    fn opts(resolution: Resolution, background: Background) -> PngOptions {
        PngOptions {
            resolution,
            background,
        }
    }

    #[test]
    fn size_follows_the_dpi_or_the_pixel_width() {
        // 10 mm + the 0.1 mm hairline (#450) + 1 mm margin each side =
        // 12.1 mm; at 254 DPI that is just over 121 px, rounded up to 122.
        let png =
            to_png_model(&square(), &opts(Resolution::Dpi(254.0), Background::White)).unwrap();
        let img = decode(&png);
        assert_eq!((img.width(), img.height()), (122, 122));
        let png =
            to_png_model(&square(), &opts(Resolution::Width(600), Background::White)).unwrap();
        let img = decode(&png);
        assert_eq!((img.width(), img.height()), (600, 600));
    }

    #[test]
    fn background_is_transparent_or_white_and_the_outline_is_drawn() {
        let o = opts(Resolution::Dpi(254.0), Background::Transparent);
        let img = decode(&to_png_model(&square(), &o).unwrap());
        // A corner is in the margin: nothing there.
        assert_eq!(img.pixel(1, 1).unwrap().alpha(), 0);
        // The bottom edge of the square (y = 0 mm) is 1 mm above the image's
        // bottom, at y = 110 px: a one-pixel hairline on a pixel boundary,
        // so rows 109 and 110 share it. Black, at full strength together.
        let (a, b) = (img.pixel(60, 109).unwrap(), img.pixel(60, 110).unwrap());
        assert!(
            u16::from(a.alpha()) + u16::from(b.alpha()) > 200,
            "{a:?} {b:?}"
        );
        assert!(a.red() < 32 && b.red() < 32);
        let o = opts(Resolution::Dpi(254.0), Background::White);
        let img = decode(&to_png_model(&square(), &o).unwrap());
        let corner = img.pixel(1, 1).unwrap();
        assert_eq!(
            (corner.red(), corner.green(), corner.blue(), corner.alpha()),
            (255, 255, 255, 255)
        );
        // Inside the square stays background: no fill on a cut layer.
        assert_eq!(img.pixel(60, 60).unwrap().red(), 255);
    }

    #[test]
    fn oversized_or_invalid_resolutions_are_refused_not_allocated() {
        let huge = opts(Resolution::Dpi(1.0e6), Background::White);
        assert!(matches!(
            to_png_model(&square(), &huge),
            Err(ExportError::Unsupported(_))
        ));
        for bad in [0.0, -5.0, f64::NAN, f64::INFINITY] {
            let o = opts(Resolution::Dpi(bad), Background::White);
            assert!(to_png_model(&square(), &o).is_err(), "{bad}");
        }
        let o = opts(Resolution::Width(0), Background::White);
        assert!(to_png_model(&square(), &o).is_err());
        assert!(
            opts(Resolution::Width(MAX_SIDE + 1), Background::White)
                .size(10.0, 10.0)
                .is_err()
        );
    }

    #[test]
    fn an_empty_model_still_makes_a_valid_image() {
        let png = to_png_model(&ExportModel::default(), &PngOptions::default()).unwrap();
        let img = decode(&png);
        assert!(img.width() >= 1 && img.height() >= 1);
    }

    /// Per-layer files (#291) share the whole design's frame, so they line
    /// up: same size whatever each layer holds.
    #[test]
    fn per_layer_pngs_share_one_canvas() {
        let mut m = square();
        let mut small = layer(OperationKind::Score, rect(10, 2.0, 2.0, 4.0, 4.0));
        small.id = locus_doc::LayerId::new(2);
        m.layers.push(small);
        let files = crate::split_layers(&m);
        assert_eq!(files.len(), 2);
        let o = opts(Resolution::Dpi(254.0), Background::Transparent);
        let sizes: Vec<(u32, u32)> = files
            .iter()
            .map(|f| {
                let img = decode(&to_png_model(&f.model, &o).unwrap());
                (img.width(), img.height())
            })
            .collect();
        assert_eq!(sizes, [(122, 122), (122, 122)]);
    }

    #[test]
    fn a_selection_exports_only_its_geometry() {
        let mut m = square();
        m.layers[0]
            .paths
            .extend(crate::pipeline::chain(rect(10, 20.0, 0.0, 30.0, 5.0)));
        let keep = (10..14).map(EntityId::new).collect();
        let only = m.only(&keep);
        let segs: usize = only
            .layers
            .iter()
            .flat_map(|l| &l.paths)
            .map(|p| p.segs.len())
            .sum();
        assert_eq!(segs, 4);
        let b = only.bounds().unwrap();
        assert!((b.width() - 10.0).abs() < 1e-9 && (b.height() - 5.0).abs() < 1e-9);
        let o = opts(Resolution::Dpi(254.0), Background::White);
        let img = decode(&to_png_model(&only, &o).unwrap());
        assert_eq!((img.width(), img.height()), (122, 71));
    }

    #[test]
    fn the_registry_offers_png() {
        let r = crate::Registry::builtin();
        let png = r.get("png").unwrap().export(&square()).unwrap();
        assert_eq!(
            (png.extension.as_str(), png.mime.as_str()),
            ("png", "image/png")
        );
        assert_eq!(&png.bytes[..8], b"\x89PNG\r\n\x1a\n");
    }
}
