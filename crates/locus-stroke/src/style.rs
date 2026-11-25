// SPDX-License-Identifier: AGPL-3.0-or-later

//! Line-style parameters (SPEC §4 entity style, ADR 0019 decision 1). They
//! live here so `locus-doc` can store them and this crate can expand them
//! without a dependency cycle; `locus-doc` re-exports them.

use crate::StrokeError;
use crate::dash::Dash;
use serde::{Deserialize, Serialize};

/// End cap of an open path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Cap {
    /// Flat, at the end point.
    Butt,
    /// A half disc: what a round pen draws. The default (owner, 2026-11-24).
    #[default]
    Round,
    /// Flat, half the width past the end point.
    Square,
}

/// Corner treatment between two pieces of a path.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub enum Join {
    /// An arc about the corner. The default (owner, 2026-11-24).
    #[default]
    Round,
    /// Sharp, while the miter length stays within `limit` × the width
    /// (SVG's `stroke-miterlimit`); bevelled beyond it.
    Miter { limit: f64 },
    /// Cut straight across the corner.
    Bevel,
}

/// Where a closed path's stroke lies; open paths are always centred.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Align {
    #[default]
    Centre,
    Inside,
    Outside,
}

/// Smallest and largest miter limit (SVG requires ≥ 1).
pub const MIN_MITER: f64 = 1.0;
pub const MAX_MITER: f64 = 100.0;
/// SVG's and Illustrator's default.
pub const DEFAULT_MITER: f64 = 4.0;

fn is_default<T: Default + PartialEq>(v: &T) -> bool {
    *v == T::default()
}

/// An entity's line style beyond colour and width (#428). Every field is
/// omitted from `.locus` at its default, so an unstyled file is unchanged.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LineStyle {
    #[serde(skip_serializing_if = "is_default")]
    pub cap: Cap,
    #[serde(skip_serializing_if = "is_default")]
    pub join: Join,
    #[serde(skip_serializing_if = "is_default")]
    pub align: Align,
    /// Dash pattern (#427); `None` is a solid line.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dash: Option<Dash>,
    /// Extra strokes drawn with this one (#429), in paint order; at most
    /// [`crate::MAX_STROKE_LAYERS`].
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub extra: Vec<crate::multi::StrokeLayer>,
}

impl LineStyle {
    #[must_use]
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// Does a **Styled** machine layer follow this object's strokes rather
    /// than its centreline (owner decision 2026-11-25)? Only line styles
    /// count: dashes and extra strokes (later width profiles and brushes).
    /// Width, caps, joins and alignment alone don't: a widened plain line
    /// still cuts once along its centreline.
    #[must_use]
    pub fn machine_styled(&self) -> bool {
        self.dash.is_some() || !self.extra.is_empty()
    }

    /// # Errors
    /// [`StrokeError::BadValue`] for a miter limit out of range or not finite.
    pub fn validate(&self) -> Result<(), StrokeError> {
        if let Join::Miter { limit } = self.join
            && !(limit.is_finite() && (MIN_MITER..=MAX_MITER).contains(&limit))
        {
            return Err(StrokeError::BadValue {
                what: "miter limit",
                value: limit,
            });
        }
        if let Some(d) = &self.dash {
            d.validate()?;
        }
        if self.extra.len() > crate::multi::MAX_STROKE_LAYERS {
            return Err(StrokeError::Limit {
                what: "extra strokes",
                count: self.extra.len(),
            });
        }
        for l in &self.extra {
            l.validate()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_round_and_centred() {
        let s = LineStyle::default();
        assert_eq!(
            (s.cap, s.join, s.align),
            (Cap::Round, Join::Round, Align::Centre)
        );
        assert!(s.is_default() && s.validate().is_ok());
    }

    #[test]
    fn miter_limit_is_bounded() {
        for bad in [0.5, f64::NAN, f64::INFINITY, 101.0] {
            let s = LineStyle {
                join: Join::Miter { limit: bad },
                ..LineStyle::default()
            };
            assert!(s.validate().is_err(), "{bad}");
        }
        let ok = LineStyle {
            join: Join::Miter { limit: 1.0 },
            ..LineStyle::default()
        };
        assert!(ok.validate().is_ok());
    }
}
