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
use locus_doc::StrokeStyles;
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
    /// For an engrave fill line or ring (#455): its row (hatch lines on the
    /// global grid, rings counted inward) and the spacing, so the preview
    /// can thin out rows too close to see apart.
    pub row: Option<(u64, f64)>,
    /// Follows its object's styled strokes on a Styled layer, not the
    /// centreline: drawn with a halo (owner decision 2026-11-25).
    pub styled: bool,
}

impl MachinePath {
    /// Is this row drawn at `pt_per_mm`? Rows closer than 4 points are
    /// thinned to every k-th, so the spacing reads at any zoom (#455).
    pub(crate) fn shown_at(&self, pt_per_mm: f64) -> bool {
        let Some((row, spacing)) = self.row else {
            return true;
        };
        let gap = spacing * pt_per_mm;
        if !(gap.is_finite() && gap > 0.0) {
            return true;
        }
        let k = (4.0 / gap).ceil().max(1.0);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let k = k as u64;
        row % k == 0
    }
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
        let mut ring = 0u64;
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
            // The engrave fill (#455): hatch lines, or the rings of an
            // offset fill (whose outlines are all rings).
            let fill = match &layer.operation {
                Operation::Engrave(e) => !p.closed || e.mode == locus_doc::EngraveMode::OffsetFill,
                _ => false,
            };
            let row = match &layer.operation {
                Operation::Engrave(e) if fill && e.hatch_spacing > 0.0 => {
                    if p.closed {
                        ring += 1;
                        Some((ring, e.hatch_spacing))
                    } else {
                        // Hatch rows sit on a global grid: the distance from
                        // the origin across the line, in spacings.
                        let (a, b) = (points[0], points[points.len() - 1]);
                        let n = (b - a).normalized(0.0).map(Vec2::perp);
                        n.map(|n| {
                            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                            let r = (a.dot(n) / e.hatch_spacing).round().abs() as u64;
                            (r, e.hatch_spacing)
                        })
                    }
                }
                _ => None,
            };
            let styled = layer.stroke_styles == StrokeStyles::Styled
                && p.segs.first().is_some_and(|s| {
                    layer
                        .styles
                        .get(&s.entity)
                        .is_some_and(|st| st.line.machine_styled())
                });
            out.push(MachinePath {
                points,
                kind,
                hatch: fill,
                band,
                row,
                styled,
            });
        }
        // An engrave fill's outline, which Fill mode doesn't mark: drawn
        // faint like the fill and never thinned, so the area always shows
        // (#531).
        for p in &layer.edges {
            let mut points = Vec::new();
            for s in &p.segs {
                push_seg(s, &mut points);
            }
            if points.len() >= 2 {
                out.push(MachinePath {
                    points,
                    kind,
                    hatch: true,
                    band: 0.0,
                    row: None,
                    styled: false,
                });
            }
        }
    }
    out
}

/// What the preview draws: the exported model, without hidden layers (#503).
/// A hidden layer is still exported (export follows the layer's Export
/// switch, not its eye), so [`layer_notes`] says so instead of drawing it.
pub(crate) fn preview_model(doc: &Document, profile: Option<&MachineProfile>) -> ExportModel {
    let mut model = locus_export::prepare_for(doc, false, profile).model;
    let shown = |id: locus_doc::LayerId| doc.layer(id).is_some_and(|l| l.visible);
    model.layers.retain(|l| shown(l.id));
    model.fills.retain(|f| shown(f.layer));
    model
}

/// Does `layer` hold an object whose line style a Centreline machine layer
/// ignores (caps, joins, dashes, extra strokes, #502)?
fn has_line_styles(doc: &Document, layer: locus_doc::LayerId) -> bool {
    doc.entities()
        .iter()
        .any(|(_, e)| e.layer == layer && !e.construction && e.style.line.machine_styled())
}

