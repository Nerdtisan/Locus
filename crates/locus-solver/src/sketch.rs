// SPDX-License-Identifier: AGPL-3.0-or-later

//! The solver's input: points, curves and constraints (SPEC §4 v1 curves, §5 v1
//! constraints), independent of `locus-doc`. A document is lowered into this
//! form; every item carries the caller's [`Key`] (the document's `EntityId` or
//! constraint id) so reports name document items, not solver indices.
//!
//! Lengths are millimetres, angles radians (as in `locus-doc`).

use locus_geom::Vec2;

/// The caller's identifier for a point, curve or constraint.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Key(pub u64);

/// Index into [`Sketch::points`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PointId(pub u32);

/// Index into [`Sketch::curves`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CurveId(pub u32);

/// Index into [`Sketch::constraints`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ConstraintId(pub u32);

impl PointId {
    pub fn index(self) -> usize {
        self.0 as usize
    }
}
impl CurveId {
    pub fn index(self) -> usize {
        self.0 as usize
    }
}
impl ConstraintId {
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    pub key: Key,
    pub at: Vec2,
}

/// SPEC §4 v1 curves. Curves share points by index, so a line and an arc that
/// meet use the same point (implicit coincidence).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CurveKind {
    Line {
        a: PointId,
        b: PointId,
    },
    /// The radius is an unknown of the solve.
    Circle {
        centre: PointId,
        r: f64,
    },
    /// Counter-clockwise from `a` to `b` about `centre`. The radius is
    /// `|a − centre|`; the solver keeps `|b − centre|` equal to it.
    Arc {
        centre: PointId,
        a: PointId,
        b: PointId,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Curve {
    pub key: Key,
    pub kind: CurveKind,
}

/// SPEC §5 v1 constraints. Distances in mm, angles in radians.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ConstraintKind {
    // Geometric (§5)
    Coincident(PointId, PointId),
    /// On a line (extended), or on a circle or arc (its whole circle).
    PointOnCurve(PointId, CurveId),
    Horizontal(CurveId),
    Vertical(CurveId),
    Parallel(CurveId, CurveId),
    Perpendicular(CurveId, CurveId),
    /// Line–circle/arc, or circle/arc–circle/arc (external or internal,
    /// whichever is nearer).
    Tangent(CurveId, CurveId),
    Collinear(CurveId, CurveId),
    Concentric(CurveId, CurveId),
    EqualLength(CurveId, CurveId),
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
    /// Unsigned perpendicular distance from a point to a line.
    PointLineDistance {
        p: PointId,
        line: CurveId,
        d: f64,
    },
    /// Distance between parallel lines: from `m`'s first point to line `l`
    /// (unsigned). It does not itself make the lines parallel.
    LineLineDistance {
        l: CurveId,
        m: CurveId,
        d: f64,
    },
    /// Signed: `q.x − p.x = d`.
    HorizontalDistance {
        p: PointId,
        q: PointId,
        d: f64,
    },
    /// Signed: `q.y − p.y = d`.
    VerticalDistance {
        p: PointId,
        q: PointId,
        d: f64,
    },
    /// Signed counter-clockwise angle from line `l`'s direction to line `m`'s.
    Angle {
        l: CurveId,
        m: CurveId,
        rad: f64,
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

impl ConstraintKind {
    /// Short name for reports and tests.
    pub fn name(&self) -> &'static str {
        use ConstraintKind::*;
        match self {
            Coincident(..) => "coincident",
            PointOnCurve(..) => "point-on-curve",
            Horizontal(..) => "horizontal",
            Vertical(..) => "vertical",
            Parallel(..) => "parallel",
            Perpendicular(..) => "perpendicular",
            Tangent(..) => "tangent",
            Collinear(..) => "collinear",
            Concentric(..) => "concentric",
            EqualLength(..) => "equal-length",
            EqualRadius(..) => "equal-radius",
            Symmetric { .. } => "symmetric",
            Midpoint { .. } => "midpoint",
            Fix { .. } => "fix",
            Distance { .. } => "distance",
            PointLineDistance { .. } => "point-line-distance",
            LineLineDistance { .. } => "line-line-distance",
            HorizontalDistance { .. } => "horizontal-distance",
            VerticalDistance { .. } => "vertical-distance",
            Angle { .. } => "angle",
            Radius { .. } => "radius",
            Diameter { .. } => "diameter",
        }
    }

    /// Dimensional constraints carry a value and may be driven (§5).
    pub fn is_dimension(&self) -> bool {
        use ConstraintKind::*;
        matches!(
            self,
            Distance { .. }
                | PointLineDistance { .. }
                | LineLineDistance { .. }
                | HorizontalDistance { .. }
                | VerticalDistance { .. }
                | Angle { .. }
                | Radius { .. }
                | Diameter { .. }
        )
    }

    /// The points and curves this constraint refers to, in a fixed order.
    pub fn refs(&self) -> (Vec<PointId>, Vec<CurveId>) {
        use ConstraintKind::*;
        match *self {
            Coincident(p, q)
            | Distance { p, q, .. }
            | HorizontalDistance { p, q, .. }
            | VerticalDistance { p, q, .. } => (vec![p, q], vec![]),
            Fix { p, .. } => (vec![p], vec![]),
            PointOnCurve(p, c) | Midpoint { p, line: c } | PointLineDistance { p, line: c, .. } => {
                (vec![p], vec![c])
            }
            Symmetric { p, q, about } => (vec![p, q], vec![about]),
            Horizontal(c) | Vertical(c) | Radius { c, .. } | Diameter { c, .. } => {
                (vec![], vec![c])
            }
            Parallel(a, b)
            | Perpendicular(a, b)
            | Tangent(a, b)
            | Collinear(a, b)
            | Concentric(a, b)
            | EqualLength(a, b)
            | EqualRadius(a, b)
            | LineLineDistance { l: a, m: b, .. }
            | Angle { l: a, m: b, .. } => (vec![], vec![a, b]),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Constraint {
    pub key: Key,
    pub kind: ConstraintKind,
    /// A driven (reference) dimension is measured, not enforced (§5).
    pub driven: bool,
}

/// Geometry plus constraints. Point positions and circle radii are the current
/// values: where a solve starts and where its result is written.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sketch {
    pub points: Vec<Point>,
    pub curves: Vec<Curve>,
    pub constraints: Vec<Constraint>,
}

/// A sketch that refers to geometry it doesn't have. Input is untrusted, so
/// every reference is checked before solving.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum SketchError {
    #[error("curve {0:?} refers to a missing point")]
    CurveMissingPoint(Key),
    #[error("constraint {0:?} refers to a missing point or curve")]
    MissingRef(Key),
    #[error("constraint {key:?} ({name}) needs {expected}")]
    WrongKind {
        key: Key,
        name: &'static str,
        expected: &'static str,
    },
    #[error("value of constraint {0:?} is not finite")]
    NotFinite(Key),
    #[error("drag of a missing point, or to a non-finite position")]
    BadDrag,
}

/// Builders for tests and lowering: keys default to the item's index.
impl Sketch {
    pub fn point(&mut self, x: f64, y: f64) -> PointId {
        let id = PointId(self.points.len() as u32);
        self.points.push(Point {
            key: Key(u64::from(id.0)),
            at: Vec2::new(x, y),
        });
        id
    }
    pub fn curve(&mut self, kind: CurveKind) -> CurveId {
        let id = CurveId(self.curves.len() as u32);
        self.curves.push(Curve {
            key: Key(u64::from(id.0)),
            kind,
        });
        id
    }
    pub fn line(&mut self, a: PointId, b: PointId) -> CurveId {
        self.curve(CurveKind::Line { a, b })
    }
    pub fn circle(&mut self, centre: PointId, r: f64) -> CurveId {
        self.curve(CurveKind::Circle { centre, r })
    }
    pub fn arc(&mut self, centre: PointId, a: PointId, b: PointId) -> CurveId {
        self.curve(CurveKind::Arc { centre, a, b })
    }
    pub fn constrain(&mut self, kind: ConstraintKind) -> ConstraintId {
        let id = ConstraintId(self.constraints.len() as u32);
        self.constraints.push(Constraint {
            key: Key(u64::from(id.0)),
            kind,
            driven: false,
        });
        id
    }

    pub fn point_at(&self, p: PointId) -> Option<Vec2> {
        self.points.get(p.index()).map(|p| p.at)
    }

    /// Line end points.
    pub fn line_points(&self, c: CurveId) -> Option<(PointId, PointId)> {
        match self.curves.get(c.index())?.kind {
            CurveKind::Line { a, b } => Some((a, b)),
            _ => None,
        }
    }

    /// Centre point of a circle or arc.
    pub fn centre(&self, c: CurveId) -> Option<PointId> {
        match self.curves.get(c.index())?.kind {
            CurveKind::Circle { centre, .. } | CurveKind::Arc { centre, .. } => Some(centre),
            CurveKind::Line { .. } => None,
        }
    }

    /// Unknowns before any constraint: two per point, one per circle radius.
    pub fn unknowns(&self) -> usize {
        2 * self.points.len()
            + self
                .curves
                .iter()
                .filter(|c| matches!(c.kind, CurveKind::Circle { .. }))
                .count()
    }
}
