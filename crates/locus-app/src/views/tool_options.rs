// SPDX-License-Identifier: AGPL-3.0-or-later
//! The active tool's options in the tool strip: offset, polygon sides and
//! text settings. Move, Rotate, Scale, Fillet and Chamfer have their own
//! command dialog on the canvas (design §6.6–6.7, #217).

use crate::modify_tool::Modify;
use crate::state::AppState;
use crate::tools::Tool;
use crate::views::text_ui;

impl AppState {
    /// The active tool's options, after the tool strip's buttons.
    pub fn tool_options_ui(&mut self, ui: &mut egui::Ui) {
        self.corner_sizes(ui);
        if self.tool() == Tool::Text {
            self.text_tool_ui(ui);
            ui.separator();
        }
        self.tools.host.tool_mut().dialog(ui);
    }

    /// The offset distance, while Offset is active (SPEC §6.2). Fillet and
    /// chamfer sizes are in their command dialog (#217).
    pub(crate) fn corner_sizes(&mut self, ui: &mut egui::Ui) {
        if self.modify() != Some(Modify::Offset) {
            return;
        }
        let c = &mut self.tools.corner;
        ui.label("Distance");
        ui.add(egui::DragValue::new(&mut c.offset).speed(0.1).suffix(" mm"));
        ui.checkbox(&mut c.sharp, "Sharp corners");
        if let Some(rid) = self.offset_selected()
            && let Some((mut d, mut sharp)) = self.offset_params(rid)
        {
            ui.separator();
            ui.label("Picked offset");
            // Typed, not dragged: each change is one undo step.
            let r1 = ui
                .add(
                    egui::DragValue::new(&mut d)
                        .speed(0.0)
                        .max_decimals(3)
                        .suffix(" mm"),
                )
                .on_hover_text("Click to type a new distance");
            let r2 = ui.checkbox(&mut sharp, "Sharp");
            if (r1.changed() || r2.changed()) && d != 0.0 {
                self.set_offset_params(rid, d, sharp);
            }
            if ui
                .button("Explode")
                .on_hover_text("Turn the offset into ordinary geometry")
                .clicked()
            {
                self.finish_offset(rid, true);
            }
            if ui.button("Remove").clicked() {
                self.finish_offset(rid, false);
            }
        }
        ui.separator();
    }

    /// Tool-bar controls while the Text tool is active.
    pub fn text_tool_ui(&mut self, ui: &mut egui::Ui) {
        // Keep the strip compact: the controls go in a wrapped row.
        text_ui::controls(
            ui,
            &mut self.tools.text_settings,
            &mut self.tools.fonts,
            true,
            1,
        );
    }
}
