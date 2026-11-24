// SPDX-License-Identifier: AGPL-3.0-or-later

//! Raster reference images (#390, SPEC §4, §13 M4 reference import).
//!
//! A PNG or JPEG traced over: it lives on its own locked Reference layer,
//! is never exported (SPEC §7) and is drawn under the geometry at its own
//! opacity. The document keeps the file's original bytes, so a `.locus`
//! file opens anywhere; the app decodes them for display. Images are
//! untrusted input: the byte size, the pixel size and the count are bounded,
//! and the bytes must start like the format they claim.
//!
//! The bytes sit behind an [`Arc`], so undo patches and document clones
//! share one copy: changing the placement or opacity never copies the image.

use std::sync::Arc;

use locus_geom::Vec2;
use serde::{Deserialize, Serialize};

use crate::error::DocError;

/// Largest image file kept in a document.
pub const MAX_IMAGE_BYTES: usize = 32 * 1024 * 1024;
/// Largest side, pixels.
pub const MAX_IMAGE_SIDE: u32 = 16_384;
/// Most pixels (64 MP: 256 MB decoded as RGBA).
pub const MAX_IMAGE_PIXELS: u64 = 64_000_000;
/// Most reference images in one document.
pub const MAX_IMAGES: usize = 64;
/// Smallest and largest size of one pixel, mm (1 nm to 1 m), so a placement
/// can never put the image out of the finite world.
pub const MIN_MM_PER_PX: f64 = 1.0e-6;
pub const MAX_MM_PER_PX: f64 = 1.0e3;
/// Largest |origin| coordinate, mm (1 km, as imports, `locus_import`).
pub const MAX_ORIGIN_MM: f64 = 1.0e6;
/// The pixel size of an image that does not say: 96 dpi, as browsers use.
pub const DEFAULT_MM_PER_PX: f64 = 25.4 / 96.0;

/// The image file's format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImageFormat {
    Png,
    Jpeg,
}

impl ImageFormat {
    /// The format whose signature `bytes` start with.
    #[must_use]
    pub fn sniff(bytes: &[u8]) -> Option<Self> {
        if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            Some(Self::Png)
        } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
            Some(Self::Jpeg)
        } else {
            None
        }
    }
}

/// Where the image sits and how it is drawn. Changing it never touches the
/// pixels ([`Command::SetImagePlacement`](crate::Command::SetImagePlacement)).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ImagePlacement {
    /// The image's bottom-left corner, mm.
    pub origin: Vec2,
    /// The size of one pixel, mm (the scale Calibrate sets, #391).
    pub mm_per_px: f64,
    /// `0..=1`.
    pub opacity: f64,
    /// Turned counter-clockwise about the image's centre, radians, in
    /// `-π..=π` (#463, format v22). Omitted while 0.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub rotation: f64,
    /// A pixel's height, mm, when it differs from its width `mm_per_px`
    /// (a free X/Y scale, #463, format v23). Omitted for square pixels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mm_per_px_y: Option<f64>,
}

impl ImagePlacement {
    /// A pixel's width and height, mm.
    #[must_use]
    pub fn pixel(&self) -> Vec2 {
        Vec2::new(self.mm_per_px, self.mm_per_px_y.unwrap_or(self.mm_per_px))
    }
}

#[allow(clippy::trivially_copy_pass_by_ref)] // serde's signature
fn is_zero(v: &f64) -> bool {
    *v == 0.0
}

/// `a` wrapped into `-π..=π`.
#[must_use]
pub fn wrap_angle(a: f64) -> f64 {
    use std::f64::consts::{PI, TAU};
    let w = a.rem_euclid(TAU);
    if w > PI { w - TAU } else { w }
}

