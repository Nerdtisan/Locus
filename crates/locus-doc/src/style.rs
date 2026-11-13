// SPDX-License-Identifier: AGPL-3.0-or-later

//! Per-object style (SPEC §4, §7; #287, ADR 0017): stroke colour, stroke
//! width, pen number, fill and opacity belong to each entity, not its layer.
//!
//! The layer keeps only its operation. Which parts of a style reach the
//! machine depends on that operation (SPEC §7): Draw layers export each
//! object's style, machine layers the profile's colour and hairline.

use crate::error::DocError;
use crate::operation::{MAX_LENGTH, MAX_PEN};
use serde::{Deserialize, Serialize};

/// Stroke colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Stroke {
    /// The layer's display colour. The default, so recolouring a layer
    /// recolours the objects that never had a colour of their own.
    #[default]
    ByLayer,
    /// An explicit `[r, g, b]`.
    Colour([u8; 3]),
}

/// Fill of a closed shape. On Engrave layers the fill is the hatch (SPEC §7),
/// whatever this says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Fill {
    #[default]
    None,
    Colour([u8; 3]),
}

/// One entity's style (SPEC §4).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Style {
    pub stroke: Stroke,
    /// Stroke (pen) width, mm, `>0..=MAX_LENGTH`. Drives the stroke-width
    /// preview and stroke-edge dimensions on Draw layers (ADR 0014).
    pub width: f64,
    /// Plotter pen, `1..=MAX_PEN`.
    pub pen: u32,
    pub fill: Fill,
    /// `0..=1`.
    pub opacity: f64,
}

/// The pen a Draw layer had before M5 (`DrawParams::default()`), so files that
/// never changed it migrate to the default style and save nothing extra.
pub const DEFAULT_WIDTH: f64 = 0.4;

impl Default for Style {
    fn default() -> Self {
        Self {
            stroke: Stroke::ByLayer,
            width: DEFAULT_WIDTH,
            pen: 1,
            fill: Fill::None,
            opacity: 1.0,
        }
    }
}

impl Style {
    /// Is this the default style? Default styles are left out of `.locus`
    /// files, so points and unstyled curves cost nothing (SPEC §10).
    #[must_use]
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// # Errors
    /// [`DocError::BadValue`] naming the offending field.
    pub fn validate(&self) -> Result<(), DocError> {
        if !(self.width.is_finite() && self.width > 0.0 && self.width <= MAX_LENGTH) {
            return Err(DocError::BadValue {
                what: "stroke width",
                value: self.width,
            });
        }
        if !(1..=MAX_PEN).contains(&self.pen) {
            return Err(DocError::BadValue {
                what: "pen number",
                value: f64::from(self.pen),
            });
        }
        if !(self.opacity.is_finite() && (0.0..=1.0).contains(&self.opacity)) {
            return Err(DocError::BadValue {
                what: "opacity",
                value: self.opacity,
            });
        }
        Ok(())
    }

    /// This style with `patch` applied.
    #[must_use]
    pub fn patched(mut self, patch: &StylePatch) -> Self {
        if let Some(s) = patch.stroke {
            self.stroke = s;
        }
        if let Some(w) = patch.width {
            self.width = w;
        }
        if let Some(p) = patch.pen {
            self.pen = p;
        }
        if let Some(f) = patch.fill {
            self.fill = f;
        }
        if let Some(o) = patch.opacity {
            self.opacity = o;
        }
        self
    }
}

/// The fields a [`Command::SetStyle`](crate::Command::SetStyle) changes;
/// `None` leaves that field as each object has it. This is how a mixed
/// selection is edited: typing a width sets every width and keeps each
/// object's own colour (`docs/ux/m5-design.md` §2.1).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct StylePatch {
    pub stroke: Option<Stroke>,
    pub width: Option<f64>,
    pub pen: Option<u32>,
    pub fill: Option<Fill>,
    pub opacity: Option<f64>,
}

impl StylePatch {
    /// A patch that sets every field to `style` (the eyedropper).
    #[must_use]
    pub fn all(style: Style) -> Self {
        Self {
            stroke: Some(style.stroke),
            width: Some(style.width),
            pen: Some(style.pen),
            fill: Some(style.fill),
            opacity: Some(style.opacity),
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_valid_and_matches_the_old_draw_pen() {
        let s = Style::default();
        s.validate().unwrap();
        assert!(s.is_default());
        assert_eq!((s.width, s.pen), (0.4, 1));
    }

    #[test]
    fn out_of_range_values_are_rejected() {
        let d = Style::default();
        for bad in [
            Style { width: 0.0, ..d },
            Style {
                width: f64::NAN,
                ..d
            },
            Style {
                width: MAX_LENGTH + 1.0,
                ..d
            },
            Style { pen: 0, ..d },
            Style {
                pen: MAX_PEN + 1,
                ..d
            },
            Style { opacity: 1.5, ..d },
            Style {
                opacity: f64::INFINITY,
                ..d
            },
        ] {
            assert!(matches!(bad.validate(), Err(DocError::BadValue { .. })));
        }
    }

    #[test]
    fn a_patch_changes_only_its_fields() {
        let red = Style {
            stroke: Stroke::Colour([255, 0, 0]),
            ..Style::default()
        };
        let p = StylePatch {
            width: Some(0.7),
            ..StylePatch::default()
        };
        let out = red.patched(&p);
        assert_eq!(out.stroke, Stroke::Colour([255, 0, 0]));
        assert_eq!(out.width, 0.7);
        assert_eq!(
            red.patched(&StylePatch::all(Style::default())),
            Style::default()
        );
    }
}
