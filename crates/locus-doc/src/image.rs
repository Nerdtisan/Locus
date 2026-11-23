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
        if !(self.mm_per_px.is_finite()
            && (MIN_MM_PER_PX..=MAX_MM_PER_PX).contains(&self.mm_per_px))
        {
            return Err(DocError::BadValue {
                what: "image scale",
                value: self.mm_per_px,
            });
        }
        if !(self.opacity.is_finite() && (0.0..=1.0).contains(&self.opacity)) {
            return Err(DocError::BadValue {
                what: "image opacity",
                value: self.opacity,
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
        let s = self.placement.mm_per_px;
        Vec2::new(f64::from(self.width_px) * s, f64::from(self.height_px) * s)
    }

    /// The image's corners, bottom-left and top-right, mm.
    #[must_use]
    pub fn rect(&self) -> (Vec2, Vec2) {
        let o = self.placement.origin;
        (o, o + self.size_mm())
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
