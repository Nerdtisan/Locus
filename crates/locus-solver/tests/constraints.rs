// SPDX-License-Identifier: AGPL-3.0-or-later

//! One focused test per SPEC §5 v1 constraint and curve pairing (#58
//! geometric, #59 dimensional). Each tiny sketch starts at least a millimetre
//! off; the solve must converge and the independent checker must agree.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{check, fixtures};
use locus_solver::{LmSolver, Solver};

fn solves(name: &str) {
    let case = fixtures::coverage()
        .into_iter()
        .find(|c| c.name == name)
        .unwrap_or_else(|| panic!("no coverage case {name}"));
    let mut sk = case.start.clone();
    let kind = sk.constraints[case.target.index()].kind;
    assert!(
        check::violation(&sk, &kind) > 1e-3,
        "{name} starts satisfied"
    );
    let rep = LmSolver::new().solve(&mut sk).unwrap();
    assert!(rep.converged, "{name}: {rep:?}");
    let (v, at) = check::worst(&sk);
    assert!(v < check::SATISFIED, "{name}: worst {v:e} at {at:?}");
}

macro_rules! cases {
    ($($test:ident => $name:literal,)*) => {
        $(#[test] fn $test() { solves($name) })*

        #[test]
        fn every_coverage_case_has_a_test() {
            let named = [$($name),*];
            for c in fixtures::coverage() {
                assert!(named.contains(&c.name), "untested case {}", c.name);
            }
        }
    };
}

cases! {
    // Geometric (#58)
    coincident => "coincident",
    point_on_line => "point-on-line",
    point_on_circle => "point-on-circle",
    point_on_arc => "point-on-arc",
    horizontal => "horizontal",
    vertical => "vertical",
    parallel => "parallel",
    perpendicular => "perpendicular",
    tangent_line_circle => "tangent line-circle",
    tangent_circle_circle => "tangent circle-circle",
    tangent_line_arc_fillet => "tangent line-arc",
    tangent_line_arc_apart => "tangent line-arc apart",
    tangent_arc_arc => "tangent arc-arc",
    tangent_circle_arc => "tangent circle-arc",
    collinear => "collinear",
    concentric => "concentric",
    concentric_arc => "concentric arc",
    equal_length => "equal length",
    equal_radius => "equal radius",
    equal_radius_arc => "equal radius arc",
    symmetric => "symmetric",
    midpoint => "midpoint",
    fix => "fix",
    // Dimensional (#59)
    distance_point_point => "distance p-p",
    distance_point_line => "distance p-l",
    distance_line_line => "distance l-l",
    horizontal_distance => "horizontal distance",
    vertical_distance => "vertical distance",
    angle => "angle",
    radius_circle => "radius circle",
    radius_arc => "radius arc",
    diameter => "diameter",
}
