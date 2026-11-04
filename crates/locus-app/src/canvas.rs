// SPDX-License-Identifier: AGPL-3.0-or-later
//! The sketch canvas: owns the document (behind the command API), its
//! render scene and the view, and handles zoom/pan input.
//!
//! Picking and selection arrive with #67.

use crate::app::Status;
use crate::render::{Emphasis, Frame, Palette};
use crate::scene::Scene;
use crate::settings::Settings;
use crate::view::View;
use crate::{demo, grid};
use egui::{PointerButton, Sense};
use locus_doc::{Command, DocError, Document, Editor};
use locus_geom::Vec2;

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
        };
        c.load(demo::sketch());
        c
    }
}

impl Canvas {
    /// Replace the document (new / sample). Clears the undo history.
    pub fn load(&mut self, doc: Document) {
        self.editor = Editor::new(doc);
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

    pub fn select_all(&mut self) {}

    pub fn clear_selection(&mut self) {}

    pub fn summary(&self) -> String {
        let n = self.editor.document().entities().len();
        format!("{n} entities · {} drawn", self.drawn)
    }

    pub fn properties_ui(&mut self, ui: &mut egui::Ui, _settings: &Settings) {
        ui.weak("Nothing selected.");
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

        let painter = painter.with_clip_rect(rect);
        grid::draw(&painter, &self.view, rect, dark);
        let frame = Frame {
            painter: &painter,
            view: &self.view,
            rect,
            pal,
            units: settings.units,
        };
        self.drawn = frame.draw_items(&self.scene, |_| Emphasis::None);
        frame.draw_annotations(&self.scene);
        grid::rulers(&painter, &self.view, rect);

        status.cursor = hover.map(|p| self.view.to_world(p, rect));
        status.extent_mm = self.view.extent_mm(rect);
        status.grid = grid::label(grid::minor_step(self.view.pt_per_mm).0);
    }
}
