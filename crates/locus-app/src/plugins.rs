// SPDX-License-Identifier: AGPL-3.0-or-later
//! Plugins in the desktop app (SPEC §15; #182, #183, #184).
//!
//! - **Install** a `.locusplugin` into the app's plugin folder; the host
//!   checks the archive, manifest and component first (§15.7).
//! - **Manage**: the installed list, each labelled Unsigned until signing
//!   lands (§15.4), with uninstall.
//! - **Generators**: the parameter panel is drawn from the plugin's schema
//!   (§15.3). Inserting or editing is one command, so one undo step (§11). A
//!   failed call shows its error and changes nothing.
//! - **Modifiers** (§15.2): applied to the selected chain as a
//!   non-destructive object; the sketch re-runs the installed plugin when
//!   the chain changes ([`RegistryRunner`]).
//! - **Exporters** (§15.2): listed in File ▸ Export beside the built-ins,
//!   never replacing one.
//! - **Plugin objects**: edit their parameters, or update them to the
//!   installed plugin version, explicitly; a generator never re-runs on its
//!   own (§15.5). Without the plugin they keep working from their cached
//!   geometry, with a "plugin missing" badge (modifiers: "out of date" once
//!   their source changes).
//!
//! Desktop-only (SPEC §0). Calls run on the UI thread under the host's
//! limits (a CPU budget, 256 MB, 5 s).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use egui::{RichText, Ui};
use std::sync::Arc;

use locus_doc::generated::{PluginRef, input_hash, modifier_input};
use locus_doc::{Command, EntityId, RecipeId, RecipeKind};
use locus_plugin_host::archive::MAX_ARCHIVE_BYTES;
use locus_plugin_host::doc::{
    from_stored, paths_to_cache, plugin_ref, resolve, to_cache, to_paths,
};
use locus_plugin_host::export::PluginExporter;
use locus_plugin_host::registry::{Installed, Registry};
use locus_plugin_host::runner::RegistryRunner;
use locus_plugin_host::types::{Kind, Param, ParamKind, ParamSpec, ParamValue};
use locus_plugin_host::{Host, Plugin};

use crate::canvas::Canvas;
use crate::file_io::FileIo;

/// The app's plugin folder, next to its settings.
fn plugin_dir() -> Option<PathBuf> {
    eframe::storage_dir("locus").map(|d| d.join("plugins"))
}

fn read_limited(path: &Path, limit: usize) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let mut data = Vec::new();
    std::fs::File::open(path)
        .and_then(|f| f.take(limit as u64 + 1).read_to_end(&mut data))
        .map_err(|e| format!("couldn't read the file ({})", e.kind()))?;
    if data.len() > limit {
        return Err("the file is too large to be a plugin".into());
    }
    Ok(data)
}

#[derive(Debug, Clone)]
enum Target {
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
    fn modifier(&self) -> bool {
        matches!(self, Target::Modify(_) | Target::EditModifier(_))
    }
}

/// The open parameter panel.
#[derive(Debug)]
struct Panel {
    id: String,
    name: String,
    schema: Vec<ParamSpec>,
    values: Vec<ParamValue>,
    target: Target,
    error: Option<String>,
}

#[derive(Debug, Default)]
pub struct Plugins {
    host: Option<Host>,
    installed: Vec<Installed>,
    listed: bool,
    /// Loaded components by plugin id; dropped on install and uninstall.
    loaded: BTreeMap<String, Plugin>,
    panel: Option<Panel>,
    manage: bool,
    /// Plugin id awaiting "really uninstall?".
    confirm: Option<String>,
    message: Option<String>,
    runner: Option<Arc<RegistryRunner>>,
    /// The installed set changed: rebuild the export formats.
    exporters_dirty: bool,
}

impl Plugins {
    fn registry() -> Result<Registry, String> {
        plugin_dir()
            .map(Registry::new)
            .ok_or_else(|| "this system has no folder for plugins".to_owned())
    }