/// Per-layer notes the export model can't give (#502, #503): hidden layers
/// that still export, and machine layers on Centreline that ignore their
/// objects' line styles.
pub(crate) fn layer_notes(doc: &Document) -> Vec<String> {
    let mut layers: Vec<_> = doc.layers().iter().collect();
    layers.sort_by_key(|(id, l)| (l.export_order, id.get()));
    let mut out = Vec::new();
    for (id, l) in layers {
        let exported = l.export_enabled && l.operation.is_exportable();
        if !l.visible && exported {
            out.push(format!(
                "{}: hidden, so not shown here, but still exported. Turn its Export off to leave it out.",
                l.name
            ));
        }
        if l.visible
            && is_machine(&l.operation)
            && l.stroke_styles == StrokeStyles::Centreline
            && has_line_styles(doc, *id)
        {
            out.push(format!(
                "{}: cuts centrelines, so its objects' dashes and extra strokes are ignored. Switch it to Styled to cut them.",
                l.name
            ));
        }
    }
    out
}

/// A shown, exported layer that gives the preview nothing says why (#530),
/// so an empty preview never looks like a fault.
pub(crate) fn empty_layer_notes(doc: &Document, model: &ExportModel) -> Vec<String> {
    let mut layers: Vec<_> = doc.layers().iter().collect();
    layers.sort_by_key(|(id, l)| (l.export_order, id.get()));
    layers
        .into_iter()
        .filter(|(_, l)| l.visible && l.export_enabled && l.operation.is_exportable())
        .filter(|(id, _)| {
            !model
                .layers
                .iter()
                .any(|m| m.id == **id && (!m.paths.is_empty() || !m.edges.is_empty()))
        })
        .map(|(_, l)| format!("{}: nothing to show: it has no curves to export.", l.name))
        .collect()
}

fn is_machine(op: &Operation) -> bool {
    matches!(
        op,
        Operation::Cut(_) | Operation::Score | Operation::Engrave(_) | Operation::DragKnife(_)
    )
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
            Operation::Engrave(e) => {
                let what = match e.mode {
                    locus_doc::EngraveMode::Line => "the outline only, no fill".to_owned(),
                    locus_doc::EngraveMode::OffsetFill => format!(
                        "rings {} mm apart, drawn faint",
                        crate::views::machine_profile::trim_mm(e.hatch_spacing)
                    ),
                    m => format!(
                        "{}hatch lines {} mm apart, drawn faint",
                        if m.line() { "the outline and " } else { "" },
                        crate::views::machine_profile::trim_mm(e.hatch_spacing)
                    ),
                };
                out.push(format!(
                    "{}: Engrave {} ({}): {what}, so each region's own fill still shows. Overlaps: {}.",
                    layer.name,
                    e.mode.name(),
                    if e.mode.line() { "edge traced" } else { "no edge" },
                    e.overlap.name().to_lowercase(),
                ));
            }
            _ => {}
        }
    }
    if draw {
        out.push(
            "Draw layers have no machine compensation: shown at each object's pen width."
                .to_owned(),
        );
    }
    if paths(model).iter().any(|p| p.styled) {
        out.push(
            "Paths with a soft halo follow their objects' line styles (dashes perforate, extra strokes cut in parallel); the rest follow the centreline."
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
            let model = preview_model(doc, profile);
            self.view.machine_offset =
                locus_export::prepare::placement(doc, profile).map_or(Vec2::ZERO, |(o, _)| o);
            self.view.machine = paths(&model);
            self.view.machine_notes = notes(profile, &model);
            self.view.machine_notes.extend(layer_notes(doc));
            self.view
                .machine_notes
                .extend(empty_layer_notes(doc, &model));
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
        for p in self.view.machine.iter().filter(|p| p.shown_at(pt_per_mm)) {
            let colour = crate::views::op_chip::colour(p.kind, dark);
            let pts: Vec<Pos2> = p
                .points
                .iter()
                .map(|q| self.view.camera.to_screen(*q - back, rect))
                .collect();
            if let Some(band) = band_px(p, pt_per_mm, self.view.stroke_on) {
                painter.add(egui::Shape::line(
                    pts.clone(),
                    Stroke::new(band, colour.gamma_multiply(0.3)),
                ));
            }
            // The engrave fill is faint, so its spacing reads and the
            // region's own fill shows through (#455).
            let (w, c) = if p.hatch {
                (0.75, colour.gamma_multiply(0.45))
            } else {
                (1.75, colour)
            };
            if p.styled {
                // The halo marks a path that follows its object's styled
                // strokes (owner decision 2026-11-25).
                painter.add(egui::Shape::line(
                    pts.clone(),
                    Stroke::new(w + 5.0, colour.gamma_multiply(0.25)),
                ));
            }
            painter.add(egui::Shape::line(pts, Stroke::new(w, c)));
        }
    }
}

