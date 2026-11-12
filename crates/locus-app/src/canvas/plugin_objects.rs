// SPDX-License-Identifier: AGPL-3.0-or-later
//! Plugin objects on the canvas (#183, #184, SPEC §15.2, §15.3, §15.5):
//! finding the one the selection belongs to, and describing it. Running plugins is
//! `crate::plugins` on desktop and `crate::web_plugins` in the browser.

use super::Canvas;
use locus_doc::generated::{PluginRef, PluginValue};
use locus_doc::{EntityId, LayerId, RecipeId, RecipeKind};
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
    pub fn insert_point(&self) -> Vec2 {
        self.view.center
    }

    /// The layer new geometry goes on.
    pub fn current_layer(&self) -> LayerId {
        self.active_layer()
    }

    /// Hands the sketch something that re-runs modifier plugins when their
    /// sources change (#184); kept for documents opened later.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn set_plugin_runner(
        &mut self,
        runner: Option<std::sync::Arc<dyn locus_doc::generated::PluginRunner>>,
    ) {
        self.plugin_runner = runner.clone();
        self.sketcher.set_plugin_runner(runner);
    }

    /// The selected curves a modifier can take (lines, arcs, circles).
    pub fn selected_curves(&self) -> Vec<EntityId> {
        let doc = self.sketcher.document();
        self.selection
            .iter()
            .filter(|id| {
                doc.line(*id).is_some() || doc.arc(*id).is_some() || doc.circle(*id).is_some()
            })
            .collect()
    }

    /// Whether a modifier object's output was made from older source
    /// geometry (its plugin is missing or failed).
    pub fn plugin_object_stale(&self, rid: RecipeId) -> bool {
        locus_doc::generated::is_stale(self.sketcher.document(), rid)
    }

    /// The plugin object the selection belongs to: an instance it drew, one
    /// of a generator object's frame points, or a modifier object's source.
    /// The flag is true for a modifier.
    pub fn selected_plugin_object(&self) -> Option<(RecipeId, PluginRef, bool)> {
        let doc = self.sketcher.document();
        let generated = |rid: RecipeId| match doc.recipe(rid).map(|r| &r.kind) {
            Some(RecipeKind::Generated { plugin, .. }) => Some((rid, plugin.clone(), false)),
            Some(RecipeKind::Modified { plugin, .. }) => Some((rid, plugin.clone(), true)),
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

    /// Explodes a plugin object, keeping the plugin's constraints that still
    /// fit (#213). One undo step; returns how many were dropped.
    pub fn explode_plugin_object(&mut self, rid: RecipeId) -> Result<usize, locus_doc::DocError> {
        let out = self.sketcher.explode_plugin_object(rid)?;
        self.rebuild();
        Ok(out.dropped)
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
    }
}
