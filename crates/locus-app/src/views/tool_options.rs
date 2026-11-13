// SPDX-License-Identifier: AGPL-3.0-or-later
//! The active tool's options in the tool strip: polygon sides and text
//! settings. Move, Rotate, Scale, Fillet, Chamfer and Offset have their own
//! command dialog on the canvas (design §6.6–6.7, #217, #218).

use crate::state::AppState;
use crate::tools::Tool;
use crate::views::text_ui;

impl AppState {
    /// The active tool's options, after the tool strip's buttons.
    pub fn tool_options_ui(&mut self, ui: &mut egui::Ui) {
        if self.tool() == Tool::Text {
            self.text_tool_ui(ui);
            ui.separator();
        }
        self.tools.host.tool_mut().dialog(ui);
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
