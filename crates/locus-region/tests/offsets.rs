// SPDX-License-Identifier: AGPL-3.0-or-later

//! Property tests for offsets (ADR 0010) against an exact oracle: a round
//! offset of A by d is the points in A or within d of ∂A (grow), or in A and
//! farther than |d| from ∂A (shrink). Distances to lines and arcs are exact,
//! so no reference implementation is needed. Shapes sit on a 0.5 mm grid and
//! d on 0.25 steps, so exact touching and collapse are common.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use core::f64::consts::PI;
use locus_geom::{EXPORT_TOL, Vec2};
use locus_region::{Area, BoolOp, Edge, Join, Loop, Seg, boolean, offset};
use proptest::prelude::*;

fn line(a: Vec2, b: Vec2) -> Edge {
    Edge {
        seg: Seg::Line { a, b },
        source: 0,
    }
}

#[derive(Debug, Clone)]
enum S {
    Rect(f64, f64, f64, f64),
    Circle(f64, f64, f64),
    Slot(f64, f64, f64, f64),
}

impl S {
    fn lp(&self) -> Loop {
        let v = Vec2::new;
        match *self {
            S::Rect(x, y, w, h) => {
                let p = [v(x, y), v(x + w, y), v(x + w, y + h), v(x, y + h)];
                Loop {
                    edges: (0..4).map(|i| line(p[i], p[(i + 1) % 4])).collect(),
                }
            }
            S::Circle(x, y, r) => Loop {
                edges: vec![Edge {
                    seg: Seg::Arc {
                        center: v(x, y),
                        radius: r,
                        start: 0.0,
                        sweep: 2.0 * PI,
                    },
                    source: 0,
                }],
            },
            S::Slot(x, y, len, r) => {
                let (c1, c2) = (v(x, y), v(x + len, y));
                let n = v(0.0, r);
                let arc = |c, start| Edge {
                    seg: Seg::Arc {
                        center: c,
                        radius: r,
                        start,
                        sweep: PI,
                    },
                    source: 0,
                };
                Loop {
                    edges: vec![
                        line(c1 - n, c2 - n),
                        arc(c2, -PI / 2.0),
                        line(c2 + n, c1 + n),
                        arc(c1, PI / 2.0),
                    ],
                }
            }
        }
    }
}

fn shape() -> impl Strategy<Value = S> {
    let g = |lo: i32, hi: i32| (lo..=hi).prop_map(|v| f64::from(v) * 0.5);
    prop_oneof![
        (g(0, 16), g(0, 16), g(1, 16), g(1, 16)).prop_map(|(x, y, w, h)| S::Rect(x, y, w, h)),
        (g(0, 20), g(0, 20), g(1, 8)).prop_map(|(x, y, r)| S::Circle(x, y, r)),
        (g(0, 16), g(0, 16), g(1, 16), g(1, 6)).prop_map(|(x, y, l, r)| S::Slot(x, y, l, r)),
    ]
}

fn op() -> impl Strategy<Value = BoolOp> {
    prop_oneof![
        Just(BoolOp::Union),
        Just(BoolOp::Subtract),
        Just(BoolOp::Intersect)
    ]
}

fn dist(a: &Area, q: Vec2) -> f64 {
    a.edges()
        .map(|e| e.seg.distance(q))
        .fold(f64::INFINITY, f64::min)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(400))]

    #[test]
    fn round_offsets_match_the_exact_oracle(
        first in shape(),
        rest in prop::collection::vec((shape(), op()), 0..3),
        q in -12i32..=12,
    ) {
        let mut a = Area { loops: vec![first.lp()] };
        for (s, o) in rest {
            let c = boolean(&a, &Area { loops: vec![s.lp()] }, o);
            if !c.is_empty() {
                a = c;
            }
        }
        let d = if q == 0 { 0.25 } else { f64::from(q) * 0.25 };
        let out = offset(&a, d, Join::Round);
        let w = d.abs();
        for l in &out.loops {
            let n = l.edges.len();
            for i in 0..n {
                prop_assert!(l.edges[i].seg.end().distance(l.edges[(i + 1) % n].seg.start()) < 1e-6, "open loop");
            }
        }
        // Every boundary point is |d| from ∂A.
        for e in out.edges() {
            for k in 0..=16 {
                let p = e.seg.at(f64::from(k) / 16.0);
                prop_assert!((dist(&a, p) - w).abs() <= EXPORT_TOL, "boundary point {p:?} off the offset");
            }
        }
        // Every probe away from the offset curve is classified like the oracle.
        let bb = a.loops.iter().filter_map(Loop::bbox).reduce(locus_geom::Aabb::union).unwrap();
        let pad = w + 0.5;
        for i in 0..40 {
            for j in 0..40 {
                let p = Vec2::new(
                    bb.min.x - pad + (bb.width() + 2.0 * pad) * (f64::from(i) + 0.31) / 40.0,
                    bb.min.y - pad + (bb.height() + 2.0 * pad) * (f64::from(j) + 0.577) / 40.0,
                );
                let dp = dist(&a, p);
                if (dp - w).abs() <= EXPORT_TOL || dp <= EXPORT_TOL {
                    continue;
                }
                let truth = if d > 0.0 { a.contains(p) || dp <= w } else { a.contains(p) && dp > w };
                prop_assert_eq!(out.contains(p), truth, "probe {:?}, d = {}", p, d);
            }
        }
    }
}
