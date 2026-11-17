// SPDX-License-Identifier: AGPL-3.0-or-later
//! Raster reference images on the canvas (#390): decoded once into a
//! texture, drawn under the sketch at their own opacity, never exported.
//!
//! The cache holds each image's bytes (`Arc`), so an entry is rebuilt only
//! when the document's image is replaced, not when it moves or fades. A file
//! that can't be decoded draws as an outlined box with a note, never a
//! panic: `.locus` files are untrusted input.

use std::collections::HashMap;
use std::sync::Arc;

use egui::{Align2, Color32, FontId, Painter, Rect, Stroke, TextureHandle, pos2};
use locus_doc::LayerId;
use locus_geom::Vec2;

use crate::state::AppState;

/// One decoded image: the bytes it came from and its texture, `None` when
/// the bytes could not be decoded.
#[derive(Clone)]
struct Entry {
    data: Arc<[u8]>,
    tex: Option<TextureHandle>,
}

/// Textures for the document's reference images, by layer.
#[derive(Clone, Default)]
pub(crate) struct ImageCache {
    entries: HashMap<LayerId, Entry>,
}

impl std::fmt::Debug for ImageCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImageCache")
            .field("images", &self.entries.len())
            .finish()
    }
}

impl ImageCache {
    /// The texture for `layer`'s image, decoding it on first use. `None`
    /// if it can't be decoded.
    fn texture(
        &mut self,
        ctx: &egui::Context,
        layer: LayerId,
        data: &Arc<[u8]>,
    ) -> Option<TextureHandle> {
        let fresh = self
            .entries
            .get(&layer)
            .is_some_and(|e| Arc::ptr_eq(&e.data, data));
        if !fresh {
            let max = ctx.input(|i| i.max_texture_side);
            let max = u32::try_from(max).unwrap_or(u32::MAX);
            let tex = locus_import::raster::decode_fit(data, max)
                .ok()
                .and_then(|d| {
                    let size = [
                        usize::try_from(d.width).ok()?,
                        usize::try_from(d.height).ok()?,
                    ];
                    let img = egui::ColorImage::from_rgba_unmultiplied(size, &d.pixels);
                    Some(ctx.load_texture(
                        format!("locus-ref-image-{}", layer.get()),
                        img,
                        egui::TextureOptions::LINEAR,
                    ))
                });
            self.entries.insert(
                layer,
                Entry {
                    data: Arc::clone(data),
                    tex,
                },
            );
        }
        self.entries.get(&layer).and_then(|e| e.tex.clone())
    }

    /// Forget images that are gone from the document.
    fn retain(&mut self, keep: impl Fn(&LayerId) -> bool) {
        self.entries.retain(|l, _| keep(l));
    }

    /// Is `layer`'s image decoded (for tests)?
    #[cfg(test)]
    pub(crate) fn decoded(&self, layer: LayerId) -> Option<bool> {
        self.entries.get(&layer).map(|e| e.tex.is_some())
    }
}

/// The part of `image` (world rectangle `lo`–`hi`) inside the world window
/// `w_lo`–`w_hi`: that world rectangle and its UV rectangle (v down). The
/// arithmetic is in f64 so a deep zoom never overflows f32 screen space.
fn visible_part(lo: Vec2, hi: Vec2, w_lo: Vec2, w_hi: Vec2) -> Option<(Vec2, Vec2, Rect)> {
    let a = Vec2::new(lo.x.max(w_lo.x), lo.y.max(w_lo.y));
    let b = Vec2::new(hi.x.min(w_hi.x), hi.y.min(w_hi.y));
    if a.x >= b.x || a.y >= b.y {
        return None;
    }
    let (sx, sy) = (hi.x - lo.x, hi.y - lo.y);
    let u = |x: f64| ((x - lo.x) / sx) as f32;
    // Row 0 is the image's top, at world `hi.y`.
    let v = |y: f64| ((hi.y - y) / sy) as f32;
    Some((
        a,
        b,
        Rect::from_min_max(pos2(u(a.x), v(b.y)), pos2(u(b.x), v(a.y))),
    ))
}

