// SPDX-License-Identifier: AGPL-3.0-or-later

//! Line, circle and arc primitives (SPEC §4, Entities v1): evaluation, bounding
//! box, length, closest point, tangent and intersections.
//!
//! Degenerate shapes (zero-length line, zero-radius circle/arc) never panic and
//! never produce NaN: queries fall back to the single point they collapse to,
//! tangents are `None`, and intersections return nothing for them.

use crate::bbox::Aabb;
use crate::tolerance::SOLVER_TOL;
use crate::vec2::Vec2;
use core::f64::consts::TAU;
use serde::{Deserialize, Serialize};

/// Line segment from `a` to `b`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Line {
    pub a: Vec2,
    pub b: Vec2,
}

/// Circle. A radius at or below [`SOLVER_TOL`] is degenerate (a point).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Circle {
    pub center: Vec2,
    pub radius: f64,
}

/// Arc by centre / start / end (SPEC §4), counter-clockwise from `start` to
/// `end`. The radius is `|start − center|`; `end` only fixes the end angle (it
/// need not be exactly on the circle). Equal start and end angles give a
/// zero-sweep (degenerate) arc, not a full circle: use [`Circle`] for that.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Arc {
    pub center: Vec2,
    pub start: Vec2,
    pub end: Vec2,
}

impl Line {
    #[must_use]
    pub const fn new(a: Vec2, b: Vec2) -> Self {
        Self { a, b }
    }

    #[must_use]
    pub fn length(&self) -> f64 {
        self.a.distance(self.b)
    }

    #[must_use]
    pub fn is_degenerate(&self) -> bool {
        self.length() <= SOLVER_TOL
    }

    /// Point at parameter `t` (0 = `a`, 1 = `b`); not clamped.
    #[must_use]
    pub fn eval(&self, t: f64) -> Vec2 {
        self.a.lerp(self.b, t)
    }

    #[must_use]
    pub fn bbox(&self) -> Aabb {
        Aabb::from_points(self.a, self.b)
    }

    /// Unit direction a→b, `None` when degenerate.
    #[must_use]
    pub fn tangent(&self) -> Option<Vec2> {
        (self.b - self.a).normalized(SOLVER_TOL)
    }

    /// Parameter of the closest point on the segment, clamped to [0, 1].
    #[must_use]
    pub fn closest_t(&self, p: Vec2) -> f64 {
        let d = self.b - self.a;
        let l2 = d.length_sq();
        if l2 <= SOLVER_TOL * SOLVER_TOL {
            return 0.0;
        }
        ((p - self.a).dot(d) / l2).clamp(0.0, 1.0)
    }

    #[must_use]
    pub fn closest_point(&self, p: Vec2) -> Vec2 {
        self.eval(self.closest_t(p))
    }

    #[must_use]
    pub fn distance_to(&self, p: Vec2) -> f64 {
        p.distance(self.closest_point(p))
    }

    /// Segment–segment intersection. `None` when they miss, are parallel or
    /// collinear (an overlap has no single point), or either is degenerate.
    #[must_use]
    pub fn intersect_line(&self, o: &Line) -> Option<Vec2> {
        let (r, s) = (self.b - self.a, o.b - o.a);
        let denom = r.cross(s);
        let (lr, ls) = (r.length(), s.length());
        if lr <= SOLVER_TOL || ls <= SOLVER_TOL || denom.abs() <= SOLVER_TOL * lr * ls {
            return None;
        }
        let qp = o.a - self.a;
        let t = qp.cross(s) / denom;
        let u = qp.cross(r) / denom;
        let (et, eu) = (SOLVER_TOL / lr, SOLVER_TOL / ls);
        ((-et..=1.0 + et).contains(&t) && (-eu..=1.0 + eu).contains(&u)).then(|| self.eval(t))
    }

    /// Intersections of the segment with a circle (0, 1 for tangent, or 2).
    #[must_use]
    pub fn intersect_circle(&self, c: &Circle) -> Vec<Vec2> {
        line_circle_points(self, c.center, c.radius)
    }

    /// Intersections of the segment with an arc.
    #[must_use]
    pub fn intersect_arc(&self, arc: &Arc) -> Vec<Vec2> {
        let Some(r) = arc.radius_checked() else {
            return Vec::new();
        };
        line_circle_points(self, arc.center, r)
            .into_iter()
            .filter(|p| arc.contains_point_angle(*p))
            .collect()
    }
}

impl Circle {
    #[must_use]
    pub const fn new(center: Vec2, radius: f64) -> Self {
        Self { center, radius }
    }