impl ImagePlacement {
    /// # Errors
    /// [`DocError::BadValue`] naming the offending field.
    pub fn validate(&self) -> Result<(), DocError> {
        let o = self.origin;
        for v in [o.x, o.y] {
            if !(v.is_finite() && v.abs() <= MAX_ORIGIN_MM) {
                return Err(DocError::BadValue {
                    what: "image position",
                    value: v,
                });
            }
        }
        for s in [Some(self.mm_per_px), self.mm_per_px_y]
            .into_iter()
            .flatten()
        {
            if !(s.is_finite() && (MIN_MM_PER_PX..=MAX_MM_PER_PX).contains(&s)) {
                return Err(DocError::BadValue {
                    what: "image scale",
                    value: s,
                });
            }
        }
        if !(self.opacity.is_finite() && (0.0..=1.0).contains(&self.opacity)) {
            return Err(DocError::BadValue {
                what: "image opacity",
                value: self.opacity,
            });
        }
        let pi = std::f64::consts::PI;
        if !(self.rotation.is_finite() && (-pi..=pi).contains(&self.rotation)) {
            return Err(DocError::BadValue {
                what: "image rotation",
                value: self.rotation,
            });
        }
        Ok(())
    }
}

/// A raster reference: the file's bytes, its pixel size and its placement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RefImage {
    pub format: ImageFormat,
    pub width_px: u32,
    pub height_px: u32,
    /// The original file, base64 in `.locus` files (format v16).
    #[serde(with = "base64_bytes")]
    pub data: Arc<[u8]>,
    pub placement: ImagePlacement,
}

impl RefImage {
    /// Its size on the canvas, mm.
    #[must_use]
    pub fn size_mm(&self) -> Vec2 {
        let s = self.placement.pixel();
        Vec2::new(
            f64::from(self.width_px) * s.x,
            f64::from(self.height_px) * s.y,
        )
    }

    /// The image's corners before its rotation, bottom-left and top-right,
    /// mm. With a rotation, see [`RefImage::corners`].
    #[must_use]
    pub fn rect(&self) -> (Vec2, Vec2) {
        let o = self.placement.origin;
        (o, o + self.size_mm())
    }

    /// The centre it turns about, mm.
    #[must_use]
    pub fn centre(&self) -> Vec2 {
        self.placement.origin + self.size_mm() * 0.5
    }

    /// Its corners as drawn: bottom-left, bottom-right, top-right, top-left
    /// of the image, turned by its rotation (#463).
    #[must_use]
    pub fn corners(&self) -> [Vec2; 4] {
        let (lo, hi) = self.rect();
        let turn = crate::transform::Transform::Rotate {
            center: self.centre(),
            angle: self.placement.rotation,
        };
        [lo, Vec2::new(hi.x, lo.y), hi, Vec2::new(lo.x, hi.y)].map(|p| turn.point(p))
    }

    /// Where image pixel coordinates `p` (x right, y down from the top-left
    /// corner, in pixels) are on the page, mm: through the placement, its
    /// pixel size and its turn (Auto trace, #461).
    #[must_use]
    pub fn pixel_to_world(&self, p: Vec2) -> Vec2 {
        let px = self.placement.pixel();
        let local = self.placement.origin
            + Vec2::new(p.x * px.x, (f64::from(self.height_px) - p.y) * px.y);
        crate::transform::Transform::Rotate {
            center: self.centre(),
            angle: self.placement.rotation,
        }
        .point(local)
    }

    /// Whether the drawn image covers `w`.
    #[must_use]
    pub fn contains(&self, w: Vec2) -> bool {
        let back = crate::transform::Transform::Rotate {
            center: self.centre(),
            angle: -self.placement.rotation,
        };
        let q = back.point(w);
        let (lo, hi) = self.rect();
        q.x >= lo.x && q.x <= hi.x && q.y >= lo.y && q.y <= hi.y
    }

    /// The box around the drawn image.
    #[must_use]
    pub fn bounds(&self) -> locus_geom::Aabb {
        let c = self.corners();
        locus_geom::Aabb::from_points(c[0], c[2]).union(locus_geom::Aabb::from_points(c[1], c[3]))
    }

