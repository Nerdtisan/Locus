// SPDX-License-Identifier: AGPL-3.0-or-later

//! Closed-profile (region) detection, non-destructive booleans and offsets
//! (SPEC §6.3). Booleans keep true arcs with an exact line/arc arrangement
//! owned here ([ADR 0008](../../docs/adr/0008-booleans-and-arcs.md)).
//!
//! Core crate: no GUI deps, no filesystem/thread/clock use, builds for
//! `wasm32-unknown-unknown`. Input is plain geometry keyed by entity ID, so
//! the crate does not depend on `locus-doc`.

// Tests may unwrap; library code may not (CLAUDE.md, Code Style).
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod arrange;
pub mod region;
pub mod seg;

pub use arrange::SNAP;
pub use region::{Curve, Region, RegionKey, Shape, detect};
pub use seg::{Area, Edge, Loop, Seg};
