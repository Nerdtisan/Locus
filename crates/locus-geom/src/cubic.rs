// SPDX-License-Identifier: AGPL-3.0-or-later

//! Cubic Bézier curves (SPEC §4 Entities, cubic Bézier) and their conversion
//! to polylines and to arcs.
//!
//! Formats with true curves (SVG `C`, PDF `c`) take the control points as they
//! are. Everything else — DXF R12, region detection, overlap/intersection
//! checks — works on lines and arcs, so [`Cubic::to_pieces`] approximates the
//! curve by a chain of G1 *biarcs* within a caller-given tolerance (the export
//! tolerance, [`crate::EXPORT_TOL`], for DXF).

use crate::bbox::Aabb;
use crate::primitives::{Arc, Line};
use crate::vec2::Vec2;

/// Recursion limit for adaptive subdivision (2^16 pieces at most).
const MAX_DEPTH: u32 = 16;
/// Samples per candidate piece when measuring the approximation error.
const SAMPLES: usize = 24;

/// A cubic Bézier: end points `p0`, `p1` and control points `c0` (next to
/// `p0`) and `c1` (next to `p1`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cubic {
    pub p0: Vec2,
    pub c0: Vec2,
    pub c1: Vec2,
    pub p1: Vec2,
}

/// One piece of an arc approximation. Arcs are counter-clockwise, as
/// everywhere (SPEC §4); a clockwise piece is stored with start and end
/// swapped, so the pieces are *not* oriented along the curve.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Piece {
    Line(Line),
    Arc(Arc),
}

impl Cubic {
    #[must_use]
    pub const fn new(p0: Vec2, c0: Vec2, c1: Vec2, p1: Vec2) -> Self {
        Self { p0, c0, c1, p1 }
    }

    /// Point at parameter `t` ∈ [0, 1].
    #[must_use]
    pub fn eval(&self, t: f64) -> Vec2 {
        let u = 1.0 - t;
        self.p0 * (u * u * u)
            + self.c0 * (3.0 * u * u * t)
            + self.c1 * (3.0 * u * t * t)
            + self.p1 * (t * t * t)
    }

    /// First derivative at `t`.
    #[must_use]
    pub fn derivative(&self, t: f64) -> Vec2 {
        let u = 1.0 - t;
        (self.c0 - self.p0) * (3.0 * u * u)
            + (self.c1 - self.c0) * (6.0 * u * t)
            + (self.p1 - self.c1) * (3.0 * t * t)
    }

    /// De Casteljau split at `t`.
    #[must_use]
    pub fn split(&self, t: f64) -> (Cubic, Cubic) {
        let a = self.p0.lerp(self.c0, t);
        let b = self.c0.lerp(self.c1, t);
        let c = self.c1.lerp(self.p1, t);
        let d = a.lerp(b, t);
        let e = b.lerp(c, t);
        let m = d.lerp(e, t);
        (Cubic::new(self.p0, a, d, m), Cubic::new(m, e, c, self.p1))
    }

    /// Unit tangent in the direction of travel at the start, falling back to
    /// the next distinct control point when `c0` sits on `p0`.
    #[must_use]
    pub fn start_tangent(&self) -> Option<Vec2> {
        [self.c0, self.c1, self.p1]
            .into_iter()
            .find_map(|q| (q - self.p0).normalized(1e-12))
    }

    /// Unit tangent in the direction of travel at the end.
    #[must_use]
    pub fn end_tangent(&self) -> Option<Vec2> {
        [self.c1, self.c0, self.p0]
            .into_iter()
            .find_map(|q| (self.p1 - q).normalized(1e-12))
    }

    /// Tight bounding box (curve extrema, not the control polygon).
    #[must_use]
    pub fn bbox(&self) -> Aabb {
        let mut b = Aabb::from_points(self.p0, self.p1);
        for t in self.extrema() {
            b = b.include(self.eval(t));
        }
        b
    }

    /// Parameters in (0, 1) where x' or y' vanishes.
    fn extrema(&self) -> Vec<f64> {
        let mut out = Vec::new();
        for axis in 0..2 {
            let g = |p: Vec2| if axis == 0 { p.x } else { p.y };
            let (a, b, c, d) = (g(self.p0), g(self.c0), g(self.c1), g(self.p1));
            // B'(t)/3 = at² + bt + c with:
            let qa = -a + 3.0 * b - 3.0 * c + d;
            let qb = 2.0 * (a - 2.0 * b + c);
            let qc = b - a;
            if qa.abs() < 1e-14 {
                if qb.abs() > 1e-14 {
                    out.push(-qc / qb);
                }
            } else {
                let disc = qb * qb - 4.0 * qa * qc;
                if disc >= 0.0 {
                    let s = disc.sqrt();
                    out.push((-qb + s) / (2.0 * qa));
                    out.push((-qb - s) / (2.0 * qa));
                }
            }
        }
        out.retain(|t| *t > 0.0 && *t < 1.0);
        out
    }

