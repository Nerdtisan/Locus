// SPDX-License-Identifier: AGPL-3.0-or-later
//! Plugin objects on the canvas (#183, SPEC §15.3, §15.5): finding the one
//! the selection belongs to, and describing it. Running plugins is the
//! desktop-only `crate::plugins`; this part builds everywhere, so the web
//! build shows a plugin object's details from its cached geometry too.

use super::Canvas;
#[cfg(not(target_arch = "wasm32"))]
use locus_doc::LayerId;
use locus_doc::generated::{PluginRef, PluginValue};
use locus_doc::{RecipeId, RecipeKind};
#[cfg(not(target_arch = "wasm32"))]
use locus_geom::Vec2;

/// Formats a stored parameter value for display.
pub fn show_value(v: &PluginValue) -> String {
    match v {
        PluginValue::Number(n) => {
            let s = format!("{n:.4}");
            s.trim_end_matches('0').trim_end_matches('.').to_owned()
        }
        PluginValue::Integer(n) => n.to_string(),
        PluginValue::Boolean(b) => if *b { "yes" } else { "no" }.to_owned(),
        PluginValue::Choice(c) => format!("option {}", c + 1),
        PluginValue::Text(t) => format!("{t:?}"),
    }
}

impl Canvas {
    /// Where a newly inserted plugin object goes: the middle of the view.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn insert_point(&self) -> Vec2 {
        self.view.center
    }

    /// The layer new geometry goes on.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn current_layer(&self) -> LayerId {
        self.active_layer()
    }

    /// The plugin object the selection belongs to: an instance it drew, or
    /// one of its two frame points.
    pub fn selected_plugin_object(&self) -> Option<(RecipeId, PluginRef)> {
        let doc = self.sketcher.document();
        let generated = |rid: RecipeId| match doc.recipe(rid).map(|r| &r.kind) {
            Some(RecipeKind::Generated { plugin, .. }) => Some((rid, plugin.clone())),
            _ => None,
        };
        self.selection.iter().find_map(|id| {
            doc.driven_by(id).and_then(generated).or_else(|| {
                doc.recipes()
                    .iter()
                    .filter(|(_, r)| r.sources.contains(&id))
                    .find_map(|(rid, _)| generated(*rid))
            })
        })
    }

    /// The plugin, version and parameters of a plugin object, read-only.
    pub fn plugin_object_ui(&self, ui: &mut egui::Ui, plugin: &PluginRef) {
        egui::Grid::new("plugin-object")
            .num_columns(2)
            .show(ui, |ui| {
                ui.label("Plugin");
                ui.monospace(&plugin.id);
                ui.end_row();
                ui.label("Version");
                ui.monospace(&plugin.version);
                ui.end_row();
                for p in &plugin.params {
                    ui.label(&p.key);
                    ui.monospace(show_value(&p.value));
                    ui.end_row();
                }
            });
        ui.weak("Drag or constrain its two frame points to place and turn it.");
    }
}
