// SPDX-License-Identifier: AGPL-3.0-or-later

//! Solver spike runners (issues #39, #40, #41): candidate backends on the shared
//! harness, natively (`cargo run --release --bin table`, `cargo bench`) and in
//! `wasm32-unknown-unknown` (`web/index.html`).

pub mod analysis;
#[cfg(feature = "ezpz")]
pub mod ezpz_backend;
pub mod own;
#[cfg(feature = "solidcraft")]
pub mod solidcraft;

use solver_harness::{table, Clock};

/// Cells in the generated sketch: 72 cells = 504 curves + 720 points (SPEC §3's
/// "500 entities", counting curves only).
pub const CELLS: usize = 72;

/// The full table for every backend.
pub fn run_all(clock: &dyn Clock, cells: usize) -> String {
    #[allow(unused_mut)]
    let mut reports = Vec::new();
    #[cfg(feature = "solidcraft")]
    reports.push(measured(&solidcraft::SolidCraft, clock, cells));
    #[cfg(feature = "ezpz")]
    {
        reports.push(measured(&ezpz_backend::Ezpz, clock, cells));
        reports.push(measured(&ezpz_backend::EzpzRank, clock, cells));
    }
    reports.push(measured(&own::Own::LDL, clock, cells));
    #[cfg(feature = "nalgebra")]
    reports.push(measured(&own::Own::NALGEBRA, clock, cells));
    let _ = (clock, cells);
    table(&reports)
}

/// One backend's report, after a small discarded warm-up run, so caches and the
/// allocator are in the same state for every backend whatever its turn.
#[allow(dead_code)]
fn measured<B: solver_harness::Backend>(
    b: &B,
    clock: &dyn Clock,
    cells: usize,
) -> solver_harness::BackendReport {
    let _ = solver_harness::run(b, clock, 8);
    solver_harness::run(b, clock, cells)
}

/// The wasm entry points: no wasm-bindgen, just a host-imported clock and a
/// buffer the page reads the table from.
#[cfg(target_arch = "wasm32")]
mod web {
    use super::*;
    use std::sync::Mutex;

    #[link(wasm_import_module = "env")]
    extern "C" {
        fn now_ms() -> f64;
    }

    struct PerfNow;

    impl Clock for PerfNow {
        fn now_ms(&self) -> f64 {
            // SAFETY: a host import with no arguments that returns a number.
            unsafe { now_ms() }
        }
    }

    static OUT: Mutex<String> = Mutex::new(String::new());

    /// Run the suite; returns the table's length in bytes (read via `out_ptr`).
    #[no_mangle]
    pub extern "C" fn run_suite(cells: u32) -> u32 {
        let text = run_all(&PerfNow, cells as usize);
        let len = text.len() as u32;
        if let Ok(mut out) = OUT.lock() {
            *out = text;
        }
        len
    }

    /// Address of the last table in linear memory.
    #[no_mangle]
    pub extern "C" fn out_ptr() -> *const u8 {
        OUT.lock().map_or(core::ptr::null(), |o| o.as_ptr())
    }
}
