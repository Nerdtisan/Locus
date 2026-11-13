// SPDX-License-Identifier: AGPL-3.0-or-later
//! Machine bed outline (#237, SPEC §7): the selected profile's bed drawn on
//! the canvas so geometry past the edge is visible before export.

use crate::settings::Settings;
use crate::state::AppState;
use egui::{Color32, Painter, Pos2, Rect, Stroke, StrokeKind};
use locus_geom::Vec2;

impl AppState {
    /// Draw the document's profile bed, if it has one. The origin corner is
    /// marked with a small filled square.
    pub(crate) fn draw_bed(&self, painter: &Painter, rect: Rect, dark: bool, settings: &Settings) {
        let Some(id) = self.doc.sketcher.document().profile() else {
            return;
        };
        let Some(bed) = locus_export::profile::find(id, &settings.profiles).and_then(|p| p.bed)
        else {
            return;
        };
        let b = bed.rect();
        let a = self
            .view
            .camera
            .to_screen(Vec2::new(b.min.x, b.max.y), rect);
        let c = self
            .view
            .camera
            .to_screen(Vec2::new(b.max.x, b.min.y), rect);
        let screen = Rect::from_two_pos(a, c);
        let colour = if dark {
            Color32::from_rgb(120, 200, 255)
        } else {
            Color32::from_rgb(0, 110, 190)
        };
        painter.rect_stroke(screen, 0.0, Stroke::new(1.5, colour), StrokeKind::Middle);
        let origin: Pos2 = self.view.camera.to_screen(bed.origin_point(), rect);
        painter.rect_filled(
            Rect::from_center_size(origin, egui::vec2(6.0, 6.0)),
            0.0,
            colour,
        );
    }

    /// Show the pen width / kerf of Draw and Cut layers at true scale.
    pub fn stroke_preview(&self) -> bool {
        self.view.stroke_on
    }

    pub fn set_stroke_preview(&mut self, on: bool) {
        self.view.stroke_on = on;
    }

    pub(crate) fn draw_stroke_preview(&self, frame: &crate::render::Frame<'_>, dark: bool) {
        if !self.view.stroke_on {
            return;
        }
        let doc = self.doc.sketcher.document();
        let colour = if dark {
            Color32::from_rgba_unmultiplied(255, 190, 80, 90)
        } else {
            Color32::from_rgba_unmultiplied(230, 120, 0, 90)
        };
        frame.draw_strokes(&self.doc.scene, |id| doc.stroke_width(id), colour);
    }
}
