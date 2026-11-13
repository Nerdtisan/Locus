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
    /// Move/Copy, Rotate or Sketch Scale the selection (#219): three tools
    /// sharing one dialog and an on-canvas manipulator (design §6.7).
    Transform(TransformMode),
    /// Live offset of a chain (#164).
    Offset,
}

impl Modify {
    #[cfg(test)]
    pub const ALL: [Modify; 9] = [
        Modify::Trim,
        Modify::Extend,
        Modify::Break,
        Modify::Fillet,
        Modify::Chamfer,
        Modify::Offset,
        Modify::Transform(TransformMode::Move),
        Modify::Transform(TransformMode::Rotate),
        Modify::Transform(TransformMode::Scale),
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Modify::Trim => "Trim",
            Modify::Extend => "Extend",
            Modify::Break => "Break",
            Modify::Fillet => "Fillet",
            Modify::Chamfer => "Chamfer",
            Modify::Transform(m) => m.tool_label(),
            Modify::Offset => "Offset",
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
            (Modify::Offset, _) => {
                "Offset: click a curve to offset its chain, or an offset to edit it (+ grows / goes left)"
            }
            (Modify::Transform(TransformMode::Move), _) => {
                "Move: select geometry, then drag an arrow or the centre, or type a distance; Enter applies"
            }
            (Modify::Transform(TransformMode::Rotate), _) => {
                "Rotate: select geometry, then drag the ring's knob or type an angle (drag the centre to move the pivot); Enter applies"
            }
            (Modify::Transform(TransformMode::Scale), _) => {
                "Scale: select geometry, then drag the corner handle or type a factor (drag the centre to move the base point); Enter applies"
            }
        }
    }

    /// The plan for a click on `id` at `at` (one-curve tools).
    pub fn plan(self, doc: &Document, id: EntityId, at: Vec2) -> Result<Plan, ModifyError> {
        match self {
            Modify::Trim => modify::trim(doc, id, at),
            Modify::Extend => modify::extend(doc, id, at),
            Modify::Break => modify::split(doc, id, at),
            Modify::Fillet | Modify::Chamfer | Modify::Transform(_) | Modify::Offset => {
                Err(ModifyError::NoCorner)
            }
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

    pub const fn label(self) -> &'static str {
        match self {
            ChamferMode::Equal => "Equal distance",
            ChamferMode::TwoDistances => "Two distances",
            ChamferMode::DistanceAngle => "Distance and angle",
        }
    }
}

/// Sizes for the fillet and chamfer tools, in mm and degrees. The dialog
/// edits them as text, so a size can be an expression (#217); see
/// [`CornerText`].
#[derive(Clone, Debug, PartialEq)]
pub struct CornerSettings {
    pub radius: f64,
    pub mode: ChamferMode,
    pub d1: f64,
    pub d2: f64,
    pub angle_deg: f64,
    /// Offset distance for a new offset (mm; + grows / goes left).
    pub offset: f64,
    /// New offsets keep sharp corners (Fusion's default) or round them.
    pub sharp: bool,
    /// New offsets take the whole chain through the picked curve (#218).
    pub offset_chain: bool,
    /// What the dialog's fields hold: numbers in display units, or
    /// expressions over parameters (SPEC §5).
    pub text: CornerText,
}

/// The fillet and chamfer fields as typed (#217).
#[derive(Clone, Debug, PartialEq)]
pub struct CornerText {
    pub radius: String,
    pub d1: String,
    pub d2: String,
    pub angle: String,
    /// The Offset tool's distance (#218).
    pub offset: String,
}

impl Default for CornerText {
    fn default() -> Self {
        Self {
            radius: "2".to_owned(),
            d1: "2".to_owned(),
            d2: "2".to_owned(),
            angle: "45".to_owned(),
            offset: "2".to_owned(),
        }
    }
}