impl AppState {
    /// Draw every visible layer's reference image under the sketch (#390).
    pub(crate) fn draw_images(
        &mut self,
        ctx: &egui::Context,
        painter: &Painter,
        rect: Rect,
        dark: bool,
    ) {
        let doc = self.doc.sketcher.document();
        let images: Vec<(LayerId, locus_doc::RefImage)> = doc
            .images()
            .iter()
            .filter(|(l, _)| doc.layer(**l).is_some_and(|l| l.visible))
            .map(|(l, i)| (*l, i.clone()))
            .collect();
        let all: Vec<LayerId> = doc.images().keys().copied().collect();
        self.view.images.retain(|l| all.contains(l));
        let cam = self.view.camera;
        let (c0, c1) = (
            cam.to_world(rect.left_bottom(), rect),
            cam.to_world(rect.right_top(), rect),
        );
        for (layer, img) in images {
            let (lo, hi) = img.rect();
            let Some((a, b, uv)) = visible_part(lo, hi, c0, c1) else {
                continue;
            };
            let screen = Rect::from_two_pos(cam.to_screen(a, rect), cam.to_screen(b, rect));
            match self.view.images.texture(ctx, layer, &img.data) {
                Some(tex) => {
                    let alpha = (img.placement.opacity.clamp(0.0, 1.0) * 255.0).round() as u8;
                    let tint = Color32::from_white_alpha(alpha);
                    painter.image(tex.id(), screen, uv, tint);
                }
                None => {
                    let ink = if dark {
                        Color32::from_gray(150)
                    } else {
                        Color32::from_gray(110)
                    };
                    painter.rect_stroke(
                        screen,
                        0.0,
                        Stroke::new(1.0, ink),
                        egui::StrokeKind::Inside,
                    );
                    painter.text(
                        screen.center(),
                        Align2::CENTER_CENTER,
                        "This image can't be shown",
                        FontId::proportional(12.0),
                        ink,
                    );
                }
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // test fixtures
mod tests {
    use super::*;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba([200, 0, 0, 255]));
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    #[test]
    fn the_visible_part_maps_to_its_uvs() {
        let (lo, hi) = (Vec2::new(0.0, 0.0), Vec2::new(10.0, 20.0));
        // The whole image.
        let (_, _, uv) =
            visible_part(lo, hi, Vec2::new(-5.0, -5.0), Vec2::new(50.0, 50.0)).unwrap();
        assert_eq!(uv, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)));
        // The bottom-right quarter: u from 0.5, v (down) from 0.5.
        let (a, b, uv) = visible_part(lo, hi, Vec2::new(5.0, -5.0), Vec2::new(50.0, 10.0)).unwrap();
        assert_eq!((a, b), (Vec2::new(5.0, 0.0), Vec2::new(10.0, 10.0)));
        assert_eq!(uv, Rect::from_min_max(pos2(0.5, 0.5), pos2(1.0, 1.0)));
        assert!(visible_part(lo, hi, Vec2::new(20.0, 0.0), Vec2::new(30.0, 5.0)).is_none());
    }

    /// #390 end to end in real egui frames: an imported PNG is decoded and
    /// drawn; a damaged file in a `.locus` draws a note instead of
    /// panicking.
    #[test]
    fn imported_images_draw_and_bad_ones_do_not_panic() {
        let mut c = AppState::default();
        c.load(locus_doc::Document::new());
        let bytes = png(40, 20);
        let (cmd, _) = locus_import::reference_command_at(
            c.document(),
            "photo.png",
            &bytes,
            Vec2::new(150.0, 100.0),
        )
        .unwrap();
        c.apply(&cmd).unwrap();
        let layer = *c.document().images().keys().next().unwrap();
        let ctx = egui::Context::default();
        let settings = crate::settings::Settings::default();
        let run = |c: &mut AppState| {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(
                    pos2(0.0, 0.0),
                    egui::vec2(800.0, 600.0),
                )),
                ..Default::default()
            };
            let mut status = crate::app::Status::default();
            let _ = ctx.run_ui(input, |ui| c.canvas_ui(ui, &settings, &mut status));
        };
        run(&mut c);
        run(&mut c);
        assert_eq!(c.view.images.decoded(layer), Some(true));
        // A file whose image bytes are damaged (but still claim PNG) loads,
        // since the document checks only the signature and sizes, and draws
        // a note.
        let text = locus_doc::file::save(c.document()).unwrap();
        let good = {
            use base64::Engine as _;
            base64::engine::general_purpose::STANDARD.encode(&bytes)
        };
        let damaged = {
            use base64::Engine as _;
            let mut b = bytes.clone();
            b.truncate(30);
            base64::engine::general_purpose::STANDARD.encode(&b)
        };
        let doc = locus_doc::file::load(&text.replace(&good, &damaged)).unwrap();
        c.load(doc);
        run(&mut c);
        assert_eq!(c.view.images.decoded(layer), Some(false));
    }
}
