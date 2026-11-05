// SPDX-License-Identifier: AGPL-3.0-or-later

//! Booleans-that-keep-arcs spike (#120, ADR 0008). Throwaway code.

pub mod cav;
pub mod flat;
pub mod geom;
pub mod metrics;
pub mod own;

use geom::Shape;
use own::Op;

/// The options compared by the spike.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    /// Flatten at 0.2 µm sagitta → i_overlay → arc refit at 0.5 µm.
    FlatRefit,
    /// Flatten at 1 µm sagitta → i_overlay, polygons only (no refit).
    FlatLines,
    /// cavalier_contours.
    Cavalier,
    /// Locus-owned exact line/arc arrangement.
    Own,
}

pub const METHODS: [Method; 4] = [Method::FlatRefit, Method::FlatLines, Method::Cavalier, Method::Own];

impl Method {
    pub fn name(self) -> &'static str {
        match self {
            Method::FlatRefit => "A: i_overlay + refit",
            Method::FlatLines => "A0: i_overlay lines",
            Method::Cavalier => "B: cavalier_contours",
            Method::Own => "C: Locus-owned",
        }
    }
    /// `None` = not supported for these operands.
    pub fn run(self, a: &Shape, b: &Shape, op: Op) -> Option<Shape> {
        match self {
            Method::FlatRefit => Some(flat::boolean_refit(a, b, op, 2e-4, 5e-4)),
            Method::FlatLines => Some(flat::as_lines(&flat::polygons(a, b, op, 5e-4))),
            Method::Cavalier => cav::boolean(a, b, op),
            Method::Own => Some(own::boolean(a, b, op)),
        }
    }
}

// Entry points for the wasm32 size measurement (one cdylib per feature set).
#[cfg(feature = "wasm-own")]
#[unsafe(no_mangle)]
pub extern "C" fn spike_own(r: f64) -> f64 {
    let a = Shape::new(vec![geom::rect(0.0, 0.0, 10.0, 10.0)]);
    let b = Shape::new(vec![geom::circle(geom::p(10.0, 10.0), r)]);
    own::boolean(&a, &b, Op::Union).area()
}

#[cfg(feature = "wasm-cav")]
#[unsafe(no_mangle)]
pub extern "C" fn spike_cav(r: f64) -> f64 {
    let a = Shape::new(vec![geom::rect(0.0, 0.0, 10.0, 10.0)]);
    let b = Shape::new(vec![geom::circle(geom::p(10.0, 10.0), r)]);
    cav::boolean(&a, &b, Op::Union).map_or(0.0, |s| s.area())
}

#[cfg(feature = "wasm-flat")]
#[unsafe(no_mangle)]
pub extern "C" fn spike_flat(r: f64) -> f64 {
    let a = Shape::new(vec![geom::rect(0.0, 0.0, 10.0, 10.0)]);
    let b = Shape::new(vec![geom::circle(geom::p(10.0, 10.0), r)]);
    flat::boolean_refit(&a, &b, Op::Union, 2e-4, 5e-4).area()
}
