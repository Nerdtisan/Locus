// SPDX-License-Identifier: AGPL-3.0-or-later

//! Per-object style (SPEC §4, §7; #287, ADR 0017): stroke colour, stroke
//! width, pen number, fill and opacity belong to each entity, not its layer.
//!
//! The layer keeps only its operation. Which parts of a style reach the
//! machine depends on that operation (SPEC §7): Draw layers export each
//! object's style, machine layers the profile's colour and hairline.

use crate::error::DocError;
use crate::operation::{MAX_LENGTH, MAX_PEN};
use locus_stroke::{Align, Cap, Dash, Join, LineStyle};
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
    /// No fill, keeping the colour last chosen, so turning fill back on
    /// restores it (#388, format v15). Draws and exports as [`Fill::None`].
    Off([u8; 3]),
}

impl Fill {
    /// The colour that is painted, if any.
    #[must_use]
    pub fn painted(self) -> Option<[u8; 3]> {
        match self {
            Fill::Colour(c) => Some(c),
            Fill::None | Fill::Off(_) => None,
        }
    }

    /// The colour shown in the fill swatch: painted or remembered.
    #[must_use]
    pub fn colour(self) -> Option<[u8; 3]> {
        match self {
            Fill::Colour(c) | Fill::Off(c) => Some(c),
            Fill::None => None,
        }
    }

    /// Fill turned on or off, keeping the colour (#388). Turning on a fill
    /// that never had a colour uses `fallback`, or stays unfilled without.
    #[must_use]
    pub fn switched(self, on: bool, fallback: Option<[u8; 3]>) -> Fill {
        match (on, self) {
            (true, Fill::Off(c)) => Fill::Colour(c),
            (true, Fill::None) => fallback.map_or(Fill::None, Fill::Colour),
            (false, Fill::Colour(c)) => Fill::Off(c),
            (_, f) => f,
        }
    }
}

/// One entity's style (SPEC §4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// Caps, joins and alignment (#428, format v25; ADR 0019 decision 1).
    /// Omitted at its default (round caps and joins, centred), so files
    /// that never styled a line are unchanged.
    #[serde(default, skip_serializing_if = "LineStyle::is_default")]
    pub line: LineStyle,
}

/// How a machine layer (Cut, Score, Engrave, Drag knife) treats its
/// objects' line styles (#428, ADR 0019 decision 4; owner default
/// 2026-11-24: Centreline). Draw layers ignore it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum StrokeStyles {
    /// Cut or mark the real path, as before M6. Styles are visual only.
    #[default]
    Centreline,
    /// Cut or mark the stroke's outline (Engrave hatches it).
    Styled,
}

impl StrokeStyles {
    #[must_use]
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
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
            line: LineStyle::default(),
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
        self.line.validate().map_err(|e| match e {
            locus_stroke::StrokeError::BadValue { what, value } => {
                DocError::BadValue { what, value }
            }
            other => DocError::Invalid(other.to_string()),
        })?;
        // Fills belong to regions since format v19 (#451): an object has
        // none, so no fill can hide another object's line.
        if self.fill != Fill::None {
            return Err(DocError::Invalid(
                "objects have no fill: fill a region instead (SetRegionFill)".to_owned(),
            ));
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
        if let Some(on) = patch.fill_on {
            let fallback = patch.fill.and_then(Fill::painted);
            self.fill = self.fill.switched(on, fallback);
        } else if let Some(f) = patch.fill {
            self.fill = f;
        }
        if let Some(o) = patch.opacity {
            self.opacity = o;
        }
        if let Some(c) = patch.cap {
            self.line.cap = c;
        }
        if let Some(j) = patch.join {
            self.line.join = j;
        }
        if let Some(a) = patch.align {
            self.line.align = a;
        }
        if let Some(d) = patch.dash {
            self.line.dash = d;
        }
        if let Some(x) = &patch.extra {
            self.line.extra.clone_from(x);
        }
        self
    }
}

/// The fields a [`Command::SetStyle`](crate::Command::SetStyle) changes;
/// `None` leaves that field as each object has it. This is how a mixed
/// selection is edited: typing a width sets every width and keeps each
/// object's own colour (`docs/ux/m5-design.md` §2.1).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct StylePatch {
    pub stroke: Option<Stroke>,
    pub width: Option<f64>,
    pub pen: Option<u32>,
    pub fill: Option<Fill>,
    pub opacity: Option<f64>,
    /// Turn each object's fill on or off, keeping its own colour (#388), so
    /// a mixed selection keeps its colours. With this set, `fill` is only
    /// the colour for objects that never had one.
    pub fill_on: Option<bool>,
    /// End caps, corner joins and alignment (#428).
    pub cap: Option<Cap>,
    pub join: Option<Join>,
    pub align: Option<Align>,
    /// Dash pattern (#427): `Some(None)` makes the line solid.
    pub dash: Option<Option<Dash>>,
    /// Extra strokes (#429): the whole list, in paint order.
    pub extra: Option<Vec<locus_stroke::StrokeLayer>>,
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
            fill_on: None,
            cap: Some(style.line.cap),
            join: Some(style.line.join),
            align: Some(style.line.align),
            dash: Some(style.line.dash),
            extra: Some(style.line.extra.clone()),
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
            Style {
                width: 0.0,
                ..d.clone()
            },
            Style {
                width: f64::NAN,
                ..d.clone()
            },
            Style {
                width: MAX_LENGTH + 1.0,
                ..d.clone()
            },
            Style {
                pen: 0,
                ..d.clone()
            },
            Style {
                pen: MAX_PEN + 1,
                ..d.clone()
            },
            Style {
                opacity: 1.5,
                ..d.clone()
            },
            Style {
                opacity: f64::INFINITY,
                ..d.clone()
            },
        ] {
            assert!(matches!(bad.validate(), Err(DocError::BadValue { .. })));
        }
    }

    /// Regression (#388): turning fill off kept no colour, so turning it
    /// back on reset it to the default. Off now remembers it, per object.
    #[test]
    fn fill_off_and_on_keeps_each_colour() {
        let red = Style {
            fill: Fill::Colour([200, 0, 0]),
            ..Style::default()
        };
        let blue = Style {
            fill: Fill::Colour([0, 0, 200]),
            ..Style::default()
        };
        let plain = Style::default();
        let switch = |on| StylePatch {
            fill_on: Some(on),
            fill: Some(Fill::Colour([9, 9, 9])),
            ..StylePatch::default()
        };
        let off: Vec<Style> = [red, blue, plain]
            .iter()
            .map(|s| s.clone().patched(&switch(false)))
            .collect();
        assert_eq!(off[0].fill, Fill::Off([200, 0, 0]));
        assert_eq!(off[0].fill.painted(), None, "draws unfilled");
        assert_eq!(off[2].fill, Fill::None, "nothing to remember");
        let on: Vec<Fill> = off
            .iter()
            .map(|s| s.clone().patched(&switch(true)).fill)
            .collect();
        assert_eq!(
            on,
            [
                Fill::Colour([200, 0, 0]),
                Fill::Colour([0, 0, 200]),
                Fill::Colour([9, 9, 9])
            ],
            "each keeps its own colour; one that never had one takes the fallback"
        );
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
        let out = red.clone().patched(&p);
        assert_eq!(out.stroke, Stroke::Colour([255, 0, 0]));
        assert_eq!(out.width, 0.7);
        assert_eq!(
            red.patched(&StylePatch::all(Style::default())),
            Style::default()
        );
    }
}
