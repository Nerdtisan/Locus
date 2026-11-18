// SPDX-License-Identifier: AGPL-3.0-or-later
//! Machine preview (#176, #423, SPEC §7, §8): every layer's machine result,
//! drawn over the sketch from the same `prepare_for` call the exporters use
//! with the document's machine profile, so what is shown is what will be
//! cut: kerf-compensated Cut paths with their kerf band (#224), blade-offset
//! Drag-knife paths, Engrave hatch, Score hairlines and Draw strokes at each
//! object's pen width, each tinted by its operation (#420). When the preview
//! adds nothing to the sketch, its legend says why.
//!
//! #423: the preview used to draw only layers whose output differs from the
//! sketch (a compensated Cut, a Drag knife with an offset, Engrave), from a
//! model prepared without the profile. A new document has one Draw layer and
//! Cut's default kerf is 0, so turning it on changed nothing and said nothing.

use crate::state::AppState;
use egui::{Painter, Pos2, Rect, Stroke};
use locus_doc::{Compensation, Document, Operation, OperationKind};
use locus_export::{ExportModel, MachineProfile, PathSeg, Shape};
use locus_geom::{EXPORT_TOL, Vec2};

/// One polyline to draw: its operation, its true width and whether it is a
/// hatch line (drawn thinner).
#[derive(Debug, Clone)]
pub(crate) struct MachinePath {
    pub points: Vec<Vec2>,
    pub kind: OperationKind,
    pub hatch: bool,
    /// The width the machine makes, mm, drawn as a band of that true width:
    /// the kerf on a Cut layer (#224), the pen on a Draw layer; 0 for a
    /// hairline.
    pub band: f64,
}

/// Arc flattening step for the preview, radians.
const STEP: f64 = 0.05;

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
            Shape::Cubic(c) => pts.extend(c.flatten(EXPORT_TOL)),
        }
    }
    if s.reversed {
        pts.reverse();
    }
    out.extend(pts);
}

/// The preview polylines of a prepared model: every exported layer.
pub(crate) fn paths(model: &ExportModel) -> Vec<MachinePath> {
    let mut out = Vec::new();
    for layer in &model.layers {
        let kind = layer.operation.kind();
        if kind == OperationKind::Reference {
            continue;
        }
        for p in &layer.paths {
            let mut points = Vec::new();
            for s in &p.segs {
                push_seg(s, &mut points);
            }
            if points.len() < 2 {
                continue;
            }
            let band = match &layer.operation {
                Operation::Cut(c) => c.kerf_width,
                Operation::Draw => layer.paint(p).width,
                _ => 0.0,
            };
            out.push(MachinePath {
                points,
                kind,
                hatch: kind == OperationKind::Engrave && !p.closed,
                band,
            });
        }
    }
    out
}