impl Default for CornerSettings {
    fn default() -> Self {
        Self {
            radius: 2.0,
            mode: ChamferMode::Equal,
            d1: 2.0,
            d2: 2.0,
            angle_deg: 45.0,
            offset: 2.0,
            sharp: true,
            offset_chain: true,
            text: CornerText::default(),
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

/// Which transform the Move, Rotate or Scale tool applies.
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

    pub const fn label(self) -> &'static str {
        match self {
            TransformMode::Move => "Move",
            TransformMode::Rotate => "Rotate",
            TransformMode::Scale => "Scale",
        }
    }

    /// The tool's name, as Fusion names it.
    pub const fn tool_label(self) -> &'static str {
        match self {
            TransformMode::Move => "Move/Copy",
            TransformMode::Rotate => "Rotate",
            TransformMode::Scale => "Sketch Scale",
        }
    }
}

/// A part of the on-canvas manipulator being dragged (design §6.7).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Grip {
    /// The X arrow: moves along X only.
    MoveX,
    /// The Y arrow: moves along Y only.
    MoveY,
    /// The centre square in Move, or a selected curve: moves freely.
    MoveFree,
    /// The centre square in Rotate and Scale: the pivot / base point.
    Pivot,
    /// The knob on the rotation ring.
    Ring,
    /// The corner handle that sets the scale factor.
    Factor,
}

/// The value part of [`TransformSettings`], kept while a grip is dragged.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TransformValues {
    pub dx: f64,
    pub dy: f64,
    pub angle_deg: f64,
    pub factor: f64,
    pub pivot: Option<Vec2>,
}

/// The values the Move, Rotate and Scale tools apply (mm, degrees, factor),
/// shared by the dialog fields and the manipulator: two views of one value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TransformSettings {
    pub dx: f64,
    pub dy: f64,
    pub angle_deg: f64,
    pub factor: f64,
    pub copy: bool,
    /// The rotation pivot / scale base point; `None` is the selection's
    /// centre.
    pub pivot: Option<Vec2>,
    /// The grip being dragged, the world point it was pressed at and the
    /// values then.
    pub grip: Option<(Grip, Vec2, TransformValues)>,
    /// The axis a typed Move distance goes along: X unless Y was dragged last.
    pub along_y: bool,
}

impl Default for TransformSettings {
    fn default() -> Self {
        Self {
            dx: 0.0,
            dy: 0.0,
            angle_deg: 0.0,
            factor: 1.0,
            copy: false,
            pivot: None,
            grip: None,
            along_y: false,
        }
    }
}

impl TransformSettings {
    /// Back to "no change", keeping Copy (the tool restarts).
    pub fn reset(&mut self) {
        *self = Self {
            copy: self.copy,
            ..Self::default()
        };
    }

    pub fn values(&self) -> TransformValues {
        TransformValues {
            dx: self.dx,
            dy: self.dy,
            angle_deg: self.angle_deg,
            factor: self.factor,
            pivot: self.pivot,
        }
    }

