// SPDX-License-Identifier: AGPL-3.0-or-later
//! Plugins in the web build (SPEC §15, ADR 0016, #253).
//!
//! - **Install** a `.locusplugin` from the browser's file picker. The same
//!   checks as on desktop: the archive and manifest are parsed, the component
//!   is checked and described in the worker, and its id, version, API major
//!   and kinds must match the manifest. Only then is it stored in OPFS
//!   (`plugins/<id>.locusplugin`).
//! - **Generators** (Insert), **modifiers** (Apply to selection) and
//!   **exporters** (Export with …, a download) run as on desktop, through the
//!   same parameter panel and the same one-command apply (`plugin_ui`).
//! - Every call runs in a **fresh Web Worker** that imports nothing into the
//!   plugin, caps its memory at 256 MB and is terminated after 5 s (§15.4).
//!   Its output is validated in the worker and parsed with bounds here. A
//!   failed or cancelled call changes nothing.
//!
//! Differences from desktop, by design of the browser: calls are always
//! asynchronous, so a modifier whose source is edited is not re-run inside
//! that edit (it shows "out of date"; Edit parameters re-runs it), and Cancel
//! drops the result at once while the worker is stopped by its timeout.
//! Importer and validator plugins are desktop-only for now.

use std::sync::mpsc::{Receiver, Sender, channel};

use crate::actions;
use egui::{RichText, Ui};
use js_sys::Uint8Array;
use locus_doc::RecipeId;
use locus_doc::generated::{PluginRef, modifier_input};
use locus_plugin_host::archive::Archive;
use locus_plugin_host::core_host::wire::{Reply, Request, parse_reply};
use locus_plugin_host::doc::{defaults, from_stored, resolve, to_paths};
use locus_plugin_host::manifest::{Manifest, api_major};
use locus_plugin_host::types::{FormatInfo, Kind, Param, ParamSpec};
use wasm_bindgen::prelude::*;

use crate::plugin_ui::{
    Done, Panel, PanelAction, Placement, Target, command, doc_parameters, show_panel,
};
use crate::state::AppState;

#[wasm_bindgen(module = "/src/web_plugins/web.js")]
extern "C" {
    #[wasm_bindgen(catch)]
    async fn plugin_call(
        component: Uint8Array,
        request: &str,
        timeout_ms: f64,
    ) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(catch)]
    async fn pick_bytes(accept: &str) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(catch)]
    async fn plugin_ids() -> Result<JsValue, JsValue>;
    #[wasm_bindgen(catch)]
    async fn plugin_read(id: &str) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(catch)]
    async fn plugin_write(id: &str, bytes: Uint8Array) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(catch)]
    async fn plugin_remove(id: &str) -> Result<JsValue, JsValue>;
}

/// Per-call timeout, the §15.4 default.
const TIMEOUT_MS: f64 = 5000.0;

/// An installed plugin: its manifest and its component.
#[derive(Debug, Clone)]
struct WebPlugin {
    manifest: Manifest,
    component: Vec<u8>,
}

/// Results of async browser work, delivered to the UI thread.
#[derive(Debug)]
enum Msg {
    /// The installed plugins read from OPFS, and how many couldn't be read.
    Listed(Vec<WebPlugin>, usize),
    /// `Ok(None)`: the picker was cancelled.
    Picked(Result<Option<Vec<u8>>, String>),
    /// A worker's answer to call `n`.
    Reply(u64, Result<Reply, String>),
    Stored(Result<WebPlugin, String>),
    Removed(Result<String, String>),
}

/// What the call in flight is for.
#[derive(Debug)]
enum Pending {
    /// Checking an archive before storing it.
    Install(Box<Archive>),
    /// Fetching a schema to open the panel.
    Open {
        id: String,
        target: Target,
        stored: Option<PluginRef>,
    },
    /// Running a generator or modifier; applied as one command.
    Run {
        place: Placement,
        input: Option<Vec<locus_doc::generated::GenPath>>,
        params: Vec<Param>,
        manifest: Box<Manifest>,
    },
    /// Asking an exporter for its format, then exporting.
    Format {
        id: String,
        model: locus_plugin_host::types::ExportModel,
    },
    Export {
        format: FormatInfo,
    },
}

