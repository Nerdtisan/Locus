// SPDX-License-Identifier: AGPL-3.0-or-later
//! The sketch canvas: owns the document (behind the command API), its
//! render scene, the view and the selection, and handles pointer input.

use crate::app::Status;
use crate::hit::{self, BoxMode, Mods, Selection};
use crate::render::{Emphasis, Frame, Palette};
use crate::scene::Scene;
use crate::settings::{Settings, Units};
use crate::view::View;
use crate::{demo, grid};
use egui::{Color32, PointerButton, Pos2, Sense, Stroke};
use locus_doc::{Command, DocError, Document, Editor, EntityId, Geom};
use locus_geom::{Aabb, Vec2};

/// Pick radius in screen points (converted to mm by the zoom).
const HIT_TOL_PT: f64 = 6.0;
/// A drag shorter than this is a click, not a box.
const MIN_BOX_PT: f32 = 4.0;

pub struct Canvas {
    /// Every document change goes through `Editor::apply` (SPEC §11).
    editor: Editor,
    scene: Scene,
    view: View,
    /// Keep the view fitted to the content, re-fitting when the canvas
    /// resizes, until the user pans or zooms (the web canvas is not at its
    /// final size on the first frame).
    auto_fit: bool,
    fit_size: egui::Vec2,
    drawn: usize,
    /// Selection and hover are UI state, never part of the document (§11).
    selection: Selection,
    hover: Option<EntityId>,
    /// Screen position where a rubber-band box started.
    rubber: Option<Pos2>,
}

impl Default for Canvas {
    fn default() -> Self {
        let mut c = Self {
            editor: Editor::new(Document::new()),
            scene: Scene::default(),
            view: View::default(),
            auto_fit: true,
            fit_size: egui::Vec2::ZERO,
            drawn: 0,
            selection: Selection::default(),
            hover: None,
            rubber: None,
        };
        c.load(demo::sketch());
        c
    }
}

impl Canvas {
    /// Replace the document (new / sample). Clears the undo history.
    pub fn load(&mut self, doc: Document) {
        self.editor = Editor::new(doc);
        self.selection.clear();
        self.rebuild();
        self.request_fit();
    }

    /// Run a command and refresh the derived scene.
    #[allow(dead_code)] // first caller: tools and the properties panel (M1)
    pub fn apply(&mut self, cmd: &Command) -> Result<(), DocError> {
        self.editor.apply(cmd)?;
        self.rebuild();
        Ok(())
    }

    fn rebuild(&mut self) {
        self.scene = Scene::build(self.editor.document());
        let scene = &self.scene;
        self.selection
            .retain(|id| scene.items.iter().any(|i| i.id == id));
        self.hover = None;
    }

    /// World bounds of the content; a sheet-sized default while empty.
    fn bounds(&self) -> (Vec2, Vec2) {
        let (lo, hi) = self
            .scene
            .bounds
            .map_or((Vec2::new(0.0, 0.0), Vec2::new(300.0, 200.0)), |b| {
                (b.min, b.max)
            });
        // Room for dimension lines and glyphs, which sit outside the geometry.
        let pad = (hi - lo) * 0.08;
        (lo - pad, hi + pad)
    }

    pub fn request_fit(&mut self) {
        self.auto_fit = true;
        self.fit_size = egui::Vec2::ZERO;
    }

    pub fn select_all(&mut self) {
        self.selection
            .set_all(self.scene.items.iter().map(|i| i.id));
    }

    pub fn clear_selection(&mut self) {
        self.selection.clear();
    }

    pub fn summary(&self) -> String {
        let n = self.editor.document().entities().len();
        format!(
            "{n} entities · {} drawn · {} selected",
            self.drawn,
            self.selection.len()
        )
    }

