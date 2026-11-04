// SPDX-License-Identifier: AGPL-3.0-or-later

//! Export pipeline and SVG/PDF/DXF writers (SPEC §8). Core crate: returns
//! bytes, never touches the filesystem; must build for wasm32.
//!
//! Stages, each a plain function over [`ExportModel`] so every writer shares
//! them: [`collect`] → [`validate`] → [`autofix`] (optional) → [`order`] → writer.

// Tests may unwrap; library code may not (CLAUDE.md, Code Style).
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod fix;
pub mod order;
pub mod overlap;
pub mod pipeline;
pub mod svg;
pub mod validate;

pub use fix::{Fix, FixKind, autofix};
pub use order::{order, travel_length};
pub use pipeline::{ExportLayer, ExportModel, ExportPath, PathSeg, Shape, chain, collect};
pub use svg::{to_svg, to_svg_model};
pub use validate::{Issue, IssueKind, Report, Severity, validate};
