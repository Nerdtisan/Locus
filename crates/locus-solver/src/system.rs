// SPDX-License-Identifier: AGPL-3.0-or-later

//! A [`Sketch`] as a least-squares system (ADR 0007): unknowns (point
//! coordinates, circle radii) and residual blocks. One block per driving
//! constraint, plus each arc's implicit equal-radius row and an optional drag
//! hold. A block touches at most [`N`] unknowns and yields one or two rows, all
//! in millimetres, with exact gradients from [`crate::dual`]. The solver and the
//! rank analysis evaluate the same rows.

use crate::dual::{D, N, V2};
use crate::sketch::{
    ConstraintId, ConstraintKind, CurveId, CurveKind, PointId, Sketch, SketchError,
};
use locus_geom::Vec2;

/// What a block stands for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Eq {
    /// The arc's both ends on its circle.
    Arc(CurveId),
    /// A driving constraint.
    User(ConstraintId),
    /// Drag: point `p` held at `at`, rows scaled by `w`.
    Hold { p: PointId, at: Vec2, w: f64 },
}

/// One block: its unknowns and where its rows and gradients live.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Block {
    pub eq: Eq,
    pub vars: Vec<usize>,
    pub rows: usize,
    /// First residual row.
    pub r0: usize,
    /// First Jacobian value: `rows * vars.len()` values, row-major.
    pub j0: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct System {
    /// Unknown index of each circle's radius, per curve.
    pub radius: Vec<Option<usize>>,
    pub n: usize,
    pub blocks: Vec<Block>,
    pub m: usize,
    pub nj: usize,
}

impl System {
    /// Check every reference and lay out unknowns and blocks. Driven
    /// constraints are left out (they measure, §5). `extra` holds blocks
    /// appended after the constraints (a drag hold).
    pub fn new(sk: &Sketch, extra: Option<Eq>) -> Result<Self, SketchError> {
        validate(sk)?;
        let mut n = 2 * sk.points.len();
        let radius = sk
            .curves
            .iter()
            .map(|c| match c.kind {
                CurveKind::Circle { .. } => {
                    n += 1;
                    Some(n - 1)
                }
                _ => None,
            })
            .collect();
        let mut s = System {
            radius,
            n,
            blocks: Vec::new(),
            m: 0,
            nj: 0,
        };
        for (i, c) in sk.curves.iter().enumerate() {
            if matches!(c.kind, CurveKind::Arc { .. }) {
                s.push(sk, Eq::Arc(CurveId(i as u32)));
            }
        }
        for (i, c) in sk.constraints.iter().enumerate() {
            if !c.driven {
                s.push(sk, Eq::User(ConstraintId(i as u32)));
            }
        }
        if let Some(e) = extra {
            s.push(sk, e);
        }
        Ok(s)
    }

    fn push(&mut self, sk: &Sketch, eq: Eq) {
        let (vars, rows) = self.shape(sk, eq);
        // Every §5 v1 kind touches at most two lines' worth of unknowns.
        debug_assert!(vars.len() <= N, "{eq:?} touches {} unknowns", vars.len());
        let b = Block {
            eq,
            rows,
            r0: self.m,
            j0: self.nj,
            vars,
        };
        self.m += b.rows;
        self.nj += b.rows * b.vars.len();
        self.blocks.push(b);
    }

