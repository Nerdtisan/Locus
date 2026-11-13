// SPDX-License-Identifier: AGPL-3.0-or-later
//! The command dialog (design §2): docked to the canvas's top-right corner
//! while a dialog tool runs (Move, Rotate, Scale, and later Offset, Pattern,
//! Mirror, Text). It floats over the canvas, so the pointer over it never
//! reaches the canvas.

use egui::{Align2, Rect, RichText};

/// Draw a command dialog titled `title` with `add`'s contents.
pub(crate) fn command_dialog<R>(
    ui: &egui::Ui,
    rect: Rect,
    title: &str,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    egui::Area::new(egui::Id::new("command-dialog"))
        .order(egui::Order::Foreground)
        .anchor(
            Align2::RIGHT_TOP,
            rect.right_top().to_vec2() - ui.ctx().content_rect().right_top().to_vec2()
                + egui::vec2(-12.0, 12.0),
        )
        .show(ui.ctx(), |ui| {
            egui::Frame::window(ui.style())
                .show(ui, |ui| {
                    ui.set_min_width(220.0);
                    ui.label(RichText::new(title.to_uppercase()).strong());
                    ui.separator();
                    add(ui)
                })
                .inner
        })
        .inner
}
