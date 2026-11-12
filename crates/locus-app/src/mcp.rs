// SPDX-License-Identifier: AGPL-3.0-or-later
//! The MCP server in the desktop app (SPEC §16, #229, #230).
//!
//! - **Off by default.** The user turns it on in View ▸ MCP server…, after a
//!   dialog saying the app sends nothing anywhere and that what their AI
//!   client does with returned data is outside the app (§16.4). The choice
//!   is remembered for this install.
//! - While it runs, the status bar always shows it, and it listens on
//!   127.0.0.1 only, with a bearer token made fresh each time it starts.
//! - Read, write and export are granted separately (read only by default);
//!   writes are asked for each time, once per session, or not at all, and
//!   export always asks unless the user allows it.
//! - The AI's tool calls run here, in the frame loop, through the canvas:
//!   each is one labelled undo step and the user watches it happen.
//!
//! Never logs the token or anything the AI sent (§16.4).

use std::sync::Arc;

use locus_doc::{Applied, Command, DocError, LayerId, Sketcher};
use locus_mcp::attach::{self, Jobs, Prompts};
use locus_mcp::http::Attached;
use locus_mcp::plugins::RegistryPlugins;
use locus_mcp::{ApprovalMode, Grants, LocusServer, Options, Plugins, Scope, Workspace};
use serde::{Deserialize, Serialize};

use crate::canvas::Canvas;

const STORAGE_KEY: &str = "locus.mcp";
/// A fixed default so client configurations keep working across launches.
pub const DEFAULT_PORT: u16 = 47811;

/// What is remembered for this install.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Saved {
    enabled: bool,
    port: u16,
    grants: Grants,
}

impl Default for Saved {
    fn default() -> Self {
        Self {
            enabled: false,
            port: DEFAULT_PORT,
            grants: Grants::default(),
        }
    }
}

struct Live {
    _http: Attached,
    url: String,
    token: String,
    server: LocusServer,
    jobs: Jobs,
    prompts: Arc<Prompts>,
}

#[derive(Default)]
pub struct Mcp {
    saved: Saved,
    live: Option<Live>,
    window: bool,
    confirm_enable: bool,
    show_token: bool,
    error: Option<String>,
    plugins: Option<RegistryPlugins>,
    exporters: Option<locus_export::Registry>,
}

/// The canvas as the server's document (see `locus_mcp::Workspace`).
struct CanvasWs<'a> {
    canvas: &'a mut Canvas,
    exporters: &'a locus_export::Registry,
    plugins: Option<&'a mut RegistryPlugins>,
}

impl Workspace for CanvasWs<'_> {
    fn sketcher(&self) -> &Sketcher {
        self.canvas.sketcher()
    }

    fn apply(&mut self, cmd: &Command) -> Result<Applied, DocError> {
        self.canvas.apply_ai(cmd)
    }

    fn default_layer(&self) -> Option<LayerId> {
        Some(self.canvas.ai_layer())
    }

    fn exporters(&self) -> &locus_export::Registry {
        self.exporters
    }

    fn plugins(&mut self) -> Option<&mut dyn Plugins> {
        match &mut self.plugins {
            Some(p) => Some(&mut **p),
            None => None,
        }
    }
}

fn scopes_label(g: Grants) -> String {
    let s: Vec<&str> = [(g.read, "read"), (g.write, "write"), (g.export, "export")]
        .into_iter()
        .filter_map(|(on, n)| on.then_some(n))
        .collect();
    if s.is_empty() {
        "no access".into()
    } else {
        s.join(", ")
    }
}

impl Mcp {
    /// Restore the saved choice; start the server if the user left it on.
    pub fn load(ctx: &egui::Context, storage: Option<&dyn eframe::Storage>) -> Self {
        let saved = storage
            .and_then(|s| s.get_string(STORAGE_KEY))
            .and_then(|j| serde_json::from_str(&j).ok())
            .unwrap_or_default();
        let mut m = Self {
            saved,
            ..Self::default()
        };
        if m.saved.enabled {
            m.start(ctx);
        }
        m
    }