    /// Unknowns and row count of a block (references already validated).
    fn shape(&self, sk: &Sketch, eq: Eq) -> (Vec<usize>, usize) {
        let mut pts: Vec<PointId> = Vec::new();
        let mut out: Vec<usize> = Vec::new();
        let curve = |c: CurveId, pts: &mut Vec<PointId>, out: &mut Vec<usize>| match sk
            .curves
            .get(c.index())
            .map(|c| c.kind)
        {
            Some(CurveKind::Line { a, b }) => pts.extend([a, b]),
            Some(CurveKind::Circle { centre, .. }) => {
                pts.push(centre);
                out.extend(self.radius.get(c.index()).copied().flatten());
            }
            Some(CurveKind::Arc { centre, a, .. }) => pts.extend([centre, a]),
            None => {}
        };
        let mut rows = 1;
        use ConstraintKind::*;
        match eq {
            Eq::Arc(c) => {
                if let Some(CurveKind::Arc { centre, a, b }) =
                    sk.curves.get(c.index()).map(|c| c.kind)
                {
                    pts.extend([centre, a, b]);
                }
            }
            Eq::Hold { p, .. } => {
                pts.push(p);
                rows = 2;
            }
            Eq::User(i) => {
                let Some(k) = sk.constraints.get(i.index()).map(|c| c.kind) else {
                    return (Vec::new(), 0);
                };
                match k {
                    Coincident(..)
                    | Fix { .. }
                    | Midpoint { .. }
                    | Symmetric { .. }
                    | Collinear(..)
                    | Concentric(..) => rows = 2,
                    _ => {}
                }
                match k {
                    // Only the centres take part.
                    Concentric(a, b) => pts.extend(sk.centre(a).into_iter().chain(sk.centre(b))),
                    // Line `l` and `m`'s first point.
                    LineLineDistance { l, m, .. } => {
                        curve(l, &mut pts, &mut out);
                        pts.extend(sk.line_points(m).map(|(a, _)| a));
                    }
                    _ => {
                        let (ps, cs) = k.refs();
                        pts.extend(ps);
                        for c in cs {
                            curve(c, &mut pts, &mut out);
                        }
                    }
                }
            }
        }
        out.extend(pts.iter().flat_map(|p| [2 * p.index(), 2 * p.index() + 1]));
        out.sort_unstable();
        out.dedup();
        (out, rows)
    }

    /// Unknowns from the sketch.
    pub fn x0(&self, sk: &Sketch) -> Vec<f64> {
        let mut x = vec![0.0; self.n];
        for (i, p) in sk.points.iter().enumerate() {
            x[2 * i] = p.at.x;
            x[2 * i + 1] = p.at.y;
        }
        for (c, r) in sk.curves.iter().zip(&self.radius) {
            if let (CurveKind::Circle { r: v, .. }, Some(i)) = (c.kind, r) {
                x[*i] = v;
            }
        }
        x
    }

    /// Write unknowns back into the sketch.
    pub fn write(&self, x: &[f64], sk: &mut Sketch) {
        for (i, p) in sk.points.iter_mut().enumerate() {
            if let (Some(px), Some(py)) = (x.get(2 * i), x.get(2 * i + 1)) {
                p.at = Vec2::new(*px, *py);
            }
        }
        for (c, r) in sk.curves.iter_mut().zip(&self.radius) {
            if let (CurveKind::Circle { r: v, .. }, Some(i)) = (&mut c.kind, r)
                && let Some(xv) = x.get(*i)
            {
                *v = *xv;
            }
        }
    }

    /// Residuals into `r` (len `m`) and, if given, Jacobian values into `j`
    /// (len `nj`).
    pub fn eval(&self, sk: &Sketch, x: &[f64], r: &mut [f64], mut j: Option<&mut [f64]>) {
        let mut out = [D::c(0.0); 2];
        for b in &self.blocks {
            let cx = Ctx {
                sk,
                radius: &self.radius,
                x,
                vars: &b.vars,
            };
            cx.rows(b.eq, &mut out);
            let nv = b.vars.len();
            for (k, d) in out.iter().take(b.rows).enumerate() {
                if let Some(slot) = r.get_mut(b.r0 + k) {
                    *slot = d.v;
                }
                if let Some(j) = j.as_deref_mut()
                    && let (Some(dst), Some(src)) =
                        (j.get_mut(b.j0 + k * nv..b.j0 + (k + 1) * nv), d.g.get(..nv))
                {
                    dst.copy_from_slice(src);
                }
            }
        }
    }

    /// Same unknowns and blocks: a factorisation of one can be reused for the
    /// other (only values differ).
    pub fn same_pattern(&self, o: &System) -> bool {
        self.n == o.n
            && self.blocks.len() == o.blocks.len()
            && self
                .blocks
                .iter()
                .zip(&o.blocks)
                .all(|(a, b)| a.vars == b.vars && a.rows == b.rows)
    }
}

