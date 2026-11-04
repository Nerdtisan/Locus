// SPDX-License-Identifier: AGPL-3.0-or-later

//! Incremental drag solve (#63): warm-started frames, soft targets out of
//! reach, and the SPEC §3 budget (< 16 ms per frame at 500 entities) as a CI
//! gate.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{check, fixtures};
use locus_geom::Vec2;
use locus_solver::{ConstraintKind, LmSolver, PointId, Sketch, Solver};
use std::time::Instant;

/// SPEC §3: incremental re-solve during a drag in under 16 ms at 500
/// entities. `locus-solver` builds optimised even in the dev profile (root
/// `Cargo.toml`), so this holds in `cargo test` as in a release build; the
/// measured worst frame is ~1.5 ms natively (ADR 0007), so 16 ms leaves
/// room for slow CI runners.
const FRAME_BUDGET_MS: f64 = 16.0;

#[test]
fn drag_frames_stay_within_the_section_3_budget_at_500_entities() {
    let f = fixtures::generated(72);
    assert_eq!(f.exact.curves.len(), 504);
    for d in &f.drags {
        let mut sk = f.exact.clone();
        let mut solver = LmSolver::new();
        let mut worst = 0.0f64;
        // The first frame includes the drag's symbolic analysis.
        for &to in &d.path {
            let t = Instant::now();
            let rep = solver.drag(&mut sk, d.point, to).unwrap();
            worst = worst.max(t.elapsed().as_secs_f64() * 1e3);
            assert!(rep.converged && rep.at_target, "{}: {rep:?}", d.name);
        }
        assert!(check::satisfied(&sk), "{}", d.name);
        println!("{}: worst frame {worst:.2} ms", d.name);
        assert!(
            worst < FRAME_BUDGET_MS,
            "{}: worst frame {worst:.1} ms",
            d.name
        );
    }
}

/// A point on a fixed circle, dragged outside it.
fn on_circle() -> (Sketch, PointId) {
    let mut sk = Sketch::default();
    let c = sk.point(0.0, 0.0);
    let circle = sk.circle(c, 5.0);
    let p = sk.point(0.0, 5.0);
    sk.constrain(ConstraintKind::Fix {
        p: c,
        at: Vec2::ZERO,
    });
    sk.constrain(ConstraintKind::Radius { c: circle, r: 5.0 });
    sk.constrain(ConstraintKind::PointOnCurve(p, circle));
    (sk, p)
}

#[test]
fn unreachable_target_pulls_the_point_as_near_as_it_can_go() {
    let (mut sk, p) = on_circle();
    let mut solver = LmSolver::new();
    let rep = solver.drag(&mut sk, p, Vec2::new(20.0, 0.0)).unwrap();
    assert!(rep.converged, "{rep:?}");
    assert!(!rep.at_target);
    assert!(check::satisfied(&sk));
    // The nearest point of the circle to (20, 0).
    let at = sk.point_at(p).unwrap();
    assert!(at.distance(Vec2::new(5.0, 0.0)) < 1e-3, "{at:?}");
    // And back within reach: the hard target holds again.
    let rep = solver.drag(&mut sk, p, Vec2::new(0.0, -5.0)).unwrap();
    assert!(rep.converged && rep.at_target, "{rep:?}");
}

#[test]
fn dragging_a_fixed_point_leaves_the_sketch_alone() {
    let f = fixtures::filleted_rect();
    let mut sk = f.exact.clone();
    let rep = LmSolver::new()
        .drag(&mut sk, PointId(0), Vec2::new(30.0, 30.0))
        .unwrap();
    assert!(rep.converged && !rep.at_target, "{rep:?}");
    for (a, b) in sk.points.iter().zip(&f.exact.points) {
        assert!(a.at.distance(b.at) < 1e-6, "{a:?} moved from {b:?}");
    }
}
