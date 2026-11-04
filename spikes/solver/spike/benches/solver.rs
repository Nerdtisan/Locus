// SPDX-License-Identifier: AGPL-3.0-or-later

//! `cargo bench`: prints the harness table, then criterion timings of the two
//! numbers SPEC §3 budgets: full solve and one drag frame at 500 entities.

use criterion::{criterion_group, criterion_main, BatchSize, Criterion};
use solver_harness::{fixtures, Backend, Clock};
use solver_spike::{ezpz_backend::Ezpz, solidcraft::SolidCraft, CELLS};
use std::hint::black_box;
use std::time::Instant;

struct Native(Instant);

impl Clock for Native {
    fn now_ms(&self) -> f64 {
        self.0.elapsed().as_secs_f64() * 1e3
    }
}

fn bench_backend<B: Backend>(c: &mut Criterion, b: &B) {
    let mut g = c.benchmark_group(b.name());
    g.sample_size(10);
    for f in [fixtures::filleted_rect(), fixtures::generated(CELLS)] {
        let Ok(start) = b.load(&f.start) else {
            continue;
        };
        drop(start);
        g.bench_function(format!("{}/full-solve", f.name), |bch| {
            bch.iter_batched(
                || b.load(&f.start).ok(),
                |st| st.map(|mut st| black_box(b.solve(&mut st))),
                BatchSize::LargeInput,
            )
        });
        for d in &f.drags {
            // One frame: a step along the drag from the solved exact geometry.
            let Some(&to) = d.path.first() else { continue };
            g.bench_function(format!("{}/{}-frame", f.name, d.name), |bch| {
                bch.iter_batched(
                    || {
                        b.load(&f.exact).ok().map(|mut st| {
                            b.solve(&mut st);
                            st
                        })
                    },
                    |st| st.map(|mut st| black_box(b.drag(&mut st, d.point, to))),
                    BatchSize::LargeInput,
                )
            });
        }
    }
    g.finish();
}

fn benches(c: &mut Criterion) {
    println!("{}", solver_spike::run_all(&Native(Instant::now()), CELLS));
    bench_backend(c, &SolidCraft);
    bench_backend(c, &Ezpz);
}

criterion_group!(solver, benches);
criterion_main!(solver);
