// SPDX-License-Identifier: AGPL-3.0-or-later

//! Drag quality: how far geometry that should stay put moves during a drag.
//! `cargo run --release --bin probe [cells]`.

use solver_harness::{fixtures, Backend, Sketch};

fn drift<B: Backend>(b: &B, cells: usize) {
    let f = fixtures::generated(cells);
    // Points of cells left of the free gap: the chain-drag must not move them
    // (their free spokes could turn, but a good drag leaves them alone).
    let still = 10 * (cells / 2);
    for d in &f.drags {
        let Ok(mut st) = b.load(&f.exact) else { return };
        b.solve(&mut st);
        let mut prev: Sketch = f.exact.clone();
        b.read(&st, &mut prev);
        let (mut left, mut step) = (0.0f64, 0.0f64);
        for &to in &d.path {
            b.drag(&mut st, d.point, to);
            let mut out = f.exact.clone();
            b.read(&st, &mut out);
            for i in 0..out.points.len() {
                let m = out.points[i].dist(prev.points[i]);
                if i < still && d.name == "drag-chain" {
                    left = left.max(m);
                }
                step += m * m;
            }
            prev = out;
        }
        println!(
            "{:<28} {:<11} far-side max move {:.2e} mm, mean frame |dx| {:.3} mm",
            b.name(),
            d.name,
            left,
            (step / d.path.len() as f64).sqrt()
        );
    }
}

fn main() {
    let cells = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(72);
    #[cfg(feature = "ezpz")]
    drift(&solver_spike::ezpz_backend::Ezpz, cells);
    drift(&solver_spike::own::Own::LDL, cells);
    #[cfg(feature = "nalgebra")]
    drift(&solver_spike::own::Own::NALGEBRA, cells);
}