    #[must_use]
    pub fn is_degenerate(&self) -> bool {
        !usable_radius(self.radius)
    }

    /// Point at `angle` radians CCW from +x.
    #[must_use]
    pub fn eval(&self, angle: f64) -> Vec2 {
        self.center + Vec2::from_angle(angle) * self.radius
    }

    #[must_use]
    pub fn length(&self) -> f64 {
        TAU * self.radius.max(0.0)
    }

    #[must_use]
    pub fn bbox(&self) -> Aabb {
        let r = self.radius.max(0.0);
        Aabb {
            min: self.center - Vec2::new(r, r),
            max: self.center + Vec2::new(r, r),
        }
    }

    /// Unit CCW tangent at `angle`, `None` when degenerate.
    #[must_use]
    pub fn tangent(&self, angle: f64) -> Option<Vec2> {
        (!self.is_degenerate()).then(|| Vec2::from_angle(angle).perp())
    }

    /// Closest point on the circle. At the centre every point is equally close;
    /// the +x point is returned. A degenerate circle returns its centre.
    #[must_use]
    pub fn closest_point(&self, p: Vec2) -> Vec2 {
        if self.is_degenerate() {
            return self.center;
        }
        match (p - self.center).normalized(f64::MIN_POSITIVE) {
            Some(u) => self.center + u * self.radius,
            None => self.center + Vec2::new(self.radius, 0.0),
        }
    }

    /// Intersections with another circle (0, 1 for tangent, or 2).
    #[must_use]
    pub fn intersect_circle(&self, o: &Circle) -> Vec<Vec2> {
        circle_circle_points(self.center, self.radius, o.center, o.radius)
    }

    /// Intersections with an arc.
    #[must_use]
    pub fn intersect_arc(&self, arc: &Arc) -> Vec<Vec2> {
        let Some(r) = arc.radius_checked() else {
            return Vec::new();
        };
        circle_circle_points(self.center, self.radius, arc.center, r)
            .into_iter()
            .filter(|p| arc.contains_point_angle(*p))
            .collect()
    }

    /// Intersections with a line segment.
    #[must_use]
    pub fn intersect_line(&self, l: &Line) -> Vec<Vec2> {
        l.intersect_circle(self)
    }
}

impl Arc {
    #[must_use]
    pub const fn new(center: Vec2, start: Vec2, end: Vec2) -> Self {
        Self { center, start, end }
    }

    #[must_use]
    pub fn radius(&self) -> f64 {
        self.center.distance(self.start)
    }

    /// Radius, or `None` when the arc is degenerate (zero radius).
    fn radius_checked(&self) -> Option<f64> {
        let r = self.radius();
        usable_radius(r).then_some(r)
    }

    #[must_use]
    pub fn start_angle(&self) -> f64 {
        (self.start - self.center).angle()
    }

    #[must_use]
    pub fn end_angle(&self) -> f64 {
        (self.end - self.center).angle()
    }

    /// CCW sweep in radians, in [0, 2π).
    #[must_use]
    pub fn sweep(&self) -> f64 {
        let s = (self.end_angle() - self.start_angle()).rem_euclid(TAU);
        // rem_euclid can return exactly TAU for tiny negative inputs.
        if s >= TAU { 0.0 } else { s }
    }

    /// Zero radius or zero sweep.
    #[must_use]
    pub fn is_degenerate(&self) -> bool {
        self.radius_checked().is_none()
            || self.sweep() <= SOLVER_TOL / self.radius().max(SOLVER_TOL)
    }

    /// Point at parameter `t` (0 = start, 1 = end) on the arc's circle.
    #[must_use]
    pub fn eval(&self, t: f64) -> Vec2 {
        self.center + Vec2::from_angle(self.start_angle() + self.sweep() * t) * self.radius()
    }

    /// Arc length.
    #[must_use]
    pub fn length(&self) -> f64 {
        self.radius() * self.sweep()
    }

    /// True when the direction from the centre to `p` lies within the sweep
    /// (angular tolerance of [`SOLVER_TOL`] / radius at each end).
    #[must_use]
    pub fn contains_point_angle(&self, p: Vec2) -> bool {
        let Some(r) = self.radius_checked() else {
            return false;
        };
        let tol = SOLVER_TOL / r;
        let rel = ((p - self.center).angle() - self.start_angle()).rem_euclid(TAU);
        let sweep = self.sweep();
        rel <= sweep + tol || rel >= TAU - tol
    }