#[derive(Debug)]
pub struct WebPlugins {
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    ctx: egui::Context,
    installed: Vec<WebPlugin>,
    panel: Option<Panel>,
    pending: Option<(u64, Pending)>,
    /// When the call in flight started (egui time, seconds).
    started: f64,
    next_call: u64,
    manage: bool,
    confirm: Option<String>,
    message: Option<String>,
}

impl WebPlugins {
    pub fn new(ctx: &egui::Context) -> WebPlugins {
        let (tx, rx) = channel();
        let p = WebPlugins {
            tx,
            rx,
            ctx: ctx.clone(),
            installed: Vec::new(),
            panel: None,
            pending: None,
            started: 0.0,
            next_call: 0,
            manage: false,
            confirm: None,
            message: None,
        };
        p.spawn(async {
            let ids: Vec<String> = match plugin_ids().await {
                Ok(v) => v
                    .as_string()
                    .and_then(|j| serde_json::from_str(&j).ok())
                    .unwrap_or_default(),
                Err(_) => Vec::new(),
            };
            let (mut found, mut bad) = (Vec::new(), 0);
            for id in ids {
                // The stored archive is as untrusted as the original was.
                let parsed = plugin_read(&id)
                    .await
                    .ok()
                    .and_then(|v| v.dyn_into::<Uint8Array>().ok())
                    .and_then(|b| Archive::parse(&b.to_vec()).ok())
                    .filter(|a| a.manifest.id == id);
                match parsed {
                    Some(a) => found.push(WebPlugin {
                        manifest: a.manifest,
                        component: a.wasm,
                    }),
                    None => bad += 1,
                }
            }
            found.sort_by(|a, b| a.manifest.id.cmp(&b.manifest.id));
            Msg::Listed(found, bad)
        });
        p
    }

    fn spawn(&self, fut: impl std::future::Future<Output = Msg> + 'static) {
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        wasm_bindgen_futures::spawn_local(async move {
            let _ = tx.send(fut.await);
            ctx.request_repaint();
        });
    }

    fn get(&self, id: &str) -> Option<&WebPlugin> {
        self.installed.iter().find(|p| p.manifest.id == id)
    }

    /// Sends one request to a fresh worker; `pending` says what it's for.
    fn call(&mut self, component: &[u8], request: &Request, pending: Pending) {
        if self.pending.is_some() {
            self.message = Some("Another plugin call is still running.".into());
            return;
        }
        let Ok(json) = serde_json::to_string(request) else {
            self.message = Some("The request couldn't be encoded.".into());
            return;
        };
        self.next_call += 1;
        let n = self.next_call;
        self.pending = Some((n, pending));
        self.started = self.ctx.input(|i| i.time);
        let bytes = Uint8Array::from(component);
        self.spawn(async move {
            let reply = match plugin_call(bytes, &json, TIMEOUT_MS).await {
                Ok(v) => v
                    .as_string()
                    .ok_or_else(|| "the plugin worker sent garbage".to_owned())
                    .and_then(|j| parse_reply(&j).map_err(|e| e.message)),
                Err(_) => Err("the plugin worker failed".to_owned()),
            };
            Msg::Reply(n, reply)
        });
    }

    // ── Menu ────────────────────────────────────────────────────────────────

    /// The plugin commands that become registry actions (design §6.13).
    pub fn plugin_actions(&self) -> Vec<actions::PluginAction> {
        actions::plugin_actions(self.installed.iter().map(|p| &p.manifest))
    }

    /// UTILITIES ▸ Install plugin… (#325): pick a `.locusplugin`.
    pub fn open_install(&mut self) {
        self.spawn(async {
            let r = match pick_bytes(".locusplugin").await {
                Ok(v) if v.is_null() || v.is_undefined() => Ok(None),
                Ok(v) => match v.dyn_into::<Uint8Array>() {
                    Ok(b) => Ok(Some(b.to_vec())),
                    Err(v) => Err(v
                        .as_string()
                        .unwrap_or_else(|| "Could not read the file.".into())),
                },
                Err(_) => Err("Could not open the file picker.".into()),
            };
            Msg::Picked(r)
        });
    }

