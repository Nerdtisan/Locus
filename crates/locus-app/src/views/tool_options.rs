// SPDX-License-Identifier: AGPL-3.0-or-later
//! The active tool's options in the tool strip: fillet radius, chamfer,
//! offset, polygon sides and text settings. Move, Rotate and Scale have
//! their own command dialog on the canvas (design §6.7).

use crate::modify_tool::{ChamferMode, Modify};
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

    /// Fillet radius and chamfer size, while those tools are active (mm and
    /// degrees; SPEC §6.2).
    pub(crate) fn corner_sizes(&mut self, ui: &mut egui::Ui) {
        fn len(v: &mut f64) -> egui::DragValue<'_> {
            egui::DragValue::new(v)
                .range(0.001..=10_000.0)
                .speed(0.1)
                .suffix(" mm")
        }
        let m = self.modify();
        let c = &mut self.tools.corner;
        match m {
            Some(Modify::Fillet) => {
                ui.label("Radius");
                ui.add(len(&mut c.radius));
                ui.separator();
            }
            Some(Modify::Chamfer) => {
                egui::ComboBox::from_id_salt("chamfer-mode")
                    .selected_text(c.mode.label())
                    .show_ui(ui, |ui| {
                        for m in ChamferMode::ALL {
                            ui.selectable_value(&mut c.mode, m, m.label());
                        }
                    });
                ui.add(len(&mut c.d1));
                match c.mode {
                    ChamferMode::Equal => {}
                    ChamferMode::TwoDistances => {
                        ui.add(len(&mut c.d2));
                    }
                    ChamferMode::DistanceAngle => {
                        ui.add(
                            egui::DragValue::new(&mut c.angle_deg)
                                .range(0.1..=179.9)
                                .suffix("°"),
                        );
                    }
                }
                ui.separator();
            }
            Some(Modify::Offset) => {
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
            _ => {}
        }
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