    fn host(&mut self) -> Result<&Host, String> {
        if self.host.is_none() {
            self.host = Some(Host::new().map_err(|e| e.to_string())?);
        }
        self.host
            .as_ref()
            .ok_or_else(|| "the plugin host didn't start".to_owned())
    }

    fn refresh(&mut self) {
        self.installed = Self::registry().map(|r| r.list()).unwrap_or_default();
        self.listed = true;
    }

    /// Connects the plugins to the sketch (modifier re-runs) and to File ▸
    /// Export (exporter plugins). Called once at start-up.
    pub fn attach(&mut self, canvas: &mut Canvas, files: &mut FileIo) {
        if let Ok(reg) = Self::registry() {
            let runner = Arc::new(RegistryRunner::new(reg));
            canvas.set_plugin_runner(Some(runner.clone()));
            self.runner = Some(runner);
        }
        self.refresh();
        self.update_exporters(files);
    }

    /// The built-in formats plus every installed exporter plugin that loads.
    /// A plugin can't take a built-in's id (`Registry::add`).
    fn update_exporters(&mut self, files: &mut FileIo) {
        let mut formats = locus_export::Registry::builtin();
        let ids: Vec<String> = self
            .installed
            .iter()
            .filter(|i| i.manifest.kinds.contains(&Kind::Exporter))
            .map(|i| i.manifest.id.clone())
            .collect();
        let mut skipped = Vec::new();
        for id in ids {
            let loaded = Self::registry().and_then(|reg| {
                let host = self.host()?;
                let (_, p) = reg.load(host, &id).map_err(|e| e.to_string())?;
                PluginExporter::new(p).map_err(|e| e.to_string())
            });
            match loaded {
                Ok(e) => {
                    if !formats.add(Box::new(e)) {
                        skipped.push(id);
                    }
                }
                Err(_) => skipped.push(id),
            }
        }
        if !skipped.is_empty() {
            self.message = Some(format!(
                "Some exporter plugins couldn't be added to File ▸ Export \
                 (they failed to load or use a format id that's taken): {}",
                skipped.join(", ")
            ));
        }
        files.set_exporters(formats);
        self.exporters_dirty = false;
    }

    /// After an install or uninstall: reload plugins everywhere.
    fn changed(&mut self) {
        self.loaded.clear();
        if let Some(r) = &self.runner {
            r.reload();
        }
        self.refresh();
        self.exporters_dirty = true;
    }

    fn installed(&mut self, id: &str) -> Option<&Installed> {
        if !self.listed {
            self.refresh();
        }
        self.installed.iter().find(|i| i.manifest.id == id)
    }

    fn plugin(&mut self, id: &str) -> Result<&Plugin, String> {
        if !self.loaded.contains_key(id) {
            let reg = Self::registry()?;
            let host = self.host()?;
            let (_, p) = reg.load(host, id).map_err(|e| e.to_string())?;
            self.loaded.insert(id.to_owned(), p);
        }
        self.loaded
            .get(id)
            .ok_or_else(|| "the plugin isn't loaded".to_owned())
    }

    // ── Menu ────────────────────────────────────────────────────────────────

    pub fn menu(&mut self, ui: &mut Ui, canvas: &Canvas) {
        if !self.listed {
            self.refresh();
        }
        if ui.button("Install plugin…").clicked() {
            ui.close();
            self.install();
        }
        if ui.button("Manage plugins…").clicked() {
            ui.close();
            self.refresh();
            self.manage = true;
        }
        ui.separator();
        let generators: Vec<(String, String)> = self
            .installed
            .iter()
            .filter(|i| i.manifest.kinds.contains(&Kind::Generator))
            .map(|i| (i.manifest.id.clone(), i.manifest.name.clone()))
            .collect();
        if generators.is_empty() {
            ui.weak("No generator plugins installed.");
        }
        for (id, name) in generators {
            if ui.button(format!("Insert {name}…")).clicked() {
                ui.close();
                self.open(&id, Target::Insert, None);
            }
        }
        let modifiers: Vec<(String, String)> = self
            .installed
            .iter()
            .filter(|i| i.manifest.kinds.contains(&Kind::Modifier))
            .map(|i| (i.manifest.id.clone(), i.manifest.name.clone()))
            .collect();
        if !modifiers.is_empty() {
            ui.separator();
            let curves = canvas.selected_curves();
            for (id, name) in modifiers {
                if ui
                    .add_enabled(
                        !curves.is_empty(),
                        egui::Button::new(format!("Apply {name} to selection…")),
                    )
                    .on_disabled_hover_text("Select a chain of lines and arcs, or a circle.")
                    .clicked()
                {
                    ui.close();
                    self.open(&id, Target::Modify(curves.clone()), None);
                }
            }
        }
    }

