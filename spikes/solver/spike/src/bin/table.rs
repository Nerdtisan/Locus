// SPDX-License-Identifier: AGPL-3.0-or-later

//! Print the comparison table natively: `cargo run --release --bin table [cells]`.

use solver_harness::Clock;
use std::time::Instant;

struct Native(Instant);

impl Clock for Native {
    fn now_ms(&self) -> f64 {
        self.0.elapsed().as_secs_f64() * 1e3
    }
}

fn main() {
    let cells = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(solver_spike::CELLS);
    print!("{}", solver_spike::run_all(&Native(Instant::now()), cells));
}
