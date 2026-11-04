// SPDX-License-Identifier: AGPL-3.0-or-later

//! Document model, layers, IDs, the serialisable command API with undo/redo,
//! and the `.locus` format (SPEC §4, §10, §11).
//!
//! Core crate: no GUI deps, no filesystem/thread/clock use, builds for wasm32.

// Tests may unwrap; library code may not (CLAUDE.md, Code Style).
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod command;
pub mod constraint;
pub mod document;
pub mod entity;
pub mod error;
pub mod file;
pub mod ids;

#[cfg(test)]
mod testutil;

pub use command::{Applied, Command, Editor};
pub use constraint::{Constraint, Dimension};
pub use document::{Document, EntityStore, Layer, Operation, Parameter, View};
pub use entity::{Entity, EntityKind, Geom};
pub use error::DocError;
pub use file::FileError;
pub use ids::{ConstraintId, EntityId, LayerId};
