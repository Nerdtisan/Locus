// SPDX-License-Identifier: AGPL-3.0-or-later
//! The sketch canvas: background, adaptive grid, axes and rulers, plus
//! zoom/pan input. Entity drawing (#66) and picking (#67) build on this.

use crate::app::Status;
use crate::grid;
use crate::math::V2;
use crate::settings::Settings;
use crate::view::View;
use egui::{Color32, PointerButton, Sense};

#[derive(Debug)]
pub struct Canvas {
    view: View,
    /// Keep the view fitted to the content, re-fitting when the canvas
    /// resizes, until the user pans or zooms (the web canvas is not at its
    /// final size on the first frame).
    auto_fit: bool,
    fit_size: egui::Vec2,
}

impl Default for Canvas {
    fn default() -> Self {
        Self {
            view: View::default(),
            auto_fit: true,
            fit_size: egui::Vec2::ZERO,
        }
    }
}

impl Canvas {
    /// World bounds of the content; a sheet-sized default while empty.
    fn bounds(&self) -> (V2, V2) {
        (V2::new(0.0, 0.0), V2::new(300.0, 200.0))
    }

    pub fn request_fit(&mut self) {
        self.auto_fit = true;
        self.fit_size = egui::Vec2::ZERO;
    }

    pub fn select_all(&mut self) {}

    pub fn clear_selection(&mut self) {}

    pub fn summary(&self) -> String {
        "0 entities".to_owned()
    }

    pub fn properties_ui(&mut self, ui: &mut egui::Ui, _settings: &Settings) {
        ui.weak("Nothing selected.");
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, _settings: &Settings, status: &mut Status) {
        let (resp, painter) = ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
        let rect = resp.rect;
        let dark = ui.visuals().dark_mode;
        painter.rect_filled(
            rect,
            0.0,
            if dark {
                Color32::from_gray(24)
            } else {
                Color32::from_gray(250)
            },
        );

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
        grid::rulers(&painter, &self.view, rect);

        status.cursor = hover.map(|p| self.view.to_world(p, rect));
        status.extent_mm = self.view.extent_mm(rect);
        status.grid = grid::label(grid::minor_step(self.view.pt_per_mm).0);
    }
}