    /// UTILITIES ▸ Manage plugins… and OUTPUT ▸ Exporter plugins… (#325).
    pub fn open_manager(&mut self) {
        self.manage = true;
    }

    pub fn menu(&mut self, ui: &mut Ui, reg: &mut actions::Registry, canvas: &AppState) {
        reg.sync_plugins(self.plugin_actions());
        if ui.button("Install plugin…").clicked() {
            ui.close();
            self.open_install();
        }
        if ui.button("Plugin manager…").clicked() {
            ui.close();
            self.open_manager();
        }
        ui.separator();
        let of = |k: Kind| -> Vec<(String, String)> {
            self.installed
                .iter()
                .filter(|p| p.manifest.kinds.contains(&k))
                .map(|p| (p.manifest.id.clone(), p.manifest.name.clone()))
                .collect()
        };
        let (generators, modifiers, exporters) =
            (of(Kind::Generator), of(Kind::Modifier), of(Kind::Exporter));
        if generators.is_empty() && modifiers.is_empty() && exporters.is_empty() {
            ui.weak("No plugins installed.");
        }
        for kind in [
            actions::PluginKind::Generator,
            actions::PluginKind::Modifier,
        ] {
            for a in reg.plugin_actions(kind) {
                let on = reg.enabled(a, canvas);
                let mut r = ui.add_enabled(on.yes(), egui::Button::new(a.label.as_ref()));
                if let actions::Enabled::No(why) = &on {
                    r = r.on_disabled_hover_text(why.as_ref());
                }
                if r.clicked()
                    && let actions::Op::Plugin { plugin, kind } = &a.op
                {
                    ui.close();
                    actions::PluginHost::run_action(self, plugin, *kind, canvas);
                }
            }
        }
        for (id, name) in exporters {
            if ui
                .button(format!("Export with {name}…"))
                .on_hover_text("Validates the drawing, runs the plugin and downloads the file.")
                .clicked()
            {
                ui.close();
                self.export(&id, canvas);
            }
        }
    }

    fn open(&mut self, id: &str, target: Target, stored: Option<PluginRef>) {
        let Some(p) = self.get(id) else {
            self.message = Some("The plugin isn't installed.".into());
            return;
        };
        let kind = if target.modifier() {
            Kind::Modifier
        } else {
            Kind::Generator
        };
        let component = p.component.clone();
        self.call(
            &component,
            &Request::Schema(kind),
            Pending::Open {
                id: id.to_owned(),
                target,
                stored,
            },
        );
    }

    fn export(&mut self, id: &str, canvas: &AppState) {
        let Some(p) = self.get(id) else {
            return;
        };
        let prepared = locus_export::prepare(canvas.document(), false);
        if prepared.blocked() {
            self.message =
                Some("The drawing has export errors. Check it with File > Export first.".into());
            return;
        }
        let model = locus_plugin_host::model::to_model(&prepared.model);
        let component = p.component.clone();
        self.call(
            &component,
            &Request::Format,
            Pending::Format {
                id: id.to_owned(),
                model,
            },
        );
    }

    // ── Results ─────────────────────────────────────────────────────────────

