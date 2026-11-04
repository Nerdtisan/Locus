// SPDX-License-Identifier: AGPL-3.0-or-later

//! DOF and redundancy by Jacobian rank (issue #42 prototype, SPEC §3, §5 Solver UX).
//!
//! ezpz solves but reports neither a DOF count nor redundant constraints, so this
//! checks that a backend-independent analysis on top of it is cheap enough. It
//! linearises the harness constraints at the solved geometry (central differences
//! of signed residual rows, each touching a handful of unknowns), then inserts the
//! rows one constraint at a time into a sparse row-echelon form with partial
//! pivoting. A row that reduces to zero depends on the rows before it: its
//! constraint removes no freedom (redundant, or conflicting if it is not
//! satisfied). DOF = unknowns - rank.

use solver_harness::{Constraint, Curve, Sketch, Vec2};

/// Pivot tolerance on rows scaled to unit length (cf. SolidCraft's 1e-7 on its
/// equilibrated Jacobian).
const RANK_TOL: f64 = 1e-8;
const STEP: f64 = 1e-6;
/// Rows are in mm per mm (or per radian-ish); a raw gradient this small is noise.
const MIN_ROW_NORM: f64 = 1e-6;

/// Result of [`analyse`].
#[derive(Clone, Debug, PartialEq)]
pub struct Freedom {
    pub dof: usize,
    /// Constraints (harness indices) with at least one dependent row, in order.
    pub dependent: Vec<usize>,
}

/// Unknown index of each point coordinate and circle radius.
struct Layout {
    radius: Vec<Option<usize>>,
    n: usize,
}

impl Layout {
    fn new(sk: &Sketch) -> Self {
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
        Layout { radius, n }
    }
}

fn line(sk: &Sketch, l: usize) -> Option<(usize, usize)> {
    match sk.curves.get(l)? {
        Curve::Line { a, b } => Some((*a, *b)),
        _ => None,
    }
}

/// Unknowns a constraint's rows depend on.
fn vars(sk: &Sketch, lay: &Layout, k: &Constraint) -> Vec<usize> {
    let mut pts = Vec::new();
    let mut out = Vec::new();
    let curve = |c: usize, pts: &mut Vec<usize>, out: &mut Vec<usize>| match sk.curves.get(c) {
        Some(Curve::Line { a, b }) => pts.extend([*a, *b]),
        Some(Curve::Circle { c: cc, .. }) => {
            pts.push(*cc);
            out.extend(lay.radius.get(c).copied().flatten());
        }
        Some(Curve::Arc { c, a, .. }) => pts.extend([*c, *a]),
        None => {}
    };
    use Constraint::*;
    match *k {
        Coincident(p, q) | Distance { p, q, .. } => pts.extend([p, q]),
        HorizontalDistance { p, q, .. } | VerticalDistance { p, q, .. } => pts.extend([p, q]),
        Fix { p, .. } => pts.push(p),
        PointOnCurve(p, c) => {
            pts.push(p);
            curve(c, &mut pts, &mut out);
        }
        Midpoint { p, line } | PointLineDistance { p, line, .. } => {
            pts.push(p);
            curve(line, &mut pts, &mut out);
        }
        Symmetric { p, q, about } => {
            pts.extend([p, q]);
            curve(about, &mut pts, &mut out);
        }
        Horizontal(c) | Vertical(c) | Radius { c, .. } | Diameter { c, .. } => {
            curve(c, &mut pts, &mut out)
        }
        Parallel(a, b) | Perpendicular(a, b) | Tangent(a, b) | Collinear(a, b) => {
            curve(a, &mut pts, &mut out);
            curve(b, &mut pts, &mut out);
        }
        Concentric(a, b) | EqualLength(a, b) | EqualRadius(a, b) => {
            curve(a, &mut pts, &mut out);
            curve(b, &mut pts, &mut out);
        }
        LineLineDistance { l: a, m: b, .. } | Angle { l: a, m: b, .. } => {
            curve(a, &mut pts, &mut out);
            curve(b, &mut pts, &mut out);
        }
    }
    out.extend(pts.iter().flat_map(|p| [2 * p, 2 * p + 1]));
    out.sort_unstable();
    out.dedup();
    out
}