    /// Arc length, from a fine flattening (error far below the export
    /// tolerance).
    #[must_use]
    pub fn length(&self) -> f64 {
        self.flatten(1e-4)
            .windows(2)
            .map(|w| w[0].distance(w[1]))
            .sum()
    }

    /// Polyline within `tol` (mm) of the curve, including both end points.
    #[must_use]
    pub fn flatten(&self, tol: f64) -> Vec<Vec2> {
        let mut out = vec![self.p0];
        self.flatten_into(tol.max(1e-9), 0, &mut out);
        out
    }

    fn flatten_into(&self, tol: f64, depth: u32, out: &mut Vec<Vec2>) {
        // The curve lies within 3/4 of the control points' distance from the
        // chord, so this bound is safe.
        let chord = Line::new(self.p0, self.p1);
        let dev = chord.distance_to(self.c0).max(chord.distance_to(self.c1));
        if depth >= MAX_DEPTH || 0.75 * dev <= tol {
            out.push(self.p1);
            return;
        }
        let (a, b) = self.split(0.5);
        a.flatten_into(tol, depth + 1, out);
        b.flatten_into(tol, depth + 1, out);
    }

    /// Closest point on the curve to `p`, to within the flattening tolerance
    /// (hit-testing and snapping).
    #[must_use]
    pub fn closest_point(&self, p: Vec2) -> Vec2 {
        self.flatten(1e-3)
            .windows(2)
            .map(|w| Line::new(w[0], w[1]).closest_point(p))
            .min_by(|a, b| a.distance(p).total_cmp(&b.distance(p)))
            .unwrap_or(self.p0)
    }

    /// Distance from `p` to the curve, to within the flattening tolerance
    /// (hit-testing).
    #[must_use]
    pub fn distance_to(&self, p: Vec2) -> f64 {
        polyline_distance(&self.flatten(1e-3), p)
    }

    /// The curve as lines and arcs, every point of the curve within `tol` of
    /// the chain, consecutive pieces sharing their end points exactly (the
    /// first piece starts at `p0`, the last ends at `p1`). Degenerate
    /// (zero-length) curves give no pieces.
    #[must_use]
    pub fn to_pieces(&self, tol: f64) -> Vec<Piece> {
        let mut out = Vec::new();
        // Half the budget: the error is measured at samples, not everywhere.
        self.pieces_into(tol.max(1e-9) * 0.5, 0, &mut out);
        out
    }

    fn pieces_into(&self, tol: f64, depth: u32, out: &mut Vec<Piece>) {
        let straight = self.is_straight(tol);
        if straight {
            if self.p0 != self.p1 {
                out.push(Piece::Line(Line::new(self.p0, self.p1)));
            }
            return;
        }
        if let Some(pieces) = self.biarc()
            && self.max_error(&pieces) <= tol
        {
            out.extend(pieces);
            return;
        }
        if depth >= MAX_DEPTH {
            // Give up on arcs: a chord is the best remaining piece.
            out.push(Piece::Line(Line::new(self.p0, self.p1)));
            return;
        }
        let (a, b) = self.split(0.5);
        a.pieces_into(tol, depth + 1, out);
        b.pieces_into(tol, depth + 1, out);
    }

    /// All control points within `tol` of the chord (so the curve is, too).
    fn is_straight(&self, tol: f64) -> bool {
        if self.p0 == self.p1 {
            return self.c0 == self.p0 && self.c1 == self.p0;
        }
        let chord = Line::new(self.p0, self.p1);
        0.75 * chord.distance_to(self.c0).max(chord.distance_to(self.c1)) <= tol
            && (self.c0 - self.p0).dot(self.p1 - self.p0) >= 0.0
            && (self.p1 - self.c1).dot(self.p1 - self.p0) >= 0.0
    }

