// SPDX-License-Identifier: AGPL-3.0-or-later

//! The harness fixtures through the `Solver` trait (#57): the SPEC §13 M0
//! sketch and its variants, the 500-entity generated sketch and its drags,
//! and untrusted-input handling.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{check, fixtures};
use locus_geom::Vec2;
use locus_solver::{
    ConstraintKind, CurveKind, Key, LmSolver, PointId, Sketch, SketchError, Solver,
};

fn solved(f: &fixtures::Fixture) -> Sketch {
    let mut sk = f.start.clone();
    let rep = LmSolver::new().solve(&mut sk).unwrap();
    assert!(rep.converged, "{}: {rep:?}", f.name);
    let (v, at) = check::worst(&sk);
    assert!(v < check::SATISFIED, "{}: worst {v:e} at {at:?}", f.name);
    sk
}

#[test]
fn harness_fixtures_hold_at_their_exact_geometry() {
    for f in [
        fixtures::filleted_rect(),
        fixtures::filleted_rect_redundant(),
        fixtures::generated(72),
    ] {
        let (v, at) = check::worst(&f.exact);
        assert!(v < 1e-9, "{}: {v} at {at:?}", f.name);
        assert!(!check::satisfied(&f.start), "{} starts solved", f.name);
    }
}

#[test]
fn m0_fixture_solves() {
    solved(&fixtures::filleted_rect());
    solved(&fixtures::filleted_rect_under());
}

#[test]
fn redundant_but_consistent_still_solves() {
    solved(&fixtures::filleted_rect_redundant());
}

#[test]
fn conflict_fails_and_names_both_constraints() {
    for f in [
        fixtures::filleted_rect_conflict(),
        fixtures::generated_conflict(72),
    ] {
        let mut sk = f.start.clone();
        let rep = LmSolver::new().solve(&mut sk).unwrap();
        assert!(!rep.converged, "{}", f.name);
        // A failed solve leaves the sketch as it was.
        assert_eq!(sk, f.start, "{}", f.name);
        for c in &f.conflict {
            let key = sk.constraints[c.index()].key;
            assert!(
                rep.unsatisfied.contains(&key),
                "{}: {key:?} not in {:?}",
                f.name,
                rep.unsatisfied
            );
        }
    }
}

#[test]
fn generated_500_entity_sketch_solves() {
    solved(&fixtures::generated(72));
}

#[test]
fn drags_reach_every_target_and_keep_every_constraint() {
    let f = fixtures::generated(72);
    for d in &f.drags {
        let mut sk = f.exact.clone();
        let mut solver = LmSolver::new();
        for (i, &to) in d.path.iter().enumerate() {
            let rep = solver.drag(&mut sk, d.point, to).unwrap();
            assert!(rep.converged, "{} frame {i}: {rep:?}", d.name);
            let (v, at) = check::worst(&sk);
            assert!(
                v < check::SATISFIED,
                "{} frame {i}: {v:e} at {at:?}",
                d.name
            );
            assert!(sk.point_at(d.point).unwrap().distance(to) < 1e-9);
        }
    }
}

#[test]
fn driven_dimension_measures_instead_of_driving() {
    let mut sk = fixtures::filleted_rect().exact;
    // A 30 mm horizontal distance between the fillet centres would conflict
    // with the 50 mm distance; driven, it only measures.
    let id = sk.constrain(ConstraintKind::HorizontalDistance {
        p: PointId(0),
        q: PointId(1),
        d: 30.0,
    });
    sk.constraints[id.index()].driven = true;
    let rep = LmSolver::new().solve(&mut sk).unwrap();
    assert!(rep.converged);
    let (key, v) = rep.driven[0];
    assert_eq!(key, sk.constraints[id.index()].key);
    assert!((v - 50.0).abs() < 1e-9, "{v}");
}

#[test]
fn bad_input_is_an_error_not_a_panic() {
    let mut sk = Sketch::default();
    let p = sk.point(0.0, 0.0);
    let q = sk.point(1.0, 0.0);
    let l = sk.line(p, q);
    let mut solver = LmSolver::new();

    let mut bad = sk.clone();
    bad.constrain(ConstraintKind::Horizontal(locus_solver::CurveId(7)));
    assert!(matches!(
        solver.solve(&mut bad),
        Err(SketchError::MissingRef(_))
    ));

    let mut bad = sk.clone();
    bad.constrain(ConstraintKind::Radius { c: l, r: 1.0 });
    assert!(matches!(
        solver.solve(&mut bad),
        Err(SketchError::WrongKind { .. })
    ));

    let mut bad = sk.clone();
    bad.constrain(ConstraintKind::Distance { p, q, d: f64::NAN });
    assert!(matches!(
        solver.solve(&mut bad),
        Err(SketchError::NotFinite(Key(0)))
    ));

    let mut bad = sk.clone();
    bad.curve(CurveKind::Arc {
        centre: p,
        a: q,
        b: PointId(9),
    });
    assert!(matches!(
        solver.solve(&mut bad),
        Err(SketchError::CurveMissingPoint(_))
    ));

    let mut bad = sk.clone();
    bad.points[0].at = Vec2::new(f64::INFINITY, 0.0);
    assert!(matches!(
        solver.solve(&mut bad),
        Err(SketchError::NotFinite(_))
    ));

    assert!(matches!(
        solver.drag(&mut sk.clone(), PointId(5), Vec2::ZERO),
        Err(SketchError::BadDrag)
    ));
    assert!(matches!(
        solver.drag(&mut sk.clone(), p, Vec2::new(f64::NAN, 0.0)),
        Err(SketchError::BadDrag)
    ));
}
