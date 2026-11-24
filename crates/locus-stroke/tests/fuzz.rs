// SPDX-License-Identifier: AGPL-3.0-or-later

//! Random paths × random styles (ADR 0019 decision 3): expansion never
//! panics, its outline is closed and finite, and its area is bounded by the
//! stroke's reach.

#![allow(clippy::unwrap_used)]

use core::f64::consts::PI;
use locus_geom::Vec2;
use locus_region::Seg;
use locus_stroke::{
    Align, Cap, Chain, Dash, DashAlign, Join, Limits, LineStyle, bounds, expand, mesh,
};
use proptest::prelude::*;

fn seg_strategy() -> impl Strategy<Value = (bool, f64, f64, f64)> {
    (any::<bool>(), 0.2f64..20.0, -PI..PI, -3.0f64..3.0)
}

fn build(parts: &[(bool, f64, f64, f64)], closed: bool) -> Chain {
    let mut p = Vec2::new(0.0, 0.0);
    let mut dir = 0.0f64;
    let mut segs = Vec::new();
    for &(is_arc, len, turn, sweep) in parts {
        dir += turn;
        if is_arc && sweep.abs() > 0.05 {
            let r = len / sweep.abs();
            let n = Vec2::from_angle(dir).perp() * sweep.signum();
            let center = p + n * r;
            let start = (p - center).angle();
            let s = Seg::Arc {
                center,
                radius: r,
                start,
                sweep,
            };
            p = s.end();
            dir += sweep;
            segs.push(s);
        } else {
            let q = p + Vec2::from_angle(dir) * len;
            segs.push(Seg::Line { a: p, b: q });
            p = q;
        }
    }
    if closed && segs.len() > 1 {
        let start = segs[0].start();
        if p.distance(start) > 1e-6 {
            segs.push(Seg::Line { a: p, b: start });
        }
    }
    Chain {
        segs,
        closed: closed && p.distance(Vec2::ZERO) > 1e-6,
    }
}

fn style_strategy() -> impl Strategy<Value = LineStyle> {
    (
        (0u8..3, 0u8..3, 1.0f64..10.0, 0u8..3),
        prop::option::of((0.0f64..4.0, 0.1f64..4.0, -5.0f64..5.0, any::<bool>())),
    )
        .prop_map(|((c, j, limit, a), dash)| LineStyle {
            cap: [Cap::Butt, Cap::Round, Cap::Square][usize::from(c)],
            join: [Join::Round, Join::Miter { limit }, Join::Bevel][usize::from(j)],
            align: [Align::Centre, Align::Inside, Align::Outside][usize::from(a)],
            dash: dash.map(|(d, g, phase, corners)| {
                let align = if corners {
                    DashAlign::Corners
                } else {
                    DashAlign::Exact
                };
                Dash::new(&[d, g], phase, align).unwrap()
            }),
        })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]
    #[test]
    fn expansion_is_total_and_bounded(
        parts in prop::collection::vec(seg_strategy(), 1..6),
        closed in any::<bool>(),
        style in style_strategy(),
        width in 0.05f64..8.0,
    ) {
        let chain = build(&parts, closed);
        let out = expand(&chain, width, &style, &Limits::default()).unwrap();
        let a = out.area();
        prop_assert!(a.is_finite() && a >= -1e-6);
        for l in &out.loops {
            let k = l.edges.len();
            for i in 0..k {
                let (e, f) = (&l.edges[i].seg, &l.edges[(i + 1) % k].seg);
                prop_assert!(e.end().distance(f.start()) <= 1e-5);
            }
        }
        if let Some(bb) = bounds(&chain, width, &style) {
            // The outline lies in its bounds, so its area can't exceed theirs.
            prop_assert!(a <= bb.width() * bb.height() + 1e-6);
            for e in out.edges() {
                for t in [0.0, 0.5, 1.0] {
                    let q = e.seg.at(t);
                    prop_assert!(bb.contains(q, 1e-6), "{q:?} outside {bb:?}");
                }
            }
        }
        let mut tris = Vec::new();
        mesh(&chain, width, &style, 0.01, &Limits::default(), &mut tris).unwrap();
        prop_assert!(tris.iter().flatten().all(|p| p.is_finite()));
    }
}
