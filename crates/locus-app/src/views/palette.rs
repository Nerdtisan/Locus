// SPDX-License-Identifier: AGPL-3.0-or-later
//! The sketch palette pane (#285, M5 design §2): the linetype for new
//! geometry, display toggles and the active tool's options. Every toggle is
//! a registry action, so the palette, menus and ribbon can't disagree.

use crate::actions::{self, Cx, Item, Registry, Style};

const SKETCH: &[Item] = &[
    Item::Action("sketch.linetype_construction"),
    Item::Action("constrain.auto"),
];

const DISPLAY: &[Item] = &[
    Item::Action("view.machine_preview"),
    Item::Action("view.stroke_width"),
];

pub fn ui(ui: &mut egui::Ui, reg: &Registry, cx: &mut Cx<'_>) {
    egui::CollapsingHeader::new("Sketch")
        .default_open(true)
        .show(ui, |ui| {
            actions::show(ui, reg, cx, SKETCH, Style::Pane);
        });
    egui::CollapsingHeader::new("Display")
        .default_open(true)
        .show(ui, |ui| {
            actions::show(ui, reg, cx, DISPLAY, Style::Pane);
        });
    egui::CollapsingHeader::new("Tool options")
        .default_open(true)
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| cx.state.tool_options_ui(ui));
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_palette_item_is_a_registered_toggle() {
        let reg = Registry::default();
        for item in SKETCH.iter().chain(DISPLAY) {
            if let Item::Action(id) = item {
                let a = reg.get(id);
                assert!(
                    a.is_some_and(|a| a.op.kind() == actions::ActionKind::Toggle),
                    "{id}"
                );
            }
        }
    }
}