    fn install(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Locus plugin", &["locusplugin"])
            .pick_file()
        else {
            return;
        };
        let result = read_limited(&path, MAX_ARCHIVE_BYTES).and_then(|bytes| {
            let reg = Self::registry()?;
            let host = self.host()?;
            reg.install(host, &bytes).map_err(|e| e.to_string())
        });
        self.message = Some(match result {
            Ok(i) => {
                format!(
                    "Installed {} {} ({}).",
                    i.manifest.name,
                    i.manifest.version,
                    i.trust_label().to_lowercase()
                )
            }
            Err(e) => format!("Couldn't install the plugin: {e}"),
        });
        self.changed();
    }

    // ── Parameter panel ─────────────────────────────────────────────────────

    /// Opens the panel for a plugin; `stored` pre-fills an existing object's
    /// values (ones the current schema no longer has are dropped).
    fn open(&mut self, id: &str, target: Target, stored: Option<&PluginRef>) {
        let modifier = target.modifier();
        let opened = self.plugin(id).and_then(|p| {
            let schema = if modifier {
                p.modifier_schema()
            } else {
                p.generator_schema()
            }
            .map_err(|e| e.to_string())?;
            Ok((p.info().name.clone(), schema))
        });
        match opened {
            Ok((name, schema)) => {
                let values = match stored {
                    Some(r) => from_stored(&schema, &r.params),
                    None => locus_plugin_host::doc::defaults(&schema),
                }
                .into_iter()
                .map(|p| p.value)
                .collect();
                self.panel = Some(Panel {
                    id: id.to_owned(),
                    name,
                    schema,
                    values,
                    target,
                    error: None,
                });
            }
            Err(e) => self.message = Some(format!("Couldn't open the plugin: {e}")),
        }
    }

    /// Runs the plugin and applies one command; `Err` changes nothing.
    fn run(
        &mut self,
        canvas: &mut Canvas,
        id: &str,
        values: Vec<Param>,
        target: Target,
    ) -> Result<(), String> {
        let plugin = self.plugin(id)?;
        let info = plugin.info().clone();
        if target.modifier() {
            let schema = plugin.modifier_schema().map_err(|e| e.to_string())?;
            let params = resolve(&schema, &values).map_err(|e| e.to_string())?;
            let doc = canvas.document();
            let sources = match &target {
                Target::Modify(curves) => curves.clone(),
                Target::EditModifier(rid) => doc
                    .recipe(*rid)
                    .map(|r| r.sources.clone())
                    .ok_or("the modifier object is gone")?,
                _ => Vec::new(),
            };
            let input = modifier_input(doc, &sources).map_err(|e| e.to_string())?;
            let out = plugin
                .modify(&to_paths(&input), &params)
                .map_err(|e| e.to_string())?;
            let kind = RecipeKind::Modified {
                plugin: plugin_ref(&info.id, &info.version, &params),
                input_hash: input_hash(&input),
                output: paths_to_cache(&out),
            };
            let cmd = match target {
                Target::EditModifier(rid) => Command::EditRecipe { id: rid, kind },
                _ => Command::AddRecipe { kind, sources },
            };
            return canvas.apply(&cmd).map_err(|e| e.to_string());
        }
        let schema = plugin.generator_schema().map_err(|e| e.to_string())?;
        let params = resolve(&schema, &values).map_err(|e| e.to_string())?;
        let generated = plugin.generate(&params).map_err(|e| e.to_string())?;
        let pref = plugin_ref(&info.id, &info.version, &params);
        let output = to_cache(&generated);
        let cmd = match target {
            Target::Insert => Command::AddGenerated {
                layer: canvas.current_layer(),
                origin: canvas.insert_point(),
                plugin: pref,
                output,
            },
            Target::Edit(rid) | Target::EditModifier(rid) => Command::EditRecipe {
                id: rid,
                kind: RecipeKind::Generated {
                    plugin: pref,
                    output,
                },
            },
            Target::Modify(_) => return Err("not a generator".into()),
        };
        canvas.apply(&cmd).map_err(|e| e.to_string())
    }

