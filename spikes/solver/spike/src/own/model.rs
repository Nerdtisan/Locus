// SPDX-License-Identifier: AGPL-3.0-or-later

//! The harness sketch as a least-squares problem: unknowns (point coordinates,
//! circle radii) and residual blocks (one per constraint, plus each arc's
//! implicit equal-radius row and an optional drag hold). Each block touches at
//! most [`N`] unknowns and yields one or two rows with exact gradients.

use super::dual::{D, N, V2};
use solver_harness::{Constraint, Curve, PointId, Sketch, Vec2};

/// What a block stands for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Eq {
    /// Arc `c`: both ends on its circle.
    Arc(usize),
    /// Harness constraint `i`.
    User(usize),
    /// Drag: point `p` held at `at`.
    Hold(PointId, Vec2),
}

/// Shape of one block: its unknowns and where its rows and gradients live.
#[derive(Clone, Debug)]
pub struct Block {
    pub eq: Eq,
    pub vars: Vec<usize>,
    pub rows: usize,
    /// First residual row.
    pub r0: usize,
    /// First Jacobian value: `rows * vars.len()` values, row-major.
    pub j0: usize,
}

#[derive(Clone, Debug)]
pub struct Model {
    pub sk: Sketch,
    /// Unknown index of each circle's radius.
    pub radius: Vec<Option<usize>>,
    pub n: usize,
    pub blocks: Vec<Block>,
    pub m: usize,
    pub nj: usize,
}

fn line_of(sk: &Sketch, c: usize) -> Option<(usize, usize)> {
    match sk.curves.get(c)? {
        Curve::Line { a, b } => Some((*a, *b)),
        _ => None,
    }
}

impl Model {
    /// Validate topology and lay out unknowns and blocks. `Err` on references
    /// to missing or wrong-kind geometry (the input is untrusted).
    pub fn new(sk: &Sketch, hold: Option<(PointId, Vec2)>) -> Result<Self, String> {
        let mut n = 2 * sk.points.len();
        let radius = sk
            .curves
            .iter()
            .map(|c| match c {
                Curve::Circle { .. } => {
                    n += 1;
                    Some(n - 1)
                }
                _ => None,
            })
            .collect();
        let mut m = Model {
            sk: sk.clone(),
            radius,
            n,
            blocks: Vec::new(),
            m: 0,
            nj: 0,
        };
        for (c, cv) in sk.curves.iter().enumerate() {
            let pts: &[usize] = match cv {
                Curve::Line { a, b } => &[*a, *b],
                Curve::Circle { c, .. } => &[*c],
                Curve::Arc { c, a, b } => &[*c, *a, *b],
            };
            if pts.iter().any(|p| *p >= sk.points.len()) {
                return Err(format!("curve {c} refers to a missing point"));
            }
            if matches!(cv, Curve::Arc { .. }) {
                m.push(Eq::Arc(c))?;
            }
        }
        for i in 0..sk.constraints.len() {
            m.push(Eq::User(i))?;
        }
        if let Some((p, at)) = hold {
            m.push(Eq::Hold(p, at))?;
        }
        Ok(m)
    }

    fn push(&mut self, eq: Eq) -> Result<(), String> {
        let (vars, rows) = self.shape(eq)?;
        if vars.len() > N {
            return Err(format!("{eq:?} touches {} unknowns", vars.len()));
        }
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
        Ok(())
    }

