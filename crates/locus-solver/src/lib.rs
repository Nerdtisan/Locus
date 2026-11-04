// SPDX-License-Identifier: AGPL-3.0-or-later

//! Geometric constraint solver (SPEC §3, §5; ADR 0007).
//!
//! A [`Sketch`] (points, curves, constraints, each with the caller's [`Key`])
//! is solved by a [`Solver`]. The backend, [`LmSolver`], is Locus's own sparse
//! Levenberg–Marquardt: one residual definition per constraint with exact
//! dual-number gradients, `λI` damping (minimum-norm steps, so a drag moves as
//! little as it can), and a sparse LDLᵀ whose ordering and symbolic analysis
//! are reused while the constraint set is unchanged.
//!
//! Core crate: no GUI dependencies, no I/O, clock or threads; builds for
//! `wasm32-unknown-unknown`. Input is untrusted and checked up front
//! ([`SketchError`]); nothing on the solve path panics on it.

// Tests may unwrap; library code may not (CLAUDE.md, Code Style).
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod dual;
mod ldl;
mod lm;
pub mod sketch;
mod solver;
mod system;

pub use sketch::{
    Constraint, ConstraintId, ConstraintKind, Curve, CurveId, CurveKind, Key, Point, PointId,
    Sketch, SketchError,
};
pub use solver::{LmSolver, SolveReport, Solver};