    fn poll(&mut self, canvas: &mut AppState) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Listed(found, bad) => {
                    self.installed = found;
                    if bad > 0 {
                        self.message = Some(format!(
                            "{bad} installed plugin(s) couldn't be read and were skipped."
                        ));
                    }
                }
                Msg::Picked(Ok(None)) => {}
                Msg::Picked(Err(e)) => self.message = Some(e),
                Msg::Picked(Ok(Some(bytes))) => match Archive::parse(&bytes) {
                    Ok(a) => {
                        let component = a.wasm.clone();
                        self.call(
                            &component,
                            &Request::Describe,
                            Pending::Install(Box::new(a)),
                        );
                    }
                    Err(e) => self.message = Some(format!("Couldn't install the plugin: {e}")),
                },
                Msg::Stored(Ok(p)) => {
                    self.message = Some(format!(
                        "Installed {} {} (unsigned).",
                        p.manifest.name, p.manifest.version
                    ));
                    self.installed.retain(|q| q.manifest.id != p.manifest.id);
                    self.installed.push(p);
                    self.installed
                        .sort_by(|a, b| a.manifest.id.cmp(&b.manifest.id));
                }
                Msg::Stored(Err(e)) => {
                    self.message = Some(format!("Couldn't install the plugin: {e}"))
                }
                Msg::Removed(Ok(id)) => self.installed.retain(|p| p.manifest.id != id),
                Msg::Removed(Err(e)) => self.message = Some(format!("Couldn't uninstall: {e}")),
                Msg::Reply(n, reply) => {
                    let Some((m, pending)) = self.pending.take() else {
                        continue;
                    };
                    if m != n {
                        // A cancelled call's late answer.
                        self.pending = Some((m, pending));
                        continue;
                    }
                    self.finish(canvas, pending, reply);
                }
            }
        }
    }

    fn fail(&mut self, e: String) {
        match self.panel.as_mut() {
            Some(p) => p.error = Some(e),
            None => self.message = Some(format!("The plugin call failed: {e}")),
        }
    }

    fn finish(&mut self, canvas: &mut AppState, pending: Pending, reply: Result<Reply, String>) {
        let reply = match reply {
            Ok(r) => r,
            Err(e) => {
                if let Pending::Install(_) = pending {
                    self.message = Some(format!("Couldn't install the plugin: {e}"));
                } else {
                    self.fail(e);
                }
                return;
            }
        };
        match (pending, reply) {
            (Pending::Install(a), Reply::Described { info, api, kinds }) => {
                let m = &a.manifest;
                let problem = if info.id != m.id {
                    Some("the plugin reports a different id")
                } else if info.version != m.version {
                    Some("the plugin reports a different version")
                } else if api_major(&api) != api_major(&m.api) {
                    Some("the plugin was built against a different API major")
                } else if kinds != m.kinds {
                    Some("the kinds in plugin.toml aren't the ones the plugin provides")
                } else {
                    None
                };
                if let Some(p) = problem {
                    self.message = Some(format!(
                        "Couldn't install the plugin: it doesn't match its manifest: {p}"
                    ));
                    return;
                }
                let plugin = WebPlugin {
                    manifest: a.manifest.clone(),
                    component: a.wasm.clone(),
                };
                let bytes = match Archive::write(
                    &a.manifest_toml,
                    &a.wasm,
                    a.icon.as_deref(),
                    a.readme.as_deref(),
                ) {
                    Ok(b) => b,
                    Err(e) => {
                        self.message = Some(format!("Couldn't install the plugin: {e}"));
                        return;
                    }
                };
                self.spawn(async move {
                    let id = plugin.manifest.id.clone();
                    let r = plugin_write(&id, Uint8Array::from(bytes.as_slice()))
                        .await
                        .map(|_| plugin)
                        .map_err(|_| "the browser's storage refused it".to_owned());
                    Msg::Stored(r)
                });
            }
            (Pending::Open { id, target, stored }, Reply::Schema(schema)) => {
                let name = self
                    .get(&id)
                    .map_or_else(|| id.clone(), |p| p.manifest.name.clone());
                let values = match &stored {
                    Some(r) => from_stored(&schema, &r.params),
                    None => defaults(&schema),
                }
                .into_iter()
                .map(|p| p.value)
                .collect();
                self.panel = Some(Panel {
                    id,
                    name,
                    schema,
                    values,
                    target,
                    error: None,
                });
            }
            (
                Pending::Run {
                    place,
                    input,
                    params,
                    manifest,
                },
                reply,
            ) => {
                let info = locus_plugin_host::types::PluginInfo {
                    id: manifest.id.clone(),
                    name: manifest.name.clone(),
                    version: manifest.version.clone(),
                };
                let done = match (reply, &input) {
                    (Reply::Generated(g), None) => Done::generated(&info, &params, &g),
                    (Reply::Paths(out), Some(input)) => Done::modified(&info, &params, input, &out),
                    _ => return self.fail("the plugin gave the wrong kind of answer".into()),
                };
                match command(place, done)
                    .and_then(|cmd| canvas.apply(&cmd).map_err(|e| e.to_string()))
                {
                    Ok(()) => self.panel = None,
                    Err(e) => self.fail(e),
                }
            }
            (Pending::Format { id, model }, Reply::Format(format)) => {
                if let Some(p) = self.get(&id) {
                    let component = p.component.clone();
                    self.call(
                        &component,
                        &Request::Export(model),
                        Pending::Export { format },
                    );
                }
            }
            (Pending::Export { format }, Reply::Bytes(bytes)) => {
                crate::file_io::download_file(
                    &format!("drawing.{}", format.extension),
                    &bytes,
                    &format.mime,
                );
            }
            _ => self.fail("the plugin gave the wrong kind of answer".into()),
        }
    }

    // ── Windows ─────────────────────────────────────────────────────────────

    pub fn windows(&mut self, ctx: &egui::Context, canvas: &mut AppState) {
        self.poll(canvas);
        if self.pending.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
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

    fn panel_window(&mut self, ctx: &egui::Context, canvas: &mut AppState) {
        let running = self
            .pending
            .as_ref()
            .filter(|(_, p)| matches!(p, Pending::Run { .. }))
            .map(|_| {
                std::time::Duration::from_secs_f64((ctx.input(|i| i.time) - self.started).max(0.0))
            });
        let Some(panel) = self.panel.as_mut() else {
            return;
        };
        let action = show_panel(ctx, panel, &doc_parameters(canvas), running);
        let (id, target, schema) = (panel.id.clone(), panel.target.clone(), panel.schema.clone());
        match action {
            PanelAction::None => {}
            PanelAction::Close => self.panel = None,
            PanelAction::Stop => {
                // Drop the result; the worker is ended by its timeout.
                self.pending = None;
                self.fail("Cancelled. Nothing was changed.".into());
            }
            PanelAction::Apply(values) => {
                if let Some(p) = self.panel.as_mut() {
                    p.error = None;
                }
                if let Err(e) = self.run(canvas, &id, &schema, &values, target) {
                    self.fail(e);
                }
            }
        }
    }

    fn run(
        &mut self,
        canvas: &AppState,
        id: &str,
        schema: &[ParamSpec],
        values: &[Param],
        target: Target,
    ) -> Result<(), String> {
        let p = self.get(id).ok_or("the plugin isn't installed")?.clone();
        let params = resolve(schema, values).map_err(|e| e.to_string())?;
        let doc = canvas.document();
        let (sources, input) = if target.modifier() {
            let sources = match &target {
                Target::Modify(curves) => curves.clone(),
                Target::EditModifier(rid) => doc
                    .recipe(*rid)
                    .map(|r| r.sources.clone())
                    .ok_or("the modifier object is gone")?,
                _ => Vec::new(),
            };
            let input = modifier_input(doc, &sources).map_err(|e| e.to_string())?;
            (sources, Some(input))
        } else {
            (Vec::new(), None)
        };
        let request = match &input {
            Some(i) => Request::Modify {
                input: to_paths(i),
                params: params.clone(),
            },
            None => Request::Generate(params.clone()),
        };
        let place = Placement {
            target,
            sources,
            layer: canvas.current_layer(),
            origin: canvas.insert_point(),
        };
        self.call(
            &p.component,
            &request,
            Pending::Run {
                place,
                input,
                params,
                manifest: Box::new(p.manifest.clone()),
            },
        );
        Ok(())
    }

    fn manage_window(&mut self, ctx: &egui::Context) {
        if !self.manage {
            return;
        }
        let mut open = true;
        let mut uninstall = None;
        egui::Window::new("Plugin manager")
            .open(&mut open)
            .collapsible(false)
            .show(ctx, |ui| {
                if self.installed.is_empty() {
                    ui.weak("No plugins installed.");
                }
                for p in &self.installed {
                    let m = &p.manifest;
                    ui.group(|ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(&m.name).strong());
                            ui.label(&m.version);
                            ui.colored_label(ui.visuals().warn_fg_color, "Unsigned");
                        });
                        let kinds: Vec<&str> = m.kinds.iter().map(|k| k.name()).collect();
                        ui.weak(format!("{} · {} · {} · {}", m.id, kinds.join(", "), m.author, m.licence));
                        ui.weak("Capabilities: none (no files, network, clock or system access)");
                        ui.horizontal(|ui| {
                            if self.confirm.as_deref() == Some(m.id.as_str()) {
                                if ui.button("Really uninstall").clicked() {
                                    uninstall = Some(m.id.clone());
                                }
                                if ui.button("Keep").clicked() {
                                    self.confirm = None;
                                }
                            } else if ui.button("Uninstall…").clicked() {
                                self.confirm = Some(m.id.clone());
                            }
                        });
                    });
                }
                ui.weak(
                    "Plugins run in a browser worker with nothing to call, 256 MB and 5 s per call. \
                     They are stored in this browser only.",
                );
            });
        if let Some(id) = uninstall {
            self.confirm = None;
            self.spawn(async move {
                let r = plugin_remove(&id)
                    .await
                    .map(|_| id)
                    .map_err(|_| "the browser's storage refused it".to_owned());
                Msg::Removed(r)
            });
        }
        if !open {
            self.manage = false;
            self.confirm = None;
        }
    }

    // ── Properties ──────────────────────────────────────────────────────────

    /// The selected plugin object's section of the Properties panel.
    pub fn properties_ui(&mut self, ui: &mut Ui, canvas: &mut AppState) {
        let Some((rid, pref, modifier)) = canvas.selected_plugin_object() else {
            return;
        };
        ui.label(
            RichText::new(if modifier {
                "Modifier"
            } else {
                "Plugin object"
            })
            .strong(),
        );
        let kind = if modifier {
            Kind::Modifier
        } else {
            Kind::Generator
        };
        let installed = self
            .get(&pref.id)
            .is_some_and(|p| p.manifest.kinds.contains(&kind));
        if installed {
            if ui.button("Edit parameters…").clicked() {
                let target = if modifier {
                    Target::EditModifier(rid)
                } else {
                    Target::Edit(rid)
                };
                self.open(&pref.id, target, Some(pref.clone()));
            }
        } else {
            ui.colored_label(ui.visuals().warn_fg_color, "⚠ Plugin missing")
                .on_hover_text(
                    "Shown and exported from its saved geometry. Install the plugin to change it.",
                );
        }
        if modifier && canvas.plugin_object_stale(rid) {
            ui.colored_label(ui.visuals().warn_fg_color, "⚠ Out of date")
                .on_hover_text(
                    "Its source changed. In the browser, modifiers re-run when you apply \
                     Edit parameters.",
                );
        }
        canvas.plugin_object_ui(ui, &pref);
        explode_button(ui, canvas, rid, &mut self.message);
        ui.separator();
    }
}

fn explode_button(ui: &mut Ui, canvas: &mut AppState, rid: RecipeId, message: &mut Option<String>) {
    if ui
        .button("Explode")
        .on_hover_text(
            "Turn it into ordinary, editable geometry, keeping the plugin's constraints \
             where they still fit. One undo step.",
        )
        .clicked()
    {
        match canvas.explode_plugin_object(rid) {
            Ok(0) => {}
            Ok(n) => {
                *message = Some(format!(
                    "Exploded. {n} of the plugin's constraints were left out."
                ))
            }
            Err(e) => *message = Some(format!("Couldn't explode it: {e}")),
        }
    }
}

impl actions::PluginHost for WebPlugins {
    fn run_action(&mut self, plugin: &str, kind: actions::PluginKind, canvas: &AppState) {
        match kind {
            actions::PluginKind::Generator => self.open(plugin, Target::Insert, None),
            actions::PluginKind::Modifier => {
                self.open(plugin, Target::Modify(canvas.selected_curves()), None);
            }
        }
    }
}