    pub fn save(&self, storage: &mut dyn eframe::Storage) {
        if let Ok(j) = serde_json::to_string(&self.saved) {
            storage.set_string(STORAGE_KEY, j);
        }
    }

    fn start(&mut self, ctx: &egui::Context) {
        self.error = None;
        let token = match locus_mcp::http::new_token() {
            Ok(t) => t,
            Err(e) => {
                self.error = Some(e.to_string());
                return;
            }
        };
        let wake = {
            let ctx = ctx.clone();
            move || ctx.request_repaint()
        };
        let (backend, jobs) = attach::jobs(wake.clone());
        let prompts = Prompts::new(wake);
        let server = LocusServer::new(
            backend,
            prompts.clone(),
            self.saved.grants,
            Options::default(),
        );
        match Attached::start(self.saved.port, server.clone(), token.clone()) {
            Ok(http) => {
                self.live = Some(Live {
                    url: http.url(),
                    _http: http,
                    token,
                    server,
                    jobs,
                    prompts,
                });
                self.saved.enabled = true;
                if self.plugins.is_none() {
                    self.plugins = crate::plugins::plugin_dir().map(|d| {
                        RegistryPlugins::new(locus_plugin_host::registry::Registry::new(d))
                    });
                }
            }
            Err(e) => {
                self.error = Some(format!(
                    "The MCP server couldn't start: {e}. Another program may be using port {}; choose another.",
                    self.saved.port
                ));
                self.saved.enabled = false;
            }
        }
    }

    fn stop(&mut self) {
        if let Some(live) = self.live.take() {
            live.prompts.decline_all();
        }
        self.saved.enabled = false;
    }

    fn set_grants(&mut self, g: Grants) {
        self.saved.grants = g;
        if let Some(live) = &self.live {
            live.server.set_grants(g);
        }
    }

    pub fn menu(&mut self, ui: &mut egui::Ui) {
        if ui.button("MCP server (AI clients)…").clicked() {
            self.window = true;
        }
    }

    /// The always-visible indicator while the server runs (§16.4).
    pub fn status(&mut self, ui: &mut egui::Ui) {
        let Some(live) = &self.live else {
            return;
        };
        let waiting = live.prompts.current().is_some();
        let text = if waiting {
            "● MCP: approval needed".to_owned()
        } else {
            format!("● MCP on ({})", scopes_label(self.saved.grants))
        };
        let colour = if waiting {
            ui.visuals().warn_fg_color
        } else {
            egui::Color32::from_rgb(40, 170, 90)
        };
        let r = ui
            .add(egui::Button::new(egui::RichText::new(text).color(colour)).frame(false))
            .on_hover_text(format!(
                "AI clients can connect at {}. Click for settings.",
                live.url
            ));
        if r.clicked() {
            self.window = true;
        }
    }

    /// Run queued AI calls, then draw the dialogs.
    pub fn update(&mut self, ctx: &egui::Context, canvas: &mut Canvas) {
        if let Some(live) = &self.live {
            let exporters = self
                .exporters
                .get_or_insert_with(locus_export::Registry::builtin);
            let mut ws = CanvasWs {
                canvas,
                exporters,
                plugins: self.plugins.as_mut(),
            };
            live.jobs.drain(&mut ws);
        }
        self.approval(ctx);
        self.enable_dialog(ctx);
        self.settings_window(ctx);
    }

