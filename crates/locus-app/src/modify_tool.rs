// SPDX-License-Identifier: AGPL-3.0-or-later
//! Modify tools (SPEC §6.2): click a curve to trim, extend or break it, or
//! two curves (or the corner they share) to fillet or chamfer them. The plans
//! come from `locus_doc::{modify, corner}`; the canvas runs each as one
//! command.

use locus_doc::corner::{self, ChamferSize};
use locus_doc::modify::{self, ModifyError, Plan};
use locus_doc::transform::Transform;
use locus_doc::{Document, EntityId};
use locus_geom::Vec2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Modify {
    Trim,
    Extend,
    Break,
    Fillet,
    Chamfer,
    /// Move, rotate or scale the selection by typed values.
    Transform,
}

impl Modify {
    pub const ALL: [Modify; 6] = [
        Modify::Trim,
        Modify::Extend,
        Modify::Break,
        Modify::Fillet,
        Modify::Chamfer,
        Modify::Transform,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Modify::Trim => "Trim",
            Modify::Extend => "Extend",
            Modify::Break => "Break",
            Modify::Fillet => "Fillet",
            Modify::Chamfer => "Chamfer",
            Modify::Transform => "Move/Rotate/Scale",
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
            (Modify::Transform, _) => {
                "Select geometry, set the values, then Apply (about the selection's centre)"
            }
        }
    }

    /// The plan for a click on `id` at `at` (one-curve tools).
    pub fn plan(self, doc: &Document, id: EntityId, at: Vec2) -> Result<Plan, ModifyError> {
        match self {
            Modify::Trim => modify::trim(doc, id, at),
            Modify::Extend => modify::extend(doc, id, at),
            Modify::Break => modify::split(doc, id, at),
            Modify::Fillet | Modify::Chamfer | Modify::Transform => Err(ModifyError::NoCorner),
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

/// Which transform the Move/Rotate/Scale tool applies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransformMode {
    Move,
    Rotate,
    Scale,
}

impl TransformMode {
    pub const ALL: [TransformMode; 3] = [
        TransformMode::Move,
        TransformMode::Rotate,
        TransformMode::Scale,
    ];

    pub fn label(self) -> &'static str {
        match self {
            TransformMode::Move => "Move",
            TransformMode::Rotate => "Rotate",
            TransformMode::Scale => "Scale",
        }
    }
}

/// Typed values for the Move/Rotate/Scale tool (mm, degrees, factor).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TransformSettings {
    pub mode: TransformMode,
    pub dx: f64,
    pub dy: f64,
    pub angle_deg: f64,
    pub factor: f64,
    pub copy: bool,
}

impl Default for TransformSettings {
    fn default() -> Self {
        Self {
            mode: TransformMode::Move,
            dx: 10.0,
            dy: 0.0,
            angle_deg: 90.0,
            factor: 2.0,
            copy: false,
        }
    }
}

impl TransformSettings {
    /// The transform about `center` (the selection's centre).
    pub fn transform(&self, center: Vec2) -> Transform {
        match self.mode {
            TransformMode::Move => Transform::Move(Vec2::new(self.dx, self.dy)),
            TransformMode::Rotate => Transform::Rotate {
                center,
                angle: self.angle_deg.to_radians(),
            },
            TransformMode::Scale => Transform::Scale {
                center,
                factor: self.factor,
            },
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

    #[test]
    fn transform_settings_build_transforms() {
        let c = Vec2::new(1.0, 2.0);
        let mut s = TransformSettings::default();
        assert_eq!(s.transform(c), Transform::Move(Vec2::new(10.0, 0.0)));
        s.mode = TransformMode::Rotate;
        assert_eq!(
            s.transform(c),
            Transform::Rotate {
                center: c,
                angle: core::f64::consts::FRAC_PI_2
            }
        );
        s.mode = TransformMode::Scale;
        assert_eq!(
            s.transform(c),
            Transform::Scale {
                center: c,
                factor: 2.0
            }
        );
    }
}
