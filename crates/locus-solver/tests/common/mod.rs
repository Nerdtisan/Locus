// SPDX-License-Identifier: AGPL-3.0-or-later

//! The solver harness (issue #41, moved from `spikes/solver/` per ADR 0002 and
//! 0007): fixtures and an independent residual checker. Solves are judged by
//! the checker, never by the solver's own "converged" flag (SPEC §3).

#![allow(dead_code)] // Each test binary uses part of the harness.
#![allow(clippy::unwrap_used, clippy::expect_used)]

pub mod check;
pub mod fixtures;
