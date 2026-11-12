// SPDX-License-Identifier: AGPL-3.0-or-later

//! Parsers for reference geometry: SVG (#241) and DXF (#242).
//!
//! Imports are **never** editable native documents (SPEC §1 non-goals): the
//! parsers turn a file into [`ImportShape`]s in millimetres, and
//! `locus_doc::reference::import_shapes_command` lands them on a locked
//! Reference layer (§4, §7) as one undo step.
//!
//! Files are untrusted input (CLAUDE.md): sizes and counts are bounded by the
//! constants below, nothing here panics on malformed data, and nothing is
//! fetched: external `href`s and entities are ignored. The crate has no
//! filesystem, thread or clock dependency, so it builds for
//! `wasm32-unknown-unknown`.

mod affine;
mod svg;

use locus_doc::reference::ImportShape;

pub use svg::import_svg;

/// Largest file the importers read.
pub const MAX_INPUT_BYTES: usize = 32 * 1024 * 1024;
/// Most shapes one import produces; more is an error, not a truncation.
pub const MAX_SHAPES: usize = 250_000;
/// Largest |coordinate| kept, in mm (1 km): shapes beyond it are dropped.
pub const MAX_COORD_MM: f64 = 1.0e6;
/// Deepest group / block nesting followed.
pub const MAX_DEPTH: usize = 32;
/// Most warnings kept (a hostile file could make one per shape).
pub const MAX_WARNINGS: usize = 50;

/// What an importer made.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Imported {
    pub shapes: Vec<ImportShape>,
    /// Things that were skipped or approximated, for the user (bounded).
    pub warnings: Vec<String>,
}

impl Imported {
    pub(crate) fn warn(&mut self, msg: impl Into<String>) {
        if self.warnings.len() < MAX_WARNINGS {
            self.warnings.push(msg.into());
        } else if self.warnings.len() == MAX_WARNINGS {
            self.warnings.push("…more warnings omitted".to_owned());
        }
    }

    /// Adds a shape unless it is non-finite or out of range.
    pub(crate) fn push(&mut self, shape: ImportShape) -> Result<(), ImportError> {
        if !shape_in_range(&shape) {
            self.warn("skipped geometry that is out of range");
            return Ok(());
        }
        if self.shapes.len() >= MAX_SHAPES {
            return Err(ImportError::TooMuchGeometry);
        }
        self.shapes.push(shape);
        Ok(())
    }
}

fn shape_in_range(s: &ImportShape) -> bool {
    let ok = |p: &locus_geom::Vec2| {
        p.x.is_finite() && p.y.is_finite() && p.x.abs() <= MAX_COORD_MM && p.y.abs() <= MAX_COORD_MM
    };
    match s {
        ImportShape::Line { a, b } => ok(a) && ok(b),
        ImportShape::Arc { center, start, end } => ok(center) && ok(start) && ok(end),
        ImportShape::Circle { center, radius } => {
            ok(center) && radius.is_finite() && *radius > 0.0 && *radius <= MAX_COORD_MM
        }
        ImportShape::Cubic { a, c1, c2, b } => ok(a) && ok(c1) && ok(c2) && ok(b),
    }
}

/// Why a file could not be imported. Messages are safe to show: they never
/// echo file contents.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ImportError {
    #[error("the file is larger than the {} MB import limit", MAX_INPUT_BYTES / (1024 * 1024))]
    TooLarge,
    #[error("the file has more than {MAX_SHAPES} shapes")]
    TooMuchGeometry,
    #[error("the file is not valid SVG")]
    BadSvg,

    #[error("the file has no drawable geometry")]
    Empty,
}