    /// The G1 biarc through `p0`/`p1` with the curve's end tangents, joined
    /// where the two arcs' tangent-length parameters are equal (Bolton's
    /// construction). `None` when it degenerates.
    fn biarc(&self) -> Option<[Piece; 2]> {
        let (t0, t1) = (self.start_tangent()?, self.end_tangent()?);
        let v = self.p1 - self.p0;
        let t = t0 + t1;
        let (vt, vv, tt) = (v.dot(t), v.dot(v), t.dot(t));
        let k = 4.0 - tt;
        let d = if k.abs() < 1e-9 {
            // Equal tangents: linear in d.
            if vt.abs() < 1e-12 {
                return None;
            }
            vv / (2.0 * vt)
        } else {
            let disc = vt * vt + k * vv;
            if disc < 0.0 {
                return None;
            }
            (-vt + disc.sqrt()) / k
        };
        if !d.is_finite() || d <= 0.0 {
            return None;
        }
        let a = self.p0 + t0 * d;
        let b = self.p1 - t1 * d;
        let j = a.lerp(b, 0.5);
        let first = arc_with_tangent(self.p0, t0, j)?;
        // Run the second arc backwards from p1 so its tangent is the known one.
        let second = arc_with_tangent(self.p1, -t1, j)?;
        let second = match second {
            Piece::Line(l) => Piece::Line(Line::new(l.b, l.a)),
            arc => arc,
        };
        Some([first, second])
    }

    /// Largest distance from sampled curve points to the nearest piece.
    fn max_error(&self, pieces: &[Piece]) -> f64 {
        let mut worst = 0.0f64;
        for i in 0..=SAMPLES {
            let q = self.eval(i as f64 / SAMPLES as f64);
            let d = pieces
                .iter()
                .map(|p| p.distance_to(q))
                .fold(f64::INFINITY, f64::min);
            worst = worst.max(d);
        }
        // The biarc must also not stray: its own ends are checked via the
        // reverse measure on its midpoints.
        let fine = self.flatten(5e-5);
        for p in pieces {
            for k in 1..SAMPLES {
                let s = p.eval(k as f64 / SAMPLES as f64);
                worst = worst.max(polyline_distance(&fine, s));
            }
        }
        worst
    }
}

impl Piece {
    fn distance_to(&self, p: Vec2) -> f64 {
        match self {
            Piece::Line(l) => l.distance_to(p),
            Piece::Arc(a) => p.distance(a.closest_point(p)),
        }
    }

    fn eval(&self, t: f64) -> Vec2 {
        match self {
            Piece::Line(l) => l.eval(t),
            Piece::Arc(a) => a.eval(t),
        }
    }

    /// Start and end as stored (CCW for arcs).
    #[must_use]
    pub fn ends(&self) -> (Vec2, Vec2) {
        match self {
            Piece::Line(l) => (l.a, l.b),
            Piece::Arc(a) => (a.start, a.end),
        }
    }
}

fn polyline_distance(pl: &[Vec2], p: Vec2) -> f64 {
    pl.windows(2)
        .map(|w| Line::new(w[0], w[1]).distance_to(p))
        .fold(f64::INFINITY, f64::min)
}