/// Read unknown `v` from the sketch.
fn get(sk: &Sketch, lay: &Layout, v: usize) -> f64 {
    if v < 2 * sk.points.len() {
        let p = at(sk, v / 2);
        return if v.is_multiple_of(2) { p.x } else { p.y };
    }
    for (c, r) in lay.radius.iter().enumerate() {
        if *r == Some(v) {
            if let Some(Curve::Circle { r, .. }) = sk.curves.get(c) {
                return *r;
            }
        }
    }
    0.0
}

fn set(sk: &mut Sketch, lay: &Layout, v: usize, x: f64) {
    if v < 2 * sk.points.len() {
        if let Some(p) = sk.points.get_mut(v / 2) {
            if v.is_multiple_of(2) {
                p.x = x;
            } else {
                p.y = x;
            }
        }
        return;
    }
    if let Some(c) = lay.radius.iter().position(|r| *r == Some(v)) {
        if let Some(Curve::Circle { r, .. }) = sk.curves.get_mut(c) {
            *r = x;
        }
    }
}

fn signed_dist(p: Vec2, a: Vec2, b: Vec2) -> f64 {
    let d = b - a;
    d.cross(p - a) / d.len().max(1e-12)
}

/// Point `i`, or the origin for a bad index (the input is untrusted: no panics).
fn at(sk: &Sketch, i: usize) -> Vec2 {
    sk.points.get(i).copied().unwrap_or_default()
}

fn round(sk: &Sketch, c: usize) -> (Vec2, f64) {
    match sk.curves.get(c) {
        Some(Curve::Circle { c, r }) => (at(sk, *c), *r),
        Some(Curve::Arc { c, a, .. }) => (at(sk, *c), at(sk, *a).dist(at(sk, *c))),
        _ => (Vec2::default(), 0.0),
    }
}

/// The end point line `l` shares with arc `c`, if any.
fn shared_end(sk: &Sketch, l: usize, c: usize) -> Option<usize> {
    let (a, b) = line(sk, l)?;
    match sk.curves.get(c)? {
        Curve::Arc { a: s, b: e, .. } => [a, b].into_iter().find(|p| p == s || p == e),
        _ => None,
    }
}