    fn approval(&self, ctx: &egui::Context) {
        let Some(live) = &self.live else { return };
        let Some(req) = live.prompts.current() else {
            return;
        };
        let mut answer = None;
        egui::Window::new("Allow the AI client?")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 60.0))
            .show(ctx, |ui| {
                let what = match req.scope {
                    Scope::Export => "export the document and receive the file",
                    _ => "change the document",
                };
                ui.label(format!("Your AI client wants to {what}:"));
                ui.strong(&req.summary);
                if req.scope == Scope::Write {
                    ui.weak("It will be one step you can undo.");
                }
                ui.horizontal(|ui| {
                    if ui.button("Allow").clicked() {
                        answer = Some(true);
                    }
                    if ui.button("Deny").clicked() {
                        answer = Some(false);
                    }
                });
            });
        if let Some(yes) = answer {
            live.prompts.answer(yes);
        }
    }

    fn enable_dialog(&mut self, ctx: &egui::Context) {
        if !self.confirm_enable {
            return;
        }
        let (mut yes, mut no) = (false, false);
        egui::Window::new("Turn on the MCP server?")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.set_max_width(420.0);
                ui.label(
                    "This lets an AI client on this computer (Claude Desktop, Claude Code, a local LLM app…) \
                     read your open document and, if you allow it, change or export it.",
                );
                ui.add_space(4.0);
                ui.label(
                    "Locus itself sends nothing anywhere: it has no AI model, no cloud service and no telemetry. \
                     It only listens on this computer (127.0.0.1), for clients that have the access token.",
                );
                ui.add_space(4.0);
                ui.label(
                    "What your AI client does with the data it reads is outside Locus. \
                     If it uses a cloud model, your drawing goes to that service.",
                );
                ui.add_space(4.0);
                ui.weak("You can turn it off at any time. It starts with read access only.");
                ui.horizontal(|ui| {
                    yes = ui.button("Turn on").clicked();
                    no = ui.button("Cancel").clicked();
                });
            });
        if yes {
            self.confirm_enable = false;
            self.start(ctx);
        } else if no {
            self.confirm_enable = false;
        }
    }

    fn settings_window(&mut self, ctx: &egui::Context) {
        if !self.window {
            return;
        }
        let mut open = true;
        let mut toggle = None;
        let mut grants = self.saved.grants;
        egui::Window::new("MCP server")
            .open(&mut open)
            .resizable(true)
            .default_width(460.0)
            .show(ctx, |ui| {
                match &self.live {
                    None => {
                        ui.label("Off. AI clients can't connect.");
                        ui.horizontal(|ui| {
                            ui.label("Port");
                            ui.add(egui::DragValue::new(&mut self.saved.port).range(1024..=65535));
                        });
                        if ui.button("Turn on…").clicked() {
                            toggle = Some(true);
                        }
                    }
                    Some(live) => {
                        ui.horizontal(|ui| {
                            ui.colored_label(egui::Color32::from_rgb(40, 170, 90), "● On");
                            ui.monospace(&live.url);
                            if ui.button("Turn off").clicked() {
                                toggle = Some(false);
                            }
                        });
                    }
                }
                if let Some(e) = &self.error {
                    ui.colored_label(ui.visuals().error_fg_color, e);
                }
                ui.separator();
                ui.strong("Access");
                ui.checkbox(&mut grants.read, "Read the document");
                ui.checkbox(&mut grants.write, "Change the document");
                ui.checkbox(&mut grants.export, "Export files");
                ui.add_enabled_ui(grants.write, |ui| {
                    ui.label("Before a change:");
                    ui.radio_value(&mut grants.approval, ApprovalMode::AskEachWrite, "Ask every time");
                    ui.radio_value(
                        &mut grants.approval,
                        ApprovalMode::AskPerSession,
                        "Ask once until the server is turned off",
                    );
                    ui.radio_value(&mut grants.approval, ApprovalMode::Allow, "Don't ask");
                });
                ui.add_enabled_ui(grants.export, |ui| {
                    ui.checkbox(&mut grants.export_without_asking, "Export without asking");
                });
                if let Some(live) = &self.live {
                    ui.separator();
                    ui.strong("Connect a client");
                    ui.horizontal(|ui| {
                        ui.label("Token");
                        if self.show_token {
                            ui.monospace(&live.token);
                        } else {
                            ui.monospace("••••••••");
                        }
                        ui.checkbox(&mut self.show_token, "Show");
                        if ui.button("Copy token").clicked() {
                            ui.ctx().copy_text(live.token.clone());
                        }
                    });
                    ui.weak("A new token is made each time the server starts; update your client then.");
                    let json = client_json(&live.url, &live.token);
                    let cli = claude_code_command(&live.url, &live.token);
                    ui.horizontal(|ui| {
                        if ui.button("Copy JSON config").on_hover_text("For clients configured with an mcpServers JSON entry (Claude Code .mcp.json, LM Studio mcp.json, …)").clicked() {
                            ui.ctx().copy_text(json);
                        }
                        if ui.button("Copy Claude Code command").clicked() {
                            ui.ctx().copy_text(cli);
                        }
                    });
                    ui.weak("See docs/mcp.md for other clients.");
                    let log = live.server.activity();
                    if !log.is_empty() {
                        ui.separator();
                        ui.strong("Recent AI calls");
                        egui::ScrollArea::vertical().max_height(140.0).show(ui, |ui| {
                            for a in log.iter().rev() {
                                let mark = if a.ok { "✓" } else { "✗" };
                                ui.label(format!("{mark} {}", a.summary));
                            }
                        });
                    }
                }
            });
        if grants != self.saved.grants {
            self.set_grants(grants);
        }
        match toggle {
            Some(true) => self.confirm_enable = true,
            Some(false) => self.stop(),
            None => {}
        }
        if !open {
            self.window = false;
        }
    }
}

