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
//! - **Allow Docker clients** (off by default, #270): also accept the
//!   `host.docker.internal` host name, so clients in Docker Desktop
//!   containers (e.g. Open WebUI) can connect. The bind stays 127.0.0.1;
//!   the token, `Origin` refusal and everything else still apply.
//! - **Allow network clients** (off by default, #272): also listen on one
//!   private address of this PC for clients in an allowlist (empty by
//!   default). Plain HTTP on a network can be sniffed, so the dialog says so
//!   and recommends an SSH tunnel or a VPN (e.g. Tailscale) instead. A token
//!   can be kept across restarts so the other machine's config stays valid.
//! - The AI's tool calls run here, in the frame loop, through the canvas:
//!   each is one labelled undo step and the user watches it happen.
//!
//! Never logs the token or anything the AI sent (§16.4).

use std::sync::Arc;

use locus_doc::{Applied, Command, DocError, LayerId, Sketcher};
use locus_mcp::attach::{self, Jobs, Prompts};
use locus_mcp::http::{Access, Attached, Cidr, DOCKER_HOST, Network};
use locus_mcp::plugins::RegistryPlugins;
use locus_mcp::{ApprovalMode, Grants, LocusServer, Options, Plugins, Scope, Workspace};
use serde::{Deserialize, Serialize};

use crate::state::AppState;

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
    /// Accept `Host: host.docker.internal` (#270). Off by default.
    allow_docker: bool,
    /// Also listen on `network_bind` for allowlisted clients (#272). Off.
    allow_network: bool,
    /// The private address to listen on for network clients.
    network_bind: Option<std::net::Ipv4Addr>,
    /// Client IPs or CIDR ranges (private only). Empty by default.
    allowlist: Vec<String>,
    /// Reuse the token across restarts (stored with these settings).
    keep_token: bool,
    /// The kept token, only while `keep_token` is on.
    token: Option<String>,
}

impl Default for Saved {
    fn default() -> Self {
        Self {
            enabled: false,
            port: DEFAULT_PORT,
            grants: Grants::default(),
            allow_docker: false,
            allow_network: false,
            network_bind: None,
            allowlist: Vec::new(),
            keep_token: false,
            token: None,
        }
    }
}

impl Saved {
    /// The network part of the access rules, or why it can't be used.
    fn network(&self) -> Result<Option<Network>, String> {
        if !self.allow_network {
            return Ok(None);
        }
        let bind = self
            .network_bind
            .ok_or("choose which network address to listen on")?;
        let allow = self
            .allowlist
            .iter()
            .map(|s| s.parse::<Cidr>())
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Some(Network { bind, allow }))
    }
}

struct Live {
    http: Attached,
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
    /// The allowlist entry being typed, and why it was refused.
    new_client: String,
    client_error: Option<String>,
    plugins: Option<RegistryPlugins>,
    exporters: Option<locus_export::Registry>,
}

