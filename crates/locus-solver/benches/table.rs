// SPDX-License-Identifier: AGPL-3.0-or-later

//! `cargo bench -p locus-solver`: the harness table (issue #41, #63) on the
//! fixtures, as in ADR 0007. Sizes from `LOCUS_BENCH_CELLS` (default `72,144`:
//! 504 and 1,008 curves). The CI gate for the §3 drag budget is the test in
//! `tests/drag.rs`; this prints the numbers to record.

#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "../tests/common/mod.rs"]
mod common;

use common::{check, fixtures};
use locus_solver::{LmSolver, Solver};
use std::time::Instant;

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1e3
}

/// Mean of `reps` runs of `f`, in ms.
fn mean(reps: usize, mut f: impl FnMut()) -> f64 {
    let t = Instant::now();
    for _ in 0..reps {
        f();
    }
    ms(t) / reps as f64
}

fn row(label: &str, value: String) {
    println!("| {label} | {value} |");
}

fn main() {
    let sizes: Vec<usize> = std::env::var("LOCUS_BENCH_CELLS")
        .ok()
        .map(|v| v.split(',').filter_map(|s| s.trim().parse().ok()).collect())
        .unwrap_or_else(|| vec![72, 144]);

    let rect = fixtures::filleted_rect();
    println!("| measure | value |\n|---|---|");
    row(
        "fillet-rect: full solve, ms",
        format!(
            "{:.4}",
            mean(200, || {
                let mut sk = rect.start.clone();
                LmSolver::new().solve(&mut sk).unwrap();
            })
        ),
    );
    for cells in sizes {
        let f = fixtures::generated(cells);
        let n = format!("{} curves", f.exact.curves.len());
        let mut solver = LmSolver::new();
        let solve = mean(10, || {
            let mut sk = f.start.clone();
            assert!(solver.solve(&mut sk).unwrap().converged);
        });
        row(&format!("{n}: full solve, ms"), format!("{solve:.3}"));
        let t = Instant::now();
        let a = solver.analyse(&f.exact).unwrap();
        row(&format!("{n}: analyse (DOF {} ✓ = {}), ms", f.expected_dof, a.dof), format!("{:.3}", ms(t)));
        for d in &f.drags {
            let mut sk = f.exact.clone();
            let mut solver = LmSolver::new();
            let (mut total, mut worst) = (0.0f64, 0.0f64);
            for &to in &d.path {
                let t = Instant::now();
                solver.drag(&mut sk, d.point, to).unwrap();
                let e = ms(t);
                total += e;
                worst = worst.max(e);
            }
            assert!(check::satisfied(&sk));
            row(
                &format!("{n}: {} frame, mean / max ms", d.name),
                format!("{:.3} / {:.3}", total / d.path.len() as f64, worst),
            );
        }
        let c = fixtures::generated_conflict(cells);
        let t = Instant::now();
        let a = solver.analyse(&c.start).unwrap();
        row(
            &format!("{n}: conflict detected and named ({}), ms", a.conflicts.len()),
            format!("{:.3}", ms(t)),
        );
    }
}
