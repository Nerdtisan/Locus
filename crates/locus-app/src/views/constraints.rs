// SPDX-License-Identifier: AGPL-3.0-or-later
//! The Constraints pane: every constraint, conflicting ones in red, with
//! driven toggles and removal (#72).

use crate::constrain;
use crate::render::Palette;
use crate::settings::Settings;
use crate::state::AppState;
use locus_doc::{Command, Edge, EntityId};

impl AppState {
    /// The constraints panel: every constraint, conflicting ones in red,
    /// with driven toggles and removal (#72).
    pub fn constraints_ui(&mut self, ui: &mut egui::Ui, settings: &Settings) {
        let (flagged, _) = self.flagged();
        let doc = self.doc.sketcher.document();
        if doc.constraints().is_empty() {
            ui.weak("No constraints yet.");
            return;
        }
        let red = Palette::new(ui.visuals().dark_mode).conflict;
        let mut select: Option<Vec<EntityId>> = None;
        let mut action: Option<Command> = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            for (&id, c) in doc.constraints() {
                ui.horizontal(|ui| {
                    let text = constrain::describe(doc, c, settings.units);
                    let text = if flagged.contains(&id) {
                        egui::RichText::new(text).color(red)
                    } else {
                        egui::RichText::new(text)
                    };
                    if ui.selectable_label(false, text).clicked() {
                        select = Some(c.references());
                    }
                    if let Some(d) = c.dimension() {
                        if c.has_stroke_edge(doc) {
                            let mut edge = d.edge;
                            egui::ComboBox::from_id_salt(("dim-edge", id))
                                .selected_text(edge.name())
                                .width(96.0)
                                .show_ui(ui, |ui| {
                                    for e in Edge::ALL {
                                        ui.selectable_value(&mut edge, e, e.name());
                                    }
                                })
                                .response
                                .on_hover_text(
                                    "Measure to the centreline or to the edge of the pen line / cut",
                                );
                            if edge != d.edge {
                                action = Some(Command::SetDimensionEdge { id, edge });
                            }
                        }
                        let mut driven = d.driven;
                        if ui
                            .checkbox(&mut driven, "driven")
                            .on_hover_text("Measured instead of enforced")
                            .changed()
                        {
                            action = Some(Command::SetDriven { id, driven });
                        }
                    }
                    if ui.small_button("✕").on_hover_text("Remove").clicked() {
                        action = Some(Command::RemoveConstraint { id });
                    }
                });
            }
        });
        if let Some(ids) = select {
            self.selection.items.set_all(ids);
        }
        if let Some(cmd) = action {
            self.apply_reporting(&cmd);
        }
    }
}