/// Signed residual rows (smooth where the harness checker takes magnitudes).
fn rows(sk: &Sketch, k: &Constraint, out: &mut Vec<f64>) {
    let p = |i: usize| sk.points.get(i).copied().unwrap_or_default();
    let l =
        |c: usize| line(sk, c).map_or((Vec2::default(), Vec2::default()), |(a, b)| (p(a), p(b)));
    let unit = |a: (Vec2, Vec2), b: (Vec2, Vec2)| {
        let (da, db) = (a.1 - a.0, b.1 - b.0);
        let s = (da.len() * db.len()).max(1e-12);
        (da.cross(db) / s, da.dot(db) / s)
    };
    use Constraint::*;
    match *k {
        Coincident(a, b) => out.extend([p(a).x - p(b).x, p(a).y - p(b).y]),
        PointOnCurve(q, c) => match line(sk, c) {
            Some(_) => {
                let (a, b) = l(c);
                out.push(signed_dist(p(q), a, b));
            }
            None => {
                let (cc, r) = round(sk, c);
                out.push(p(q).dist(cc) - r);
            }
        },
        Horizontal(c) => out.push(l(c).1.y - l(c).0.y),
        Vertical(c) => out.push(l(c).1.x - l(c).0.x),
        Parallel(a, b) => out.push(unit(l(a), l(b)).0),
        Perpendicular(a, b) => out.push(unit(l(a), l(b)).1),
        Tangent(a, b) => match (line(sk, a), line(sk, b)) {
            (Some(_), _) | (_, Some(_)) => {
                let (ln, c) = if line(sk, a).is_some() {
                    (a, b)
                } else {
                    (b, a)
                };
                let (la, lb) = l(ln);
                let (cc, r) = round(sk, c);
                match shared_end(sk, ln, c) {
                    // A fillet: `distance(centre, line) - radius` peaks at zero
                    // when the line ends on the arc, so its gradient vanishes at
                    // the solution. Use the cosine between line and radius.
                    Some(q) => {
                        let (dir, rad) = (lb - la, p(q) - cc);
                        out.push(dir.dot(rad) / (dir.len() * rad.len()).max(1e-12));
                    }
                    None => out.push(signed_dist(cc, la, lb).abs() - r),
                }
            }
            _ => {
                let ((c1, r1), (c2, r2)) = (round(sk, a), round(sk, b));
                let d = c1.dist(c2);
                let (ext, int) = (d - (r1 + r2), d - (r1 - r2).abs());
                out.push(if ext.abs() <= int.abs() { ext } else { int });
            }
        },
        Collinear(a, b) => {
            let (a0, a1) = l(a);
            let (b0, b1) = l(b);
            out.extend([signed_dist(b0, a0, a1), signed_dist(b1, a0, a1)]);
        }
        Concentric(a, b) => {
            let d = round(sk, a).0 - round(sk, b).0;
            out.extend([d.x, d.y]);
        }
        EqualLength(a, b) => out.push(l(a).0.dist(l(a).1) - l(b).0.dist(l(b).1)),
        EqualRadius(a, b) => out.push(round(sk, a).1 - round(sk, b).1),
        Symmetric { p: a, q: b, about } => {
            let (la, lb) = l(about);
            let dir = lb - la;
            out.push(signed_dist((p(a) + p(b)) * 0.5, la, lb));
            out.push((p(b) - p(a)).dot(dir) / dir.len().max(1e-12));
        }
        Midpoint { p: q, line: c } => {
            let (a, b) = l(c);
            let m = (a + b) * 0.5;
            out.extend([p(q).x - m.x, p(q).y - m.y]);
        }
        Fix { p: q, at } => out.extend([p(q).x - at.x, p(q).y - at.y]),
        Distance { p: a, q: b, d } => out.push(p(a).dist(p(b)) - d),
        PointLineDistance { p: q, line: c, d } => {
            let (a, b) = l(c);
            out.push(signed_dist(p(q), a, b).abs() - d);
        }
        LineLineDistance { l: a, m: b, d } => {
            let (a0, a1) = l(a);
            out.push(signed_dist(l(b).0, a0, a1).abs() - d);
        }
        HorizontalDistance { p: a, q: b, d } => out.push(p(b).x - p(a).x - d),
        VerticalDistance { p: a, q: b, d } => out.push(p(b).y - p(a).y - d),
        Angle { l: a, m: b, deg } => {
            let (s, c) = unit(l(a), l(b));
            let e = s.atan2(c) - deg.to_radians();
            out.push(e.sin());
        }
        Radius { c, r } => out.push(round(sk, c).1 - r),
        Diameter { c, d } => out.push(2.0 * round(sk, c).1 - d),
    }
}

/// Jacobian rows of `rows` by central differences over the touched unknowns.
fn jacobian_rows(
    sk: &mut Sketch,
    lay: &Layout,
    vs: &[usize],
    f: &dyn Fn(&Sketch, &mut Vec<f64>),
) -> Vec<Vec<(usize, f64)>> {
    let mut base = Vec::new();
    f(sk, &mut base);
    let mut out: Vec<Vec<(usize, f64)>> = vec![Vec::new(); base.len()];
    let (mut plus, mut minus) = (Vec::new(), Vec::new());
    for &v in vs {
        let x0 = get(sk, lay, v);
        let h = STEP * x0.abs().max(1.0);
        set(sk, lay, v, x0 + h);
        plus.clear();
        f(sk, &mut plus);
        set(sk, lay, v, x0 - h);
        minus.clear();
        f(sk, &mut minus);
        set(sk, lay, v, x0);
        for (row, (a, b)) in out.iter_mut().zip(plus.iter().zip(&minus)) {
            let d = (a - b) / (2.0 * h);
            if d != 0.0 {
                row.push((v, d));
            }
        }
    }
    out
}

/// Sparse row-echelon form: `pivot[c]` is the row whose first entry is column `c`.
struct Echelon {
    pivot: Vec<Option<Vec<(usize, f64)>>>,
}

/// `a - k * b` for sorted sparse rows.
fn axpy(a: &[(usize, f64)], k: f64, b: &[(usize, f64)]) -> Vec<(usize, f64)> {
    let mut out = Vec::with_capacity(a.len() + b.len());
    let (mut i, mut j) = (0, 0);
    while i < a.len() || j < b.len() {
        let (ca, cb) = (
            a.get(i).map_or(usize::MAX, |x| x.0),
            b.get(j).map_or(usize::MAX, |x| x.0),
        );
        let (c, v) = if ca < cb {
            i += 1;
            (ca, a[i - 1].1)
        } else if cb < ca {
            j += 1;
            (cb, -k * b[j - 1].1)
        } else {
            i += 1;
            j += 1;
            (ca, a[i - 1].1 - k * b[j - 1].1)
        };
        if v.abs() > RANK_TOL * 1e-3 {
            out.push((c, v));
        }
    }
    out
}

