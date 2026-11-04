// SPDX-License-Identifier: AGPL-3.0-or-later

//! The sketch every backend is given: the SPEC §4 v1 curve entities and the §5 v1
//! constraint set, in millimetres. Backends translate this into their own model.

use core::ops::{Add, Mul, Sub};

/// Index into [`Sketch::points`].
pub type PointId = usize;
/// Index into [`Sketch::curves`].
pub type CurveId = usize;
/// Index into [`Sketch::constraints`].
pub type ConstraintId = usize;

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Vec2 {
    pub x: f64,
    pub y: f64,
}

impl Vec2 {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
    pub fn dot(self, o: Self) -> f64 {
        self.x * o.x + self.y * o.y
    }
    pub fn cross(self, o: Self) -> f64 {
        self.x * o.y - self.y * o.x
    }
    pub fn len(self) -> f64 {
        self.dot(self).sqrt()
    }
    pub fn dist(self, o: Self) -> f64 {
        (self - o).len()
    }
}

impl Add for Vec2 {
    type Output = Self;
    fn add(self, o: Self) -> Self {
        Self::new(self.x + o.x, self.y + o.y)
    }
}

impl Sub for Vec2 {
    type Output = Self;
    fn sub(self, o: Self) -> Self {
        Self::new(self.x - o.x, self.y - o.y)
    }
}

impl Mul<f64> for Vec2 {
    type Output = Self;
    fn mul(self, k: f64) -> Self {
        Self::new(self.x * k, self.y * k)
    }
}

/// SPEC §4 v1 curves. Points are shared by index, so a line and an arc that meet
/// at a corner use the same point (implicit coincidence, as in most sketchers).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Curve {
    Line {
        a: PointId,
        b: PointId,
    },
    /// The radius is an unknown of the solve.
    Circle {
        c: PointId,
        r: f64,
    },
    /// Counter-clockwise from `a` to `b` around `c`. The radius is `|a - c|`; every
    /// solver must keep `|b - c|` equal to it (checked as an implicit constraint).
    Arc {
        c: PointId,
        a: PointId,
        b: PointId,
    },
}

/// SPEC §5 v1 constraints. Distances are in mm, angles in degrees (§5 expressions
/// default to degrees). Signed values are noted per variant.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Constraint {
    // Geometric (§5)
    Coincident(PointId, PointId),
    /// On a line (extended) or on a circle or arc (its whole circle).
    PointOnCurve(PointId, CurveId),
    Horizontal(CurveId),
    Vertical(CurveId),
    Parallel(CurveId, CurveId),
    Perpendicular(CurveId, CurveId),
    /// Line–circle/arc (distance from the centre equals the radius) or
    /// circle–circle (external or internal, whichever is closer).
    Tangent(CurveId, CurveId),
    Collinear(CurveId, CurveId),
    Concentric(CurveId, CurveId),
    /// Equal line lengths.
    EqualLength(CurveId, CurveId),
    /// Equal radii of circles or arcs.
    EqualRadius(CurveId, CurveId),
    /// `p` and `q` mirror each other about line `about`.
    Symmetric {
        p: PointId,
        q: PointId,
        about: CurveId,
    },
    Midpoint {
        p: PointId,
        line: CurveId,
    },
    /// The point stays at `at`.
    Fix {
        p: PointId,
        at: Vec2,
    },
    // Dimensional (§5)
    /// Point–point distance.
    Distance {
        p: PointId,
        q: PointId,
        d: f64,
    },
    /// Perpendicular distance from a point to a line (unsigned).
    PointLineDistance {
        p: PointId,
        line: CurveId,
        d: f64,
    },
    /// Line–line distance for parallel lines: from `m`'s first point to line `l`
    /// (unsigned). It does not itself make the lines parallel.
    LineLineDistance {
        l: CurveId,
        m: CurveId,
        d: f64,
    },
    /// Signed: `q.x - p.x = d`.
    HorizontalDistance {
        p: PointId,
        q: PointId,
        d: f64,
    },
    /// Signed: `q.y - p.y = d`.
    VerticalDistance {
        p: PointId,
        q: PointId,
        d: f64,
    },
    /// Signed counter-clockwise angle from line `l`'s direction to line `m`'s.
    Angle {
        l: CurveId,
        m: CurveId,
        deg: f64,
    },
    Radius {
        c: CurveId,
        r: f64,
    },
    Diameter {
        c: CurveId,
        d: f64,
    },
}

impl Constraint {
    /// Short name for reports.
    pub fn name(&self) -> &'static str {
        match self {
            Constraint::Coincident(..) => "coincident",
            Constraint::PointOnCurve(..) => "point-on-curve",
            Constraint::Horizontal(..) => "horizontal",
            Constraint::Vertical(..) => "vertical",
            Constraint::Parallel(..) => "parallel",
            Constraint::Perpendicular(..) => "perpendicular",
            Constraint::Tangent(..) => "tangent",
            Constraint::Collinear(..) => "collinear",
            Constraint::Concentric(..) => "concentric",
            Constraint::EqualLength(..) => "equal length",
            Constraint::EqualRadius(..) => "equal radius",
            Constraint::Symmetric { .. } => "symmetric",
            Constraint::Midpoint { .. } => "midpoint",
            Constraint::Fix { .. } => "fix",
            Constraint::Distance { .. } => "distance p-p",
            Constraint::PointLineDistance { .. } => "distance p-l",
            Constraint::LineLineDistance { .. } => "distance l-l",
            Constraint::HorizontalDistance { .. } => "horizontal distance",
            Constraint::VerticalDistance { .. } => "vertical distance",
            Constraint::Angle { .. } => "angle",
            Constraint::Radius { .. } => "radius",
            Constraint::Diameter { .. } => "diameter",
        }
    }
}

/// Geometry plus constraints. `points` and circle radii are the current values
/// (the start of a solve, or a backend's result written back).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sketch {
    pub points: Vec<Vec2>,
    pub curves: Vec<Curve>,
    pub constraints: Vec<Constraint>,
}

impl Sketch {
    pub fn point(&mut self, x: f64, y: f64) -> PointId {
        self.points.push(Vec2::new(x, y));
        self.points.len() - 1
    }
    pub fn curve(&mut self, c: Curve) -> CurveId {
        self.curves.push(c);
        self.curves.len() - 1
    }
    pub fn line(&mut self, a: PointId, b: PointId) -> CurveId {
        self.curve(Curve::Line { a, b })
    }
    pub fn constrain(&mut self, k: Constraint) -> ConstraintId {
        self.constraints.push(k);
        self.constraints.len() - 1
    }
    /// Unknowns before any constraint: two per point, one per circle radius.
    pub fn unknowns(&self) -> usize {
        2 * self.points.len()
            + self
                .curves
                .iter()
                .filter(|c| matches!(c, Curve::Circle { .. }))
                .count()
    }
}