    /// The transform for `mode`, about `center` when no pivot was placed.
    pub fn transform(&self, mode: TransformMode, center: Vec2) -> Transform {
        let center = self.pivot.unwrap_or(center);
        match mode {
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

    /// True when applying would change nothing.
    pub fn is_identity(&self, mode: TransformMode) -> bool {
        match mode {
            TransformMode::Move => self.dx == 0.0 && self.dy == 0.0,
            TransformMode::Rotate => self.angle_deg.rem_euclid(360.0) == 0.0,
            TransformMode::Scale => self.factor == 1.0,
        }
    }

    /// A value typed on the canvas: the distance along the last-used axis,
    /// the angle, or the factor.
    pub fn set_typed(&mut self, mode: TransformMode, v: f64) {
        match mode {
            TransformMode::Move if self.along_y => self.dy = v,
            TransformMode::Move => self.dx = v,
            TransformMode::Rotate => self.angle_deg = v,
            TransformMode::Scale => self.factor = v,
        }
    }

    /// Drag the grip pressed at `from` to `to` (world mm); `center` is the
    /// selection's centre. `coarse` (Shift) snaps rotation to 15° and the
    /// factor to 0.1 and distances to 1 mm; otherwise to 1°, 0.01 and
    /// 0.1 mm. The pivot isn't rounded: it snaps to points instead.
    pub fn drag(&mut self, to: Vec2, center: Vec2, coarse: bool) {
        let Some((grip, from, v)) = self.grip else {
            return;
        };
        let d = to - from;
        let pivot = v.pivot.unwrap_or(center);
        let mm = |x: f64| {
            let step = if coarse { 1.0 } else { 0.1 };
            (x / step).round() * step
        };
        match grip {
            Grip::MoveX => {
                self.dx = mm(v.dx + d.x);
                self.dy = v.dy;
                self.along_y = false;
            }
            Grip::MoveY => {
                self.dx = v.dx;
                self.dy = mm(v.dy + d.y);
                self.along_y = true;
            }
            Grip::MoveFree => {
                self.dx = mm(v.dx + d.x);
                self.dy = mm(v.dy + d.y);
            }
            Grip::Pivot => self.pivot = Some(to),
            Grip::Ring => {
                let (p0, p1) = (from - pivot, to - pivot);
                let turn = (p1.y.atan2(p1.x) - p0.y.atan2(p0.x)).to_degrees();
                let deg = (v.angle_deg + turn + 180.0).rem_euclid(360.0) - 180.0;
                let step = if coarse { 15.0 } else { 1.0 };
                self.angle_deg = (deg / step).round() * step;
            }
            Grip::Factor => {
                let r0 = from.distance(pivot);
                if r0 > f64::EPSILON {
                    let f = v.factor * to.distance(pivot) / r0;
                    let step = if coarse { 0.1 } else { 0.01 };
                    self.factor = ((f / step).round() * step).max(step);
                }
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

    #[test]
    fn transform_settings_build_transforms() {
        let c = Vec2::new(1.0, 2.0);
        let mut s = TransformSettings {
            dx: 10.0,
            angle_deg: 90.0,
            factor: 2.0,
            ..TransformSettings::default()
        };
        assert_eq!(
            s.transform(TransformMode::Move, c),
            Transform::Move(Vec2::new(10.0, 0.0))
        );
        assert_eq!(
            s.transform(TransformMode::Rotate, c),
            Transform::Rotate {
                center: c,
                angle: core::f64::consts::FRAC_PI_2
            }
        );
        // A placed pivot wins over the selection's centre.
        s.pivot = Some(Vec2::ZERO);
        assert_eq!(
            s.transform(TransformMode::Scale, c),
            Transform::Scale {
                center: Vec2::ZERO,
                factor: 2.0
            }
        );
        s.reset();
        for m in TransformMode::ALL {
            assert!(s.is_identity(m));
        }
    }

    #[test]
    fn dragging_grips_sets_the_values() {
        let c = Vec2::ZERO;
        let mut s = TransformSettings::default();
        // The X arrow moves along X only.
        s.grip = Some((Grip::MoveX, Vec2::new(5.0, 0.0), s.values()));
        s.drag(Vec2::new(12.04, 3.0), c, false);
        assert!(
            (s.dx - 7.0).abs() < 1e-9 && s.dy == 0.0,
            "rounded to 0.1 mm"
        );
        // A quarter turn of the ring knob is 90°.
        s.grip = Some((Grip::Ring, Vec2::new(10.0, 0.0), s.values()));
        s.drag(Vec2::new(0.0, 10.0), c, false);
        assert!((s.angle_deg - 90.0).abs() < 1e-9);
        // Shift snaps to 15°.
        s.grip = Some((Grip::Ring, Vec2::new(10.0, 0.0), s.values()));
        s.drag(Vec2::new(10.0, 2.0), c, true);
        assert!((s.angle_deg - 105.0).abs() < 1e-9);
        // Pulling the corner handle twice as far doubles the factor.
        s.grip = Some((Grip::Factor, Vec2::new(4.0, 4.0), s.values()));
        s.drag(Vec2::new(8.0, 8.0), c, false);
        assert!((s.factor - 2.0).abs() < 1e-9);
        // Typed values go to the last-used axis.
        s.set_typed(TransformMode::Move, 4.0);
        assert_eq!(s.dx, 4.0);
        s.grip = Some((Grip::MoveY, Vec2::ZERO, s.values()));
        s.drag(Vec2::new(0.0, 1.0), c, false);
        s.set_typed(TransformMode::Move, 9.0);
        assert_eq!((s.dx, s.dy), (4.0, 9.0));
    }
}