    #[must_use]
    pub fn bbox(&self) -> Aabb {
        let end = self.center
            + (self.end - self.center)
                .normalized(f64::MIN_POSITIVE)
                .map_or(Vec2::ZERO, |u| u * self.radius());
        let mut b = Aabb::from_points(self.start, end);
        let r = self.radius();
        // Cardinal extremes of the circle that the sweep passes through.
        for dir in [
            Vec2::new(r, 0.0),
            Vec2::new(0.0, r),
            Vec2::new(-r, 0.0),
            Vec2::new(0.0, -r),
        ] {
            let p = self.center + dir;
            if self.contains_point_angle(p) {
                b = b.include(p);
            }
        }
        b
    }

    /// Unit CCW tangent at parameter `t`, `None` when degenerate.
    #[must_use]
    pub fn tangent(&self, t: f64) -> Option<Vec2> {
        self.radius_checked()?;
        Some(Vec2::from_angle(self.start_angle() + self.sweep() * t).perp())
    }

    /// Closest point on the arc: the radial projection when it falls within the
    /// sweep, otherwise the nearer endpoint (on the arc's circle).
    #[must_use]
    pub fn closest_point(&self, p: Vec2) -> Vec2 {
        let Some(r) = self.radius_checked() else {
            return self.center;
        };
        let end = self.eval(1.0);
        if let Some(u) = (p - self.center).normalized(f64::MIN_POSITIVE) {
            let q = self.center + u * r;
            if self.contains_point_angle(q) {
                return q;
            }
        }
        let start = self.eval(0.0);
        if p.distance(start) <= p.distance(end) {
            start
        } else {
            end
        }
    }

    /// Intersections with a line segment.
    #[must_use]
    pub fn intersect_line(&self, l: &Line) -> Vec<Vec2> {
        l.intersect_arc(self)
    }

    /// Intersections with a circle.
    #[must_use]
    pub fn intersect_circle(&self, c: &Circle) -> Vec<Vec2> {
        c.intersect_arc(self)
    }

    /// Intersections with another arc.
    #[must_use]
    pub fn intersect_arc(&self, o: &Arc) -> Vec<Vec2> {
        let (Some(r0), Some(r1)) = (self.radius_checked(), o.radius_checked()) else {
            return Vec::new();
        };
        circle_circle_points(self.center, r0, o.center, r1)
            .into_iter()
            .filter(|p| self.contains_point_angle(*p) && o.contains_point_angle(*p))
            .collect()
    }
}

/// A radius is usable when finite and above the solver tolerance (NaN-safe).
fn usable_radius(r: f64) -> bool {
    r.is_finite() && r > SOLVER_TOL
}

/// Segment `l` against the circle (`c`, `r`): 0, 1 (tangent) or 2 points.
fn line_circle_points(l: &Line, c: Vec2, r: f64) -> Vec<Vec2> {
    let d = l.b - l.a;
    let len = d.length();
    if len <= SOLVER_TOL || !usable_radius(r) {
        return Vec::new();
    }
    let u = d / len;
    let t0 = (c - l.a).dot(u); // foot of the centre, in length units
    let h = (c - l.a).cross(u).abs(); // centre-to-line distance
    if h > r + SOLVER_TOL {
        return Vec::new();
    }
    let ts: Vec<f64> = if r - h <= SOLVER_TOL {
        vec![t0]
    } else {
        let half = (r * r - h * h).sqrt();
        vec![t0 - half, t0 + half]
    };
    ts.into_iter()
        .filter(|t| (-SOLVER_TOL..=len + SOLVER_TOL).contains(t))
        .map(|t| l.a + u * t.clamp(0.0, len))
        .collect()
}

