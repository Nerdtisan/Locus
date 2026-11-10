// SPDX-License-Identifier: AGPL-3.0-or-later
//! Modify tools (SPEC §6.2): click a curve to trim, extend or break it, or
//! two curves (or the corner they share) to fillet or chamfer them. The plans
//! come from `locus_doc::{modify, corner}`; the canvas runs each as one
//! command.

use locus_doc::corner::{self, ChamferSize};
use locus_doc::modify::{self, ModifyError, Plan};
use locus_doc::{Document, EntityId};
use locus_geom::Vec2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Modify {
    Trim,
    Extend,
    Break,
    Fillet,
    Chamfer,
}

impl Modify {
    pub const ALL: [Modify; 5] = [
        Modify::Trim,
        Modify::Extend,
        Modify::Break,
        Modify::Fillet,
        Modify::Chamfer,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Modify::Trim => "Trim",
            Modify::Extend => "Extend",
            Modify::Break => "Break",
            Modify::Fillet => "Fillet",
            Modify::Chamfer => "Chamfer",
        }
    }

    /// Fillet and chamfer take two curves; the rest one.
    pub fn takes_two(self) -> bool {
        matches!(self, Modify::Fillet | Modify::Chamfer)
    }

    /// The status-bar prompt, given whether the first of two curves is picked.
    pub fn prompt(self, first: bool) -> &'static str {
        match (self, first) {
            (Modify::Trim, _) => "Trim: click the piece of a curve to remove (Esc to finish)",
            (Modify::Extend, _) => "Extend: click near the end of a line or arc (Esc to finish)",
            (Modify::Break, _) => {
                "Break: click a curve to split it where others cross (Esc to finish)"
            }
            (Modify::Fillet | Modify::Chamfer, false) => {
                "Click the first curve, or the corner point two curves share (Esc to finish)"
            }
            (Modify::Fillet | Modify::Chamfer, true) => "Click the second curve at the corner",
        }
    }

    /// The plan for a click on `id` at `at` (one-curve tools).
    pub fn plan(self, doc: &Document, id: EntityId, at: Vec2) -> Result<Plan, ModifyError> {
        match self {
            Modify::Trim => modify::trim(doc, id, at),
            Modify::Extend => modify::extend(doc, id, at),
            Modify::Break => modify::split(doc, id, at),
            Modify::Fillet | Modify::Chamfer => Err(ModifyError::NoCorner),
        }
    }

    /// The plan for the corner of `a` and `b` (two-curve tools).
    pub fn plan_corner(
        self,
        doc: &Document,
        a: EntityId,
        b: EntityId,
        s: &CornerSettings,
    ) -> Result<Plan, ModifyError> {
        match self {
            Modify::Fillet => corner::fillet(doc, a, b, s.radius),
            Modify::Chamfer => corner::chamfer(doc, a, b, s.chamfer_size()),
            _ => Err(ModifyError::NoCorner),
        }
    }
}

/// How a chamfer is sized in the tool strip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChamferMode {
    Equal,
    TwoDistances,
    DistanceAngle,
}

impl ChamferMode {
    pub const ALL: [ChamferMode; 3] = [
        ChamferMode::Equal,
        ChamferMode::TwoDistances,
        ChamferMode::DistanceAngle,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ChamferMode::Equal => "Equal distance",
            ChamferMode::TwoDistances => "Two distances",
            ChamferMode::DistanceAngle => "Distance and angle",
        }
    }
}

/// Sizes for the fillet and chamfer tools, in mm and degrees.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CornerSettings {
    pub radius: f64,
    pub mode: ChamferMode,
    pub d1: f64,
    pub d2: f64,
    pub angle_deg: f64,
}

impl Default for CornerSettings {
    fn default() -> Self {
        Self {
            radius: 2.0,
            mode: ChamferMode::Equal,
            d1: 2.0,
            d2: 2.0,
            angle_deg: 45.0,
        }
    }
}

impl CornerSettings {
    pub fn chamfer_size(&self) -> ChamferSize {
        match self.mode {
            ChamferMode::Equal => ChamferSize::Equal(self.d1),
            ChamferMode::TwoDistances => ChamferSize::Distances(self.d1, self.d2),
            ChamferMode::DistanceAngle => {
                ChamferSize::DistanceAngle(self.d1, self.angle_deg.to_radians())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chamfer_sizes_follow_the_mode() {
        let mut s = CornerSettings {
            d1: 3.0,
            d2: 1.0,
            ..CornerSettings::default()
        };
        assert_eq!(s.chamfer_size(), ChamferSize::Equal(3.0));
        s.mode = ChamferMode::TwoDistances;
        assert_eq!(s.chamfer_size(), ChamferSize::Distances(3.0, 1.0));
        s.mode = ChamferMode::DistanceAngle;
        assert_eq!(
            s.chamfer_size(),
            ChamferSize::DistanceAngle(3.0, 45f64.to_radians())
        );
        assert!(Modify::Fillet.takes_two() && !Modify::Trim.takes_two());
    }
}
