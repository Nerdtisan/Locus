// SPDX-License-Identifier: AGPL-3.0-or-later

//! Named tolerances (SPEC §4). Use these instead of ad-hoc epsilons.

/// Coincidence / convergence tolerance for the solver, in mm.
pub const SOLVER_TOL: f64 = 1e-6;

/// Coincidence / closure tolerance for export validation, in mm.
pub const EXPORT_TOL: f64 = 1e-3;
