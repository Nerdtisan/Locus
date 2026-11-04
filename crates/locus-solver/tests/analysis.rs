// SPDX-License-Identifier: AGPL-3.0-or-later

//! DOF and sketch status (#60), conflict and redundancy sets with the
//! pre-check API (#61), and driven dimensions (#62), on fixtures with known
//! answers.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::fixtures;
use locus_solver::{
    Check, Constraint, ConstraintId, ConstraintKind, Key, LmSolver, PointId, Sketch, Solver, Status,
};

fn key(sk: &Sketch, id: ConstraintId) -> Key {
    sk.constraints[id.index()].key
}

fn driving(sk: &Sketch, kind: ConstraintKind) -> Constraint {
    Constraint {
        key: Key(sk.constraints.len() as u64 + 1000),
        kind,
        driven: false,
    }
}

// ── #60: DOF and status ───────────────────────────────────────────────────

#[test]
fn fixtures_report_their_hand_counted_dof() {
    for f in [
        fixtures::filleted_rect(),
        fixtures::filleted_rect_under(),
        fixtures::generated(72),
    ] {
        let a = LmSolver::new().analyse(&f.start).unwrap();
        assert!(a.converged, "{}", f.name);
        assert_eq!(a.dof, f.expected_dof, "{}", f.name);
        assert!(
            a.redundant.is_empty() && a.conflicts.is_empty(),
            "{}: {a:?}",
            f.name
        );
    }
}

#[test]
fn fully_constrained_sketch_has_every_entity_fixed() {
    let a = LmSolver::new()
        .analyse(&fixtures::filleted_rect().start)
        .unwrap();
    assert_eq!(a.status, Status::FullyConstrained);
    assert!(a.points.iter().chain(&a.curves).all(|f| f.is_fixed()));
}

#[test]
fn under_constrained_sketch_flags_what_can_move() {
    // Without the fillet radius the four fillets can grow together. The bottom
    // centres stay put (one is fixed, the other 50 mm along level with it), as
    // does the circle concentric to the fixed one; the sides move out and the
    // top centres move down.
    let f = fixtures::filleted_rect_under();
    let a = LmSolver::new().analyse(&f.start).unwrap();
    assert_eq!(a.status, Status::UnderConstrained);
    assert_eq!(a.dof, 1);
    assert!(
        a.points[0].is_fixed() && a.points[1].is_fixed(),
        "bottom centres"
    );
    assert_eq!(a.points[2].dof, 1, "top-right centre");
    assert_eq!(a.points[4].dof, 1, "a tangent point");
    let circle = f.start.curves.len() - 1;
    assert!(
        a.curves[circle].is_fixed(),
        "circle concentric to the fixed centre"
    );
    assert!(a.curves.iter().all(|c| c.dof <= 1));
}

#[test]
fn free_geometry_counts_its_own_freedom() {
    let mut sk = Sketch::default();
    let p = sk.point(0.0, 0.0);
    let q = sk.point(10.0, 1.0);
    let l = sk.line(p, q);
    let lone = sk.point(5.0, 5.0);
    sk.constrain(ConstraintKind::Horizontal(l));
    let a = LmSolver::new().analyse(&sk).unwrap();
    assert_eq!(a.dof, 5);
    assert_eq!(a.status, Status::UnderConstrained);
    assert_eq!(a.points[lone.index()].dof, 2);
    assert_eq!(a.curves[l.index()].dof, 3);
}

#[test]
fn generated_sketch_flags_the_free_spokes() {
    // DOF 37 = one free spoke on each of the 36 even cells, plus the free gap.
    let f = fixtures::generated(72);
    let a = LmSolver::new().analyse(&f.exact).unwrap();
    assert_eq!(a.dof, 37);
    // Cell 0 is fixed (its spoke point, index 9, is free).
    assert!((0..9).all(|i| a.points[i].is_fixed()));
    assert_eq!(a.points[9].dof, 1);
    // An odd cell's spoke is at a fixed angle.
    assert!(a.points[19].is_fixed());
}

// ── #61: redundancy and conflicts ─────────────────────────────────────────

#[test]
fn redundant_constraint_is_reported_with_its_minimal_set() {
    // parallel(bottom, top) is implied by the two horizontals, nothing else.
    let f = fixtures::filleted_rect_redundant();
    let a = LmSolver::new().analyse(&f.start).unwrap();
    assert!(a.converged);
    assert_eq!(a.status, Status::OverConstrained);
    assert!(a.conflicts.is_empty());
    assert_eq!(a.redundant.len(), 1, "{a:?}");
    let set = &a.redundant[0];
    let parallel = f.start.constraints.last().unwrap();
    assert_eq!(set.constraint, parallel.key);
    let horizontals: Vec<Key> = f
        .start
        .constraints
        .iter()
        .filter(|c| matches!(c.kind, ConstraintKind::Horizontal(_)))
        .map(|c| c.key)
        .collect();
    let mut want = horizontals.clone();
    want.push(parallel.key);
    assert_eq!(set.constraints, want);
    // The entities: the two lines (and nothing else).
    assert_eq!(set.curves.len(), 2);
}

