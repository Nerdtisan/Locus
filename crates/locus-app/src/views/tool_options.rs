// SPDX-License-Identifier: AGPL-3.0-or-later
//! The active tool's options in the tool strip: polygon sides. Move, Rotate,
//! Scale, Fillet, Chamfer, Offset, Pattern, Mirror and Text have their own
//! command dialog on the canvas (design §6.5–6.9).

use crate::state::AppState;

impl AppState {
    /// The active tool's options, after the tool strip's buttons.
    pub fn tool_options_ui(&mut self, ui: &mut egui::Ui) {
        self.tools.host.tool_mut().dialog(ui);
    }
}
