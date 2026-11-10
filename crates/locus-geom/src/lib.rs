// SPDX-License-Identifier: AGPL-3.0-or-later

//! Geometry primitives and maths in f64 millimetres (SPEC §3, §4).
//!
//! Core crate: no GUI dependencies, no filesystem/thread/clock use, builds for
//! `wasm32-unknown-unknown`.

// Tests may unwrap; library code may not (CLAUDE.md, Code Style).
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod bbox;
pub mod cubic;
pub mod primitives;
pub mod tolerance;
pub mod transform;
pub mod units;
pub mod vec2;

pub use bbox::Aabb;
pub use cubic::{Cubic, Piece};
pub use primitives::{Arc, Circle, Line};
pub use tolerance::{EXPORT_TOL, SOLVER_TOL};
pub use transform::Affine2;
pub use units::Unit;
pub use vec2::{Point2, Vec2};