/// Why the preview may look like the sketch, for the legend (#423).
pub(crate) fn notes(profile: Option<&MachineProfile>, model: &ExportModel) -> Vec<String> {
    let mut out = Vec::new();
    if profile.is_none() {
        out.push(
            "No machine profile set: choose one next to the document name for its bed and hairlines."
                .to_owned(),
        );
    }
    if model.layers.iter().all(|l| l.paths.is_empty()) {
        out.push("Nothing to show: no exported layer has geometry.".to_owned());
        return out;
    }
    let mut draw = false;
    for layer in model.layers.iter().filter(|l| !l.paths.is_empty()) {
        match &layer.operation {
            Operation::Cut(c) if c.compensation == Compensation::None || c.kerf_width <= 0.0 => {
                out.push(format!(
                    "{}: no kerf compensation, so the cut follows the sketch.",
                    layer.name
                ));
            }
            Operation::DragKnife(k) if k.blade_offset <= 0.0 => out.push(format!(
                "{}: blade offset 0, so the knife follows the sketch.",
                layer.name
            )),
            Operation::Draw => draw = true,
            _ => {}
        }
    }
    if draw {
        out.push(
            "Draw layers have no machine compensation: shown at each object's pen width."
                .to_owned(),
        );
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

    /// Keep the preview's profile in step with the document and settings.
    /// The canvas calls this every frame; it recomputes only on a change.
    pub(crate) fn sync_machine_profile(&mut self, settings: &crate::settings::Settings) {
        let p = crate::views::machine_profile::current(self.document(), settings);
        if p != self.view.machine_profile {
            self.view.machine_profile = p;
            self.refresh_machine();
        }
    }

    /// Recompute the preview from the document (after every change).
    pub(crate) fn refresh_machine(&mut self) {
        if self.view.machine_on {
            let doc = self.doc.sketcher.document();
            let profile = self.view.machine_profile.as_ref();
            let model = locus_export::prepare_for(doc, false, profile).model;
            self.view.machine_offset =
                locus_export::prepare::placement(doc, profile).map_or(Vec2::ZERO, |(o, _)| o);
            self.view.machine = paths(&model);
            self.view.machine_notes = notes(profile, &model);
        } else {
            self.view.machine = Vec::new();
            self.view.machine_notes = Vec::new();
            self.view.machine_offset = Vec2::ZERO;
        }
    }

    pub(crate) fn draw_machine(&self, painter: &Painter, rect: Rect, dark: bool) {
        if !self.view.machine_on || self.tools.dragging {
            return;
        }
        let pt_per_mm = self.view.camera.pt_per_mm;
        // The paths are placed on the bed (#443); the canvas is the sketch.
        let back = self.view.machine_offset;
        for p in &self.view.machine {
            let colour = crate::views::op_chip::colour(p.kind, dark);
            let pts: Vec<Pos2> = p
                .points
                .iter()
                .map(|q| self.view.camera.to_screen(*q - back, rect))
                .collect();
            // The true width the machine makes. A kerf is a fraction of a
            // pixel at normal zoom, so it gets at least 6 px to stay visible
            // next to the sketch line (#224); a pen is drawn at its width.
            if p.band > 0.0 {
                let min = if p.kind == OperationKind::Cut {
                    6.0
                } else {
                    1.0
                };
                let band = (p.band * pt_per_mm).max(min) as f32;
                painter.add(egui::Shape::line(
                    pts.clone(),
                    Stroke::new(band, colour.gamma_multiply(0.3)),
                ));
            }
            let w = if p.hatch { 0.75 } else { 1.75 };
            painter.add(egui::Shape::line(pts, Stroke::new(w, colour)));
        }
    }
}

/// One legend line: the operation, then the layer it belongs to.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LegendRow {
    pub kind: OperationKind,
    pub layer: String,
    /// Not exported (Reference, or export turned off): drawn dim.
    pub idle: bool,
}

