// SPDX-License-Identifier: AGPL-3.0-or-later
//! The sketch palette pane (M5 design §2, #331), laid out as the approved
//! mockup: Linetype Normal | Construction, the active tool's options, then
//! two columns of checkboxes (Sketch grid, Snap, Auto-constrain, Show
//! profiles / points / dimensions / constraints / construction, Stroke
//! width). Every control is a registry toggle, so the palette, the View
//! menu and the navigation bar can't disagree (design §2.4, §7.1).

use crate::actions::{Cx, Registry};
use crate::views::canvas::dialog;

/// The checkboxes, in the mockup's order (read across, two per row), with
/// the palette's own labels.
const CHECKS: &[(&str, &str)] = &[
    ("view.grid", "Sketch grid"),
    ("view.snap_grid", "Snap"),
    ("constrain.auto", "Auto-constrain"),
    ("view.profiles", "Show profiles"),
    ("view.points", "Show points"),
    ("view.dimensions", "Show dimensions"),
    ("view.constraints", "Show constraints"),
    ("view.construction", "Show construction"),
    ("view.stroke_width", "Stroke width"),
];

/// The linetype switch's action: on means construction (design §6.10).
const LINETYPE: &str = "sketch.linetype_construction";

pub fn ui(ui: &mut egui::Ui, reg: &Registry, cx: &mut Cx<'_>) {
    if let Some(a) = reg.get(LINETYPE) {
        let construction = reg.is_on(a, cx.state, cx.settings);
        dialog::row(ui, "Linetype", |ui| {
            let picked = dialog::segmented(
                ui,
                construction,
                &[(false, "Normal"), (true, "Construction")],
            );
            if picked.is_some() {
                reg.run(a, cx);
            }
        });
    }
    // The active tool's options (polygon sides, slot width…).
    ui.horizontal_wrapped(|ui| cx.state.tool_options_ui(ui));
    // Two columns as in the mockup; one when the pane is too narrow.
    let cols = if ui.available_width() < 250.0 { 1 } else { 2 };
    egui::Grid::new("palette-checks")
        .num_columns(cols)
        .spacing([10.0, 2.0])
        .show(ui, |ui| {
            for (i, (id, label)) in CHECKS.iter().enumerate() {
                if let Some(a) = reg.get(id) {
                    let mut on = reg.is_on(a, cx.state, cx.settings);
                    let hover = if a.hint.is_empty() {
                        a.label.to_string()
                    } else {
                        a.hint.to_string()
                    };
                    if ui.checkbox(&mut on, *label).on_hover_text(hover).changed() {
                        reg.run(a, cx);
                    }
                }
                if i % cols == cols - 1 {
                    ui.end_row();
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actions::ActionKind;

    #[test]
    fn every_palette_item_is_a_registered_toggle() {
        let reg = Registry::default();
        for id in CHECKS.iter().map(|(id, _)| *id).chain([LINETYPE]) {
            assert!(
                reg.get(id)
                    .is_some_and(|a| a.op.kind() == ActionKind::Toggle),
                "{id}"
            );
        }
    }
}