    /// Unknowns and row count of a block, checking every reference.
    fn shape(&self, eq: Eq) -> Result<(Vec<usize>, usize), String> {
        let sk = &self.sk;
        let mut pts: Vec<usize> = Vec::new();
        let mut out: Vec<usize> = Vec::new();
        let curve = |c: usize, pts: &mut Vec<usize>, out: &mut Vec<usize>| -> Result<(), String> {
            match sk.curves.get(c) {
                Some(Curve::Line { a, b }) => pts.extend([*a, *b]),
                Some(Curve::Circle { c: cc, .. }) => {
                    pts.push(*cc);
                    out.extend(self.radius.get(c).copied().flatten());
                }
                Some(Curve::Arc { c, a, .. }) => pts.extend([*c, *a]),
                None => return Err(format!("no curve {c}")),
            }
            Ok(())
        };
        let is_line = |c: usize| line_of(sk, c).is_some();
        let need_line = |c: usize| -> Result<(), String> {
            if is_line(c) {
                Ok(())
            } else {
                Err(format!("curve {c} is not a line"))
            }
        };
        let need_round = |c: usize| -> Result<(), String> {
            match sk.curves.get(c) {
                Some(Curve::Circle { .. } | Curve::Arc { .. }) => Ok(()),
                _ => Err(format!("curve {c} is not a circle or arc")),
            }
        };
        let mut rows = 1;
        use Constraint::*;
        match eq {
            Eq::Arc(c) => {
                if let Some(Curve::Arc { c, a, b }) = sk.curves.get(c) {
                    pts.extend([*c, *a, *b]);
                }
            }
            Eq::Hold(p, _) => {
                pts.push(p);
                rows = 2;
            }
            Eq::User(i) => {
                let k = *sk.constraints.get(i).ok_or("no constraint")?;
                match k {
                    Coincident(p, q) => {
                        pts.extend([p, q]);
                        rows = 2;
                    }
                    Distance { p, q, .. }
                    | HorizontalDistance { p, q, .. }
                    | VerticalDistance { p, q, .. } => pts.extend([p, q]),
                    Fix { p, .. } => {
                        pts.push(p);
                        rows = 2;
                    }
                    PointOnCurve(p, c) => {
                        pts.push(p);
                        curve(c, &mut pts, &mut out)?;
                    }
                    Midpoint { p, line } => {
                        need_line(line)?;
                        pts.push(p);
                        curve(line, &mut pts, &mut out)?;
                        rows = 2;
                    }
                    PointLineDistance { p, line, .. } => {
                        need_line(line)?;
                        pts.push(p);
                        curve(line, &mut pts, &mut out)?;
                    }
                    Symmetric { p, q, about } => {
                        need_line(about)?;
                        pts.extend([p, q]);
                        curve(about, &mut pts, &mut out)?;
                        rows = 2;
                    }
                    Horizontal(c) | Vertical(c) => {
                        need_line(c)?;
                        curve(c, &mut pts, &mut out)?;
                    }
                    Radius { c, .. } | Diameter { c, .. } => {
                        need_round(c)?;
                        curve(c, &mut pts, &mut out)?;
                    }
                    Parallel(a, b)
                    | Perpendicular(a, b)
                    | EqualLength(a, b)
                    | Angle { l: a, m: b, .. } => {
                        need_line(a)?;
                        need_line(b)?;
                        curve(a, &mut pts, &mut out)?;
                        curve(b, &mut pts, &mut out)?;
                    }
                    Collinear(a, b) => {
                        need_line(a)?;
                        need_line(b)?;
                        curve(a, &mut pts, &mut out)?;
                        curve(b, &mut pts, &mut out)?;
                        rows = 2;
                    }
                    LineLineDistance { l, m, .. } => {
                        need_line(l)?;
                        need_line(m)?;
                        curve(l, &mut pts, &mut out)?;
                        if let Some((a, _)) = line_of(sk, m) {
                            pts.push(a);
                        }
                    }
                    Concentric(a, b) => {
                        need_round(a)?;
                        need_round(b)?;
                        let centre = |c| match sk.curves.get(c) {
                            Some(Curve::Circle { c, .. } | Curve::Arc { c, .. }) => *c,
                            _ => 0,
                        };
                        pts.extend([centre(a), centre(b)]);
                        rows = 2;
                    }
                    EqualRadius(a, b) => {
                        need_round(a)?;
                        need_round(b)?;
                        curve(a, &mut pts, &mut out)?;
                        curve(b, &mut pts, &mut out)?;
                    }
                    Tangent(a, b) => {
                        if is_line(a) && is_line(b) {
                            return Err("tangent between two lines".into());
                        }
                        for c in [a, b] {
                            if !is_line(c) {
                                need_round(c)?;
                            }
                        }
                        curve(a, &mut pts, &mut out)?;
                        curve(b, &mut pts, &mut out)?;
                    }
                }
            }
        }
        if pts.iter().any(|p| *p >= sk.points.len()) {
            return Err(format!("{eq:?} refers to a missing point"));
        }
        out.extend(pts.iter().flat_map(|p| [2 * p, 2 * p + 1]));
        out.sort_unstable();
        out.dedup();
        Ok((out, rows))
    }

