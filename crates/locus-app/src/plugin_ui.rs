// SPDX-License-Identifier: AGPL-3.0-or-later
//! Plugin UI shared by the desktop and web hosts (SPEC §15.3; #183, #214,
//! #253): the parameter panel drawn from a plugin's schema, and turning a
//! finished call into the one command that applies it (§11).

use std::time::Duration;

use egui::Ui;
use locus_doc::generated::{GenPath, GeneratedGeometry, PluginRef, input_hash};
use locus_doc::{Command, EntityId, LayerId, RecipeId, RecipeKind};
use locus_geom::Vec2;
use locus_plugin_host::doc::{paths_to_cache, plugin_ref, to_cache};
use locus_plugin_host::types::{
    Generated, Param, ParamKind, ParamSpec, ParamValue, Path, PluginInfo,
};

use crate::state::AppState;

#[derive(Debug, Clone)]
pub enum Target {
    /// A new generator object.
    Insert,
    /// A generator object's parameters.
    Edit(RecipeId),
    /// A new modifier object over these curves.
    Modify(Vec<EntityId>),
    /// A modifier object's parameters.
    EditModifier(RecipeId),
}

impl Target {
    pub fn modifier(&self) -> bool {
        matches!(self, Target::Modify(_) | Target::EditModifier(_))
    }
}

/// The open parameter panel.
#[derive(Debug)]
pub struct Panel {
    pub id: String,
    pub name: String,
    pub schema: Vec<ParamSpec>,
    pub values: Vec<ParamValue>,
    pub target: Target,
    pub error: Option<String>,
}

impl Panel {
    /// The current values, keyed by the schema.
    pub fn params(&self) -> Vec<Param> {
        self.schema
            .iter()
            .zip(&self.values)
            .map(|(s, v)| Param {
                key: s.key.clone(),
                value: v.clone(),
            })
            .collect()
    }
}

/// What the user did in the panel this frame.
#[derive(Debug)]
pub enum PanelAction {
    None,
    Apply(Vec<Param>),
    Close,
    /// Cancel the running call.
    Stop,
}

/// The document's parameters, for `doc-parameter` bindings (§5).
pub fn doc_parameters(canvas: &AppState) -> Vec<(String, f64)> {
    canvas
        .document()
        .parameters()
        .iter()
        .map(|(n, p)| (n.clone(), p.value))
        .collect()
}

/// The parameter panel. While a call runs (`running`), it shows a spinner,
/// the time so far and Cancel instead of the action buttons (#214).
pub fn show_panel(
    ctx: &egui::Context,
    panel: &mut Panel,
    doc_params: &[(String, f64)],
    running: Option<Duration>,
) -> PanelAction {
    let (mut apply, mut close, mut stop) = (false, false, false);
    let verb = match panel.target {
        Target::Insert | Target::Modify(_) => "Insert",
        Target::Edit(_) | Target::EditModifier(_) => "Apply",
    };
    egui::Window::new(&panel.name)
        .collapsible(false)
        .resizable(true)
        .show(ctx, |ui| {
            egui::Grid::new("plugin-params")
                .num_columns(2)
                .show(ui, |ui| {
                    for (spec, value) in panel.schema.iter().zip(panel.values.iter_mut()) {
                        ui.label(&spec.label).on_hover_text(&spec.help);
                        param_widget(ui, spec, value, doc_params);
                        ui.end_row();
                    }
                });
            if panel.schema.is_empty() {
                ui.weak("This plugin has no parameters.");
            }
            if let Some(e) = &panel.error {
                ui.colored_label(ui.visuals().error_fg_color, e);
            }
            ui.horizontal(|ui| match running {
                Some(t) => {
                    ui.spinner();
                    ui.label(format!("Running… {:.1} s", t.as_secs_f32()));
                    stop = ui
                        .button("Cancel")
                        .on_hover_text("Stop the plugin. Nothing is changed.")
                        .clicked();
                }
                None => {
                    apply = ui.button(verb).clicked();
                    close = ui.button("Close").clicked();
                }
            });
        });
    if stop {
        PanelAction::Stop
    } else if close {
        PanelAction::Close
    } else if apply {
        PanelAction::Apply(panel.params())
    } else {
        PanelAction::None
    }
}

/// A finished call's output, ready to become a command.
#[derive(Debug)]
pub enum Done {
    Generated(PluginRef, GeneratedGeometry),
    Modified(RecipeKind),
}

