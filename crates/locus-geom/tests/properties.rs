// SPDX-License-Identifier: AGPL-3.0-or-later

//! Property tests for intersections and closest-point (issue #52).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use locus_geom::{Arc, Circle, Line, Vec2};
use proptest::prelude::*;

const EPS: f64 = 1e-5;

fn pt() -> impl Strategy<Value = Vec2> {
    (-100.0..100.0f64, -100.0..100.0f64).prop_map(|(x, y)| Vec2::new(x, y))
}

fn line() -> impl Strategy<Value = Line> {
    (pt(), pt()).prop_map(|(a, b)| Line::new(a, b))
}

fn circle() -> impl Strategy<Value = Circle> {
    (pt(), 0.5..80.0f64).prop_map(|(c, r)| Circle::new(c, r))
}

fn arc() -> impl Strategy<Value = Arc> {
    (pt(), 0.5..80.0f64, -3.0..3.0f64, 0.1..6.0f64).prop_map(|(c, r, a0, sweep)| {
        Arc::new(
            c,
            c + Vec2::from_angle(a0) * r,
            c + Vec2::from_angle(a0 + sweep) * r,
        )
    })
}

proptest! {
    #[test]
    fn line_closest_is_no_farther_than_any_sample(l in line(), p in pt(), t in 0.0..1.0f64) {
        let d = l.distance_to(p);
        prop_assert!(d <= p.distance(l.eval(t)) + 1e-9);
        prop_assert!(d <= p.distance(l.a) + 1e-9 && d <= p.distance(l.b) + 1e-9);
    }

    #[test]
    fn circle_closest_distance_is_radial(c in circle(), p in pt()) {
        let d = p.distance(c.closest_point(p));
        prop_assert!((d - (p.distance(c.center) - c.radius).abs()).abs() < 1e-9);
    }

    #[test]
    fn arc_closest_is_no_farther_than_any_sample(a in arc(), p in pt(), t in 0.0..1.0f64) {
        let d = p.distance(a.closest_point(p));
        prop_assert!(d <= p.distance(a.eval(t)) + EPS);
    }

    #[test]
    fn arc_bbox_contains_samples(a in arc(), t in 0.0..1.0f64) {
        prop_assert!(a.bbox().contains(a.eval(t), EPS));
    }

    #[test]
    fn line_line_point_is_on_both(a in line(), b in line()) {
        if let Some(p) = a.intersect_line(&b) {
            prop_assert!(a.distance_to(p) < EPS);
            prop_assert!(b.distance_to(p) < EPS);
        }
    }

    #[test]
    fn line_circle_points_are_on_both(l in line(), c in circle()) {
        for p in l.intersect_circle(&c) {
            prop_assert!(l.distance_to(p) < EPS);
            prop_assert!((p.distance(c.center) - c.radius).abs() < EPS);
        }
    }

    #[test]
    fn line_through_centre_hits_circle_twice(c in circle(), angle in -3.0..3.0f64) {
        let d = Vec2::from_angle(angle) * (c.radius * 2.0);
        let l = Line::new(c.center - d, c.center + d);
        prop_assert_eq!(l.intersect_circle(&c).len(), 2);
    }

    #[test]
    fn circle_circle_points_are_on_both(a in circle(), b in circle()) {
        for p in a.intersect_circle(&b) {
            prop_assert!((p.distance(a.center) - a.radius).abs() < EPS);
            prop_assert!((p.distance(b.center) - b.radius).abs() < EPS);
        }
    }

    #[test]
    fn arc_intersections_are_on_both(a in arc(), b in arc(), l in line(), c in circle()) {
        for p in a.intersect_arc(&b) {
            prop_assert!(p.distance(a.closest_point(p)) < EPS);
            prop_assert!(p.distance(b.closest_point(p)) < EPS);
        }
        for p in a.intersect_line(&l) {
            prop_assert!(p.distance(a.closest_point(p)) < EPS);
            prop_assert!(l.distance_to(p) < EPS);
        }
        for p in a.intersect_circle(&c) {
            prop_assert!(p.distance(a.closest_point(p)) < EPS);
            prop_assert!((p.distance(c.center) - c.radius).abs() < EPS);
        }
    }

    #[test]
    fn never_nan(l in line(), a in arc(), c in circle(), p in pt()) {
        for q in [l.closest_point(p), a.closest_point(p), c.closest_point(p)] {
            prop_assert!(q.is_finite());
        }
        for q in a.intersect_line(&l).into_iter().chain(c.intersect_line(&l)) {
            prop_assert!(q.is_finite());
        }
    }
}
