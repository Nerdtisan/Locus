// SPDX-License-Identifier: AGPL-3.0-or-later

//! Solver property tests (#64, SPEC §14): random valid constraint sets
//! converge and satisfy every constraint; adding then removing a constraint
//! returns to the same DOF.
//!
//! Sketches are valid by construction: random geometry first, then
//! constraints whose values are measured from it (and points placed on the
//! curves they are constrained to), so an exact solution always exists. The
//! solve starts from a perturbed copy.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::check;
use locus_geom::Vec2;
use locus_solver::{ConstraintKind as K, CurveId, LmSolver, PointId, Sketch, Solver};
use proptest::prelude::*;
use proptest::test_runner::{Config, RngSeed};

/// A small random sketch with lines, a circle, a point on it and a midpoint,
/// plus `picks.len()` measured constraints. Degenerate picks (near-zero
/// distances, which have no gradient) are skipped.
fn build(pts: &[(f64, f64)], r: f64, picks: &[(u8, u8, u8)]) -> Sketch {
    let mut sk = Sketch::default();
    let ps: Vec<PointId> = pts.iter().map(|&(x, y)| sk.point(x, y)).collect();
    let mut lines: Vec<CurveId> = Vec::new();
    for w in ps.windows(2) {
        if sk
            .point_at(w[0])
            .unwrap()
            .distance(sk.point_at(w[1]).unwrap())
            > 2.0
        {
            lines.push(sk.line(w[0], w[1]));
        }
    }
    let centre = ps[0];
    let circle = sk.circle(centre, r);
    let c = sk.point_at(centre).unwrap();
    let on = sk.point(c.x + r * 0.6, c.y + r * 0.8);
    sk.constrain(K::PointOnCurve(on, circle));
    if let Some(&l) = lines.first() {
        let (a, b) = sk.line_points(l).unwrap();
        let m = (sk.point_at(a).unwrap() + sk.point_at(b).unwrap()) * 0.5;
        let mp = sk.point(m.x, m.y);
        sk.constrain(K::Midpoint { p: mp, line: l });
    }
    sk.constrain(K::Fix {
        p: ps[1],
        at: sk.point_at(ps[1]).unwrap(),
    });
    let np = sk.points.len();
    for &(kind, i, j) in picks {
        let p = PointId(u32::from(i) % np as u32);
        let q = PointId(u32::from(j) % np as u32);
        let (pa, qa) = (sk.point_at(p).unwrap(), sk.point_at(q).unwrap());
        let k = match kind % 6 {
            0 if pa.distance(qa) > 1.0 => K::Distance {
                p,
                q,
                d: pa.distance(qa),
            },
            1 if p != q => K::HorizontalDistance {
                p,
                q,
                d: qa.x - pa.x,
            },
            2 if p != q => K::VerticalDistance {
                p,
                q,
                d: qa.y - pa.y,
            },
            3 if lines.len() > 1 => {
                let (l, m) = (
                    lines[i as usize % lines.len()],
                    lines[j as usize % lines.len()],
                );
                if l == m {
                    continue;
                }
                let dir = |c| {
                    let (a, b) = sk.line_points(c).unwrap();
                    sk.point_at(b).unwrap() - sk.point_at(a).unwrap()
                };
                let (u, v) = (dir(l), dir(m));
                K::Angle {
                    l,
                    m,
                    rad: u.cross(v).atan2(u.dot(v)),
                }
            }
            4 if !lines.is_empty() => {
                let l = lines[j as usize % lines.len()];
                let (a, b) = sk.line_points(l).unwrap();
                let (a, b) = (sk.point_at(a).unwrap(), sk.point_at(b).unwrap());
                let d = ((b - a).cross(pa - a) / (b - a).length()).abs();
                if d < 1.0 {
                    continue;
                }
                K::PointLineDistance { p, line: l, d }
            }
            5 => K::Radius { c: circle, r },
            _ => continue,
        };
        sk.constrain(k);
    }
    sk
}

/// Every free point nudged by up to `amp` mm.
fn perturb(sk: &Sketch, noise: &[(f64, f64)]) -> Sketch {
    let mut s = sk.clone();
    for (p, (dx, dy)) in s.points.iter_mut().zip(noise.iter().cycle()).skip(2) {
        p.at += Vec2::new(*dx, *dy);
    }
    s
}

fn config() -> Config {
    Config {
        cases: 64,
        // Deterministic in CI; failures are reported with their inputs.
        rng_seed: RngSeed::Fixed(0x10c5),
        failure_persistence: None,
        ..Config::default()
    }
}

fn points() -> impl Strategy<Value = Vec<(f64, f64)>> {
    prop::collection::vec((-50.0..50.0f64, -50.0..50.0f64), 4..10)
}

fn picks() -> impl Strategy<Value = Vec<(u8, u8, u8)>> {
    prop::collection::vec(any::<(u8, u8, u8)>(), 0..12)
}

fn noise() -> impl Strategy<Value = Vec<(f64, f64)>> {
    prop::collection::vec((-0.05..0.05f64, -0.05..0.05f64), 1..8)
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn random_valid_constraint_sets_converge(pts in points(), r in 2.0..10.0f64, picks in picks(), noise in noise()) {
        let exact = build(&pts, r, &picks);
        prop_assert!(check::satisfied(&exact), "not valid by construction");
        let mut sk = perturb(&exact, &noise);
        let rep = LmSolver::new().solve(&mut sk).unwrap();
        prop_assert!(rep.converged, "{rep:?}");
        let (v, at) = check::worst(&sk);
        prop_assert!(v < check::SATISFIED, "worst {v:e} at {at:?}");
    }

    #[test]
    fn adding_then_removing_a_constraint_restores_the_dof(pts in points(), r in 2.0..10.0f64, picks in picks(), extra in any::<(u8, u8, u8)>()) {
        let base = build(&pts, r, &picks);
        let mut solver = LmSolver::new();
        let before = solver.analyse(&base).unwrap();
        let mut more = build(&pts, r, &[picks.as_slice(), &[extra]].concat());
        prop_assume!(more.constraints.len() == base.constraints.len() + 1);
        let with = solver.analyse(&more).unwrap();
        prop_assert!(with.dof <= before.dof);
        more.constraints.pop();
        let after = solver.analyse(&more).unwrap();
        prop_assert_eq!(after.dof, before.dof);
        prop_assert_eq!(after.redundant.len(), before.redundant.len());
    }
}
