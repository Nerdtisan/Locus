// SPDX-License-Identifier: AGPL-3.0-or-later

//! Display units (SPEC §4). Storage is always millimetres; these only convert
//! for display and input.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Unit {
    #[default]
    Mm,
    Cm,
    M,
    Inch,
}

impl Unit {
    /// Millimetres per one of this unit (the inch is exactly 25.4 mm).
    #[must_use]
    pub const fn mm_per_unit(self) -> f64 {
        match self {
            Unit::Mm => 1.0,
            Unit::Cm => 10.0,
            Unit::M => 1000.0,
            Unit::Inch => 25.4,
        }
    }

    /// Short suffix for display.
    #[must_use]
    pub const fn suffix(self) -> &'static str {
        match self {
            Unit::Mm => "mm",
            Unit::Cm => "cm",
            Unit::M => "m",
            Unit::Inch => "in",
        }
    }

    /// Convert a stored millimetre value to this unit.
    #[must_use]
    pub fn from_mm(self, mm: f64) -> f64 {
        mm / self.mm_per_unit()
    }

    /// Convert a value in this unit to stored millimetres.
    #[must_use]
    pub fn to_mm(self, v: f64) -> f64 {
        v * self.mm_per_unit()
    }

    /// Format a millimetre value in this unit with `decimals` places and the
    /// unit suffix, e.g. 25.4 mm in inches with 3 decimals is `"1.000 in"`.
    #[must_use]
    pub fn format_mm(self, mm: f64, decimals: usize) -> String {
        format!("{:.decimals$} {}", self.from_mm(mm), self.suffix())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inch_round_trip() {
        for mm in [0.0, 1.0, 25.4, 100.0, 0.001, 12345.678] {
            let back = Unit::Inch.to_mm(Unit::Inch.from_mm(mm));
            assert!((back - mm).abs() <= 1e-12 * mm.abs().max(1.0));
        }
        assert_eq!(Unit::Inch.from_mm(25.4), 1.0);
        assert_eq!(Unit::Inch.to_mm(1.0), 25.4);
    }

    #[test]
    fn formatting() {
        assert_eq!(Unit::Inch.format_mm(25.4, 3), "1.000 in");
        assert_eq!(Unit::Mm.format_mm(3.0, 1), "3.0 mm");
        assert_eq!(Unit::Cm.format_mm(15.0, 2), "1.50 cm");
        assert_eq!(Unit::M.format_mm(1500.0, 1), "1.5 m");
    }
}