/// The width band drawn under a preview path, in points, or `None`: the true
/// width the machine makes, shown only while **Stroke width** is on (#511),
/// like every other width on the canvas. A kerf is a fraction of a pixel at
/// normal zoom, so it gets at least 6 px to stay visible next to the sketch
/// line (#224); a pen is drawn at its width.
pub(crate) fn band_px(p: &MachinePath, pt_per_mm: f64, stroke_on: bool) -> Option<f32> {
    if !stroke_on || p.band <= 0.0 {
        return None;
    }
    let min = if p.kind == OperationKind::Cut {
        6.0
    } else {
        1.0
    };
    #[allow(clippy::cast_possible_truncation)]
    Some((p.band * pt_per_mm).max(min) as f32)
}

/// One legend line: the operation, then the layer it belongs to.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LegendRow {
    pub kind: OperationKind,
    pub layer: String,
    /// Not exported (Reference, or export turned off): drawn dim.
    pub idle: bool,
    /// Hidden with its eye: not drawn in the preview (#503).
    pub hidden: bool,
    /// The layer, for the row's Stroke styles switch.
    pub id: locus_doc::LayerId,
    /// Machine layers: Centreline or Styled (#502); `None` elsewhere.
    pub stroke_styles: Option<StrokeStyles>,
}

