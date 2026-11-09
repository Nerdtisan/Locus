// SPDX-License-Identifier: AGPL-3.0-or-later

//! Offset engine spike (#161, ADR 0010). Throwaway code.

pub mod cav;
pub mod geom;
pub mod metrics;
pub mod own;

use locus_region::{Area, Edge};

/// The options compared by the spike.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    /// Locus-owned: Minkowski sum/difference through the ADR 0008 booleans.
    Own,
    /// `cavalier_contours` `Shape::parallel_offset` (round joins only).
    Cavalier,
}

pub const METHODS: [Method; 2] = [Method::Own, Method::Cavalier];

impl Method {
    pub fn name(self) -> &'static str {
        match self {
            Method::Own => "A: Locus-owned",
            Method::Cavalier => "B: cavalier_contours",
        }
    }

    /// Offsets a closed area: `d` > 0 grows, < 0 shrinks. Round joins.
    pub fn offset(self, a: &Area, d: f64) -> Area {
        match self {
            Method::Own => own::offset(a, d, own::Join::Round),
            Method::Cavalier => cav::offset(a, d),
        }
    }

    /// One-sided offset of an open chain: `d` > 0 left of travel.
    pub fn offset_open(self, chain: &[Edge], d: f64) -> Vec<Vec<Edge>> {
        match self {
            Method::Own => own::offset_open(chain, d),
            Method::Cavalier => cav::offset_open(chain, d),
        }
    }
}

// Entry points for the wasm32 size measurement (one cdylib per feature set).
#[cfg(feature = "wasm-own")]
#[unsafe(no_mangle)]
pub extern "C" fn spike_own(d: f64) -> f64 {
    let a = geom::area(vec![geom::rounded(0.0, 0.0, 40.0, 20.0, 3.0)]);
    own::offset(&a, d, own::Join::Round).area()
}

#[cfg(feature = "wasm-cav")]
#[unsafe(no_mangle)]
pub extern "C" fn spike_cav(d: f64) -> f64 {
    let a = geom::area(vec![geom::rounded(0.0, 0.0, 40.0, 20.0, 3.0)]);
    cav::offset(&a, d).area()
}
