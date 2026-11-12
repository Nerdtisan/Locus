// SPDX-License-Identifier: AGPL-3.0-or-later

//! Export pipeline and SVG/PDF/DXF writers (SPEC §8). Core crate: returns
//! bytes, never touches the filesystem; must build for wasm32.
//!
//! Stages, each a plain function over [`ExportModel`] so every writer shares
//! them: [`collect`] → [`autofix`] (optional) → [`apply_operations`] (kerf,
//! blade offset, hatch) → [`validate`] → [`order`] → writer.

// Tests may unwrap; library code may not (CLAUDE.md, Code Style).
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod dxf;
pub mod exporter;
pub mod fix;
pub mod ops;
pub mod order;
pub mod overlap;
pub mod pdf;
pub mod pipeline;
pub mod prepare;
pub mod profile;
pub mod svg;
pub mod validate;

pub use dxf::{to_dxf, to_dxf_model};
pub use exporter::{
    DxfExporter, ExportError, Exported, Exporter, PdfExporter, Registry, SvgExporter,
};
pub use fix::{Fix, FixKind, autofix};
pub use ops::apply_operations;
pub use order::{order, travel_length};
pub use pdf::{to_pdf, to_pdf_model};
pub use pipeline::{ExportLayer, ExportModel, ExportPath, PathSeg, Shape, chain, collect};
pub use prepare::{Prepared, prepare, prepare_for};
pub use profile::{Bed, Defaults, MachineProfile, Origin, ProfileError, ProfileUnits};
pub use svg::{to_svg, to_svg_model};
pub use validate::{Issue, IssueKind, Report, Severity, validate};