/// The circular arc from `p` leaving along unit tangent `t` and ending at `q`,
/// as a CCW [`Arc`] (start/end swapped when the sense is clockwise). A line
/// when `q` is (nearly) straight ahead.
fn arc_with_tangent(p: Vec2, t: Vec2, q: Vec2) -> Option<Piece> {
    let w = q - p;
    let n = t.perp();
    let denom = 2.0 * w.dot(n);
    if denom.abs() < 1e-12 * w.length_sq().max(1e-300) {
        return Some(Piece::Line(Line::new(p, q)));
    }
    let r = w.dot(w) / denom;
    if !r.is_finite() {
        return None;
    }
    let centre = p + n * r;
    // r > 0: centre on the left, the sense is CCW.
    Some(Piece::Arc(if r > 0.0 {
        Arc::new(centre, p, q)
    } else {
        Arc::new(centre, q, p)
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: f64, y: f64) -> Vec2 {
        Vec2::new(x, y)
    }

    fn s_curve() -> Cubic {
        Cubic::new(v(0.0, 0.0), v(30.0, 40.0), v(70.0, -40.0), v(100.0, 0.0))
    }

    fn loop_free_arch() -> Cubic {
        Cubic::new(v(0.0, 0.0), v(0.0, 55.0), v(100.0, 55.0), v(100.0, 0.0))
    }

    #[test]
    fn eval_and_split_agree() {
        let c = s_curve();
        assert_eq!(c.eval(0.0), c.p0);
        assert_eq!(c.eval(1.0), c.p1);
        let (a, b) = c.split(0.3);
        assert!(a.eval(1.0).approx_eq(c.eval(0.3), 1e-12));
        assert!(b.eval(0.0).approx_eq(c.eval(0.3), 1e-12));
        assert!(a.eval(0.5).approx_eq(c.eval(0.15), 1e-12));
        assert!(b.eval(0.5).approx_eq(c.eval(0.65), 1e-12));
    }

    #[test]
    fn bbox_is_tight() {
        let c = loop_free_arch();
        let b = c.bbox();
        // Peak of the arch is 0.75 * 55 = 41.25.
        assert!((b.max.y - 41.25).abs() < 1e-9, "{b:?}");
        assert!((b.min.y - 0.0).abs() < 1e-9);
        assert!((b.max.x - 100.0).abs() < 1e-9);
    }

    #[test]
    fn flatten_stays_within_tolerance() {
        let c = s_curve();
        let pl = c.flatten(0.01);
        assert!(pl.len() > 4);
        for i in 0..=200 {
            let q = c.eval(f64::from(i) / 200.0);
            let d = pl
                .windows(2)
                .map(|w| Line::new(w[0], w[1]).distance_to(q))
                .fold(f64::INFINITY, f64::min);
            assert!(d <= 0.01 + 1e-9, "t={i}: {d}");
        }
    }

    /// Export tolerance (SPEC §8): the arc chain follows the curve to 0.001 mm
    /// both ways, ends match exactly, and the chain is connected.
    fn check_pieces(c: &Cubic, tol: f64) -> usize {
        let pieces = c.to_pieces(tol);
        assert!(!pieces.is_empty());
        for i in 0..=2000 {
            let q = c.eval(f64::from(i) / 2000.0);
            let d = pieces
                .iter()
                .map(|p| p.distance_to(q))
                .fold(f64::INFINITY, f64::min);
            assert!(d <= tol, "curve point t={i}/2000 is {d} from the arcs");
        }
        let fine = c.flatten(1e-5);
        for p in &pieces {
            for k in 0..=50 {
                let s = p.eval(f64::from(k) / 50.0);
                assert!(
                    polyline_distance(&fine, s) <= tol + 1e-5,
                    "arc strays from the curve"
                );
            }
        }
        // Connectivity: every piece end meets another piece end (or p0/p1).
        let mut ends: Vec<Vec2> = vec![c.p0, c.p1];
        for p in &pieces {
            let (a, b) = p.ends();
            ends.extend([a, b]);
        }
        for e in &ends {
            let n = ends.iter().filter(|o| o.distance(*e) < 1e-9).count();
            assert!(n >= 2, "dangling piece end at {e:?}");
        }
        pieces.len()
    }

    #[test]
    fn arcs_follow_curves_within_the_export_tolerance() {
        let tol = crate::EXPORT_TOL;
        for c in [
            s_curve(),
            loop_free_arch(),
            // A tight corner-ish curve with short handles.
            Cubic::new(v(0.0, 0.0), v(5.0, 0.0), v(10.0, 5.0), v(10.0, 10.0)),
            // Handles on the endpoints: a straight line in disguise.
            Cubic::new(v(0.0, 0.0), v(0.0, 0.0), v(10.0, 0.0), v(10.0, 0.0)),
            // Near-circular quarter (the standard 0.5523 handle).
            Cubic::new(v(10.0, 0.0), v(10.0, 5.523), v(5.523, 10.0), v(0.0, 10.0)),
        ] {
            check_pieces(&c, tol);
        }
    }

    #[test]
    fn a_quarter_circle_stays_compact() {
        let k = 0.552_284_749_830_793_4;
        let c = Cubic::new(
            v(10.0, 0.0),
            v(10.0, 10.0 * k),
            v(10.0 * k, 10.0),
            v(0.0, 10.0),
        );
        let n = check_pieces(&c, crate::EXPORT_TOL);
        assert!(n <= 16, "{n} pieces for a quarter circle");
    }

    #[test]
    fn straight_cubic_is_one_line() {
        let c = Cubic::new(v(0.0, 0.0), v(3.0, 0.0), v(7.0, 0.0), v(10.0, 0.0));
        let p = c.to_pieces(1e-3);
        assert_eq!(p.len(), 1);
        assert!(matches!(p[0], Piece::Line(_)));
    }

    #[test]
    fn degenerate_curves_do_not_panic() {
        let z = v(1.0, 1.0);
        assert!(Cubic::new(z, z, z, z).to_pieces(1e-3).is_empty());
        // Cusp / loop: anything finite is fine, but it must terminate.
        let loopy = Cubic::new(v(0.0, 0.0), v(100.0, 100.0), v(-100.0, 100.0), v(0.0, 0.0));
        assert!(!loopy.to_pieces(1e-3).is_empty());
        let nan = Cubic::new(v(f64::NAN, 0.0), z, z, z);
        let _ = nan.to_pieces(1e-3);
    }

    #[test]
    fn tangents_fall_back_past_coincident_handles() {
        let c = Cubic::new(v(0.0, 0.0), v(0.0, 0.0), v(10.0, 10.0), v(20.0, 0.0));
        let t = c.start_tangent().unwrap();
        assert!(t.approx_eq(v(1.0, 1.0).normalized(0.0).unwrap(), 1e-12));
        assert!(c.end_tangent().unwrap().x > 0.0);
    }
}