impl Echelon {
    /// Insert a row; false if it depends on the rows already in.
    fn insert(&mut self, mut row: Vec<(usize, f64)>) -> bool {
        loop {
            // Drop leading entries that are numerically zero.
            let lead = row.iter().position(|(_, v)| v.abs() > RANK_TOL);
            let Some(lead) = lead else { return false };
            row.drain(..lead);
            let (c, v) = row[0];
            let Some(slot) = self.pivot.get_mut(c) else {
                return false;
            };
            match slot.take() {
                None => {
                    *slot = Some(row);
                    return true;
                }
                Some(p) => {
                    // Partial pivoting: keep the larger leading entry as the pivot
                    // and push the other row on down.
                    let (keep, mut other) = if v.abs() > p[0].1.abs() {
                        (row, p)
                    } else {
                        (p, row)
                    };
                    let k = other[0].1 / keep[0].1;
                    other = axpy(&other, k, &keep);
                    *slot = Some(keep);
                    row = other;
                }
            }
        }
    }
}

/// Scale a row to unit length. A row that is numerically zero to begin with
/// says nothing at first order: it is cleared rather than blown up to unit
/// length, so it counts as dependent.
fn normalise(row: &mut Vec<(usize, f64)>) {
    let n = row.iter().map(|(_, v)| v * v).sum::<f64>().sqrt();
    if n < MIN_ROW_NORM {
        row.clear();
        return;
    }
    for (_, v) in row.iter_mut() {
        *v /= n;
    }
}

/// DOF and dependent constraints of `sk` at its current geometry.
pub fn analyse(sk: &Sketch) -> Freedom {
    let lay = Layout::new(sk);
    let mut work = sk.clone();
    let mut ech = Echelon {
        pivot: vec![None; lay.n],
    };
    let mut rank = 0;
    // Arcs' implicit rows first, so a user constraint is the one named.
    for c in 0..sk.curves.len() {
        if let Some(Curve::Arc { c: cc, a, b }) = sk.curves.get(c).copied() {
            let vs: Vec<usize> = [cc, a, b].iter().flat_map(|p| [2 * p, 2 * p + 1]).collect();
            let f = move |s: &Sketch, out: &mut Vec<f64>| {
                out.push(at(s, a).dist(at(s, cc)) - at(s, b).dist(at(s, cc)))
            };
            for mut r in jacobian_rows(&mut work, &lay, &vs, &f) {
                normalise(&mut r);
                rank += usize::from(ech.insert(r));
            }
        }
    }
    let mut dependent = Vec::new();
    for (i, k) in sk.constraints.iter().enumerate() {
        let vs = vars(sk, &lay, k);
        let k = *k;
        let f = move |s: &Sketch, out: &mut Vec<f64>| rows(s, &k, out);
        let mut dep = false;
        for mut r in jacobian_rows(&mut work, &lay, &vs, &f) {
            normalise(&mut r);
            if ech.insert(r) {
                rank += 1;
            } else {
                dep = true;
            }
        }
        if dep {
            dependent.push(i);
        }
    }
    Freedom {
        dof: lay.n - rank,
        dependent,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use solver_harness::fixtures;

    #[test]
    fn fixture_dof_by_rank() {
        for f in [
            fixtures::filleted_rect(),
            fixtures::filleted_rect_under(),
            fixtures::generated(72),
        ] {
            let a = analyse(&f.exact);
            assert_eq!(a.dof, f.expected_dof, "{}", f.name);
            assert!(a.dependent.is_empty(), "{}: {:?}", f.name, a.dependent);
        }
    }

    #[test]
    fn redundant_constraint_is_named() {
        let f = fixtures::filleted_rect_redundant();
        let a = analyse(&f.exact);
        assert_eq!(a.dof, 0);
        assert_eq!(a.dependent, vec![f.exact.constraints.len() - 1]);
    }
}