/// The canvas as the server's document (see `locus_mcp::Workspace`).
struct CanvasWs<'a> {
    canvas: &'a mut AppState,
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
        let network = match self.saved.network() {
            Ok(n) => n,
            Err(e) => {
                self.error = Some(format!("Network clients: {e}."));
                self.saved.enabled = false;
                return;
            }
        };
        let kept = self.saved.token.clone().filter(|_| self.saved.keep_token);
        let token = match kept.map_or_else(locus_mcp::http::new_token, Ok) {
            Ok(t) => t,
            Err(e) => {
                self.error = Some(e.to_string());
                return;
            }
        };
        self.saved.token = self.saved.keep_token.then(|| token.clone());
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
        let access = Access {
            allow_docker: self.saved.allow_docker,
            network,
        };
        match Attached::start(self.saved.port, server.clone(), token.clone(), access) {
            Ok(http) => {
                self.live = Some(Live {
                    url: http.url(),
                    http,
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

    /// A new token; restarts a running server so the old one stops working.
    fn regenerate_token(&mut self, ctx: &egui::Context) {
        self.saved.token = None;
        if self.live.is_some() {
            self.stop();
            self.start(ctx);
        }
    }

    /// The network-clients controls, shown while the server is off (#272).
    fn network_settings(&mut self, ui: &mut egui::Ui) {
        ui.checkbox(
            &mut self.saved.allow_network,
            "Allow network clients (other computers on this network)",
        )
        .on_hover_text(
            "For AI clients on other computers on your local network. Plain HTTP: not encrypted. \
             An SSH tunnel or a VPN such as Tailscale is safer and needs no setting.",
        );
        if !self.saved.allow_network {
            return;
        }
        ui.indent("mcp-network", |ui| {
            ui.colored_label(
                ui.visuals().warn_fg_color,
                "Unencrypted: anyone watching your network can read the drawing and the token.",
            );
            let addrs = locus_mcp::http::private_addresses();
            ui.horizontal(|ui| {
                ui.label("Listen on");
                let shown = self
                    .saved
                    .network_bind
                    .map_or_else(|| "choose…".to_owned(), |a| a.to_string());
                egui::ComboBox::from_id_salt("mcp-bind")
                    .selected_text(shown)
                    .show_ui(ui, |ui| {
                        for a in &addrs {
                            ui.selectable_value(
                                &mut self.saved.network_bind,
                                Some(*a),
                                a.to_string(),
                            );
                        }
                    });
            });
            if addrs.is_empty() {
                ui.weak("This computer has no private network address.");
            }
            ui.label("Allowed clients (IP or range, e.g. 192.168.1.20 or 192.168.1.0/24):");
            if self.saved.allowlist.is_empty() {
                ui.weak("None yet: no other computer can connect.");
            }
            let mut remove = None;
            for (i, c) in self.saved.allowlist.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.monospace(c);
                    if ui.small_button("Remove").clicked() {
                        remove = Some(i);
                    }
                });
            }
            if let Some(i) = remove {
                self.saved.allowlist.remove(i);
            }
            ui.horizontal(|ui| {
                ui.text_edit_singleline(&mut self.new_client);
                if ui.button("Add").clicked() {
                    match self.new_client.parse::<Cidr>() {
                        Ok(c) => {
                            let c = c.to_string();
                            if !self.saved.allowlist.contains(&c) {
                                self.saved.allowlist.push(c);
                            }
                            self.new_client.clear();
                            self.client_error = None;
                        }
                        Err(e) => self.client_error = Some(e),
                    }
                }
            });
            if let Some(e) = &self.client_error {
                ui.colored_label(ui.visuals().error_fg_color, e);
            }
        });
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
        let network = live
            .http
            .network_url()
            .map(|_| format!(" · network: {} connected", live.http.network_clients()))
            .unwrap_or_default();
        let text = if waiting {
            "● MCP: approval needed".to_owned()
        } else {
            format!("● MCP on ({}){network}", scopes_label(self.saved.grants))
        };
        let colour = if waiting {
            ui.visuals().warn_fg_color
        } else {
            egui::Color32::from_rgb(40, 170, 90)
        };
        let r = ui
            .add(egui::Button::new(egui::RichText::new(text).color(colour)).frame(false))
            .on_hover_text(if self.saved.allow_docker {
                format!(
                    "AI clients can connect at {} (Docker clients: {}). Click for settings.",
                    live.url,
                    docker_url(live.url.as_str())
                )
            } else {
                format!(
                    "AI clients can connect at {}. Click for settings.",
                    live.url
                )
            });
        if r.clicked() {
            self.window = true;
        }
    }

    /// Run queued AI calls, then draw the dialogs.
    pub fn update(&mut self, ctx: &egui::Context, canvas: &mut AppState) {
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
        crate::dialog::Dialog::new("Allow the AI client?")
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
        crate::dialog::Dialog::new("Turn on the MCP server?")
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
                if self.saved.allow_docker {
                    ui.add_space(4.0);
                    ui.label(
                        "Docker clients are allowed: programs running in Docker containers on this computer                          can also try to connect (through host.docker.internal). They still need the access token.",
                    );
                }
                if self.saved.allow_network {
                    ui.add_space(4.0);
                    ui.colored_label(
                        ui.visuals().warn_fg_color,
                        "Network clients are allowed: Locus will also listen on your local network, \
                         for the computers in your list. The connection is plain HTTP, NOT encrypted: \
                         anyone who can watch your network traffic can read your drawing and copy the \
                         access token.",
                    );
                    ui.label(
                        "Safer: leave this off and reach Locus through an SSH tunnel \
                         (ssh -L 47811:127.0.0.1:47811 you@this-pc) or a VPN such as Tailscale. \
                         Those work without this setting.",
                    );
                }
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
        let mut keep = self.saved.keep_token;
        let mut regen = false;
        crate::dialog::Dialog::new("MCP server")
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
                        ui.checkbox(
                            &mut self.saved.allow_docker,
                            "Allow Docker clients (host.docker.internal)",
                        )
                        .on_hover_text(
                            "For AI clients running in Docker Desktop on this computer, such as Open WebUI.                              Any program in a container here can then try to connect; it still needs the token.",
                        );
                        self.network_settings(ui);
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
                        if self.saved.allow_docker {
                            ui.horizontal(|ui| {
                                ui.label("Docker clients:");
                                ui.monospace(docker_url(&live.url));
                            });
                        }
                        if let Some(u) = live.http.network_url() {
                            ui.horizontal(|ui| {
                                ui.label("Network clients:");
                                ui.monospace(&u);
                            });
                            ui.colored_label(
                                ui.visuals().warn_fg_color,
                                format!(
                                    "Unencrypted HTTP · allowed: {} · {} connected",
                                    if self.saved.allowlist.is_empty() {
                                        "nobody yet".to_owned()
                                    } else {
                                        self.saved.allowlist.join(", ")
                                    },
                                    live.http.network_clients()
                                ),
                            );
                        }
                        ui.weak("Turn the server off to change the port, Docker or network access.");
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
                    ui.horizontal(|ui| {
                        if ui
                            .button("Regenerate token")
                            .on_hover_text("Clients using the old token stop working.")
                            .clicked()
                        {
                            regen = true;
                        }
                        ui.checkbox(&mut keep, "Keep this token after restarts")
                            .on_hover_text("Stored with Locus's settings on this computer.");
                    });
                    if !keep {
                        ui.weak("A new token is made each time the server starts; update your client then.");
                    }
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
        if keep != self.saved.keep_token {
            self.saved.keep_token = keep;
            self.saved.token = if keep {
                self.live.as_ref().map(|l| l.token.clone())
            } else {
                None
            };
        }
        if regen {
            self.regenerate_token(ctx);
        }
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

/// The URL a client in a Docker Desktop container uses for `url`.
pub fn docker_url(url: &str) -> String {
    url.replacen("127.0.0.1", DOCKER_HOST, 1)
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
        assert!(!s.allow_docker, "Docker clients are opt-in (#270)");
        assert!(!s.allow_network, "network clients are opt-in (#272)");
        assert!(s.allowlist.is_empty() && s.network_bind.is_none());
        assert!(!s.keep_token && s.token.is_none());
        assert_eq!(s.network(), Ok(None));
        assert!(s.grants.read && !s.grants.write && !s.grants.export);
    }

    #[test]
    fn saved_settings_round_trip_and_tolerate_junk() {
        let s = Saved {
            enabled: true,
            port: 50000,
            grants: Grants::headless(true, true, false),
            allow_docker: true,
            ..Saved::default()
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
        assert_eq!(
            docker_url("http://127.0.0.1:47811/mcp"),
            "http://host.docker.internal:47811/mcp"
        );
    }

    #[test]
    fn ai_edits_go_through_the_canvas_as_one_undo_step() {
        let mut canvas = AppState::default();
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

    #[test]
    fn network_settings_persist_and_validate() {
        let mut s = Saved {
            allow_network: true,
            ..Saved::default()
        };
        assert!(s.network().is_err(), "an address must be chosen");
        s.network_bind = Some(std::net::Ipv4Addr::new(192, 168, 1, 10));
        let n = s.network().unwrap().unwrap();
        assert!(n.allow.is_empty(), "empty allowlist admits nobody");
        s.allowlist = vec!["192.168.1.20".into(), "192.168.1.0/24".into()];
        assert_eq!(s.network().unwrap().unwrap().allow.len(), 2);
        let j = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<Saved>(&j).unwrap(), s);
        s.allowlist.push("8.8.8.8".into());
        assert!(s.network().is_err(), "public client refused");
    }
}