    /// Its placement after `t` (#463): the centre moves with `t`, the
    /// rotation adds a turn and the pixel size a scale. `None` when `t` is
    /// a free X/Y scale whose axes aren't the image's own edges: the result
    /// would be sheared, which an image can't be.
    #[must_use]
    pub fn transformed_placement(&self, t: &crate::transform::Transform) -> Option<ImagePlacement> {
        use crate::transform::Transform;
        let p = self.placement;
        let centre = t.point(self.centre());
        let px = p.pixel();
        let (pixel, rotation) = match *t {
            Transform::Move(_) => (px, p.rotation),
            Transform::Rotate { angle, .. } => (px, wrap_angle(p.rotation + angle)),
            Transform::Scale { factor, .. } => (px * factor, p.rotation),
            Transform::Stretch { sx, sy, angle, .. } => {
                // Quarter turns between the stretch's axes and the image's.
                let q = (angle - p.rotation) / std::f64::consts::FRAC_PI_2;
                if (q - q.round()).abs() > 1e-9 {
                    return None;
                }
                let (fx, fy) = if (q.round() as i64).rem_euclid(2) == 0 {
                    (sx, sy)
                } else {
                    (sy, sx)
                };
                (Vec2::new(px.x * fx, px.y * fy), p.rotation)
            }
        };
        let size = Vec2::new(
            f64::from(self.width_px) * pixel.x,
            f64::from(self.height_px) * pixel.y,
        );
        let square = (pixel.x - pixel.y).abs() <= 1e-12 * pixel.x.abs();
        Some(ImagePlacement {
            origin: centre - size * 0.5,
            mm_per_px: pixel.x,
            mm_per_px_y: (!square).then_some(pixel.y),
            rotation,
            ..p
        })
    }

    /// The image's own invariants (it is untrusted input).
    ///
    /// # Errors
    /// [`DocError::Invalid`] or [`DocError::BadValue`] saying what is wrong.
    pub fn validate(&self) -> Result<(), DocError> {
        if self.data.len() > MAX_IMAGE_BYTES {
            return Err(DocError::Invalid("the image file is too large".to_owned()));
        }
        if ImageFormat::sniff(&self.data) != Some(self.format) {
            return Err(DocError::Invalid(
                "the image data is not the format it claims".to_owned(),
            ));
        }
        let (w, h) = (self.width_px, self.height_px);
        if w == 0
            || h == 0
            || w > MAX_IMAGE_SIDE
            || h > MAX_IMAGE_SIDE
            || u64::from(w) * u64::from(h) > MAX_IMAGE_PIXELS
        {
            return Err(DocError::Invalid(format!(
                "an image must be 1 to {MAX_IMAGE_SIDE} pixels a side and at most {MAX_IMAGE_PIXELS} pixels"
            )));
        }
        self.placement.validate()?;
        // The far corner too, so the image stays in the finite world.
        let (_, top) = self.rect();
        if !(top.x.abs() <= 2.0 * MAX_ORIGIN_MM && top.y.abs() <= 2.0 * MAX_ORIGIN_MM) {
            return Err(DocError::BadValue {
                what: "image size",
                value: top.x.max(top.y),
            });
        }
        Ok(())
    }
}

/// `Arc<[u8]>` as standard base64 text, so a `.locus` file stays text and a
/// few megabytes don't become a list of numbers.
mod base64_bytes {
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD;
    use serde::{Deserialize, Deserializer, Serializer};
    use std::sync::Arc;

    pub fn serialize<S: Serializer>(data: &Arc<[u8]>, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&STANDARD.encode(data))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Arc<[u8]>, D::Error> {
        let text = String::deserialize(d)?;
        // Bound before decoding: base64 is 4/3 of the bytes.
        if text.len() > super::MAX_IMAGE_BYTES / 3 * 4 + 4 {
            return Err(serde::de::Error::custom("the image file is too large"));
        }
        STANDARD
            .decode(text.as_bytes())
            .map(Arc::from)
            .map_err(|_| serde::de::Error::custom("the image data is not valid base64"))
    }
}