/// Removing any one member of a conflict set lets the conflicting constraint
/// hold: the set is minimal.
fn assert_minimal(sk: &Sketch, extra: Constraint, set: &[Key]) {
    for k in set.iter().filter(|k| **k != extra.key) {
        let mut less = sk.clone();
        less.constraints.retain(|c| c.key != *k);
        let check = LmSolver::new().check_add(&less, extra).unwrap();
        assert!(
            !matches!(check, Check::Conflict(_)),
            "still conflicts without {k:?}: {check:?}"
        );
    }
}

#[test]
fn conflict_is_reported_with_a_minimal_set() {
    for f in [
        fixtures::filleted_rect_conflict(),
        fixtures::generated_conflict(24),
    ] {
        let a = LmSolver::new().analyse(&f.start).unwrap();
        assert!(!a.converged, "{}", f.name);
        assert_eq!(a.status, Status::OverConstrained);
        assert_eq!(a.conflicts.len(), 1, "{}: {a:?}", f.name);
        let set = &a.conflicts[0];
        for c in &f.conflict {
            assert!(
                set.constraints.contains(&key(&f.start, *c)),
                "{}: {set:?}",
                f.name
            );
        }
        // The conflict found is the later of the two fixture constraints.
        let extra = *f.start.constraints.last().unwrap();
        assert_eq!(set.constraint, extra.key);
        let mut base = f.start.clone();
        base.constraints.pop();
        assert_minimal(&base, extra, &set.constraints);
    }
}

#[test]
fn pre_check_refuses_a_conflicting_constraint_before_it_is_applied() {
    let f = fixtures::filleted_rect();
    let sk = f.exact.clone();
    let mut solver = LmSolver::new();
    // A 45 mm horizontal distance between the fillet centres fights the 50 mm
    // distance dimension.
    let extra = driving(
        &sk,
        ConstraintKind::HorizontalDistance {
            p: PointId(0),
            q: PointId(1),
            d: 45.0,
        },
    );
    let Check::Conflict(set) = solver.check_add(&sk, extra).unwrap() else {
        panic!("expected a conflict");
    };
    assert_eq!(set.constraint, extra.key);
    let width = sk
        .constraints
        .iter()
        .find(|c| matches!(c.kind, ConstraintKind::Distance { .. }))
        .unwrap();
    assert!(set.constraints.contains(&width.key), "{set:?}");
    assert!(set.points.contains(&sk.points[0].key) && set.points.contains(&sk.points[1].key));
    assert_minimal(&sk, extra, &set.constraints);
    // The sketch itself is untouched and still fully constrained.
    assert_eq!(sk, f.exact);
    assert_eq!(
        solver.analyse(&sk).unwrap().status,
        Status::FullyConstrained
    );
}

#[test]
fn pre_check_flags_redundant_and_accepts_useful_constraints() {
    let mut solver = LmSolver::new();
    let sk = fixtures::filleted_rect().exact;
    let (bottom, top) = (locus_solver::CurveId(0), locus_solver::CurveId(2));
    let parallel = driving(&sk, ConstraintKind::Parallel(bottom, top));
    assert!(matches!(
        solver.check_add(&sk, parallel).unwrap(),
        Check::Redundant(_)
    ));

    let under = fixtures::filleted_rect_under().exact;
    let arc = locus_solver::CurveId(7);
    let radius = driving(&under, ConstraintKind::Radius { c: arc, r: 4.0 });
    assert_eq!(solver.check_add(&under, radius).unwrap(), Check::Ok);
}

// ── #62: driven dimensions ────────────────────────────────────────────────

#[test]
fn driven_dimension_is_left_out_of_the_system() {
    let mut sk = fixtures::filleted_rect().exact;
    let id = sk.constrain(ConstraintKind::HorizontalDistance {
        p: PointId(0),
        q: PointId(1),
        d: 30.0,
    });
    sk.constraints[id.index()].driven = true;
    let a = LmSolver::new().analyse(&sk).unwrap();
    assert!(a.converged);
    assert_eq!(a.status, Status::FullyConstrained);
}

#[test]
fn making_a_driven_dimension_driving_is_refused_when_it_over_constrains() {
    let mut solver = LmSolver::new();
    let mut sk = fixtures::filleted_rect().exact;
    let id = sk.constrain(ConstraintKind::HorizontalDistance {
        p: PointId(0),
        q: PointId(1),
        d: 30.0,
    });
    sk.constraints[id.index()].driven = true;
    let Check::Conflict(set) = solver.check_driving(&sk, id).unwrap() else {
        panic!("expected a conflict");
    };
    assert_eq!(set.constraint, key(&sk, id));

    // At the measured value it is consistent but adds nothing.
    sk.constraints[id.index()].kind = ConstraintKind::HorizontalDistance {
        p: PointId(0),
        q: PointId(1),
        d: 50.0,
    };
    assert!(matches!(
        solver.check_driving(&sk, id).unwrap(),
        Check::Redundant(_)
    ));

    // On the under-constrained sketch, a driven fillet radius may drive.
    let mut under = fixtures::filleted_rect_under().exact;
    let r = under.constrain(ConstraintKind::Radius {
        c: locus_solver::CurveId(7),
        r: 4.0,
    });
    under.constraints[r.index()].driven = true;
    assert_eq!(solver.check_driving(&under, r).unwrap(), Check::Ok);
    assert!(solver.check_driving(&under, ConstraintId(999)).is_err());
}