impl Done {
    /// A generator's validated output, as the document stores it.
    pub fn generated(info: &PluginInfo, params: &[Param], g: &Generated) -> Done {
        Done::Generated(plugin_ref(&info.id, &info.version, params), to_cache(g))
    }

    /// A modifier's validated output for `input`.
    pub fn modified(info: &PluginInfo, params: &[Param], input: &[GenPath], out: &[Path]) -> Done {
        Done::Modified(RecipeKind::Modified {
            plugin: plugin_ref(&info.id, &info.version, params),
            input_hash: input_hash(input),
            output: paths_to_cache(out),
        })
    }
}

/// Where a call's result goes, as it was when the call started.
#[derive(Debug, Clone)]
pub struct Placement {
    pub target: Target,
    /// A new modifier object's source curves.
    pub sources: Vec<EntityId>,
    /// Where a new generator object goes.
    pub layer: LayerId,
    pub origin: Vec2,
}

/// The one command that applies a finished call (one undo step, §11). The
/// document may have changed while the call ran; a gone object makes the
/// command fail cleanly when applied.
pub fn command(place: Placement, done: Done) -> Result<Command, String> {
    Ok(match (done, place.target) {
        (Done::Modified(kind), Target::EditModifier(rid)) => Command::EditRecipe { id: rid, kind },
        (Done::Modified(kind), _) => Command::AddRecipe {
            kind,
            sources: place.sources,
        },
        (Done::Generated(plugin, output), Target::Insert) => Command::AddGenerated {
            layer: place.layer,
            origin: place.origin,
            plugin,
            output,
        },
        (Done::Generated(plugin, output), Target::Edit(rid) | Target::EditModifier(rid)) => {
            Command::EditRecipe {
                id: rid,
                kind: RecipeKind::Generated { plugin, output },
            }
        }
        (Done::Generated(..), Target::Modify(_)) => return Err("not a generator".into()),
    })
}

/// One parameter's editor, from its spec (§15.3).
pub fn param_widget(
    ui: &mut Ui,
    spec: &ParamSpec,
    value: &mut ParamValue,
    doc_params: &[(String, f64)],
) {
    match (&spec.kind, value) {
        (ParamKind::Number(n), ParamValue::Number(v)) => {
            let lo = n.min.unwrap_or(f64::MIN);
            let hi = n.max.unwrap_or(f64::MAX);
            let mut w = egui::DragValue::new(v).range(lo..=hi).speed(0.1);
            if !n.unit.is_empty() {
                w = w.suffix(format!(" {}", n.unit));
            }
            ui.add(w);
        }
        (ParamKind::Integer(n), ParamValue::Integer(v)) => {
            let lo = n.min.unwrap_or(i64::MIN);
            let hi = n.max.unwrap_or(i64::MAX);
            ui.add(egui::DragValue::new(v).range(lo..=hi));
        }
        (ParamKind::Boolean(_), ParamValue::Boolean(b)) => {
            ui.checkbox(b, "");
        }
        (ParamKind::Choice(c), ParamValue::Choice(i)) => {
            let current = c.options.get(*i as usize).cloned().unwrap_or_default();
            egui::ComboBox::from_id_salt(&spec.key)
                .selected_text(current)
                .show_ui(ui, |ui| {
                    for (k, o) in c.options.iter().enumerate() {
                        ui.selectable_value(i, k as u32, o);
                    }
                });
        }
        (ParamKind::Text(t), ParamValue::Text(s)) => {
            ui.add(egui::TextEdit::singleline(s).char_limit(t.max_length as usize));
        }
        (ParamKind::DocParameter, ParamValue::Number(v)) => {
            // Bound to a document parameter (§5): pick one; its value is passed.
            let shown = doc_params
                .iter()
                .find(|(_, x)| x.to_bits() == v.to_bits())
                .map_or_else(|| "pick a parameter".to_owned(), |(n, _)| n.clone());
            egui::ComboBox::from_id_salt(&spec.key)
                .selected_text(shown)
                .show_ui(ui, |ui| {
                    for (name, x) in doc_params {
                        if ui.selectable_label(false, name).clicked() {
                            *v = *x;
                        }
                    }
                });
        }
        _ => {
            ui.weak("(unsupported)");
        }
    }
}
