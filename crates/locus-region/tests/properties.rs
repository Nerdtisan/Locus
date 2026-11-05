// SPDX-License-Identifier: AGPL-3.0-or-later

//! Property tests for region detection (#121) on grid-snapped shapes, which
//! make shared edges, T-junctions and tangencies common (ADR 0008).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use locus_geom::{Circle, Line, Vec2};
use locus_region::{Area, BoolOp, Curve, Shape, boolean, detect};
use proptest::prelude::*;
use std::time::Instant;

#[derive(Debug, Clone)]
enum S {
    Rect(f64, f64, f64, f64),
    Circle(f64, f64, f64),
}

impl S {
    fn curves(&self, k: u64) -> Vec<Curve> {
        let l = |i: u64, a: Vec2, b: Vec2| Curve {
            key: k + i,
            shape: Shape::Line(Line::new(a, b)),
        };
        match *self {
            S::Rect(x0, y0, x1, y1) => {
                let (a, b, c, d) = (
                    Vec2::new(x0, y0),
                    Vec2::new(x1, y0),
                    Vec2::new(x1, y1),
                    Vec2::new(x0, y1),
                );
                vec![l(0, a, b), l(1, b, c), l(2, c, d), l(3, d, a)]
            }
            S::Circle(x, y, r) => vec![Curve {
                key: k,
                shape: Shape::Circle(Circle::new(Vec2::new(x, y), r)),
            }],
        }
    }
    fn inside(&self, q: Vec2) -> bool {
        match *self {
            S::Rect(x0, y0, x1, y1) => q.x > x0 && q.x < x1 && q.y > y0 && q.y < y1,
            S::Circle(x, y, r) => q.distance(Vec2::new(x, y)) < r,
        }
    }
    fn near(&self, q: Vec2) -> bool {
        match *self {
            // Conservative: anything near the rectangle's four lines.
            S::Rect(x0, y0, x1, y1) => [q.x - x0, q.x - x1, q.y - y0, q.y - y1]
                .iter()
                .any(|d| d.abs() < 1e-3),
            S::Circle(x, y, r) => (q.distance(Vec2::new(x, y)) - r).abs() < 1e-3,
        }
    }
}

fn shape() -> impl Strategy<Value = S> {
    let g = |lo: i32, hi: i32| (lo..=hi).prop_map(|v| f64::from(v) * 0.5);
    prop_oneof![
        (g(0, 16), g(0, 16), g(1, 10), g(1, 10)).prop_map(|(x, y, w, h)| S::Rect(
            x,
            y,
            x + w,
            y + h
        )),
        (g(0, 20), g(0, 20), g(1, 8)).prop_map(|(x, y, r)| S::Circle(x, y, r)),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(300))]

    /// Faces partition the covered area: a probe off every curve is in at
    /// most one region, and in exactly one when some shape covers it. (An
    /// uncovered probe may still sit in a void the shapes enclose, which is
    /// a face too.)
    #[test]
    fn faces_partition_the_covered_area(shapes in prop::collection::vec(shape(), 1..6), probes in prop::collection::vec((0.0f64..12.0, 0.0f64..12.0), 40)) {
        let curves: Vec<Curve> = shapes.iter().enumerate().flat_map(|(i, s)| s.curves(10 * i as u64)).collect();
        let regions = detect(&curves);
        for (x, y) in probes {
            let q = Vec2::new(x, y);
            if shapes.iter().any(|s| s.near(q)) {
                continue;
            }
            let covered = shapes.iter().any(|s| s.inside(q));
            let n = regions.iter().filter(|r| r.contains(q)).count();
            prop_assert!(n <= 1, "probe {:?} in {} regions", q, n);
            if covered {
                prop_assert_eq!(n, 1, "probe {:?}", q);
            }
        }
        for r in &regions {
            prop_assert!(r.area() > 0.0 && r.outer.area() > 0.0);
            prop_assert!(r.holes.iter().all(|h| h.area() < 0.0));
        }
    }
}

/// An operand as a filled area: the single region of one shape.
fn area(s: &S) -> Area {
    let rs = detect(&s.curves(0));
    assert_eq!(rs.len(), 1);
    rs[0].to_area()
}

fn op() -> impl Strategy<Value = BoolOp> {
    prop_oneof![
        Just(BoolOp::Union),
        Just(BoolOp::Subtract),
        Just(BoolOp::Intersect)
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(500))]

    /// ADR 0008 sweep as a property: off the curves, the result contains a
    /// probe exactly when the operation of the operands does; loops close,
    /// outer loops are CCW and no sliver survives.
    #[test]
    fn boolean_matches_pointwise_truth(a in shape(), b in shape(), op in op(), shift in prop_oneof![Just(0.0), -1e-7f64..1e-7], probes in prop::collection::vec((-1.0f64..14.0, -1.0f64..14.0), 60)) {
        let b = match b {
            S::Rect(x0, y0, x1, y1) => S::Rect(x0 + shift, y0, x1 + shift, y1),
            S::Circle(x, y, r) => S::Circle(x + shift, y, r),
        };
        let (aa, ba) = (area(&a), area(&b));
        let r = boolean(&aa, &ba, op);
        for (x, y) in probes {
            let q = Vec2::new(x, y);
            if a.near(q) || b.near(q) {
                continue;
            }
            let want = match op {
                BoolOp::Union => a.inside(q) || b.inside(q),
                BoolOp::Subtract => a.inside(q) && !b.inside(q),
                BoolOp::Intersect => a.inside(q) && b.inside(q),
            };
            prop_assert_eq!(r.contains(q), want, "probe {:?}", q);
        }
        for l in &r.loops {
            let n = l.edges.len();
            prop_assert!((0..n).all(|i| l.edges[i].seg.end().distance(l.edges[(i + 1) % n].seg.start()) <= 1e-6));
            prop_assert!(l.area().abs() > 1e-9);
        }
    }
}

/// Recomputed after every solve (§6.3): 500 entities must stay well inside
/// a frame. Generous bound so slow CI runners don't flake.
#[test]
fn five_hundred_entities_detect_quickly() {
    let mut curves = Vec::new();
    for i in 0..10 {
        for j in 0..10 {
            let (x, y) = (f64::from(i) * 6.0, f64::from(j) * 6.0);
            curves.extend(S::Rect(x, y, x + 5.0, y + 5.0).curves(curves.len() as u64));
            curves.extend(S::Circle(x + 5.0, y + 5.0, 1.5).curves(curves.len() as u64));
        }
    }
    assert_eq!(curves.len(), 500);
    let t = Instant::now();
    let rs = detect(&curves);
    let ms = t.elapsed().as_secs_f64() * 1e3;
    eprintln!("detect, 500 entities: {} regions in {ms:.2} ms", rs.len());
    assert!(rs.len() >= 300);
    assert!(ms < 250.0, "{ms} ms");
}