/// Circle–circle intersection: 0, 1 (tangent) or 2 points. Concentric circles
/// (including identical ones) return none.
fn circle_circle_points(c0: Vec2, r0: f64, c1: Vec2, r1: f64) -> Vec<Vec2> {
    if !usable_radius(r0) || !usable_radius(r1) {
        return Vec::new();
    }
    let d = c0.distance(c1);
    if d <= SOLVER_TOL || d > r0 + r1 + SOLVER_TOL || d < (r0 - r1).abs() - SOLVER_TOL {
        return Vec::new();
    }
    let u = (c1 - c0) / d;
    let a = (d * d + r0 * r0 - r1 * r1) / (2.0 * d);
    let h2 = r0 * r0 - a * a;
    let base = c0 + u * a;
    if h2 <= 0.0 {
        return vec![base];
    }
    let h = h2.sqrt();
    vec![base + u.perp() * h, base - u.perp() * h]
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::f64::consts::{FRAC_PI_2, PI};

    fn v(x: f64, y: f64) -> Vec2 {
        Vec2::new(x, y)
    }

    #[test]
    fn line_basics() {
        let l = Line::new(v(0.0, 0.0), v(4.0, 3.0));
        assert_eq!(l.length(), 5.0);
        assert_eq!(l.eval(0.5), v(2.0, 1.5));
        assert!(l.tangent().expect("tangent").approx_eq(v(0.8, 0.6), 1e-12));
        assert_eq!(l.closest_point(v(-10.0, -10.0)), v(0.0, 0.0));
        assert_eq!(l.closest_point(v(10.0, 10.0)), v(4.0, 3.0));
        assert!(l.closest_point(v(0.0, 5.0)).approx_eq(v(2.4, 1.8), 1e-12));
        assert_eq!(l.bbox().max, v(4.0, 3.0));
    }

    #[test]
    fn line_degenerate() {
        let l = Line::new(v(1.0, 1.0), v(1.0, 1.0));
        assert!(l.is_degenerate());
        assert!(l.tangent().is_none());
        assert_eq!(l.closest_point(v(5.0, 5.0)), v(1.0, 1.0));
        assert!(
            l.intersect_line(&Line::new(v(0.0, 0.0), v(2.0, 2.0)))
                .is_none()
        );
        assert!(
            l.intersect_circle(&Circle::new(v(1.0, 1.0), 1.0))
                .is_empty()
        );
    }

    #[test]
    fn line_line() {
        let a = Line::new(v(0.0, 0.0), v(2.0, 2.0));
        let b = Line::new(v(0.0, 2.0), v(2.0, 0.0));
        assert!(
            a.intersect_line(&b)
                .expect("x")
                .approx_eq(v(1.0, 1.0), 1e-12)
        );
        // Touching at an endpoint counts.
        let c = Line::new(v(2.0, 2.0), v(5.0, 0.0));
        assert!(a.intersect_line(&c).is_some());
        // Parallel, collinear and disjoint do not.
        assert!(
            a.intersect_line(&Line::new(v(0.0, 1.0), v(2.0, 3.0)))
                .is_none()
        );
        assert!(
            a.intersect_line(&Line::new(v(1.0, 1.0), v(3.0, 3.0)))
                .is_none()
        );
        assert!(
            a.intersect_line(&Line::new(v(3.0, 0.0), v(3.0, 5.0)))
                .is_none()
        );
    }

    #[test]
    fn line_circle() {
        let c = Circle::new(v(0.0, 0.0), 5.0);
        let through = Line::new(v(-10.0, 0.0), v(10.0, 0.0));
        assert_eq!(through.intersect_circle(&c).len(), 2);
        let chord_end = Line::new(v(0.0, 0.0), v(10.0, 0.0));
        let pts = chord_end.intersect_circle(&c);
        assert_eq!(pts.len(), 1);
        assert!(pts[0].approx_eq(v(5.0, 0.0), 1e-9));
        let tangent = Line::new(v(-10.0, 5.0), v(10.0, 5.0));
        assert_eq!(tangent.intersect_circle(&c).len(), 1);
        let miss = Line::new(v(-10.0, 6.0), v(10.0, 6.0));
        assert!(miss.intersect_circle(&c).is_empty());
    }

    #[test]
    fn circle_basics() {
        let c = Circle::new(v(1.0, 1.0), 2.0);
        assert!((c.length() - 4.0 * PI).abs() < 1e-12);
        assert_eq!(c.bbox().min, v(-1.0, -1.0));
        assert!(c.closest_point(v(10.0, 1.0)).approx_eq(v(3.0, 1.0), 1e-12));
        assert!(c.closest_point(v(1.0, 1.0)).approx_eq(v(3.0, 1.0), 1e-12));
        assert!(c.tangent(0.0).expect("t").approx_eq(v(0.0, 1.0), 1e-12));
        let z = Circle::new(v(1.0, 1.0), 0.0);
        assert!(z.is_degenerate());
        assert!(z.tangent(0.0).is_none());
        assert_eq!(z.closest_point(v(9.0, 9.0)), v(1.0, 1.0));
    }

    #[test]
    fn circle_circle() {
        let a = Circle::new(v(0.0, 0.0), 5.0);
        assert_eq!(a.intersect_circle(&Circle::new(v(6.0, 0.0), 5.0)).len(), 2);
        let t = a.intersect_circle(&Circle::new(v(10.0, 0.0), 5.0));
        assert_eq!(t.len(), 1);
        assert!(t[0].approx_eq(v(5.0, 0.0), 1e-6));
        // Internally tangent.
        assert_eq!(a.intersect_circle(&Circle::new(v(2.0, 0.0), 3.0)).len(), 1);
        // Disjoint, nested, concentric, degenerate.
        assert!(
            a.intersect_circle(&Circle::new(v(20.0, 0.0), 5.0))
                .is_empty()
        );
        assert!(
            a.intersect_circle(&Circle::new(v(1.0, 0.0), 1.0))
                .is_empty()
        );
        assert!(
            a.intersect_circle(&Circle::new(v(0.0, 0.0), 5.0))
                .is_empty()
        );
        assert!(
            a.intersect_circle(&Circle::new(v(3.0, 0.0), 0.0))
                .is_empty()
        );
    }

    fn quarter() -> Arc {
        // Quarter arc of radius 2 about the origin, +x to +y.
        Arc::new(v(0.0, 0.0), v(2.0, 0.0), v(0.0, 5.0))
    }

    #[test]
    fn arc_basics() {
        let a = quarter();
        assert_eq!(a.radius(), 2.0);
        assert!((a.sweep() - FRAC_PI_2).abs() < 1e-12);
        assert!((a.length() - PI).abs() < 1e-12);
        assert!(a.eval(0.0).approx_eq(v(2.0, 0.0), 1e-12));
        assert!(a.eval(1.0).approx_eq(v(0.0, 2.0), 1e-12));
        assert!(a.tangent(0.0).expect("t").approx_eq(v(0.0, 1.0), 1e-12));
        let b = a.bbox();
        assert!(b.min.approx_eq(v(0.0, 0.0), 1e-12));
        assert!(b.max.approx_eq(v(2.0, 2.0), 1e-12));
        // Three-quarter arc includes the -x/-y extremes it passes.
        let big = Arc::new(v(0.0, 0.0), v(2.0, 0.0), v(0.0, -2.0));
        let b = big.bbox();
        assert!(b.min.approx_eq(v(-2.0, -2.0), 1e-12));
        assert!(b.max.approx_eq(v(2.0, 2.0), 1e-12));
    }

    #[test]
    fn arc_closest_point() {
        let a = quarter();
        assert!(
            a.closest_point(v(10.0, 10.0))
                .approx_eq(v(2.0f64.sqrt(), 2.0f64.sqrt()), 1e-12)
        );
        // Outside the sweep: nearest endpoint.
        assert!(a.closest_point(v(0.0, -10.0)).approx_eq(v(2.0, 0.0), 1e-12));
        assert!(a.closest_point(v(-10.0, 1.0)).approx_eq(v(0.0, 2.0), 1e-12));
    }

    #[test]
    fn arc_degenerate() {
        let zr = Arc::new(v(1.0, 1.0), v(1.0, 1.0), v(2.0, 1.0));
        assert!(zr.is_degenerate());
        assert!(zr.tangent(0.5).is_none());
        assert_eq!(zr.closest_point(v(5.0, 5.0)), v(1.0, 1.0));
        assert!(
            zr.intersect_circle(&Circle::new(v(1.0, 1.0), 1.0))
                .is_empty()
        );
        let zs = Arc::new(v(0.0, 0.0), v(2.0, 0.0), v(4.0, 0.0));
        assert!(zs.is_degenerate());
        assert_eq!(zs.length(), 0.0);
    }

    #[test]
    fn arc_intersections() {
        let a = quarter();
        // Line through the arc's midpoint region.
        let l = Line::new(v(0.0, 0.0), v(5.0, 5.0));
        let p = a.intersect_line(&l);
        assert_eq!(p.len(), 1);
        assert!(p[0].approx_eq(v(2.0f64.sqrt(), 2.0f64.sqrt()), 1e-9));
        // Line hitting the circle on the far side, outside the sweep.
        assert!(
            a.intersect_line(&Line::new(v(0.0, 0.0), v(-5.0, -5.0)))
                .is_empty()
        );
        // Circle crossing the arc once inside the sweep.
        let c = Circle::new(v(2.0, 2.0), 2.0);
        let q = a.intersect_circle(&c);
        assert_eq!(q.len(), 2);
        // Arc–arc.
        let b = Arc::new(v(2.0, 2.0), v(4.0, 2.0), v(2.0, 0.0));
        let r = a.intersect_arc(&b);
        assert!(
            r.iter()
                .all(|p| a.contains_point_angle(*p) && b.contains_point_angle(*p))
        );
    }
}
