// SPDX-License-Identifier: AGPL-3.0-or-later

//! Parsers for reference geometry: SVG (#241) and DXF (#242), and PNG /
//! JPEG reference images (#390, [`raster`]).
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
mod dxf;
pub mod raster;
mod svg;

use locus_doc::reference::ImportShape;

pub use dxf::import_dxf;
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

    #[error("the file is not a readable ASCII DXF")]
    BadDxf,
    #[error("binary DXF files are not supported; save as ASCII DXF")]
    BinaryDxf,
    #[error("only SVG, ASCII DXF, PNG and JPEG files can be imported")]
    UnknownFormat,
    #[error("the image is damaged, or larger than 16 384 pixels a side or 64 megapixels")]
    BadImage,
    #[error("{0}")]
    Document(String),
    #[error("the file has no drawable geometry")]
    Empty,
}

/// A file format the importers read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Svg,
    Dxf,
}

/// Longest layer name (matches `locus_doc::document::MAX_LAYER_NAME`).
const MAX_LAYER_NAME: usize = 128;

/// The format of a file, by extension first and by content otherwise.
#[must_use]
pub fn detect(file_name: &str, bytes: &[u8]) -> Option<Format> {
    let ext = file_name
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase());
    match ext.as_deref() {
        Some("svg") => return Some(Format::Svg),
        Some("dxf") => return Some(Format::Dxf),
        _ => {}
    }
    let head = &bytes[..bytes.len().min(2048)];
    let head = String::from_utf8_lossy(head);
    if head.contains("<svg") {
        Some(Format::Svg)
    } else if head.contains("SECTION") {
        Some(Format::Dxf)
    } else {
        None
    }
}

/// The one command that imports `bytes` (an SVG or DXF file called
/// `file_name`) as a new locked Reference layer (SPEC §4, §7, §13 M4), plus
/// warnings about what was skipped or approximated. Shared by the GUI's
/// File ▸ Import reference… and the importer plugins' path
/// (`locus_doc::reference`), so both land the same way: one undo step, never
/// exported, never an editable native document (§1 non-goals).
pub fn reference_command(
    doc: &locus_doc::document::Document,
    file_name: &str,
    bytes: &[u8],
) -> Result<(locus_doc::command::Command, Vec<String>), ImportError> {
    reference_command_at(doc, file_name, bytes, locus_geom::Vec2::ZERO)
}

/// As [`reference_command`], placing a PNG or JPEG (found by its
/// signature, #390) centred on `centre`, mm: the middle of the view. Vector
/// files keep their own coordinates.
pub fn reference_command_at(
    doc: &locus_doc::document::Document,
    file_name: &str,
    bytes: &[u8],
    centre: locus_geom::Vec2,
) -> Result<(locus_doc::command::Command, Vec<String>), ImportError> {
    if bytes.len() > MAX_INPUT_BYTES {
        return Err(ImportError::TooLarge);
    }
    if locus_doc::image::ImageFormat::sniff(bytes).is_some() {
        let image = raster::reference_image(bytes, centre)?;
        let cmd = image_command(doc, &layer_name(file_name), image);
        let warning = "The image is placed at 96 dpi: use Calibrate to set its real scale.";
        return Ok((cmd, vec![warning.to_owned()]));
    }
    let imported = match detect(file_name, bytes).ok_or(ImportError::UnknownFormat)? {
        Format::Svg => import_svg(bytes)?,
        Format::Dxf => import_dxf(bytes)?,
    };
    let name = layer_name(file_name);
    let cmd = locus_doc::reference::import_shapes_command(doc, &name, &imported.shapes)
        .map_err(|e| ImportError::Document(e.to_string()))?;
    Ok((cmd, imported.warnings))
}

/// The layer is named after the file, without any directory.
fn layer_name(file_name: &str) -> String {
    let base = file_name.rsplit(['/', '\\']).next().unwrap_or("");
    let name: String = base.chars().take(MAX_LAYER_NAME).collect();
    if name.trim().is_empty() {
        "Reference".to_owned()
    } else {
        name
    }
}

/// A new locked Reference layer holding `image`, as one undo step. The
/// layer's ID is predicted from `Document::next_id`, as
/// `import_shapes_command` does.
fn image_command(
    doc: &locus_doc::document::Document,
    name: &str,
    image: locus_doc::image::RefImage,
) -> locus_doc::command::Command {
    use locus_doc::command::Command;
    let layer = locus_doc::LayerId::new(doc.next_id());
    Command::Compound {
        label: format!("Import {name}"),
        commands: vec![
            Command::AddLayer {
                name: name.to_owned(),
                colour: locus_doc::reference::REFERENCE_COLOUR,
                operation: locus_doc::Operation::Reference,
            },
            Command::AddImage {
                layer,
                image: Box::new(image),
            },
            // Locked last, as vector references are.
            Command::SetLayerLocked {
                id: layer,
                locked: true,
            },
        ],
    }
}
