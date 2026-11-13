// SPDX-License-Identifier: AGPL-3.0-or-later
//! App shell (#65, SPEC §12.1): egui_dock layout with the canvas in the
//! centre, properties and constraints panels, a menu bar and a status bar.
//!
//! The shell never mutates a document directly; once `locus-doc` lands every
//! edit goes through its command API (SPEC §11).

use crate::actions::{self, Cx, Registry, Style};
use crate::file_io::FileIo;
use crate::layout::{self, Tab, default_layout};
use crate::settings::{Settings, Theme, Units};
use crate::state::AppState;
use crate::views::layers::LayersView;
use crate::views::parameters::ParametersView;
use crate::views::patterns::PatternsView;
use egui_dock::{DockArea, DockState, TabViewer};
use locus_geom::Vec2;

/// What the status bar shows, refreshed by the canvas each frame.
#[derive(Debug, Default)]
pub struct Status {
    pub cursor: Option<Vec2>,
    pub extent_mm: f64,
    /// Minor grid step, already formatted ("10 mm").
    pub grid: String,
    /// Smoothed CPU time per frame in ms (the 5 000-entity smoothness check, #66).
    pub frame_ms: f32,
}

/// The registry's view of the app, borrowing the fields actions act on
/// (disjoint from `registry`, so both can be used at once).
macro_rules! cx {
    ($app:expr) => {
        Cx {
            state: &mut $app.state,
            settings: &mut $app.settings,
            plugins: Some(&mut $app.plugins),
        }
    };
}

/// Per-pane widget state, kept across frames; never part of the document.
#[derive(Default)]
struct Panes {
    layers: LayersView,
    parameters: ParametersView,
    patterns: PatternsView,
}