    pub fn windows(&mut self, ctx: &egui::Context, canvas: &mut Canvas, files: &mut FileIo) {
        if self.exporters_dirty {
            self.update_exporters(files);
        }
        self.panel_window(ctx, canvas);
        self.manage_window(ctx);
        if let Some(msg) = self.message.clone() {
            egui::Window::new("Plugins")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .show(ctx, |ui| {
                    ui.label(msg);
                    if ui.button("OK").clicked() {
                        self.message = None;
                    }
                });
        }
    }

    fn panel_window(&mut self, ctx: &egui::Context, canvas: &mut Canvas) {
        let Some(panel) = self.panel.as_mut() else {
            return;
        };
        let doc_params: Vec<(String, f64)> = canvas
            .document()
            .parameters()
            .iter()
            .map(|(n, p)| (n.clone(), p.value))
            .collect();
        let (mut apply, mut cancel) = (false, false);
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
                            param_widget(ui, spec, value, &doc_params);
                            ui.end_row();
                        }
                    });
                if panel.schema.is_empty() {
                    ui.weak("This plugin has no parameters.");
                }
                if let Some(e) = &panel.error {
                    ui.colored_label(ui.visuals().error_fg_color, e);
                }
                ui.horizontal(|ui| {
                    apply = ui.button(verb).clicked();
                    cancel = ui.button("Cancel").clicked();
                });
            });
        if cancel {
            self.panel = None;
        } else if apply {
            let (id, target) = (panel.id.clone(), panel.target.clone());
            let values: Vec<Param> = panel
                .schema
                .iter()
                .zip(&panel.values)
                .map(|(s, v)| Param {
                    key: s.key.clone(),
                    value: v.clone(),
                })
                .collect();
            match self.run(canvas, &id, values, target) {
                Ok(()) => self.panel = None,
                Err(e) => {
                    if let Some(p) = self.panel.as_mut() {
                        p.error = Some(e);
                    }
                }
            }
        }
    }

    fn manage_window(&mut self, ctx: &egui::Context) {
        if !self.manage {
            return;
        }
        let mut open = true;
        let mut uninstall = None;
        egui::Window::new("Installed plugins")
            .open(&mut open)
            .collapsible(false)
            .show(ctx, |ui| {
                if self.installed.is_empty() {
                    ui.weak("No plugins installed.");
                }
                egui::Grid::new("plugins").num_columns(4).show(ui, |ui| {
                    for i in &self.installed {
                        let m = &i.manifest;
                        ui.label(RichText::new(&m.name).strong())
                            .on_hover_text(format!("{}\n{}\n{}", m.id, m.author, m.licence));
                        ui.label(&m.version);
                        let kinds: Vec<&str> = m.kinds.iter().map(|k| k.name()).collect();
                        ui.weak(kinds.join(", "));
                        ui.horizontal(|ui| {
                            ui.colored_label(ui.visuals().warn_fg_color, i.trust_label())
                                .on_hover_text("Plugin signing isn't available yet.");
                            if self.confirm.as_deref() == Some(m.id.as_str()) {
                                if ui.button("Really uninstall").clicked() {
                                    uninstall = Some(m.id.clone());
                                }
                            } else if ui.button("Uninstall").clicked() {
                                self.confirm = Some(m.id.clone());
                            }
                        });
                        ui.end_row();
                    }
                });
                ui.weak(
                    "Plugins run sandboxed: no files, network, clock or system access. \
                     Documents keep plugin output when a plugin is removed.",
                );
            });
        if let Some(id) = uninstall {
            self.confirm = None;
            self.loaded.remove(&id);
            if let Err(e) =
                Self::registry().and_then(|r| r.uninstall(&id).map_err(|e| e.to_string()))
            {
                self.message = Some(format!("Couldn't uninstall: {e}"));
            }
            self.changed();
        }
        if !open {
            self.manage = false;
            self.confirm = None;
        }
    }

    // ── Properties ──────────────────────────────────────────────────────────

    /// The selected plugin object's section of the Properties panel.
    pub fn properties_ui(&mut self, ui: &mut Ui, canvas: &mut Canvas) {
        let Some((rid, pref, modifier)) = canvas.selected_plugin_object() else {
            return;
        };
        let kind = if modifier {
            Kind::Modifier
        } else {
            Kind::Generator
        };
        ui.label(
            RichText::new(if modifier {
                "Modifier"
            } else {
                "Plugin object"
            })
            .strong(),
        );
        let installed = self
            .installed(&pref.id)
            .map(|i| (i.manifest.version.clone(), i.manifest.kinds.contains(&kind)));
        let (edit, update) = if modifier {
            (Target::EditModifier(rid), Target::EditModifier(rid))
        } else {
            (Target::Edit(rid), Target::Edit(rid))
        };
        match installed {
            None | Some((_, false)) => {
                ui.colored_label(ui.visuals().warn_fg_color, "⚠ Plugin missing")
                    .on_hover_text(
                        "Shown and exported from its saved geometry. Install the plugin \
                         to change its parameters.",
                    );
            }
            Some((version, true)) => {
                ui.horizontal(|ui| {
                    if ui.button("Edit parameters…").clicked() {
                        self.open(&pref.id, edit, Some(&pref));
                    }
                    if version != pref.version
                        && ui
                            .button(format!("Update to {version}"))
                            .on_hover_text(
                                "Run the installed version with the same parameters. \
                                 One undo step.",
                            )
                            .clicked()
                    {
                        let values = self
                            .plugin(&pref.id)
                            .and_then(|p| {
                                if modifier {
                                    p.modifier_schema()
                                } else {
                                    p.generator_schema()
                                }
                                .map_err(|e| e.to_string())
                            })
                            .map(|schema| from_stored(&schema, &pref.params));
                        let result = values.and_then(|v| self.run(canvas, &pref.id, v, update));
                        if let Err(e) = result {
                            self.message = Some(format!("Couldn't update the object: {e}"));
                        }
                    }
                });
            }
        }
        if modifier && canvas.plugin_object_stale(rid) {
            ui.colored_label(ui.visuals().warn_fg_color, "⚠ Out of date")
                .on_hover_text(
                    "Its source changed and the plugin couldn't run again (missing, a \
                     different version, or it failed). The saved output is shown.",
                );
        }
        canvas.plugin_object_ui(ui, &pref);
        if !modifier {
            ui.weak("Drag or constrain its two frame points to place and turn it.");
        }
        if ui
            .button("Explode")
            .on_hover_text(
                "Turn it into ordinary, editable geometry, keeping the plugin's \
                 constraints where they still fit. One undo step.",
            )
            .clicked()
        {
            match canvas.explode_plugin_object(rid) {
                Ok(0) => {}
                Ok(n) => {
                    self.message = Some(format!(
                        "Exploded. {n} of the plugin's constraints were left out: they \
                         would have turned the object back to the axes, or conflicted \
                         with or repeated other constraints."
                    ));
                }
                Err(e) => self.message = Some(format!("Couldn't explode it: {e}")),
            }
        }
        ui.separator();
    }
}

/// One parameter's editor, from its spec (§15.3).
fn param_widget(
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