/// A small valid-looking PNG reference, for tests.
#[cfg(test)]
pub(crate) fn tiny_png() -> RefImage {
    RefImage {
        format: ImageFormat::Png,
        width_px: 4,
        height_px: 2,
        data: Arc::from(&b"\x89PNG\r\n\x1a\nnot really"[..]),
        placement: ImagePlacement {
            origin: Vec2::new(1.0, 2.0),
            mm_per_px: 0.5,
            opacity: 0.5,
            rotation: 0.0,
            mm_per_px_y: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_and_corners() {
        let i = tiny_png();
        i.validate().unwrap();
        assert_eq!(i.size_mm(), Vec2::new(2.0, 1.0));
        assert_eq!(i.rect().1, Vec2::new(3.0, 3.0));
    }

    /// Hostile values are refused, never stored (CLAUDE.md: untrusted input).
    #[test]
    fn bad_images_are_refused() {
        let ok = tiny_png();
        let mut bad = Vec::new();
        bad.push(RefImage {
            format: ImageFormat::Jpeg,
            ..ok.clone()
        });
        bad.push(RefImage {
            width_px: 0,
            ..ok.clone()
        });
        bad.push(RefImage {
            width_px: MAX_IMAGE_SIDE,
            height_px: MAX_IMAGE_SIDE,
            ..ok.clone()
        });
        for p in [
            ImagePlacement {
                mm_per_px: 0.0,
                ..ok.placement
            },
            ImagePlacement {
                mm_per_px: f64::NAN,
                ..ok.placement
            },
            ImagePlacement {
                opacity: 1.5,
                ..ok.placement
            },
            ImagePlacement {
                origin: Vec2::new(f64::INFINITY, 0.0),
                ..ok.placement
            },
        ] {
            bad.push(RefImage {
                placement: p,
                ..ok.clone()
            });
        }
        for b in bad {
            assert!(b.validate().is_err(), "{:?}", b.placement);
        }
    }

    /// One locked Reference layer with an image, added as one step.
    fn with_image() -> (crate::Editor, crate::LayerId) {
        use crate::{Command, Editor, LayerId};
        let mut ed = Editor::new(crate::Document::new());
        let layer = LayerId::new(ed.document().next_id());
        ed.apply(&Command::Compound {
            label: "Import photo".to_owned(),
            commands: vec![
                Command::AddLayer {
                    name: "photo".to_owned(),
                    colour: crate::reference::REFERENCE_COLOUR,
                    operation: crate::Operation::Reference,
                },
                Command::AddImage {
                    layer,
                    image: Box::new(tiny_png()),
                },
                Command::SetLayerLocked {
                    id: layer,
                    locked: true,
                },
            ],
        })
        .unwrap();
        (ed, layer)
    }

    #[test]
    fn images_go_on_reference_layers_only() {
        use crate::{Command, Document};
        let mut ed = crate::Editor::new(Document::new());
        let err = ed.apply(&Command::AddImage {
            layer: Document::default_layer(),
            image: Box::new(tiny_png()),
        });
        assert!(err.is_err(), "a Draw layer");
        let (mut ed, layer) = with_image();
        assert!(ed.document().image(layer).is_some());
        // A layer with an image stays a Reference layer.
        let op = ed.apply(&Command::SetLayerProps {
            id: layer,
            props: crate::LayerProps {
                operation: Some(crate::Operation::Draw),
                ..Default::default()
            },
        });
        assert!(op.is_err());
        ed.undo().unwrap();
        assert!(ed.document().images().is_empty(), "one undo step");
    }

    /// #463: a reference image moves, turns about a point and scales with
    /// `TransformReference` on its locked layer, one undo step each; the
    /// turn is stored and wraps into -π..=π.
    #[test]
    fn transform_reference_moves_turns_and_scales_the_image() {
        use crate::Command;
        use crate::transform::Transform;
        use std::f64::consts::{FRAC_PI_2, PI};
        let (mut ed, layer) = with_image();
        let start = ed.document().image(layer).cloned().unwrap();
        let c0 = start.centre();
        let close = |a: Vec2, b: Vec2| a.distance(b) < 1e-9;
        let apply = |ed: &mut crate::Editor, transform| {
            ed.apply(&Command::TransformReference { layer, transform })
                .unwrap();
            ed.document().image(layer).cloned().unwrap()
        };
        let moved = apply(&mut ed, Transform::Move(Vec2::new(3.0, -1.0)));
        assert!(close(moved.centre(), c0 + Vec2::new(3.0, -1.0)));
        assert_eq!(ed.undo_label(), Some("Move reference"));
        // A quarter turn about the centre.
        let turned = apply(
            &mut ed,
            Transform::Rotate {
                center: moved.centre(),
                angle: FRAC_PI_2,
            },
        );
        assert!((turned.placement.rotation - FRAC_PI_2).abs() < 1e-12);
        assert!(close(turned.centre(), moved.centre()));
        let [bl, ..] = turned.corners();
        let (lo, _) = moved.rect();
        let c = moved.centre();
        // (x, y) about c by +90° is (-y, x).
        assert!(close(bl, c + Vec2::new(c.y - lo.y, lo.x - c.x)));
        // Two more quarter turns: π, then wrapped to -π/2.
        apply(
            &mut ed,
            Transform::Rotate {
                center: c,
                angle: FRAC_PI_2,
            },
        );
        let wrapped = apply(
            &mut ed,
            Transform::Rotate {
                center: c,
                angle: FRAC_PI_2,
            },
        );
        assert!((wrapped.placement.rotation + FRAC_PI_2).abs() < 1e-12);
        assert!(wrapped.placement.rotation >= -PI);
        // Scale about a corner keeps that corner and doubles the pixel.
        let corner = wrapped.corners()[0];
        let scaled = apply(
            &mut ed,
            Transform::Scale {
                center: corner,
                factor: 2.0,
            },
        );
        assert!(close(scaled.corners()[0], corner));
        assert!((scaled.placement.mm_per_px - 2.0 * wrapped.placement.mm_per_px).abs() < 1e-12);
        assert_eq!(ed.undo_label(), Some("Scale reference"));
        for _ in 0..5 {
            ed.undo().unwrap();
        }
        assert_eq!(ed.document().image(layer), Some(&start));
    }

    /// #463: a free scale goes along the image's own edges, even turned
    /// (a quarter turn swaps X and Y), and is refused across them.
    #[test]
    fn a_free_scale_stretches_the_image_along_its_edges() {
        use crate::Command;
        use crate::transform::Transform;
        use std::f64::consts::{FRAC_PI_2, FRAC_PI_4};
        let (mut ed, layer) = with_image();
        let img = ed.document().image(layer).cloned().unwrap();
        let size = img.size_mm();
        let stretch = |angle| Transform::Stretch {
            center: img.centre(),
            sx: 3.0,
            sy: 1.0,
            angle,
        };
        ed.apply(&Command::TransformReference {
            layer,
            transform: stretch(0.0),
        })
        .unwrap();
        let wide = ed.document().image(layer).cloned().unwrap();
        assert!((wide.size_mm().x - 3.0 * size.x).abs() < 1e-9);
        assert!((wide.size_mm().y - size.y).abs() < 1e-9);
        assert!(wide.centre().distance(img.centre()) < 1e-9);
        assert!(wide.placement.mm_per_px_y.is_some());
        ed.undo().unwrap();
        // A quarter turn between the axes: the stretch goes on Y.
        ed.apply(&Command::TransformReference {
            layer,
            transform: stretch(FRAC_PI_2),
        })
        .unwrap();
        let tall = ed.document().image(layer).cloned().unwrap();
        assert!((tall.size_mm().y - 3.0 * size.y).abs() < 1e-9);
        ed.undo().unwrap();
        // Across the edges it would shear: refused, nothing changes.
        assert!(
            ed.apply(&Command::TransformReference {
                layer,
                transform: stretch(FRAC_PI_4),
            })
            .is_err()
        );
        assert_eq!(ed.document().image(layer), Some(&img));
        // A uniform scale back to square pixels drops the Y size.
        let mut p = img.placement;
        p.mm_per_px_y = Some(p.mm_per_px * 2.0);
        assert!(
            ImagePlacement {
                mm_per_px_y: Some(f64::NAN),
                ..p
            }
            .validate()
            .is_err()
        );
    }

    /// #461: image pixels map to the page through the placement: the
    /// top-left pixel corner is the drawn top-left corner, turned too.
    #[test]
    fn pixels_map_to_the_page_through_the_placement() {
        let mut img = tiny_png();
        img.placement.origin = Vec2::new(10.0, 20.0);
        img.placement.mm_per_px = 2.0;
        let (w, h) = (f64::from(img.width_px), f64::from(img.height_px));
        let [bl, br, tr, tl] = img.corners();
        assert!(img.pixel_to_world(Vec2::new(0.0, 0.0)).distance(tl) < 1e-12);
        assert!(img.pixel_to_world(Vec2::new(w, h)).distance(br) < 1e-12);
        img.placement.rotation = 1.0;
        let [bl2, _, tr2, _] = img.corners();
        assert!(img.pixel_to_world(Vec2::new(0.0, h)).distance(bl2) < 1e-12);
        assert!(img.pixel_to_world(Vec2::new(w, 0.0)).distance(tr2) < 1e-12);
        let _ = (bl, tr);
    }

    /// A turned image covers its turned corners, not its old box.
    #[test]
    fn a_turned_image_contains_points_by_its_drawn_shape() {
        let mut img = tiny_png();
        img.placement.mm_per_px = 1.0;
        img.placement.origin = Vec2::new(0.0, 0.0);
        img.width_px = 10;
        img.height_px = 2;
        assert!(img.contains(Vec2::new(9.0, 1.0)));
        img.placement.rotation = std::f64::consts::FRAC_PI_2;
        // Now 2 wide and 10 tall about the centre (5, 1).
        assert!(!img.contains(Vec2::new(9.0, 1.0)));
        assert!(img.contains(Vec2::new(5.0, 5.0)));
        let b = img.bounds();
        assert!((b.max.y - b.min.y - 10.0).abs() < 1e-9);
        assert!((b.max.x - b.min.x - 2.0).abs() < 1e-9);
    }

    #[test]
    fn bad_rotations_are_refused() {
        let ok = tiny_png();
        for r in [f64::NAN, f64::INFINITY, 4.0, -4.0] {
            let mut img = ok.clone();
            img.placement.rotation = r;
            assert!(img.validate().is_err(), "{r}");
        }
    }

    /// Placement and opacity change on the locked layer, as one undo step
    /// each, and share the pixels with the history.
    #[test]
    fn placement_changes_on_a_locked_layer_and_undoes() {
        use crate::Command;
        let (mut ed, layer) = with_image();
        let before = ed.document().image(layer).cloned().unwrap();
        let p = ImagePlacement {
            mm_per_px: 2.0,
            opacity: 0.25,
            ..before.placement
        };
        ed.apply(&Command::SetImagePlacement {
            layer,
            placement: p,
        })
        .unwrap();
        let after = ed.document().image(layer).unwrap();
        assert_eq!(after.placement, p);
        assert!(Arc::ptr_eq(&after.data, &before.data), "pixels shared");
        let bad = ImagePlacement { opacity: -1.0, ..p };
        assert!(
            ed.apply(&Command::SetImagePlacement {
                layer,
                placement: bad
            })
            .is_err()
        );
        ed.undo().unwrap();
        assert_eq!(ed.document().image(layer), Some(&before));
    }

    #[test]
    fn deleting_the_layer_takes_the_image_and_undo_brings_it_back() {
        use crate::Command;
        let (mut ed, layer) = with_image();
        ed.apply(&Command::SetLayerLocked {
            id: layer,
            locked: false,
        })
        .unwrap();
        ed.apply(&Command::DeleteLayer { id: layer }).unwrap();
        assert!(ed.document().images().is_empty());
        ed.undo().unwrap();
        assert!(ed.document().image(layer).is_some());
    }

    /// #391: calibrating an image scales its pixel size and moves its
    /// corner about the picked point; undo puts it back.
    #[test]
    fn calibrate_scales_an_image() {
        // (#464: Calibrate passes the anchor, the origin, by default; this
        // checks the command scales about whatever point it is given.)
        use crate::Command;
        let (mut ed, layer) = with_image();
        let before = ed.document().image(layer).cloned().unwrap();
        ed.apply(&Command::ScaleReference {
            layer,
            about: Vec2::new(1.0, 2.0),
            factor: 4.0,
        })
        .unwrap();
        let after = ed.document().image(layer).unwrap();
        assert_eq!(
            after.placement.origin,
            Vec2::new(1.0, 2.0),
            "the pivot stays"
        );
        assert_eq!(after.size_mm(), Vec2::new(8.0, 4.0));
        ed.undo().unwrap();
        assert_eq!(ed.document().image(layer), Some(&before));
    }

    /// #464: an image's anchor is its placement origin; scaled about it,
    /// ×2 then ×0.5, the image's corner doesn't move.
    #[test]
    fn scaling_an_image_about_its_anchor_keeps_it_in_place() {
        use crate::Command;
        let (mut ed, layer) = with_image();
        let at = crate::reference::anchor(ed.document(), layer).unwrap();
        assert_eq!(at, ed.document().image(layer).unwrap().placement.origin);
        for f in [2.0, 0.5] {
            ed.apply(&Command::ScaleReference {
                layer,
                about: crate::reference::anchor(ed.document(), layer).unwrap(),
                factor: f,
            })
            .unwrap();
            assert_eq!(
                ed.document().image(layer).unwrap().placement.origin,
                at,
                "×{f}"
            );
        }
        assert_eq!(
            ed.document().image(layer).unwrap().size_mm(),
            Vec2::new(2.0, 1.0)
        );
    }

    /// #468: an image centred on 0,0, scaled about its centre ×0.0875 and
    /// ×2, stays centred on 0,0.
    #[test]
    fn scaling_an_image_about_its_centre_keeps_it_centred() {
        use crate::Command;
        let (mut ed, layer) = with_image();
        // Centre it on 0,0 first.
        let mut p = ed.document().image(layer).unwrap().placement;
        p.origin = ed.document().image(layer).unwrap().size_mm() * -0.5;
        ed.apply(&Command::SetImagePlacement {
            layer,
            placement: p,
        })
        .unwrap();
        let mid = crate::reference::centre(ed.document(), layer).unwrap();
        assert!(mid.length() < 1e-12, "{mid:?}");
        for f in [0.0875, 2.0] {
            ed.apply(&Command::ScaleReference {
                layer,
                about: crate::reference::centre(ed.document(), layer).unwrap(),
                factor: f,
            })
            .unwrap();
            let now = crate::reference::centre(ed.document(), layer).unwrap();
            assert!(now.length() < 1e-9, "×{f}: {now:?}");
        }
    }

    #[test]
    fn base64_round_trips_and_bounds() {
        let i = tiny_png();
        let text = ron::to_string(&i).unwrap();
        assert!(text.contains("data:\"iVBORw0KGg"), "{text}");
        let back: RefImage = ron::from_str(&text).unwrap();
        assert_eq!(back, i);
        let broken = text.replace("iVBORw0KGg", "!!!!");
        assert!(ron::from_str::<RefImage>(&broken).is_err());
    }
}