    /// Initial unknowns from the sketch.
    pub fn x0(&self) -> Vec<f64> {
        let mut x = vec![0.0; self.n];
        for (i, p) in self.sk.points.iter().enumerate() {
            x[2 * i] = p.x;
            x[2 * i + 1] = p.y;
        }
        for (c, r) in self.sk.curves.iter().zip(&self.radius) {
            if let (Curve::Circle { r: v, .. }, Some(i)) = (c, r) {
                x[*i] = *v;
            }
        }
        x
    }

    /// Write unknowns back into a sketch of the same layout.
    pub fn write(&self, x: &[f64], into: &mut Sketch) {
        for (i, p) in into.points.iter_mut().enumerate() {
            if let (Some(px), Some(py)) = (x.get(2 * i), x.get(2 * i + 1)) {
                *p = Vec2::new(*px, *py);
            }
        }
        for (c, r) in into.curves.iter_mut().zip(&self.radius) {
            if let (Curve::Circle { r: v, .. }, Some(i)) = (c, r) {
                if let Some(xv) = x.get(*i) {
                    *v = *xv;
                }
            }
        }
    }

    /// Residuals into `r` (len `m`) and, if given, Jacobian values into `j` (len `nj`).
    pub fn eval(&self, x: &[f64], r: &mut [f64], mut j: Option<&mut [f64]>) {
        let mut out = [D::c(0.0); 2];
        for b in &self.blocks {
            let cx = Ctx {
                m: self,
                x,
                vars: &b.vars,
            };
            cx.rows(b.eq, &mut out);
            let nv = b.vars.len();
            for (k, d) in out.iter().take(b.rows).enumerate() {
                if let Some(slot) = r.get_mut(b.r0 + k) {
                    *slot = d.v;
                }
                if let Some(j) = j.as_deref_mut() {
                    if let Some(dst) = j.get_mut(b.j0 + k * nv..b.j0 + (k + 1) * nv) {
                        dst.copy_from_slice(&d.g[..nv]);
                    }
                }
            }
        }
    }
}

struct Ctx<'a> {
    m: &'a Model,
    x: &'a [f64],
    vars: &'a [usize],
}