/// An `mcpServers` entry for clients configured with JSON.
pub fn client_json(url: &str, token: &str) -> String {
    serde_json::json!({
        "mcpServers": {
            "locus": {
                "type": "http",
                "url": url,
                "headers": { "Authorization": format!("Bearer {token}") }
            }
        }
    })
    .to_string()
}

pub fn claude_code_command(url: &str, token: &str) -> String {
    format!(
        "claude mcp add --transport http locus {url} --header \"Authorization: Bearer {token}\""
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn off_and_read_only_by_default() {
        let s = Saved::default();
        assert!(!s.enabled);
        assert_eq!(s.grants, Grants::default());
        assert!(s.grants.read && !s.grants.write && !s.grants.export);
    }

    #[test]
    fn saved_settings_round_trip_and_tolerate_junk() {
        let s = Saved {
            enabled: true,
            port: 50000,
            grants: Grants::headless(true, true, false),
        };
        let j = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<Saved>(&j).unwrap(), s);
        assert_eq!(
            serde_json::from_str::<Saved>("{}").unwrap(),
            Saved::default()
        );
    }

    #[test]
    fn snippets_carry_url_and_token() {
        let j: serde_json::Value =
            serde_json::from_str(&client_json("http://127.0.0.1:1/mcp", "tok")).unwrap();
        assert_eq!(j["mcpServers"]["locus"]["url"], "http://127.0.0.1:1/mcp");
        assert_eq!(
            j["mcpServers"]["locus"]["headers"]["Authorization"],
            "Bearer tok"
        );
        assert!(claude_code_command("u", "t").contains("--transport http locus u"));
    }

    #[test]
    fn ai_edits_go_through_the_canvas_as_one_undo_step() {
        let mut canvas = Canvas::default();
        let before = canvas.document().entities().len();
        let (backend, jobs) = attach::jobs(|| {});
        let server = LocusServer::new(
            backend,
            Arc::new(locus_mcp::NeverApprove),
            Grants::headless(true, true, false),
            Options::default(),
        );
        // The AI client's call waits on another thread, as with HTTP.
        let call = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let args = serde_json::json!({
                "entities": [{"type": "line", "start": [0.0, 0.0], "end": [10.0, 0.0]}]
            });
            rt.block_on(server.call("add_entities", args.as_object().unwrap().clone()))
                .unwrap()
                .is_ok()
        });
        let exporters = locus_export::Registry::builtin();
        while !call.is_finished() {
            let mut ws = CanvasWs {
                canvas: &mut canvas,
                exporters: &exporters,
                plugins: None,
            };
            jobs.drain(&mut ws);
            std::thread::yield_now();
        }
        assert!(call.join().unwrap());
        assert_eq!(canvas.document().entities().len(), before + 3);
        assert_eq!(canvas.undo_label(), Some("AI: add 1 entity"));
        canvas.undo();
        assert_eq!(canvas.document().entities().len(), before);
    }
}
