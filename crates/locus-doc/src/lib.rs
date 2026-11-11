// SPDX-License-Identifier: AGPL-3.0-or-later

//! Document model, layers, IDs, the serialisable command API with undo/redo,
//! and the `.locus` format (SPEC §4, §10, §11).
//!
//! Core crate: no GUI deps, no filesystem/thread/clock use, builds for wasm32.

// Tests may unwrap; library code may not (CLAUDE.md, Code Style).
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod boolean;
pub mod command;
pub mod constraint;
pub mod corner;
pub mod document;
pub mod entity;
pub mod error;
pub mod expr;
pub mod file;
pub mod ids;
pub mod modify;
pub mod offset;
pub mod operation;
pub mod params;
pub mod recipe;
pub mod regions;
pub mod solve;
pub mod text;
pub mod transform;

#[cfg(test)]
mod testutil;

pub use boolean::{Boolean, BooleanError};
pub use command::{Applied, Command, Editor, LayerProps};
pub use constraint::{Constraint, Dimension};
pub use document::{Document, EntityStore, Layer, LayerDeletion, Parameter, View};
pub use entity::{Entity, EntityKind, Geom};
pub use error::DocError;
pub use file::FileError;
pub use ids::{BooleanId, ConstraintId, EntityId, LayerId, RecipeId};
pub use operation::{
    Compensation, CutParams, DragKnifeParams, DrawParams, EngraveParams, Operation, OperationKind,
};
pub use recipe::{MAX_INSTANCE_ENTITIES, MAX_INSTANCES, Recipe, RecipeKind};
pub use solve::{ConstraintGroup, DragFrame, LockReport, Sketcher, SolveState, Status};
pub use text::TextData;