    pub fn properties_ui(&mut self, ui: &mut egui::Ui, settings: &Settings) {
        let doc = self.editor.document();
        match self.selection.len() {
            0 => {
                ui.weak("Nothing selected.");
            }
            1 => {
                if let Some(id) = self.selection.iter().next() {
                    let rows = describe(doc, id, settings.units);
                    egui::Grid::new("props").num_columns(2).show(ui, |ui| {
                        for (k, v) in rows {
                            ui.label(k);
                            ui.monospace(v);
                            ui.end_row();
                        }
                    });
                }
            }
            n => {
                ui.label(format!("{n} entities selected"));
                let mut counts = [0usize; 4];
                for id in self.selection.iter() {
                    if let Some(e) = doc.entity(id) {
                        counts[e.geom.kind() as usize] += 1;
                    }
                }
                for (name, c) in ["Points", "Lines", "Circles", "Arcs"].iter().zip(counts) {
                    if c > 0 {
                        ui.label(format!("{name}: {c}"));
                    }
                }
            }
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, settings: &Settings, status: &mut Status) {
        let (resp, painter) = ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
        let rect = resp.rect;
        let dark = ui.visuals().dark_mode;
        let pal = Palette::new(dark);
        painter.rect_filled(rect, 0.0, pal.background);

        if self.auto_fit && rect.size() != self.fit_size && rect.width() > 10.0 {
            let (lo, hi) = self.bounds();
            self.view.fit(lo, hi, rect);
        }
        self.fit_size = rect.size();

        // Zoom about the cursor; middle/right-drag pans.
        let hover = resp.hover_pos();
        if let Some(hp) = hover {
            let (scroll, zoom) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
            let factor = zoom as f64 * (scroll as f64 * 0.0025).exp();
            if factor != 1.0 {
                self.view.zoom_about(hp, factor, rect);
                self.auto_fit = false;
            }
        }
        if resp.dragged_by(PointerButton::Middle) || resp.dragged_by(PointerButton::Secondary) {
            self.view.pan_points(resp.drag_delta());
            self.auto_fit = false;
        }

        self.pick_and_select(ui, &resp, rect);

        let painter = painter.with_clip_rect(rect);
        grid::draw(&painter, &self.view, rect, dark);
        let frame = Frame {
            painter: &painter,
            view: &self.view,
            rect,
            pal,
            units: settings.units,
        };
        let (selection, hovered) = (&self.selection, self.hover);
        self.drawn = frame.draw_items(&self.scene, |item| {
            if selection.contains(item.id) {
                Emphasis::Selected
            } else if hovered == Some(item.id) {
                Emphasis::Hover
            } else {
                Emphasis::None
            }
        });
        frame.draw_annotations(&self.scene);
        if let (Some(start), Some(now)) = (self.rubber, resp.interact_pointer_pos()) {
            draw_rubber_band(&painter, start, now);
        }
        grid::rulers(&painter, &self.view, rect);

        status.cursor = hover.map(|p| self.view.to_world(p, rect));
        status.extent_mm = self.view.extent_mm(rect);
        status.grid = grid::label(grid::minor_step(self.view.pt_per_mm).0);
    }

    /// Hover, click and box selection (#67). Left-drag is a rubber-band box;
    /// pan stays on middle/right-drag.
    fn pick_and_select(&mut self, ui: &egui::Ui, resp: &egui::Response, rect: egui::Rect) {
        let mods = ui.input(|i| Mods {
            add: i.modifiers.shift,
            toggle: i.modifiers.command,
        });
        let tol = HIT_TOL_PT / self.view.pt_per_mm;
        let pick_at = |this: &Self, p: Pos2| {
            let w = this.view.to_world(p, rect);
            hit::pick(&this.scene.items, w, tol)
        };

        if resp.drag_started_by(PointerButton::Primary) {
            self.rubber = resp.interact_pointer_pos();
        }
        let boxing = self.rubber.is_some();
        self.hover = match resp.hover_pos() {
            Some(p) if !boxing => pick_at(self, p),
            _ => None,
        };
        if self.hover.is_some() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }

        if resp.clicked_by(PointerButton::Primary)
            && let Some(p) = resp.interact_pointer_pos()
        {
            self.selection.click(pick_at(self, p), mods);
        }
        if resp.drag_stopped_by(PointerButton::Primary)
            && let Some(start) = self.rubber.take()
            && let Some(end) = resp.interact_pointer_pos()
            && (end - start).length() >= MIN_BOX_PT
        {
            let a = self.view.to_world(start, rect);
            let b = self.view.to_world(end, rect);
            let mode = BoxMode::from_drag(start.x, end.x);
            let hits = hit::box_select(&self.scene.items, Aabb::from_points(a, b), mode);
            self.selection.apply_box(&hits, mods);
        }
        if !resp.dragged_by(PointerButton::Primary) {
            self.rubber = None;
        }

        if resp.hovered() {
            let (esc, all) = ui.input(|i| {
                (
                    i.key_pressed(egui::Key::Escape),
                    i.modifiers.command && i.key_pressed(egui::Key::A),
                )
            });
            if esc {
                self.clear_selection();
            }
            if all {
                self.select_all();
            }
        }
    }
}