/// The current value of a dimensional constraint, measured from the sketch
/// (driven dimensions report this, §5). `None` for geometric constraints.
pub(crate) fn measure(sk: &Sketch, k: ConstraintKind) -> Option<f64> {
    use ConstraintKind::*;
    let radius: Vec<Option<usize>> = Vec::new();
    // Plain positions, read through the same residual code with the target set
    // to zero: the residual is then the measured value.
    let x: Vec<f64> = sk.points.iter().flat_map(|p| [p.at.x, p.at.y]).collect();
    let cx = Ctx {
        sk,
        radius: &radius,
        x: &x,
        vars: &[],
    };
    let mut out = [D::c(0.0); 2];
    let zeroed = match k {
        Distance { p, q, .. } => Distance { p, q, d: 0.0 },
        PointLineDistance { p, line, .. } => PointLineDistance { p, line, d: 0.0 },
        LineLineDistance { l, m, .. } => LineLineDistance { l, m, d: 0.0 },
        HorizontalDistance { p, q, .. } => HorizontalDistance { p, q, d: 0.0 },
        VerticalDistance { p, q, .. } => VerticalDistance { p, q, d: 0.0 },
        Radius { c, .. } => Radius { c, r: 0.0 },
        Diameter { c, .. } => Diameter { c, d: 0.0 },
        Angle { l, m, .. } => {
            let (s, c) = cx.sin_cos(cx.line(l), cx.line(m));
            return Some(s.v.atan2(c.v));
        }
        _ => return None,
    };
    cx.user(zeroed, &mut out);
    Some(out[0].v)
}

/// Check every reference, kind and value. The input is untrusted: nothing
/// past this point may panic on it.
fn validate(sk: &Sketch) -> Result<(), SketchError> {
    let np = sk.points.len();
    for p in &sk.points {
        if !p.at.is_finite() {
            return Err(SketchError::NotFinite(p.key));
        }
    }
    for c in &sk.curves {
        let pts: &[PointId] = match &c.kind {
            CurveKind::Line { a, b } => &[*a, *b],
            CurveKind::Circle { centre, r } => {
                if !r.is_finite() {
                    return Err(SketchError::NotFinite(c.key));
                }
                &[*centre]
            }
            CurveKind::Arc { centre, a, b } => &[*centre, *a, *b],
        };
        if pts.iter().any(|p| p.index() >= np) {
            return Err(SketchError::CurveMissingPoint(c.key));
        }
    }
    let kind_of = |c: CurveId| sk.curves.get(c.index()).map(|c| c.kind);
    let is_line = |c: CurveId| matches!(kind_of(c), Some(CurveKind::Line { .. }));
    let is_round = |c: CurveId| {
        matches!(
            kind_of(c),
            Some(CurveKind::Circle { .. } | CurveKind::Arc { .. })
        )
    };
    for c in &sk.constraints {
        let k = c.kind;
        let (ps, cs) = k.refs();
        if ps.iter().any(|p| p.index() >= np) || cs.iter().any(|c| c.index() >= sk.curves.len()) {
            return Err(SketchError::MissingRef(c.key));
        }
        let wrong = |expected| SketchError::WrongKind {
            key: c.key,
            name: k.name(),
            expected,
        };
        use ConstraintKind::*;
        let value = match k {
            Horizontal(l)
            | Vertical(l)
            | Midpoint { line: l, .. }
            | PointLineDistance { line: l, .. }
            | Symmetric { about: l, .. } => {
                if !is_line(l) {
                    return Err(wrong("a line"));
                }
                None
            }
            Parallel(a, b)
            | Perpendicular(a, b)
            | Collinear(a, b)
            | EqualLength(a, b)
            | LineLineDistance { l: a, m: b, .. }
            | Angle { l: a, m: b, .. } => {
                if !(is_line(a) && is_line(b)) {
                    return Err(wrong("two lines"));
                }
                None
            }
            Concentric(a, b) | EqualRadius(a, b) => {
                if !(is_round(a) && is_round(b)) {
                    return Err(wrong("two circles or arcs"));
                }
                None
            }
            Radius { c, .. } | Diameter { c, .. } => {
                if !is_round(c) {
                    return Err(wrong("a circle or arc"));
                }
                None
            }
            Tangent(a, b) => {
                if !(is_round(a) || is_round(b)) {
                    return Err(wrong("a circle or arc"));
                }
                None
            }
            Fix { at, .. } => Some(at.x + at.y),
            Coincident(..) | PointOnCurve(..) => None,
            Distance { .. } | HorizontalDistance { .. } | VerticalDistance { .. } => None,
        };
        let dim = match k {
            Distance { d, .. }
            | PointLineDistance { d, .. }
            | LineLineDistance { d, .. }
            | HorizontalDistance { d, .. }
            | VerticalDistance { d, .. }
            | Diameter { d, .. } => Some(d),
            Radius { r, .. } => Some(r),
            Angle { rad, .. } => Some(rad),
            _ => value,
        };
        if dim.is_some_and(|v| !v.is_finite()) {
            return Err(SketchError::NotFinite(c.key));
        }
    }
    Ok(())
}