pub struct App {
    dock: DockState<Tab>,
    /// Every command the UI offers (#294, design §7.1).
    registry: Registry,
    settings: Settings,
    /// The one mutable state (design §7.3).
    state: AppState,
    /// The panes' transient widget state (design §7.3).
    panes: Panes,
    status: Status,
    files: FileIo,
    #[cfg(not(target_arch = "wasm32"))]
    plugins: crate::plugins::Plugins,
    #[cfg(target_arch = "wasm32")]
    plugins: crate::web_plugins::WebPlugins,
    #[cfg(not(target_arch = "wasm32"))]
    mcp: crate::mcp::Mcp,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        crate::fonts::install(&cc.egui_ctx);
        let settings = Settings::load(cc.storage);
        cc.egui_ctx.set_theme(settings.theme.preference());
        let dock = cc.storage.map_or_else(default_layout, |s| {
            layout::load(
                s.get_string(layout::LAYOUT_V2_KEY).as_deref(),
                s.get_string(layout::LAYOUT_V1_KEY).as_deref(),
            )
        });
        #[allow(unused_mut)] // mutated only on desktop
        let mut app = Self {
            dock,
            registry: Registry::default(),
            panes: Panes::default(),
            settings,
            state: {
                let mut c = AppState::default();
                c.install_fonts(&cc.egui_ctx);
                c
            },
            status: Status::default(),
            files: FileIo::new(&cc.egui_ctx, cc.storage),
            #[cfg(not(target_arch = "wasm32"))]
            plugins: crate::plugins::Plugins::default(),
            #[cfg(target_arch = "wasm32")]
            plugins: crate::web_plugins::WebPlugins::new(&cc.egui_ctx),
            #[cfg(not(target_arch = "wasm32"))]
            mcp: crate::mcp::Mcp::load(&cc.egui_ctx, cc.storage),
        };
        #[cfg(not(target_arch = "wasm32"))]
        app.plugins.attach(&mut app.state, &mut app.files);
        app
    }

    /// Opens the first command-line argument that names a `.locus` file, as
    /// the OS passes one on a file-association double-click (packaging #245).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn open_startup_file(&mut self, args: impl Iterator<Item = String>) {
        if let Some(path) = startup_file(args) {
            self.files.open_startup_path(&mut self.state, path);
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn load_stress(&mut self) {
        self.state.load(crate::demo::stress(5000));
    }

    fn toggle_tab(&mut self, tab: Tab) {
        if let Some(loc) = self.dock.find_tab(&tab) {
            self.dock.remove_tab(loc);
        } else {
            self.dock.push_to_focused_leaf(tab);
        }
    }

    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                self.files.file_menu(ui, &mut self.state, &self.settings);
                ui.separator();
                ui.menu_button("Samples", |ui| {
                    if ui.button("Demo sketch").clicked() {
                        let ctx = ui.ctx().clone();
                        self.files
                            .open_sample(&ctx, &mut self.state, crate::demo::sketch());
                        ui.close();
                    }
                    if ui.button("Stress scene (5 000 curves)").clicked() {
                        let ctx = ui.ctx().clone();
                        self.files
                            .open_sample(&ctx, &mut self.state, crate::demo::stress(5000));
                        ui.close();
                    }
                });
                #[cfg(not(target_arch = "wasm32"))]
                {
                    ui.separator();
                    if ui.button("Quit").clicked() {
                        ui.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                }
            });
            ui.menu_button("Edit", |ui| {
                let mut cx = cx!(self);
                actions::show(ui, &self.registry, &mut cx, actions::EDIT_MENU, Style::Menu);
            });
            ui.menu_button("View", |ui| {
                {
                    let mut cx = cx!(self);
                    actions::show(ui, &self.registry, &mut cx, actions::VIEW_MENU, Style::Menu);
                }
                ui.separator();
                for tab in [
                    Tab::Properties,
                    Tab::Layers,
                    Tab::Regions,
                    Tab::Parameters,
                    Tab::Patterns,
                    Tab::Constraints,
                ] {
                    let mut shown = self.dock.find_tab(&tab).is_some();
                    if ui.checkbox(&mut shown, tab.title()).changed() {
                        self.toggle_tab(tab);
                    }
                }
                if ui.button("Reset layout").clicked() {
                    self.dock = default_layout();
                    ui.close();
                }
                ui.separator();
                ui.menu_button("Theme", |ui| {
                    for t in Theme::ALL {
                        if ui
                            .selectable_value(&mut self.settings.theme, t, t.label())
                            .clicked()
                        {
                            ui.ctx().set_theme(t.preference());
                        }
                    }
                });
                #[cfg(not(target_arch = "wasm32"))]
                self.mcp.menu(ui);
                ui.menu_button("Units", |ui| {
                    for u in Units::ALL {
                        ui.selectable_value(&mut self.settings.units, u, u.label());
                    }
                });
            });
            ui.menu_button("Sketch", |ui| {
                let mut cx = cx!(self);
                actions::show(
                    ui,
                    &self.registry,
                    &mut cx,
                    actions::SKETCH_MENU,
                    Style::Menu,
                );
            });
            ui.menu_button("Constrain", |ui| {
                let mut cx = cx!(self);
                actions::show(
                    ui,
                    &self.registry,
                    &mut cx,
                    actions::CONSTRAIN_MENU,
                    Style::Menu,
                );
            });
            ui.menu_button("Plugins", |ui| {
                self.plugins.menu(ui, &mut self.registry, &self.state);
            });
            ui.menu_button("Help", |ui| {
                ui.hyperlink_to("Locus on GitHub", "https://github.com/Nerdtisan/Locus");
                ui.label(format!("Locus v{}", env!("CARGO_PKG_VERSION")));
            });
        });
    }

    /// The tool strip under the menu (#68): Select plus the drawing tools.
    fn tool_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            {
                let mut cx = cx!(self);
                actions::show(
                    ui,
                    &self.registry,
                    &mut cx,
                    actions::TOOL_STRIP,
                    Style::Strip,
                );
            }
            ui.separator();
            self.state.tool_options_ui(ui, self.settings.units);
            ui.weak(self.state.hint());
        });
    }

    /// One button per constraint, enabled when the selection supports it (#70).
    fn constraint_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            {
                let mut cx = cx!(self);
                actions::show(
                    ui,
                    &self.registry,
                    &mut cx,
                    actions::CONSTRAINT_BAR,
                    Style::Strip,
                );
            }
            self.state.touch_modifiers_ui(ui);
        });
    }

    /// Every shortcut, from the registry's key map (design §7.1). Plain
    /// keys act while the pointer was over the canvas last frame.
    fn shortcuts(&mut self, ctx: &egui::Context) {
        let over = self.state.pointer_over();
        let mut cx = cx!(self);
        self.registry.dispatch_keys(ctx, &mut cx, over);
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let u = self.settings.units;
            match self.status.cursor {
                Some(p) => {
                    let (x, y) = match u {
                        Units::Mm => (format!("{:.3}", p.x), format!("{:.3}", p.y)),
                        Units::Inch => (format!("{:.4}", p.x / 25.4), format!("{:.4}", p.y / 25.4)),
                    };
                    ui.monospace(format!("X {x}  Y {y} {}", u.label()));
                }
                None => {
                    ui.monospace("X –  Y –");
                }
            }
            ui.separator();
            ui.label(format!("View {}", u.format_len(self.status.extent_mm)));
            ui.separator();
            ui.label(format!("Grid {}", self.status.grid));
            ui.separator();
            ui.label(self.state.summary());
            ui.separator();
            ui.label(format!("CPU {:.1} ms/frame", self.status.frame_ms));
            ui.separator();
            ui.label(self.state.solve_summary());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Always visible while the MCP server runs (SPEC §16.4).
                #[cfg(not(target_arch = "wasm32"))]
                self.mcp.status(ui);
                ui.label(if self.settings.auto_constrain {
                    "Auto-constrain on"
                } else {
                    "Auto-constrain off"
                });
            });
        });
    }
}