/// Window boxes are solid blue, crossing boxes dashed green (Fusion style).
fn draw_rubber_band(painter: &egui::Painter, start: Pos2, now: Pos2) {
    let r = egui::Rect::from_two_pos(start, now);
    let window = BoxMode::from_drag(start.x, now.x) == BoxMode::Window;
    let c = if window {
        Color32::from_rgb(70, 140, 240)
    } else {
        Color32::from_rgb(70, 190, 110)
    };
    painter.rect_filled(r, 0.0, c.gamma_multiply(0.15));
    let stroke = Stroke::new(1.0, c);
    if window {
        painter.rect_stroke(r, 0.0, stroke, egui::StrokeKind::Inside);
    } else {
        let path = [
            r.left_top(),
            r.right_top(),
            r.right_bottom(),
            r.left_bottom(),
            r.left_top(),
        ];
        painter.extend(egui::Shape::dashed_line(&path, stroke, 6.0, 4.0));
    }
}

/// Label/value rows for the properties panel.
fn describe(doc: &Document, id: EntityId, units: Units) -> Vec<(String, String)> {
    let Some(e) = doc.entity(id) else {
        return Vec::new();
    };
    let pt = |p: Vec2| format!("{}, {}", units.format_len(p.x), units.format_len(p.y));
    let kind = match e.geom {
        Geom::Point { .. } => "Point",
        Geom::Line { .. } => "Line",
        Geom::Circle { .. } => "Circle",
        Geom::Arc { .. } => "Arc",
    };
    let mut rows = vec![
        ("ID".to_owned(), id.to_string()),
        ("Type".to_owned(), kind.to_owned()),
    ];
    if let Some(l) = doc.layer(e.layer) {
        rows.push(("Layer".to_owned(), l.name.clone()));
    }
    rows.push((
        "Construction".to_owned(),
        if e.construction { "yes" } else { "no" }.to_owned(),
    ));
    match e.geom {
        Geom::Point { pos } => rows.push(("Position".to_owned(), pt(pos))),
        Geom::Line { .. } => {
            if let Some(l) = doc.line(id) {
                rows.push(("Start".to_owned(), pt(l.a)));
                rows.push(("End".to_owned(), pt(l.b)));
                rows.push(("Length".to_owned(), units.format_len(l.length())));
                let ang = (l.b - l.a).angle().to_degrees();
                rows.push(("Angle".to_owned(), format!("{ang:.2}°")));
            }
        }
        Geom::Circle { .. } => {
            if let Some(c) = doc.circle(id) {
                rows.push(("Centre".to_owned(), pt(c.center)));
                rows.push(("Radius".to_owned(), units.format_len(c.radius)));
                rows.push(("Diameter".to_owned(), units.format_len(c.radius * 2.0)));
            }
        }
        Geom::Arc { .. } => {
            if let Some(a) = doc.arc(id) {
                rows.push(("Centre".to_owned(), pt(a.center)));
                rows.push(("Radius".to_owned(), units.format_len(a.radius())));
                rows.push((
                    "Sweep".to_owned(),
                    format!("{:.2}°", a.sweep().to_degrees()),
                ));
            }
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describe_reports_a_line_and_a_circle() {
        let doc = demo::sketch();
        let line = doc
            .entities()
            .iter()
            .find(|(_, e)| matches!(e.geom, Geom::Line { .. }))
            .map(|(id, _)| id);
        let rows = line
            .map(|id| describe(&doc, id, Units::Mm))
            .unwrap_or_default();
        assert!(rows.iter().any(|(k, _)| k == "Length"));
        let circle = doc
            .entities()
            .iter()
            .find(|(_, e)| matches!(e.geom, Geom::Circle { .. }))
            .map(|(id, _)| id);
        let rows = circle
            .map(|id| describe(&doc, id, Units::Mm))
            .unwrap_or_default();
        assert!(
            rows.iter()
                .any(|(k, v)| k == "Diameter" && v == "40.000 mm")
        );
    }

    #[test]
    fn selection_does_not_touch_the_document() {
        let mut c = Canvas::default();
        let before = c.editor.document().clone();
        c.select_all();
        assert!(c.selection.len() > 1);
        c.clear_selection();
        assert_eq!(c.editor.document(), &before);
    }
}