/// The legend's rows: every layer in export order with its operation (#420),
/// shown in the Machine preview pane (#443).
pub(crate) fn legend(doc: &Document) -> Vec<LegendRow> {
    let mut layers: Vec<_> = doc.layers().iter().collect();
    layers.sort_by_key(|(id, l)| (l.export_order, id.get()));
    layers
        .into_iter()
        .map(|(id, l)| {
            let kind = l.operation.kind();
            LegendRow {
                kind,
                layer: l.name.clone(),
                idle: kind == OperationKind::Reference || !l.export_enabled,
                hidden: !l.visible,
                id: *id,
                stroke_styles: is_machine(&l.operation).then_some(l.stroke_styles),
            }
        })
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use locus_doc::{Command, CutParams, Document, Editor, Geom, LayerProps};

    /// A 20 mm line, 3 mm wide, on `layer`.
    fn wide_line(ed: &mut Editor, layer: locus_doc::LayerId, style: locus_doc::Style) {
        let mut pt = |x: f64| {
            ed.apply(&Command::AddEntity {
                layer,
                construction: false,
                style: Default::default(),
                geom: Geom::Point {
                    pos: Vec2::new(x, 0.0),
                },
            })
            .unwrap()
            .entities[0]
        };
        let (a, b) = (pt(0.0), pt(20.0));
        ed.apply(&Command::AddEntity {
            layer,
            construction: false,
            style,
            geom: Geom::Line { start: a, end: b },
        })
        .unwrap();
    }

    /// #503: a hidden layer is not drawn in the preview, with or without a
    /// profile, and the notes say it still exports.
    #[test]
    fn a_hidden_layer_is_not_previewed_but_says_it_exports() {
        let mut ed = Editor::new(Document::new());
        let l = Document::default_layer();
        wide_line(&mut ed, l, locus_doc::Style::default());
        ed.apply(&Command::SetLayerProps {
            id: l,
            props: LayerProps {
                visible: Some(false),
                ..LayerProps::default()
            },
        })
        .unwrap();
        let lb = locus_export::profile::find("lightburn", &[]).unwrap();
        for profile in [None, Some(&lb)] {
            assert!(paths(&preview_model(ed.document(), profile)).is_empty());
        }
        let n = layer_notes(ed.document());
        assert!(
            n.iter()
                .any(|s| s.contains("hidden") && s.contains("still exported")),
            "{n:?}"
        );
        assert!(legend(ed.document())[0].hidden);
    }

    /// #502: a Centreline machine layer with styled objects says it ignores
    /// them; the legend row carries the switch.
    #[test]
    fn a_centreline_layer_says_it_ignores_line_styles() {
        let mut ed = Editor::new(Document::new());
        let cut = ed
            .apply(&Command::AddLayer {
                name: "Cut".to_owned(),
                colour: [255, 0, 0],
                operation: Operation::Cut(CutParams::default()),
            })
            .unwrap()
            .layers[0];
        let mut st = locus_doc::Style {
            width: 3.0,
            ..locus_doc::Style::default()
        };
        st.line.dash = locus_doc::Dash::new(&[8.0, 3.0], 0.0, locus_doc::DashAlign::Exact).ok();
        wide_line(&mut ed, cut, st);
        let n = layer_notes(ed.document());
        assert!(
            n.iter().any(|s| s.contains("Cut: cuts centrelines")),
            "{n:?}"
        );
        let row = legend(ed.document())
            .into_iter()
            .find(|r| r.layer == "Cut")
            .unwrap();
        assert_eq!(row.stroke_styles, Some(StrokeStyles::Centreline));
        // Switched to Styled: no note, and the preview perforates.
        ed.apply(&Command::SetLayerProps {
            id: cut,
            props: LayerProps {
                stroke_styles: Some(StrokeStyles::Styled),
                ..LayerProps::default()
            },
        })
        .unwrap();
        assert!(
            !layer_notes(ed.document())
                .iter()
                .any(|s| s.contains("centrelines"))
        );
        let lb = locus_export::profile::find("lightburn", &[]).unwrap();
        let cuts = paths(&preview_model(ed.document(), Some(&lb)));
        assert!(cuts.len() > 1, "dashes cut separately: {}", cuts.len());
    }

    /// #504: after Expand to geometry the preview shows the expanded
    /// outline: on a Draw layer, and on a Cut layer under both Stroke styles.
    #[test]
    fn the_preview_shows_an_expanded_stroke() {
        for (op, styles) in [
            (Operation::Draw, StrokeStyles::Centreline),
            (
                Operation::Cut(CutParams::default()),
                StrokeStyles::Centreline,
            ),
            (Operation::Cut(CutParams::default()), StrokeStyles::Styled),
        ] {
            let mut ed = Editor::new(Document::new());
            let layer = Document::default_layer();
            ed.apply(&Command::SetLayerProps {
                id: layer,
                props: LayerProps {
                    operation: Some(op),
                    stroke_styles: Some(styles),
                    ..LayerProps::default()
                },
            })
            .unwrap();
            let st = locus_doc::Style {
                width: 3.0,
                ..locus_doc::Style::default()
            };
            wide_line(&mut ed, layer, st);
            let mut app = crate::state::AppState::default();
            app.load(ed.into_document());
            app.set_machine_preview(true);
            let line = app
                .document()
                .entities()
                .iter()
                .find(|(_, e)| matches!(e.geom, Geom::Line { .. }))
                .map(|(i, _)| i)
                .unwrap();
            app.select_entities(&[line]);
            app.expand_selection(locus_doc::expand::DEFAULT_EXPAND_TOL, false);
            assert!(app.document().entity(line).is_none(), "{op:?}: expanded");
            // The outline: a 20 × 3 stadium (round caps), 23 mm long, its
            // arcs flattened for drawing.
            let pts: Vec<Vec2> = app
                .view
                .machine
                .iter()
                .flat_map(|p| p.points.clone())
                .collect();
            assert!(!pts.is_empty(), "{op:?} {styles:?}: nothing previewed");
            let (lo, hi) = pts
                .iter()
                .fold((f64::MAX, f64::MIN), |(l, h), p| (l.min(p.x), h.max(p.x)));
            assert!(
                (lo + 1.5).abs() < 0.01 && (hi - 21.5).abs() < 0.01,
                "{op:?} {styles:?}: {lo}..{hi}"
            );
        }
    }

    /// Owner decision 2026-11-25: on a Styled layer a widened plain line
    /// cuts once on its centreline; a dashed one perforates and is marked.
    #[test]
    fn styled_layers_follow_line_styles_not_width() {
        let mut ed = Editor::new(Document::new());
        let cut = ed
            .apply(&Command::AddLayer {
                name: "Cut".to_owned(),
                colour: [255, 0, 0],
                operation: Operation::Cut(CutParams::default()),
            })
            .unwrap()
            .layers[0];
        ed.apply(&Command::SetLayerProps {
            id: cut,
            props: LayerProps {
                stroke_styles: Some(StrokeStyles::Styled),
                ..LayerProps::default()
            },
        })
        .unwrap();
        let wide = locus_doc::Style {
            width: 5.0,
            ..locus_doc::Style::default()
        };
        wide_line(&mut ed, cut, wide.clone());
        let lb = locus_export::profile::find("lightburn", &[]).unwrap();
        let ps = paths(&preview_model(ed.document(), Some(&lb)));
        assert_eq!(ps.len(), 1, "one centreline cut");
        assert!(!ps[0].styled);
        let mut dashed = wide;
        dashed.line.dash = locus_doc::Dash::new(&[4.0, 2.0], 0.0, locus_doc::DashAlign::Exact).ok();
        let mut ed2 = Editor::new(ed.document().clone());
        let line = ed2
            .document()
            .entities()
            .iter()
            .find(|(_, e)| matches!(e.geom, Geom::Line { .. }))
            .map(|(i, _)| i)
            .unwrap();
        ed2.apply(&Command::SetStyle {
            entities: vec![line],
            patch: locus_doc::StylePatch::all(dashed),
        })
        .unwrap();
        let model = preview_model(ed2.document(), Some(&lb));
        let ps = paths(&model);
        assert!(
            ps.len() > 1 && ps.iter().all(|p| p.styled),
            "perforated and marked"
        );
        assert!(notes(Some(&lb), &model).iter().any(|n| n.contains("halo")));
    }

    /// #511: with Stroke width off the preview draws no width bands, only
    /// the machine paths; with it on, the pen and kerf widths show.
    #[test]
    fn width_bands_follow_the_stroke_width_toggle() {
        let mut ed = Editor::new(Document::new());
        let l = Document::default_layer();
        wide_line(
            &mut ed,
            l,
            locus_doc::Style {
                width: 3.0,
                ..locus_doc::Style::default()
            },
        );
        let ps = paths(&preview_model(ed.document(), None));
        assert_eq!(ps.len(), 1);
        assert!(band_px(&ps[0], 4.0, false).is_none(), "toggle off: no band");
        assert_eq!(
            band_px(&ps[0], 4.0, true),
            Some(12.0),
            "toggle on: 3 mm at 4 pt/mm"
        );
    }

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
                // Line + Fill: the outline is kept too.
                operation: locus_doc::Operation::Engrave(locus_doc::EngraveParams {
                    mode: locus_doc::EngraveMode::LineFill,
                    ..Default::default()
                }),
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

    /// #455: the preview marks the engrave fill (hatch lines, or offset
    /// rings) so it can be drawn faint, keeps the outline only where the
    /// mode traces it, and the legend says what the faint lines are.
    /// #455: hatch rows closer than 4 points are thinned to every k-th on
    /// screen, so the spacing reads; zoomed in, every row shows.
    #[test]
    fn dense_hatch_rows_are_thinned_on_screen() {
        let row = |r: u64| MachinePath {
            points: vec![Vec2::ZERO, Vec2::new(1.0, 0.0)],
            kind: OperationKind::Engrave,
            hatch: true,
            band: 0.0,
            row: Some((r, 0.1)),
            styled: Default::default(),
        };
        // 0.1 mm at 10 pt/mm is 1 pt apart: every 4th row.
        let shown: Vec<u64> = (0..12).filter(|r| row(*r).shown_at(10.0)).collect();
        assert_eq!(shown, vec![0, 4, 8]);
        // At 50 pt/mm, 5 pt apart: all of them.
        assert!((0..12).all(|r| row(r).shown_at(50.0)));
    }

    #[test]
    fn engrave_fill_is_marked_and_explained_per_mode() {
        use locus_doc::{EngraveMode, EngraveParams};
        let doc = |mode: EngraveMode| {
            let mut ed = Editor::new(Document::new());
            let l = Document::default_layer();
            ed.apply(&Command::SetLayerProps {
                id: l,
                props: LayerProps {
                    operation: Some(Operation::Engrave(EngraveParams {
                        hatch_spacing: 1.0,
                        mode,
                        ..EngraveParams::default()
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
                    radius: 5.0,
                },
            })
            .unwrap();
            ed.into_document()
        };
        for (mode, outline, fill) in [
            (EngraveMode::Fill, false, true),
            (EngraveMode::Line, true, false),
            (EngraveMode::LineFill, true, true),
            (EngraveMode::OffsetFill, false, true),
        ] {
            let d = doc(mode);
            let model = locus_export::prepare(&d, false).model;
            let ps = paths(&model);
            assert_eq!(ps.iter().any(|p| !p.hatch), outline, "{mode:?} outline");
            assert_eq!(ps.iter().any(|p| p.hatch), fill, "{mode:?} fill");
            let n = notes(None, &model);
            assert!(
                n.iter()
                    .any(|s| s.contains(mode.name()) && s.contains("still shows")),
                "{mode:?}: {n:?}"
            );
        }
    }

    /// #531: an Engrave layer in Fill mode marks no outline, and its hatch
    /// rows thin out when zoomed out, so a small or narrow fill drew
    /// nothing. Its outline now shows, faint and never thinned, in every
    /// mode that fills.
    #[test]
    fn an_engrave_fill_always_shows_where_it_is() {
        use locus_doc::{EngraveMode, EngraveParams};
        for mode in [
            EngraveMode::Fill,
            EngraveMode::LineFill,
            EngraveMode::OffsetFill,
        ] {
            let mut ed = Editor::new(Document::new());
            let l = Document::default_layer();
            ed.apply(&Command::SetLayerProps {
                id: l,
                props: LayerProps {
                    operation: Some(Operation::Engrave(EngraveParams {
                        hatch_spacing: 1.0,
                        mode,
                        ..EngraveParams::default()
                    })),
                    ..LayerProps::default()
                },
            })
            .unwrap();
            // A circle whose hatch rows (95 … 105) all thin out at a far zoom.
            let c = ed
                .apply(&Command::AddEntity {
                    layer: l,
                    construction: false,
                    style: Default::default(),
                    geom: Geom::Point {
                        pos: Vec2::new(0.0, 100.0),
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
                    radius: 5.0,
                },
            })
            .unwrap();
            let ps = paths(&locus_export::prepare(ed.document(), false).model);
            let far = 0.05;
            assert!(
                ps.iter().any(|p| p.shown_at(far)),
                "{mode:?}: something shows when zoomed out"
            );
            assert!(
                ps.iter().any(|p| p.hatch && p.row.is_some()),
                "{mode:?}: the fill"
            );
        }
    }

    /// #530: a Draw layer is previewed, plain and styled objects alike,
    /// with or without a profile with a bed; an empty one says why.
    #[test]
    fn a_draw_layer_shows_plain_and_styled_objects() {
        let mut ed = Editor::new(Document::new());
        let l = Document::default_layer();
        let pt = |ed: &mut Editor, x: f64, y: f64| {
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
        let (a, b) = (pt(&mut ed, 0.0, 0.0), pt(&mut ed, 40.0, 0.0));
        let (c, d) = (pt(&mut ed, 0.0, 20.0), pt(&mut ed, 40.0, 20.0));
        let mut dashed = locus_doc::Style::default();
        dashed.line.dash = Some(
            locus_stroke::Dash::new(&[3.0, 2.0], 0.0, locus_stroke::DashAlign::Exact).unwrap(),
        );
        for (s, e, style) in [(a, b, locus_doc::Style::default()), (c, d, dashed)] {
            ed.apply(&Command::AddEntity {
                layer: l,
                construction: false,
                style,
                geom: Geom::Line { start: s, end: e },
            })
            .unwrap();
        }
        let bed = locus_export::profile::presets()
            .into_iter()
            .find(|p| p.bed.is_some());
        for profile in [None, bed] {
            let model = preview_model(ed.document(), profile.as_ref());
            let ps = paths(&model);
            assert!(
                ps.iter().filter(|p| p.kind == OperationKind::Draw).count() >= 2,
                "both objects are drawn: {}",
                ps.len()
            );
            assert!(empty_layer_notes(ed.document(), &model).is_empty());
        }
        // An empty Draw layer says why instead of looking broken.
        let empty = Document::new();
        let model = preview_model(&empty, None);
        let n = empty_layer_notes(&empty, &model);
        assert_eq!(n.len(), 1, "{n:?}");
        assert!(n[0].contains("nothing to show"));
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
        // to the origin, so its width is the preview's 40.2 mm span plus the
        // 0.1 mm hairline (half each side, #450), and the hole keeps its
        // 4.9 mm radius about the shifted centre.
        let svg = String::from_utf8(locus_export::to_svg_model(&model)).unwrap();
        assert!(((hi - lo) - 40.2).abs() < 1e-9);
        assert!(svg.contains("width=\"40.3mm\""), "{svg}");
        assert!(svg.contains("cx=\"20.15\" cy=\"15.15\" r=\"4.9\""), "{svg}");
    }
}
