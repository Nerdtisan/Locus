// SPDX-License-Identifier: AGPL-3.0-or-later

//! Exporter plugins in the `locus-export` registry (#184, §15.2, ADR 0009):
//! a plugin exporter receives exactly what the built-in SVG, DXF and PDF
//! writers receive (the validated, ordered export model) and returns bytes.
//! The host does the file dialog and the write (§15.4).

use locus_export::{ExportError, Exported, Exporter};

use crate::error::CallError;
use crate::runtime::Plugin;
use crate::types::{
    Compensation, ExportModel, FormatInfo, Layer, Operation, Path, Segment, Shape, Vec2,
};

/// A loaded exporter plugin, usable wherever a built-in exporter is.
#[derive(Debug)]
pub struct PluginExporter {
    plugin: Plugin,
    format: FormatInfo,
}

impl PluginExporter {
    /// Asks the plugin for its format; fails if it isn't an exporter.
    pub fn new(plugin: Plugin) -> Result<PluginExporter, CallError> {
        let format = plugin.export_format()?;
        Ok(PluginExporter { plugin, format })
    }

    pub fn plugin(&self) -> &Plugin {
        &self.plugin
    }
}

impl Exporter for PluginExporter {
    fn id(&self) -> &str {
        &self.format.id
    }

    fn name(&self) -> &str {
        &self.format.name
    }

    fn extension(&self) -> &str {
        &self.format.extension
    }

    fn mime(&self) -> &str {
        &self.format.mime
    }

    fn export(&self, model: &locus_export::ExportModel) -> Result<Exported, ExportError> {
        let bytes = self.plugin.export(&to_model(model)).map_err(|e| match e {
            CallError::Plugin(crate::types::PluginFailure::Unsupported(m)) => {
                ExportError::Unsupported(m)
            }
            other => ExportError::Failed(other.to_string()),
        })?;
        Ok(Exported {
            bytes,
            extension: self.format.extension.clone(),
            mime: self.format.mime.clone(),
        })
    }
}

fn v(p: locus_geom::Vec2) -> Vec2 {
    Vec2 { x: p.x, y: p.y }
}

fn shape(s: &locus_export::Shape) -> Shape {
    match s {
        locus_export::Shape::Line(l) => Shape::Line {
            a: v(l.a),
            b: v(l.b),
        },
        locus_export::Shape::Arc(a) => Shape::Arc {
            center: v(a.center),
            start: v(a.start),
            end: v(a.end),
        },
        locus_export::Shape::Circle(c) => Shape::Circle {
            center: v(c.center),
            radius: c.radius,
        },
        // `pieces` never returns a cubic; keep the end points as a line.
        locus_export::Shape::Cubic(c) => Shape::Line {
            a: v(c.p0),
            b: v(c.p1),
        },
    }
}

/// The export model in plugin terms. Béziers become the same tangent arcs
/// and lines the DXF writer uses (`Shape::pieces`), since the plugin
/// interface has no cubic (ADR 0009).
#[must_use]
pub fn to_model(m: &locus_export::ExportModel) -> ExportModel {
    ExportModel {
        layers: m
            .layers
            .iter()
            .map(|l| Layer {
                name: l.name.clone(),
                colour: u32::from_be_bytes([0, l.colour[0], l.colour[1], l.colour[2]]),
                operation: operation(&l.operation),
                paths: l
                    .paths
                    .iter()
                    .map(|p| Path {
                        segments: p
                            .segs
                            .iter()
                            .flat_map(|s| {
                                let pieces = s.shape.pieces();
                                let reversed = s.reversed;
                                let ordered: Vec<_> = if reversed {
                                    pieces.into_iter().rev().collect()
                                } else {
                                    pieces
                                };
                                ordered.into_iter().map(move |piece| Segment {
                                    shape: shape(&piece),
                                    reversed,
                                })
                            })
                            .collect(),
                        closed: p.closed,
                    })
                    .collect(),
            })
            .collect(),
    }
}

fn operation(o: &locus_doc::Operation) -> Operation {
    use locus_doc::Operation as D;
    match o {
        D::Cut(c) => Operation::Cut {
            kerf_width: c.kerf_width,
            compensation: match c.compensation {
                locus_doc::Compensation::None => Compensation::None,
                locus_doc::Compensation::Inside => Compensation::Inside,
                locus_doc::Compensation::Outside => Compensation::Outside,
            },
            passes: c.passes,
        },
        D::Score => Operation::Score,
        D::Engrave(e) => Operation::Engrave {
            hatch_spacing: e.hatch_spacing,
            hatch_angle: e.hatch_angle,
        },
        D::Draw(d) => Operation::Draw {
            pen_number: d.pen_number,
            pen_width: d.pen_width,
        },
        D::DragKnife(k) => Operation::DragKnife {
            blade_offset: k.blade_offset,
        },
        D::Reference => Operation::Reference,
    }
}