struct Viewer<'a> {
    state: &'a mut AppState,
    panes: &'a mut Panes,
    settings: &'a Settings,
    status: &'a mut Status,
    #[cfg(not(target_arch = "wasm32"))]
    plugins: &'a mut crate::plugins::Plugins,
    #[cfg(target_arch = "wasm32")]
    plugins: &'a mut crate::web_plugins::WebPlugins,
}

impl TabViewer for Viewer<'_> {
    type Tab = Tab;

    fn title(&mut self, tab: &mut Tab) -> egui::WidgetText {
        tab.title().into()
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Tab) {
        match tab {
            Tab::Canvas => self.state.canvas_ui(ui, self.settings, self.status),
            Tab::Properties => {
                self.plugins.properties_ui(ui, self.state);
                self.state.properties_ui(ui, self.settings);
            }
            Tab::Layers => self.panes.layers.ui(ui, self.state, self.settings),
            Tab::Constraints => self.state.constraints_ui(ui, self.settings),
            Tab::Regions => self.state.regions_ui(ui),
            Tab::Parameters => self.panes.parameters.ui(ui, self.state),
            Tab::Patterns => self.panes.patterns.ui(ui, self.state, self.settings.units),
        }
    }

    fn is_closeable(&self, tab: &Tab) -> bool {
        *tab != Tab::Canvas
    }

    fn scroll_bars(&self, tab: &Tab) -> [bool; 2] {
        // The canvas owns the wheel for zoom.
        if *tab == Tab::Canvas {
            [false; 2]
        } else {
            [true; 2]
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        if let Some(cpu) = frame.info().cpu_usage {
            let ms = cpu * 1000.0;
            self.status.frame_ms = if self.status.frame_ms == 0.0 {
                ms
            } else {
                self.status.frame_ms * 0.9 + ms * 0.1
            };
        }
        self.registry.sync_plugins(self.plugins.plugin_actions());
        self.shortcuts(ui.ctx());
        self.files
            .update(ui.ctx(), &mut self.state, &mut self.settings);
        #[cfg(not(target_arch = "wasm32"))]
        self.plugins
            .windows(ui.ctx(), &mut self.state, &mut self.files);
        #[cfg(target_arch = "wasm32")]
        self.plugins.windows(ui.ctx(), &mut self.state);
        #[cfg(not(target_arch = "wasm32"))]
        self.mcp.update(ui.ctx(), &mut self.state);
        egui::Panel::top("menu").show(ui, |ui| self.menu_bar(ui));
        egui::Panel::top("tools").show(ui, |ui| self.tool_bar(ui));
        egui::Panel::top("constraints").show(ui, |ui| self.constraint_bar(ui));
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));
        let mut viewer = Viewer {
            state: &mut self.state,
            panes: &mut self.panes,
            settings: &self.settings,
            status: &mut self.status,
            plugins: &mut self.plugins,
        };
        DockArea::new(&mut self.dock)
            .show_leaf_collapse_buttons(false)
            .show_inside(ui, &mut viewer);
        let ctx = ui.ctx().clone();
        let mut cx = cx!(self);
        actions::touch_menu(&ctx, &self.registry, &mut cx);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        self.settings.save(storage);
        self.files.save_state(storage);
        #[cfg(not(target_arch = "wasm32"))]
        self.mcp.save(storage);
        // v1 is left in place for older builds (design §7.4).
        if let Some(json) = layout::save(&self.dock) {
            storage.set_string(layout::LAYOUT_V2_KEY, json);
        }
    }
}

/// The first argument (after the program name) naming a `.locus` file.
#[cfg(not(target_arch = "wasm32"))]
fn startup_file(args: impl Iterator<Item = String>) -> Option<std::path::PathBuf> {
    args.skip(1).map(std::path::PathBuf::from).find(|p| {
        p.extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("locus"))
    })
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[allow(clippy::unwrap_used)]
mod startup_tests {
    use super::startup_file;

    fn args(a: &[&str]) -> impl Iterator<Item = String> {
        a.iter()
            .map(|s| (*s).to_owned())
            .collect::<Vec<_>>()
            .into_iter()
    }

    #[test]
    fn picks_the_locus_file_and_skips_flags_and_program() {
        let got = startup_file(args(&["locus", "--stress", "dir/Plate.LOCUS", "b.locus"]));
        assert_eq!(got.unwrap().file_name().unwrap(), "Plate.LOCUS");
        assert!(startup_file(args(&["plate.locus"])).is_none()); // argv[0]
        assert!(startup_file(args(&["locus", "x.svg"])).is_none());
    }
}
