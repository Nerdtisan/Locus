// SPDX-License-Identifier: AGPL-3.0-or-later

//! Duplicate and overlapping segment detection (SPEC §8 Validation: double cuts).

use crate::pipeline::Shape;
use locus_geom::{Arc, Circle, EXPORT_TOL, Line, Vec2};
use std::f64::consts::TAU;

/// How shape `b` lies relative to shape `a` when both trace the same curve.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Overlap {
    /// Share no more than a point.
    None,
    /// `a` already covers all of `b` (also identical shapes).
    AContainsB,
    /// `b` covers all of `a`.
    BContainsA,
    /// Partial overlap; the single shape that covers both.
    Partial(Shape),
}

/// Compare two shapes. Only same-curve pairs can overlap: collinear lines,
/// concentric equal-radius arcs/circles.
#[must_use]
pub fn overlap(a: &Shape, b: &Shape) -> Overlap {
    match (a, b) {
        (Shape::Line(a), Shape::Line(b)) => line_line(a, b),
        (Shape::Arc(a), Shape::Arc(b)) => arc_arc(a, b),
        (Shape::Circle(a), Shape::Circle(b)) => {
            if same_circle(a.center, a.radius, b.center, b.radius) {
                Overlap::AContainsB
            } else {
                Overlap::None
            }
        }
        (Shape::Circle(c), Shape::Arc(r)) => {
            if same_circle(c.center, c.radius, r.center, r.radius()) {
                Overlap::AContainsB
            } else {
                Overlap::None
            }
        }
        (Shape::Arc(r), Shape::Circle(c)) => {
            if same_circle(c.center, c.radius, r.center, r.radius()) {
                Overlap::BContainsA
            } else {
                Overlap::None
            }
        }
        _ => Overlap::None,
    }
}

fn same_circle(c0: Vec2, r0: f64, c1: Vec2, r1: f64) -> bool {
    c0.distance(c1) <= EXPORT_TOL && (r0 - r1).abs() <= EXPORT_TOL
}

fn line_line(a: &Line, b: &Line) -> Overlap {
    let d = a.b - a.a;
    let len = d.length();
    let Some(u) = d.normalized(EXPORT_TOL) else {
        return Overlap::None;
    };
    let off = |p: Vec2| (p - a.a).cross(u).abs();
    if off(b.a) > EXPORT_TOL || off(b.b) > EXPORT_TOL {
        return Overlap::None;
    }
    let (t0, t1) = ((b.a - a.a).dot(u), (b.b - a.a).dot(u));
    let (lo, hi) = (t0.min(t1), t0.max(t1));
    if hi.min(len) - lo.max(0.0) <= EXPORT_TOL {
        return Overlap::None;
    }
    if lo >= -EXPORT_TOL && hi <= len + EXPORT_TOL {
        Overlap::AContainsB
    } else if lo <= EXPORT_TOL && hi >= len - EXPORT_TOL {
        Overlap::BContainsA
    } else {
        Overlap::Partial(Shape::Line(Line::new(
            a.a + u * lo.min(0.0),
            a.a + u * hi.max(len),
        )))
    }
}

fn arc_arc(a: &Arc, b: &Arc) -> Overlap {
    let r = a.radius();
    if !same_circle(a.center, r, b.center, b.radius()) {
        return Overlap::None;
    }
    let eps = EXPORT_TOL / r.max(EXPORT_TOL);
    let wa = a.sweep();
    let rel = (b.start_angle() - a.start_angle()).rem_euclid(TAU);
    let wb = b.sweep();
    // `b` as intervals relative to a's start, on the two nearest turns.
    let spans = [(rel, rel + wb), (rel - TAU, rel - TAU + wb)];
    let ov = |(s, e): (f64, f64)| (e.min(wa) - s.max(0.0)).max(0.0);
    let hits: Vec<(f64, f64)> = spans.iter().copied().filter(|&s| ov(s) > eps).collect();
    match hits.as_slice() {
        [] => Overlap::None,
        [(s, e)] => {
            if *s >= -eps && *e <= wa + eps {
                Overlap::AContainsB
            } else if *s <= eps && *e >= wa - eps {
                Overlap::BContainsA
            } else {
                let (lo, hi) = (s.min(0.0), e.max(wa));
                if hi - lo >= TAU - eps {
                    return Overlap::Partial(Shape::Circle(Circle::new(a.center, r)));
                }
                let at = |t: f64| a.center + Vec2::from_angle(a.start_angle() + t) * r;
                Overlap::Partial(Shape::Arc(Arc::new(a.center, at(lo), at(hi))))
            }
        }
        _ => Overlap::Partial(Shape::Circle(Circle::new(a.center, r))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn l(a: (f64, f64), b: (f64, f64)) -> Shape {
        Shape::Line(Line::new(Vec2::new(a.0, a.1), Vec2::new(b.0, b.1)))
    }

    #[test]
    fn identical_and_reversed_lines() {
        assert_eq!(
            overlap(&l((0.0, 0.0), (10.0, 0.0)), &l((10.0, 0.0), (0.0, 0.0))),
            Overlap::AContainsB
        );
    }

    #[test]
    fn contained_partial_and_touching_lines() {
        let a = l((0.0, 0.0), (10.0, 0.0));
        assert_eq!(overlap(&a, &l((2.0, 0.0), (5.0, 0.0))), Overlap::AContainsB);
        assert_eq!(overlap(&l((2.0, 0.0), (5.0, 0.0)), &a), Overlap::BContainsA);
        assert_eq!(
            overlap(&a, &l((5.0, 0.0), (15.0, 0.0))),
            Overlap::Partial(l((0.0, 0.0), (15.0, 0.0)))
        );
        assert_eq!(overlap(&a, &l((10.0, 0.0), (20.0, 0.0))), Overlap::None);
        assert_eq!(overlap(&a, &l((0.0, 1.0), (10.0, 1.0))), Overlap::None);
    }

    fn arc(r: f64, a0: f64, a1: f64) -> Shape {
        Shape::Arc(Arc::new(
            Vec2::ZERO,
            Vec2::from_angle(a0) * r,
            Vec2::from_angle(a1) * r,
        ))
    }

    #[test]
    fn arcs_on_the_same_circle() {
        let a = arc(5.0, 0.0, 2.0);
        assert_eq!(overlap(&a, &arc(5.0, 0.0, 2.0)), Overlap::AContainsB);
        assert_eq!(overlap(&a, &arc(5.0, 0.5, 1.0)), Overlap::AContainsB);
        assert!(matches!(
            overlap(&a, &arc(5.0, 1.0, 3.0)),
            Overlap::Partial(Shape::Arc(_))
        ));
        assert_eq!(overlap(&a, &arc(5.0, 2.5, 3.0)), Overlap::None);
        assert_eq!(overlap(&a, &arc(6.0, 0.0, 2.0)), Overlap::None);
    }

    #[test]
    fn circle_covers_arc_and_duplicate_circles() {
        let c = Shape::Circle(Circle::new(Vec2::ZERO, 5.0));
        assert_eq!(overlap(&c, &arc(5.0, 0.0, 1.0)), Overlap::AContainsB);
        assert_eq!(overlap(&arc(5.0, 0.0, 1.0), &c), Overlap::BContainsA);
        assert_eq!(overlap(&c, &c), Overlap::AContainsB);
    }
}
