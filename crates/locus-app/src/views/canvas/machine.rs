// SPDX-License-Identifier: AGPL-3.0-or-later
//! Machine preview (#176, SPEC §7, §8): the kerf- and blade-compensated
//! paths and hatch fills, drawn over the sketch from the same `prepare` call
//! the exporters use, so what is shown is what will be cut.

use crate::state::AppState;
use egui::{Color32, Painter, Pos2, Rect, Stroke};
use locus_doc::{Compensation, Operation};
use locus_export::{ExportModel, PathSeg, Shape};
use locus_geom::Vec2;

/// One polyline to draw, and whether it is a hatch line (drawn thinner).
#[derive(Debug, Clone, Default)]
pub(crate) struct MachinePath {
    pub points: Vec<Vec2>,
    pub hatch: bool,
}

/// Arc flattening step for the preview, radians.
const STEP: f64 = 0.05;

/// Layers whose output differs from the sketch (SPEC §7).
fn transformed(op: &Operation) -> bool {
    match op {
        Operation::Cut(p) => p.compensation != Compensation::None && p.kerf_width > 0.0,
        Operation::DragKnife(p) => p.blade_offset > 0.0,
        Operation::Engrave(_) => true,
        _ => false,
    }
}

fn push_seg(s: &PathSeg, out: &mut Vec<Vec2>) {
    let mut pts: Vec<Vec2> = Vec::new();
    for piece in s.shape.pieces() {
        match piece {
            Shape::Line(l) => pts.extend([l.a, l.b]),
            Shape::Arc(a) => {
                let n = (a.sweep() / STEP).ceil().max(1.0) as usize;
                pts.extend((0..=n).map(|i| a.eval(i as f64 / n as f64)));
            }
            Shape::Circle(c) => {
                let n = (core::f64::consts::TAU / STEP).ceil() as usize;
                pts.extend((0..=n).map(|i| {
                    c.center
                        + Vec2::from_angle(i as f64 / n as f64 * core::f64::consts::TAU) * c.radius
                }));
            }
            Shape::Cubic(_) => {}
        }
    }
    if s.reversed {
        pts.reverse();
    }
    out.extend(pts);
}

/// The preview polylines of a prepared model.
pub(crate) fn paths(model: &ExportModel) -> Vec<MachinePath> {
    let mut out = Vec::new();
    for layer in model.layers.iter().filter(|l| transformed(&l.operation)) {
        let engrave = matches!(layer.operation, Operation::Engrave(_));
        for p in &layer.paths {
            let mut points = Vec::new();
            for s in &p.segs {
                push_seg(s, &mut points);
            }
            if points.len() >= 2 {
                out.push(MachinePath {
                    points,
                    hatch: engrave && !p.closed,
                });
            }
        }
    }
    out
}

impl AppState {
    /// Show the machine preview (View menu).
    pub fn machine_preview(&self) -> bool {
        self.view.machine_on
    }

    pub fn set_machine_preview(&mut self, on: bool) {
        self.view.machine_on = on;
        self.refresh_machine();
    }

    /// Recompute the preview from the document (after every change).
    pub(crate) fn refresh_machine(&mut self) {
        self.view.machine = if self.view.machine_on {
            paths(&locus_export::prepare(self.doc.sketcher.document(), false).model)
        } else {
            Vec::new()
        };
    }

    pub(crate) fn draw_machine(&self, painter: &Painter, rect: Rect, dark: bool) {
        if !self.view.machine_on || self.tools.dragging {
            return;
        }
        let colour = if dark {
            Color32::from_rgb(255, 120, 220)
        } else {
            Color32::from_rgb(200, 0, 150)
        };
        for p in &self.view.machine {
            let pts: Vec<Pos2> = p
                .points
                .iter()
                .map(|q| self.view.camera.to_screen(*q, rect))
                .collect();
            let w = if p.hatch { 0.75 } else { 1.5 };
            painter.add(egui::Shape::line(pts, Stroke::new(w, colour)));
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use locus_doc::{Command, CutParams, Document, Editor, Geom, LayerProps};

    #[test]
    fn preview_comes_from_the_compensated_model() {
        let mut ed = Editor::new(Document::new());
        let l = Document::default_layer();
        ed.apply(&Command::SetLayerProps {
            id: l,
            props: LayerProps {
                operation: Some(Operation::Cut(CutParams {
                    kerf_width: 1.0,
                    compensation: Compensation::Outside,
                    passes: 1,
                })),
                ..LayerProps::default()
            },
        })
        .unwrap();
        let c = ed
            .apply(&Command::AddEntity {
                layer: l,
                construction: false,
                geom: Geom::Point {
                    pos: Vec2::new(0.0, 0.0),
                },
            })
            .unwrap()
            .entities[0];
        ed.apply(&Command::AddEntity {
            layer: l,
            construction: false,
            geom: Geom::Circle {
                center: c,
                radius: 10.0,
            },
        })
        .unwrap();
        let model = locus_export::prepare(ed.document(), false).model;
        let ps = paths(&model);
        assert_eq!(ps.len(), 1);
        // The kerf-grown circle (r 10.5), not the sketch's.
        assert!(
            ps[0]
                .points
                .iter()
                .all(|p| (p.length() - 10.5).abs() < 1e-9)
        );
        // A layer without compensation shows nothing extra.
        let mut plain = model.clone();
        plain.layers[0].operation = Operation::Cut(CutParams::default());
        assert!(paths(&plain).is_empty());
    }
}
