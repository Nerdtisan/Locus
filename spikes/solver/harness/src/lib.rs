// SPDX-License-Identifier: AGPL-3.0-or-later

//! Solver benchmark harness (issue #41; SPEC §3 solver requirements, §13 M0).
//!
//! Every candidate backend is driven through [`Backend`] on the same fixtures and
//! judged by the same independent residual checker ([`check`]), never by its own
//! "solved" flag. The suite measures full solve, incremental drag re-solve
//! (target < 16 ms at 500 entities, §3), DOF count and conflict detection.
//!
//! No dependencies, no clock and no I/O: time comes in through [`Clock`], so the
//! same code runs natively and in `wasm32-unknown-unknown`. Written to move into
//! `locus-solver` in M1 as its backend test suite.

pub mod check;
pub mod fixtures;
pub mod model;
mod suite;

pub use model::{Constraint, ConstraintId, Curve, CurveId, PointId, Sketch, Vec2};
pub use suite::{run, table, BackendReport, Measure};

/// A monotonic millisecond clock (`Instant` natively, `performance.now()` on the web).
pub trait Clock {
    fn now_ms(&self) -> f64;
}

/// What a backend says about a sketch beyond placing it. `None` = not supported.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Diagnosis {
    /// Remaining degrees of freedom.
    pub dof: Option<usize>,
    /// Constraints the backend blames for a failed solve (harness indices).
    pub conflicting: Option<Vec<ConstraintId>>,
    /// Constraints the backend reports as redundant (harness indices).
    pub redundant: Option<Vec<ConstraintId>>,
    /// Anything else worth showing next to the DOF (e.g. what is reported instead).
    pub note: Option<String>,
}

/// A candidate solver, adapted to the harness model.
pub trait Backend {
    /// The backend's own sketch, kept between calls (incremental drags reuse it).
    type State;

    fn name(&self) -> &'static str;
    /// Translate the harness sketch. `Err` names what the backend cannot express.
    fn load(&self, sk: &Sketch) -> Result<Self::State, String>;
    /// Solve from the current geometry. The return value is the backend's claim;
    /// the harness checks the geometry itself.
    fn solve(&self, st: &mut Self::State) -> bool;
    /// One drag frame: move point `p` to `to` (a feasible target) and re-solve
    /// from the previous frame's geometry.
    fn drag(&self, st: &mut Self::State, p: PointId, to: Vec2) -> bool;
    /// DOF, conflicts and redundancy for the current state (may re-solve).
    fn diagnose(&self, st: &mut Self::State) -> Diagnosis;
    /// Write point positions and circle radii back into `into` (same layout as
    /// the sketch given to [`Backend::load`]).
    fn read(&self, st: &Self::State, into: &mut Sketch);
}