struct Ctx<'a> {
    sk: &'a Sketch,
    radius: &'a [Option<usize>],
    x: &'a [f64],
    vars: &'a [usize],
}

/// Signed distance from `p` to the line through `a`, `b` (left positive).
fn sd(p: V2, (a, b): (V2, V2)) -> D {
    let d = b.minus(a);
    d.cross(p.minus(a)) / d.len().max_c(1e-12)
}

impl Ctx<'_> {
    fn param(&self, i: usize) -> D {
        let v = self.x.get(i).copied().unwrap_or(0.0);
        match self.vars.iter().position(|&u| u == i) {
            Some(s) => D::var(v, s),
            None => D::c(v),
        }
    }
    fn p(&self, p: PointId) -> V2 {
        V2 {
            x: self.param(2 * p.index()),
            y: self.param(2 * p.index() + 1),
        }
    }
    fn line(&self, c: CurveId) -> (V2, V2) {
        let (a, b) = self.sk.line_points(c).unwrap_or((PointId(0), PointId(0)));
        (self.p(a), self.p(b))
    }
    /// Centre and radius of a circle or arc.
    fn round(&self, c: CurveId) -> (V2, D) {
        match self.sk.curves.get(c.index()).map(|k| k.kind) {
            Some(CurveKind::Circle { centre, r }) => {
                let rd = self
                    .radius
                    .get(c.index())
                    .copied()
                    .flatten()
                    .map_or(D::c(r), |i| self.param(i));
                (self.p(centre), rd)
            }
            Some(CurveKind::Arc { centre, a, .. }) => {
                let cc = self.p(centre);
                (cc, self.p(a).minus(cc).len())
            }
            _ => (self.p(PointId(0)), D::c(0.0)),
        }
    }
    /// Sine and cosine of the angle between two lines, scaled to millimetres
    /// by the lines' geometric-mean length. Unit-free rows would have gradients
    /// of ~1/length and weigh ~20× less than distance rows (ADR 0007).
    fn sin_cos(&self, (a0, a1): (V2, V2), (b0, b1): (V2, V2)) -> (D, D) {
        let (da, db) = (a1.minus(a0), b1.minus(b0));
        let s = (da.len() * db.len()).sqrt().max_c(1e-12);
        (da.cross(db) / s, da.dot(db) / s)
    }

    fn rows(&self, eq: Eq, out: &mut [D; 2]) {
        match eq {
            Eq::Arc(c) => {
                if let Some(CurveKind::Arc { centre, a, b }) =
                    self.sk.curves.get(c.index()).map(|k| k.kind)
                {
                    let cc = self.p(centre);
                    out[0] = self.p(a).minus(cc).len() - self.p(b).minus(cc).len();
                }
            }
            Eq::Hold { p, at, w } => {
                let q = self.p(p);
                *out = [(q.x - at.x) * w, (q.y - at.y) * w];
            }
            Eq::User(i) => {
                if let Some(c) = self.sk.constraints.get(i.index()) {
                    self.user(c.kind, out);
                }
            }
        }
    }

    fn user(&self, k: ConstraintKind, out: &mut [D; 2]) {
        let sk = self.sk;
        use ConstraintKind::*;
        match k {
            Coincident(a, b) => {
                let d = self.p(a).minus(self.p(b));
                *out = [d.x, d.y];
            }
            PointOnCurve(q, c) => {
                out[0] = if sk.line_points(c).is_some() {
                    sd(self.p(q), self.line(c))
                } else {
                    let (cc, r) = self.round(c);
                    self.p(q).minus(cc).len() - r
                }
            }
            Horizontal(c) => {
                let (a, b) = self.line(c);
                out[0] = b.y - a.y;
            }
            Vertical(c) => {
                let (a, b) = self.line(c);
                out[0] = b.x - a.x;
            }
            Parallel(a, b) => out[0] = self.sin_cos(self.line(a), self.line(b)).0,
            Perpendicular(a, b) => out[0] = self.sin_cos(self.line(a), self.line(b)).1,
            Tangent(a, b) => out[0] = self.tangent(a, b),
            Collinear(a, b) => {
                let la = self.line(a);
                let (b0, b1) = self.line(b);
                *out = [sd(b0, la), sd(b1, la)];
            }
            Concentric(a, b) => {
                let d = self.round(a).0.minus(self.round(b).0);
                *out = [d.x, d.y];
            }
            EqualLength(a, b) => {
                let ((a0, a1), (b0, b1)) = (self.line(a), self.line(b));
                out[0] = a1.minus(a0).len() - b1.minus(b0).len();
            }
            EqualRadius(a, b) => out[0] = self.round(a).1 - self.round(b).1,
            Symmetric { p, q, about } => {
                let l = self.line(about);
                let dir = l.1.minus(l.0);
                let (pp, qq) = (self.p(p), self.p(q));
                *out = [
                    sd(pp.mid(qq), l),
                    qq.minus(pp).dot(dir) / dir.len().max_c(1e-12),
                ];
            }
            Midpoint { p, line } => {
                let (a, b) = self.line(line);
                let d = self.p(p).minus(a.mid(b));
                *out = [d.x, d.y];
            }
            Fix { p, at } => {
                let q = self.p(p);
                *out = [q.x - at.x, q.y - at.y];
            }
            Distance { p, q, d } => out[0] = self.p(p).minus(self.p(q)).len() - d,
            PointLineDistance { p, line, d } => out[0] = sd(self.p(p), self.line(line)).abs() - d,
            LineLineDistance { l, m, d } => out[0] = sd(self.line(m).0, self.line(l)).abs() - d,
            HorizontalDistance { p, q, d } => out[0] = self.p(q).x - self.p(p).x - d,
            VerticalDistance { p, q, d } => out[0] = self.p(q).y - self.p(p).y - d,
            // sin(φ − θ) from the scaled sine and cosine of φ: no atan2. It also
            // holds at φ = θ + π; starting from the previous solution keeps
            // the solve on the intended branch.
            Angle { l, m, rad } => {
                let (s, c) = self.sin_cos(self.line(l), self.line(m));
                out[0] = s * rad.cos() - c * rad.sin();
            }
            Radius { c, r } => out[0] = self.round(c).1 - r,
            Diameter { c, d } => out[0] = self.round(c).1 * 2.0 - d,
        }
    }

    fn tangent(&self, a: CurveId, b: CurveId) -> D {
        let sk = self.sk;
        let (ln, c) = match (sk.line_points(a), sk.line_points(b)) {
            (Some(_), _) => (Some(a), b),
            (_, Some(_)) => (Some(b), a),
            _ => (None, b),
        };
        match ln {
            Some(ln) => {
                let (la, lb) = self.line(ln);
                let (cc, r) = self.round(c);
                match shared_end(sk, ln, c) {
                    // A fillet: `distance(centre, line) − radius` peaks at zero
                    // when the line ends on the arc, so its gradient vanishes at
                    // the solution and the rank analysis would call it
                    // redundant. Use the line direction against the radius to
                    // the shared end (a cosine times the radius: mm).
                    Some(q) => {
                        let (dir, rad) = (lb.minus(la), self.p(q).minus(cc));
                        dir.dot(rad) / dir.len().max_c(1e-12)
                    }
                    None => sd(cc, (la, lb)).abs() - r,
                }
            }
            None => {
                let ((c1, r1), (c2, r2)) = (self.round(a), self.round(b));
                let d = c1.minus(c2).len();
                let (ext, int) = (d - (r1 + r2), d - (r1 - r2).abs());
                if ext.v.abs() <= int.v.abs() { ext } else { int }
            }
        }
    }
}

/// The end point line `l` shares with arc `c`, if any.
fn shared_end(sk: &Sketch, l: CurveId, c: CurveId) -> Option<PointId> {
    let (a, b) = sk.line_points(l)?;
    match sk.curves.get(c.index())?.kind {
        CurveKind::Arc { a: s, b: e, .. } => [a, b].into_iter().find(|p| *p == s || *p == e),
        _ => None,
    }
}