impl Ctx<'_> {
    fn param(&self, i: usize) -> D {
        let v = self.x.get(i).copied().unwrap_or(0.0);
        match self.vars.iter().position(|&u| u == i) {
            Some(s) => D::var(v, s),
            None => D::c(v),
        }
    }
    fn p(&self, i: usize) -> V2 {
        V2 {
            x: self.param(2 * i),
            y: self.param(2 * i + 1),
        }
    }
    fn line(&self, c: usize) -> (V2, V2) {
        let (a, b) = line_of(&self.m.sk, c).unwrap_or((0, 0));
        (self.p(a), self.p(b))
    }
    /// Centre and radius of a circle or arc.
    fn round(&self, c: usize) -> (V2, D) {
        match self.m.sk.curves.get(c) {
            Some(Curve::Circle { c: cc, .. }) => {
                let r = self
                    .m
                    .radius
                    .get(c)
                    .copied()
                    .flatten()
                    .map_or(D::c(0.0), |i| self.param(i));
                (self.p(*cc), r)
            }
            Some(Curve::Arc { c, a, .. }) => {
                let cc = self.p(*c);
                (cc, self.p(*a).minus(cc).len())
            }
            _ => (self.p(0), D::c(0.0)),
        }
    }

    fn rows(&self, eq: Eq, out: &mut [D; 2]) {
        let sd = |p: V2, (a, b): (V2, V2)| {
            let d = b.minus(a);
            d.cross(p.minus(a)) / d.len().max_c(1e-12)
        };
        // Sine and cosine between two lines, scaled to millimetres by the lines'
        // geometric-mean length. Unit-free rows have gradients of ~1/length and
        // would weigh ~20x less than distance rows, which ill-conditions long
        // chains (measured: 3x the LM iterations on a 1,000-curve drag).
        let unit = |(a0, a1): (V2, V2), (b0, b1): (V2, V2)| {
            let (da, db) = (a1.minus(a0), b1.minus(b0));
            let s = (da.len() * db.len()).sqrt().max_c(1e-12);
            (da.cross(db) / s, da.dot(db) / s)
        };
        let sk = &self.m.sk;
        use Constraint::*;
        let k = match eq {
            Eq::Arc(c) => {
                if let Some(Curve::Arc { c, a, b }) = sk.curves.get(c) {
                    let cc = self.p(*c);
                    out[0] = self.p(*a).minus(cc).len() - self.p(*b).minus(cc).len();
                }
                return;
            }
            Eq::Hold(p, at) => {
                let q = self.p(p);
                out[0] = q.x - at.x;
                out[1] = q.y - at.y;
                return;
            }
            Eq::User(i) => match sk.constraints.get(i) {
                Some(k) => *k,
                None => return,
            },
        };
        match k {
            Coincident(a, b) => {
                let d = self.p(a).minus(self.p(b));
                *out = [d.x, d.y];
            }
            PointOnCurve(q, c) => {
                out[0] = if line_of(sk, c).is_some() {
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
            Parallel(a, b) => out[0] = unit(self.line(a), self.line(b)).0,
            Perpendicular(a, b) => out[0] = unit(self.line(a), self.line(b)).1,
            Tangent(a, b) => {
                let (ln, c) = match (line_of(sk, a), line_of(sk, b)) {
                    (Some(_), _) => (Some(a), b),
                    (_, Some(_)) => (Some(b), a),
                    _ => (None, b),
                };
                out[0] = match ln {
                    Some(ln) => {
                        let (la, lb) = self.line(ln);
                        let (cc, r) = self.round(c);
                        match shared_end(sk, ln, c) {
                            // A fillet: `distance(centre, line) - radius` peaks at
                            // zero when the line ends on the arc, so its gradient
                            // vanishes at the solution. Use the cosine between the
                            // line and the radius to the shared end instead, times
                            // the radius so the row is in millimetres.
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
                        if ext.v.abs() <= int.v.abs() {
                            ext
                        } else {
                            int
                        }
                    }
                }
            }
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
            // sin(φ − θ) from the unit sine and cosine of φ: no atan2 needed.
            Angle { l, m, deg } => {
                let (s, c) = unit(self.line(l), self.line(m));
                let t = deg.to_radians();
                out[0] = s * t.cos() - c * t.sin();
            }
            Radius { c, r } => out[0] = self.round(c).1 - r,
            Diameter { c, d } => out[0] = self.round(c).1 * 2.0 - d,
        }
    }
}

/// The end point line `l` shares with arc `c`, if any.
fn shared_end(sk: &Sketch, l: usize, c: usize) -> Option<usize> {
    let (a, b) = line_of(sk, l)?;
    match sk.curves.get(c)? {
        Curve::Arc { a: s, b: e, .. } => [a, b].into_iter().find(|p| p == s || p == e),
        _ => None,
    }
}