/// The legend's rows: every layer in export order with its operation (#420),
/// shown in the Machine preview pane (#443).
pub(crate) fn legend(doc: &Document) -> Vec<LegendRow> {
    let mut layers: Vec<_> = doc.layers().iter().collect();
    layers.sort_by_key(|(id, l)| (l.export_order, id.get()));
    layers
        .into_iter()
        .map(|(_, l)| {
            let kind = l.operation.kind();
            LegendRow {
                kind,
                layer: l.name.clone(),
                idle: kind == OperationKind::Reference || !l.export_enabled,
            }
        })
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use locus_doc::{Command, CutParams, Document, Editor, Geom, LayerProps};

    /// #420: the legend lists every layer's operation in export order and
    /// dims the ones that are not exported.
    #[test]
    fn legend_names_each_layers_operation() {
        let mut ed = Editor::new(Document::new());
        ed.apply(&Command::AddLayer {
            name: "Trace".to_owned(),
            colour: [0; 3],
            operation: Operation::Reference,
        })
        .unwrap();
        let rows = legend(ed.document());
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].kind, OperationKind::Draw);
        assert!(!rows[0].idle);
        assert_eq!(rows[1].layer, "Trace");
        assert!(rows[1].idle);
    }

    /// #423: a new document (one Draw layer, no profile) shows its strokes
    /// at their pen width and says why nothing is compensated.
    #[test]
    fn a_new_document_shows_its_draw_strokes_and_says_why() {
        let mut ed = Editor::new(Document::new());
        let l = Document::default_layer();
        let c = ed
            .apply(&Command::AddEntity {
                layer: l,
                construction: false,
                style: Default::default(),
                geom: Geom::Point {
                    pos: Vec2::new(0.0, 0.0),
                },
            })
            .unwrap()
            .entities[0];
        ed.apply(&Command::AddEntity {
            layer: l,
            construction: false,
            style: Default::default(),
            geom: Geom::Circle {
                center: c,
                radius: 10.0,
            },
        })
        .unwrap();
        let model = locus_export::prepare(ed.document(), false).model;
        let ps = paths(&model);
        assert_eq!(ps.len(), 1, "the Draw layer is shown, not skipped");
        assert_eq!(ps[0].kind, OperationKind::Draw);
        let pen = locus_doc::Style::default().width;
        assert!((ps[0].band - pen).abs() < 1e-12, "drawn at the pen width");
        let n = notes(None, &model);
        assert!(n.iter().any(|s| s.contains("No machine profile")), "{n:?}");
        assert!(n.iter().any(|s| s.contains("Draw layers")), "{n:?}");
        // With a profile, no profile note.
        let lb = locus_export::profile::find("lightburn", &[]).unwrap();
        assert!(
            !notes(Some(&lb), &model)
                .iter()
                .any(|s| s.contains("No machine profile"))
        );
        // An empty document says there is nothing to show.
        let empty = locus_export::prepare(&Document::new(), false).model;
        assert!(paths(&empty).is_empty());
        assert!(
            notes(Some(&lb), &empty)
                .iter()
                .any(|s| s.contains("Nothing to show"))
        );
    }

    /// #423: drag-knife and engrave layers show their machine paths: the
    /// blade-offset path and the hatch lines, tinted by operation.
    #[test]
    fn knife_and_engrave_show_their_machine_paths() {
        let square = |ed: &mut Editor, l: locus_doc::LayerId, x0: f64| {
            let pts: Vec<_> = [(0.0, 0.0), (20.0, 0.0), (20.0, 20.0), (0.0, 20.0)]
                .iter()
                .map(|(x, y)| {
                    ed.apply(&Command::AddEntity {
                        layer: l,
                        construction: false,
                        style: Default::default(),
                        geom: Geom::Point {
                            pos: Vec2::new(x0 + x, *y),
                        },
                    })
                    .unwrap()
                    .entities[0]
                })
                .collect();
            for i in 0..4 {
                ed.apply(&Command::AddEntity {
                    layer: l,
                    construction: false,
                    style: Default::default(),
                    geom: Geom::Line {
                        start: pts[i],
                        end: pts[(i + 1) % 4],
                    },
                })
                .unwrap();
            }
        };
        let mut ed = Editor::new(Document::new());
        let knife = Document::default_layer();
        ed.apply(&Command::SetLayerProps {
            id: knife,
            props: LayerProps {
                operation: Some(OperationKind::DragKnife.default_operation()),
                ..LayerProps::default()
            },
        })
        .unwrap();
        square(&mut ed, knife, 0.0);
        let engrave = ed
            .apply(&Command::AddLayer {
                name: "Fill".to_owned(),
                colour: [0; 3],
                operation: OperationKind::Engrave.default_operation(),
            })
            .unwrap()
            .layers[0];
        square(&mut ed, engrave, 50.0);
        let model = locus_export::prepare(ed.document(), false).model;
        let ps = paths(&model);
        let k: Vec<_> = ps
            .iter()
            .filter(|p| p.kind == OperationKind::DragKnife)
            .collect();
        assert!(!k.is_empty());
        // Swivel arcs at the corners add points beyond the square's four.
        assert!(k.iter().map(|p| p.points.len()).sum::<usize>() > 5);
        let hatch = ps
            .iter()
            .filter(|p| p.kind == OperationKind::Engrave && p.hatch)
            .count();
        assert!(hatch > 3, "hatch lines: {hatch}");
        assert!(
            ps.iter()
                .any(|p| p.kind == OperationKind::Engrave && !p.hatch),
            "outline kept"
        );
    }

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
                style: Default::default(),
                geom: Geom::Point {
                    pos: Vec2::new(0.0, 0.0),
                },
            })
            .unwrap()
            .entities[0];
        ed.apply(&Command::AddEntity {
            layer: l,
            construction: false,
            style: Default::default(),
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
        assert!((ps[0].band - 1.0).abs() < 1e-12, "the kerf band");
        assert_eq!(ps[0].kind, OperationKind::Cut);
    }

    /// #224: the preview is drawn from the very model the exporters write, so
    /// with a 0.2 mm kerf the outer contour sits 0.1 mm outside the sketch
    /// and a hole 0.1 mm inside its edge, and the SVG carries the same path.
    #[test]
    fn kerf_preview_matches_the_exported_geometry() {
        let mut ed = Editor::new(Document::new());
        let l = Document::default_layer();
        ed.apply(&Command::SetLayerProps {
            id: l,
            props: LayerProps {
                operation: Some(Operation::Cut(CutParams {
                    kerf_width: 0.2,
                    compensation: Compensation::Outside,
                    passes: 1,
                })),
                ..LayerProps::default()
            },
        })
        .unwrap();
        let point = |ed: &mut Editor, x: f64, y: f64| {
            ed.apply(&Command::AddEntity {
                layer: l,
                construction: false,
                style: Default::default(),
                geom: Geom::Point {
                    pos: Vec2::new(x, y),
                },
            })
            .unwrap()
            .entities[0]
        };
        let corners =
            [(0.0, 0.0), (40.0, 0.0), (40.0, 30.0), (0.0, 30.0)].map(|(x, y)| point(&mut ed, x, y));
        for i in 0..4 {
            ed.apply(&Command::AddEntity {
                layer: l,
                construction: false,
                style: Default::default(),
                geom: Geom::Line {
                    start: corners[i],
                    end: corners[(i + 1) % 4],
                },
            })
            .unwrap();
        }
        let centre = point(&mut ed, 20.0, 15.0);
        ed.apply(&Command::AddEntity {
            layer: l,
            construction: false,
            style: Default::default(),
            geom: Geom::Circle {
                center: centre,
                radius: 5.0,
            },
        })
        .unwrap();
        let model = locus_export::prepare(ed.document(), false).model;
        let ps = paths(&model);
        assert_eq!(ps.len(), 2, "outline and hole");
        assert!(ps.iter().all(|p| (p.band - 0.2).abs() < 1e-12));
        let all: Vec<Vec2> = ps.iter().flat_map(|p| p.points.iter().copied()).collect();
        let (lo, hi) = all
            .iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), p| {
                (lo.min(p.x), hi.max(p.x))
            });
        assert!(
            (lo + 0.1).abs() < 1e-9 && (hi - 40.1).abs() < 1e-9,
            "{lo} {hi}"
        );
        let hole: Vec<&Vec2> = all
            .iter()
            .filter(|p| p.distance(Vec2::new(20.0, 15.0)) < 6.0)
            .collect();
        assert!(!hole.is_empty());
        assert!(
            hole.iter()
                .all(|p| (p.distance(Vec2::new(20.0, 15.0)) - 4.9).abs() < 1e-9)
        );
        // Same model, same numbers in the exported file: the SVG is shifted
        // to the origin, so its 40.2 mm width is the preview's span and the
        // hole keeps its 4.9 mm radius about the shifted centre.
        let svg = String::from_utf8(locus_export::to_svg_model(&model)).unwrap();
        assert!(((hi - lo) - 40.2).abs() < 1e-9);
        assert!(svg.contains("width=\"40.2mm\""), "{svg}");
        assert!(svg.contains("cx=\"20.1\" cy=\"15.1\" r=\"4.9\""), "{svg}");
    }
}
