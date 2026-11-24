// SPDX-License-Identifier: AGPL-3.0-or-later
//! App shell (#65, SPEC §12.1): egui_dock layout with the canvas in the
//! centre, properties and constraints panels, a menu bar and a status bar.
//!
//! The shell never mutates a document directly; once `locus-doc` lands every
//! edit goes through its command API (SPEC §11).

use crate::actions::{self, Cx, Registry, Shell, Style};
use crate::file_io::FileIo;
use crate::layout::{self, Tab, default_layout};
use crate::settings::{Settings, Theme, Units};
use crate::state::AppState;
use crate::toolbar::{self, Toolbar};
use crate::views::browser::BrowserView;
use crate::views::parameters::ParametersView;
use crate::views::shortcuts::ShortcutsView;
use crate::views::tool_search::ToolSearch;
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
    browser: BrowserView,
    parameters: ParametersView,
    shortcuts: ShortcutsView,
    machine: crate::views::machine_panel::MachinePanel,
    history: crate::views::history::HistoryView,
}

/// A pane in its own OS window (#285, design §2.2). The builder keeps the
/// geometry it opened with: feeding the live geometry back each frame would
/// make egui move the window every frame.
struct PaneWindow {
    saved: layout::Window,
    open_pos: [f32; 2],
    open_size: [f32; 2],
}

impl PaneWindow {
    fn new(saved: layout::Window) -> Self {
        Self {
            open_pos: saved.pos,
            open_size: saved.size,
            saved,
        }
    }
}

pub struct App {
    dock: DockState<Tab>,
    /// Panes undocked into their own windows (#285).
    windows: Vec<PaneWindow>,
    /// A dock tab being dragged, and where the pointer last was (#285).
    tab_drag: Option<(Tab, Option<egui::Pos2>)>,
    /// Where the last tab drag ended, for placing its window.
    last_drop: Option<egui::Pos2>,
    /// Every command the UI offers (#294, design §7.1).
    registry: Registry,
    /// The ribbon's pinned actions (#299, design §2.3).
    toolbar: Toolbar,
    /// Tool search, S (#324, design §6.1).
    search: ToolSearch,
    settings: Settings,
    /// The one mutable state (design §7.3).
    state: AppState,
    /// The panes' transient widget state (design §7.3).
    panes: Panes,
    /// Machine preview as last seen, to tell the toggle from the pane (#443).
    machine_seen: bool,
    /// The main window was maximized when Locus last closed: maximize it
    /// once it is on screen (#458, see [`main_window`]).
    #[cfg(not(target_arch = "wasm32"))]
    restore_maximized: bool,
    /// Is the main window maximized now (kept for `save`).
    #[cfg(not(target_arch = "wasm32"))]
    maximized: bool,
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
        crate::theme::install(&cc.egui_ctx);
        let settings = Settings::load(cc.storage);
        cc.egui_ctx.set_theme(settings.theme.preference());
        let layout = cc.storage.map_or_else(
            || layout::load(None, None, None, None),
            |s| {
                layout::load(
                    s.get_string(layout::LAYOUT_V4_KEY).as_deref(),
                    s.get_string(layout::LAYOUT_V3_KEY).as_deref(),
                    s.get_string(layout::LAYOUT_V2_KEY).as_deref(),
                    s.get_string(layout::LAYOUT_V1_KEY).as_deref(),
                )
            },
        );
        let registry = Registry::default();
        let toolbar = Toolbar::load(
            cc.storage
                .and_then(|s| s.get_string(toolbar::TOOLBAR_KEY))
                .as_deref(),
            |id| registry.get(id).is_some(),
        );
        #[allow(unused_mut)] // mutated only on desktop
        let mut app = Self {
            dock: layout.dock,
            windows: layout.windows.into_iter().map(PaneWindow::new).collect(),
            tab_drag: None,
            last_drop: None,
            registry,
            toolbar,
            search: ToolSearch::default(),
            panes: Panes::default(),
            machine_seen: false,
            #[cfg(not(target_arch = "wasm32"))]
            restore_maximized: cc
                .storage
                .and_then(|s| s.get_string(main_window::MAXIMIZED_KEY))
                .is_some_and(|v| v == "true"),
            #[cfg(not(target_arch = "wasm32"))]
            maximized: false,
            settings,
            state: {
                let mut c = AppState::untitled();
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

    /// Is `tab` showing, docked or in its own window?
    /// Machine preview is the Machine preview pane (#443): the toggle opens
    /// or closes it, and closing the pane turns the preview off.
    fn sync_machine_pane(&mut self) {
        let shown = self.is_shown(Tab::MachinePreview);
        let on = self.state.machine_preview();
        if on != self.machine_seen {
            // The action changed the flag: follow it with the pane.
            if on != shown {
                self.toggle_tab(Tab::MachinePreview);
            }
        } else if shown != on {
            // The pane was opened or closed from the dock or View ▸ Panes.
            self.state.set_machine_preview(shown);
        }
        self.machine_seen = self.state.machine_preview();
    }

    fn is_shown(&self, tab: Tab) -> bool {
        self.dock.find_tab(&tab).is_some() || self.windows.iter().any(|w| w.saved.tab == tab)
    }

    /// Show or hide a pane. A hidden pane comes back docked at its home
    /// (design §5.1: Shortcuts under the Inspector).
    fn toggle_tab(&mut self, tab: Tab) {
        if let Some(loc) = self.dock.find_tab(&tab) {
            self.dock.remove_tab(loc);
        } else if let Some(i) = self.windows.iter().position(|w| w.saved.tab == tab) {
            self.windows.remove(i);
        } else {
            layout::redock(&mut self.dock, tab);
        }
    }

    /// Move a docked pane into its own window at `pos` (desktop points).
    fn undock(&mut self, tab: Tab, pos: egui::Pos2, size: egui::Vec2) {
        if tab == Tab::Canvas || self.windows.iter().any(|w| w.saved.tab == tab) {
            return;
        }
        if let Some(loc) = self.dock.find_tab(&tab) {
            self.dock.remove_tab(loc);
        }
        self.windows.push(PaneWindow::new(layout::Window {
            tab,
            pos: [pos.x, pos.y],
            size: [size.x.max(240.0), size.y.max(200.0)],
            monitor: None,
        }));
    }

    /// Draw each pane window (#285): an OS window on the desktop; on the
    /// web, which has one window, an in-page window. Closing it, or Dock,
    /// puts the pane back in the dock.
    fn pane_windows(&mut self, ctx: &egui::Context) {
        let mut windows = std::mem::take(&mut self.windows);
        let mut back = Vec::new();
        // eframe embeds viewports where it can't open windows (the web).
        // Its embedded windows ignore the position, so draw our own there.
        let embedded = ctx.embed_viewports();
        for (i, w) in windows.iter_mut().enumerate() {
            let mut tab = w.saved.tab;
            let mut viewer = Viewer {
                registry: &self.registry,
                state: &mut self.state,
                panes: &mut self.panes,
                settings: &mut self.settings,
                status: &mut self.status,
                doc_name: self.files.doc_name(),
                plugins: &mut self.plugins,
                undock: Vec::new(),
                dragged: None,
            };
            let pos = egui::pos2(w.open_pos[0], w.open_pos[1]);
            let size = egui::vec2(w.open_size[0], w.open_size[1]);
            let dock_it = if embedded {
                let mut open = true;
                let mut dock_it = false;
                let shown = egui::Window::new(tab.title())
                    .id(egui::Id::new(("locus-pane", tab)))
                    .open(&mut open)
                    .default_pos(pos)
                    .default_size(size)
                    .constrain(true)
                    .show(ctx, |ui| {
                        if ui
                            .button("Dock")
                            .on_hover_text("Put this pane back in the dock")
                            .clicked()
                        {
                            dock_it = true;
                        }
                        ui.separator();
                        egui::ScrollArea::both().show(ui, |ui| viewer.ui(ui, &mut tab));
                    });
                if let Some(r) = shown.map(|s| s.response.rect) {
                    w.saved.pos = [r.min.x, r.min.y];
                    w.saved.size = [r.width(), r.height()];
                }
                dock_it || !open
            } else {
                let id = egui::ViewportId::from_hash_of(("locus-pane", tab));
                // Hidden on its first frame so it appears once, in place
                // (#458).
                let hidden = crate::dialog::opening(ctx, id);
                let builder = egui::ViewportBuilder::default()
                    .with_visible(!hidden)
                    .with_title(format!("{} - Locus", tab.title()))
                    .with_position(pos)
                    .with_inner_size(size)
                    .with_min_inner_size(egui::vec2(200.0, 160.0));
                ctx.show_viewport_immediate(id, builder, |ui, _class| {
                    let mut dock_it = false;
                    egui::Panel::top("pane-window-bar").show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.strong(tab.title());
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    dock_it = ui
                                        .button("Dock")
                                        .on_hover_text("Put this pane back in the main window")
                                        .clicked();
                                },
                            );
                        });
                    });
                    egui::CentralPanel::default_margins().show(ui, |ui| {
                        egui::ScrollArea::both().show(ui, |ui| viewer.ui(ui, &mut tab));
                    });
                    ui.input(|i| {
                        let vp = i.viewport();
                        if let Some(r) = vp.outer_rect {
                            w.saved.pos = [r.min.x, r.min.y];
                        }
                        if let Some(r) = vp.inner_rect {
                            w.saved.size = [r.width(), r.height()];
                        }
                        if let Some(m) = vp.monitor_size {
                            w.saved.monitor = Some(format!("{:.0}x{:.0}", m.x, m.y));
                        }
                        dock_it || vp.close_requested()
                    })
                })
            };
            if dock_it {
                back.push(i);
            }
        }
        for i in back.into_iter().rev() {
            let w = windows.remove(i);
            layout::redock(&mut self.dock, w.saved.tab);
        }
        self.windows = windows;
    }

    /// Carry out what registry actions asked of the shell this frame.
    fn shell_requests(&mut self, ctx: &egui::Context) {
        for req in std::mem::take(&mut self.state.view.shell) {
            match req {
                Shell::ToggleShortcuts => self.toggle_tab(Tab::Shortcuts),
                Shell::ResetLayout => {
                    self.dock = default_layout();
                    self.windows.clear();
                }
                Shell::ShowPane(tab) => {
                    if !self.is_shown(tab) {
                        self.toggle_tab(tab);
                    }
                }
                Shell::Export => self.files.open_export(&self.state, &self.settings),
                Shell::ExportPng => self.files.open_export_png(&self.state, &self.settings),
                Shell::Profiles => self.files.open_profiles(),
                Shell::AutoTrace => self.state.open_trace(None),
                Shell::InsertReference(kind) => {
                    self.files.begin_import_reference(&mut self.state, kind);
                }
                Shell::InstallPlugin => self.plugins.open_install(),
                Shell::PluginManager => self.plugins.open_manager(),
                #[cfg(not(target_arch = "wasm32"))]
                Shell::Mcp => self.mcp.open(),
                // The registry disables it on the web (SPEC §16).
                #[cfg(target_arch = "wasm32")]
                Shell::Mcp => {}
                Shell::Save => self.files.save_now(&self.state),
                // At the pointer, as Fusion's shortcut box (design §6.1).
                Shell::ToolSearch => {
                    let at = ctx
                        .pointer_latest_pos()
                        .unwrap_or_else(|| ctx.content_rect().center());
                    self.search.open(at);
                }
            }
        }
    }

    /// Every pane window back into the dock.
    fn dock_all(&mut self) {
        for w in std::mem::take(&mut self.windows) {
            layout::redock(&mut self.dock, w.saved.tab);
        }
    }

    /// A dock tab released outside the dock, above all outside the main
    /// window (#285). egui_dock only completes a drop while the pointer is
    /// over one of its surfaces; anywhere else it silently cancels the drag
    /// and the tab snaps back, which is why dragging a pane out of the main
    /// window did nothing. Such a drop opens the pane in its own window
    /// under the pointer.
    fn tab_dropped_outside(
        &mut self,
        ctx: &egui::Context,
        dragged: Option<Tab>,
        dock: egui::Rect,
        origin: egui::Vec2,
    ) {
        let (pointer, released) = ctx.input(|i| (i.pointer.latest_pos(), i.pointer.any_released()));
        if let Some(tab) = dragged {
            let last = pointer.or(self.tab_drag.and_then(|(_, p)| p));
            self.tab_drag = Some((tab, last));
        }
        if !released {
            return;
        }
        let Some((tab, last)) = self.tab_drag.take() else {
            return;
        };
        self.last_drop = pointer.or(last);
        if let Some(at) = drop_outside(dock, pointer, last)
            && tab != Tab::Canvas
            && self.dock.find_tab(&tab).is_some()
        {
            self.undock(tab, at - GRAB + origin, egui::vec2(360.0, 480.0));
        }
    }

    /// egui_dock floats a tab dragged out of the dock in a window inside the
    /// main one. Once the drag ends, that becomes a real OS window at the
    /// same place (design §2.2); on the web, an in-page window.
    fn promote_floating(&mut self, ctx: &egui::Context) {
        let origin = ctx
            .input(|i| i.viewport().inner_rect)
            .map_or(egui::Vec2::ZERO, |r| r.min.to_vec2());
        let floating: Vec<_> = self
            .dock
            .iter_surfaces_indexed()
            .filter(|(_, s)| matches!(s, egui_dock::Surface::Window(..)))
            .map(|(i, _)| i)
            .collect();
        for si in floating {
            let Some(ws) = self.dock.get_window_state(si) else {
                continue;
            };
            if ws.dragged() {
                continue;
            }
            let rect = ws.rect();
            let Some(surface) = self.dock.remove_surface(si) else {
                continue;
            };
            let tabs: Vec<Tab> = surface.iter_all_tabs().map(|(_, t)| *t).collect();
            for (n, tab) in tabs.into_iter().enumerate() {
                let step = egui::vec2(24.0, 24.0) * n as f32;
                let size = if rect.is_finite() && rect.width() > 1.0 {
                    rect.size()
                } else {
                    egui::vec2(360.0, 480.0)
                };
                // A window egui_dock never drew has no rect yet: put it
                // where the tab was dropped.
                let pos = if rect.is_finite() {
                    rect.min + origin + step
                } else {
                    self.last_drop.unwrap_or(egui::pos2(120.0, 120.0)) - GRAB + origin + step
                };
                if tab == Tab::Canvas {
                    layout::redock(&mut self.dock, tab);
                } else {
                    self.undock(tab, pos, size);
                }
            }
        }
    }

    /// The app bar (#324, design §2): LOCUS, the menus, Save · Undo · Redo,
    /// the document with its machine profile and units, then tool search
    /// and the shortcut map at the right, as the mockup lays it out.
    fn app_bar(&mut self, ui: &mut egui::Ui) {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.label(egui::RichText::new("LOCUS").strong());
            ui.add_space(4.0);
            self.menus(ui);
            ui.separator();
            for id in ["file.save", "edit.undo", "edit.redo"] {
                self.bar_icon(ui, id);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                self.bar_text(ui, "view.shortcuts", "?");
                self.search_button(ui);
                ui.with_layout(
                    egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
                    |ui| self.document_label(ui),
                );
            });
        });
    }

    /// An app-bar icon button running a registry action (Save, Undo, Redo).
    fn bar_icon(&mut self, ui: &mut egui::Ui, id: &str) {
        let Some(a) = self.registry.get(id) else {
            return;
        };
        let mut cx = cx!(self);
        let enabled = self.registry.enabled(a, cx.state);
        let r = ui.add_enabled(
            enabled.yes(),
            egui::Button::new("").min_size(egui::vec2(24.0, 22.0)),
        );
        let color = if enabled.yes() {
            ui.style().interact(&r).fg_stroke.color
        } else {
            ui.visuals().weak_text_color()
        };
        crate::views::icons::paint(ui.painter(), r.rect.shrink(4.0), id, &a.label, color);
        let mut tip = self.registry.label(a, cx.state).into_owned();
        if let Some(k) = self.registry.shortcut(a) {
            tip += &format!(" ({})", actions::key_text(ui.ctx(), &k));
        } else if !a.hint.is_empty() {
            tip = a.hint.to_string();
        }
        let r = match &enabled {
            actions::Enabled::No(why) => r.on_disabled_hover_text(why.as_ref()),
            actions::Enabled::Yes => r.on_hover_text(tip),
        };
        if r.clicked() {
            self.registry.run(a, &mut cx);
        }
    }

    /// An app-bar text button running a registry action (the ?).
    fn bar_text(&mut self, ui: &mut egui::Ui, id: &str, text: &str) {
        let Some(a) = self.registry.get(id) else {
            return;
        };
        let mut cx = cx!(self);
        let mut tip = a.label.to_string();
        if let Some(k) = self.registry.shortcut(a) {
            tip += &format!(" ({})", actions::key_text(ui.ctx(), &k));
        }
        if ui.button(text).on_hover_text(tip).clicked() {
            self.registry.run(a, &mut cx);
        }
    }

    /// "Search  S": opens tool search under the button (design §6.1).
    fn search_button(&mut self, ui: &mut egui::Ui) {
        let Some(a) = self.registry.get("view.tool_search") else {
            return;
        };
        let key = self
            .registry
            .shortcut(a)
            .map(|k| actions::key_text(ui.ctx(), &k))
            .unwrap_or_default();
        let r = ui
            .add(egui::Button::new(format!("      Search   {key}")))
            .on_hover_text(a.hint.as_ref());
        let icon = egui::Rect::from_center_size(
            egui::pos2(r.rect.left() + 13.0, r.rect.center().y),
            egui::vec2(13.0, 13.0),
        );
        let c = ui.style().interact(&r).fg_stroke.color;
        crate::views::icons::paint(ui.painter(), icon, a.id.as_str(), "", c);
        if r.clicked() {
            self.search
                .open(r.rect.left_bottom() + egui::vec2(0.0, 4.0));
        }
    }

    /// "coaster-tray.locus · LightBurn (SVG) ▾ · mm", centred (mockup). The
    /// profile is the document's machine picker (#324, #419).
    fn document_label(&mut self, ui: &mut egui::Ui) {
        let profile = crate::views::machine_profile::current(self.state.document(), &self.settings);
        let weak = ui.visuals().weak_text_color();
        // Centre the three pieces as one row: measure, then pad.
        let name = egui::RichText::new(self.files.doc_name())
            .strong()
            .color(ui.visuals().strong_text_color());
        let picker_text = profile
            .as_ref()
            .map_or("No machine profile", |p| p.name.as_str())
            .to_owned();
        let units = format!("·  {}", self.settings.units.label());
        let font = egui::TextStyle::Body.resolve(ui.style());
        let w = |t: &str| {
            ui.painter()
                .layout_no_wrap(t.to_owned(), font.clone(), weak)
                .size()
                .x
        };
        let total = w(self.files.doc_name()) + w("  ·  ") + w(&picker_text) + 30.0 + w(&units);
        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.add_space(((ui.available_width() - total) / 2.0).max(0.0));
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.label(name);
            ui.label(egui::RichText::new("·").color(weak));
            let tip = profile.as_ref().map_or_else(
                || "Choose the machine this document is for: it sets new layers' operation, the bed and Machine preview, and export uses it".to_owned(),
                |p| format!("{}
Click to choose another machine", crate::views::machine_profile::summary(p)),
            );
            let manage = crate::views::machine_profile::picker_button(
                ui,
                &mut self.state,
                &self.settings,
                egui::RichText::new(picker_text).color(weak),
                &tip,
            );
            if manage {
                self.files.open_profiles();
            }
            ui.label(egui::RichText::new(units).color(weak))
                .on_hover_text("Display units (View ▸ Units)");
        });
    }

    /// File, Edit, View, Plugins and Help (design §2): the full command set.
    fn menus(&mut self, ui: &mut egui::Ui) {
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
            for tab in Tab::PANES {
                let mut shown = self.is_shown(tab);
                if crate::theme::checkbox(ui, &mut shown, tab.title()).changed() {
                    self.toggle_tab(tab);
                }
            }
            if ui
                .add_enabled(
                    !self.windows.is_empty(),
                    egui::Button::new("Dock all windows"),
                )
                .on_hover_text("Put every pane window back in the main window")
                .clicked()
            {
                self.dock_all();
                ui.close();
            }
            {
                let mut cx = cx!(self);
                actions::show(
                    ui,
                    &self.registry,
                    &mut cx,
                    actions::LAYOUT_MENU,
                    Style::Menu,
                );
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
        ui.menu_button("Plugins", |ui| {
            self.plugins.menu(ui, &mut self.registry, &self.state);
        });
        ui.menu_button("Help", |ui| {
            {
                let mut cx = cx!(self);
                actions::show(ui, &self.registry, &mut cx, actions::HELP_MENU, Style::Menu);
            }
            ui.separator();
            ui.hyperlink_to("Locus on GitHub", "https://github.com/Nerdtisan/Locus");
            ui.label(format!("Locus v{}", env!("CARGO_PKG_VERSION")));
        });
    }

    /// The ribbon's tabs with the user's pinned tools (#299, #325, design §2.3).
    fn ribbon(&mut self, ui: &mut egui::Ui) {
        // eframe stores the pins with the other settings (`save`).
        let mut cx = cx!(self);
        crate::views::ribbon::show(ui, &self.registry, &mut cx, &mut self.toolbar);
    }

    /// The active tool's options and the next click's prompt.
    fn options_bar(&mut self, ui: &mut egui::Ui) {
        let palette = self.is_shown(Tab::Palette);
        ui.horizontal_wrapped(|ui| {
            // The palette holds the tool options; with it closed they stay here.
            if !palette {
                self.state.tool_options_ui(ui);
            }
        });
    }

    /// Every shortcut, from the registry's key map (design §7.1). Plain
    /// keys act while the pointer was over the canvas last frame.
    fn shortcuts(&mut self, ctx: &egui::Context) {
        let over = self.state.pointer_over();
        let mut cx = cx!(self);
        self.registry.dispatch_keys(ctx, &mut cx, over);
    }

    /// The status bar (M5 design §2, #331), as the mockup: the next
    /// click's prompt on the left; sketch DOF, selection, cursor, snap,
    /// auto-constrain and units on the right. View extent, grid step and
    /// frame time sit in the cursor's hover text.
    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            // The right-hand items first, so a long prompt is cut short
            // instead of running under them.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 14.0;
                // Always visible while the MCP server runs (SPEC §16.4).
                #[cfg(not(target_arch = "wasm32"))]
                self.mcp.status(ui);
                let u = self.settings.units;
                ui.monospace(u.label());
                let onoff = |on: bool| if on { "on" } else { "off" };
                // Right to left: the tick is drawn before its label.
                status_tick(ui, self.settings.auto_constrain);
                ui.label("Auto-constrain");
                let d = self.settings.display;
                status_tick(ui, d.snap_grid || d.snap_geometry);
                ui.label("Snap").on_hover_text(format!(
                    "Snap to grid {} · snap to geometry {}",
                    onoff(d.snap_grid),
                    onoff(d.snap_geometry)
                ));
                let cursor = match self.status.cursor {
                    Some(p) => {
                        // Enough decimals to tell screen points apart at any
                        // zoom (#333): about a thousandth of the view.
                        let fine = |extent: f64| {
                            (-(extent / 1000.0).log10()).ceil().clamp(2.0, 10.0) as usize
                        };
                        let (x, y) = match u {
                            Units::Mm => {
                                let d = fine(self.status.extent_mm);
                                (format!("{:.d$}", p.x), format!("{:.d$}", p.y))
                            }
                            Units::Inch => {
                                let d = fine(self.status.extent_mm / 25.4).max(3);
                                (format!("{:.d$}", p.x / 25.4), format!("{:.d$}", p.y / 25.4))
                            }
                        };
                        format!("x {x}  y {y} {}", u.label())
                    }
                    None => format!("x –  y – {}", u.label()),
                };
                ui.monospace(cursor).on_hover_text(format!(
                    "View {} · grid {} · {} · CPU {:.1} ms/frame",
                    u.format_len(self.status.extent_mm),
                    self.status.grid,
                    self.state.summary(),
                    self.status.frame_ms
                ));
                let n = self.state.selection.items.len();
                ui.label(if n == 0 {
                    "Nothing selected".to_owned()
                } else {
                    format!("{n} selected")
                });
                crate::views::properties::solve_pill(ui, &self.state);
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    self.state.touch_modifiers_ui(ui);
                    let hint = self.state.hint();
                    ui.add(
                        egui::Label::new(egui::RichText::new(hint.as_str()).strong()).truncate(),
                    )
                    .on_hover_text(hint);
                });
            });
        });
    }
}

/// A painted tick or cross after a status-bar label (mockup "Snap ✓"):
/// the bundled fonts have no glyph for either.
fn status_tick(ui: &mut egui::Ui, on: bool) {
    let (r, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
    let ink = if on {
        ui.visuals().text_color()
    } else {
        ui.visuals().weak_text_color()
    };
    let s = egui::Stroke::new(1.5, ink);
    let p = ui.painter();
    if on {
        p.line_segment([r.left_center(), r.center_bottom()], s);
        p.line_segment([r.center_bottom(), r.right_top()], s);
    } else {
        let k = r.shrink(1.5);
        p.line_segment([k.left_top(), k.right_bottom()], s);
        p.line_segment([k.right_top(), k.left_bottom()], s);
    }
}

struct Viewer<'a> {
    registry: &'a Registry,
    state: &'a mut AppState,
    panes: &'a mut Panes,
    settings: &'a mut Settings,
    status: &'a mut Status,
    /// The open file's name, for the Browser's root.
    doc_name: &'a str,
    /// Tabs whose context menu asked for their own window (#285).
    undock: Vec<Tab>,
    /// The tab being dragged this frame, if any (#285).
    dragged: Option<Tab>,
    #[cfg(not(target_arch = "wasm32"))]
    plugins: &'a mut crate::plugins::Plugins,
    #[cfg(target_arch = "wasm32")]
    plugins: &'a mut crate::web_plugins::WebPlugins,
}

impl TabViewer for Viewer<'_> {
    type Tab = Tab;

    /// The mockup's pane headers (#360): small, spaced, muted capitals.
    fn title(&mut self, tab: &mut Tab) -> egui::WidgetText {
        egui::RichText::new(tab.title().to_uppercase())
            .size(10.5)
            .strong()
            .into()
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Tab) {
        match tab {
            Tab::Canvas => {
                self.state.canvas_ui(ui, self.settings, self.status);
                // The navigation bar floats over the canvas (design §2.4).
                let rect = self.state.view.rect;
                let mut cx = Cx {
                    state: self.state,
                    settings: self.settings,
                    plugins: Some(self.plugins),
                };
                crate::views::nav_bar::show(ui.ctx(), self.registry, &mut cx, rect);
            }
            Tab::Browser => {
                // One tree (design §2, #326).
                self.panes
                    .browser
                    .ui(ui, self.state, self.settings, self.doc_name);
            }
            Tab::Inspector => {
                self.plugins.properties_ui(ui, self.state);
                self.state.properties_ui(ui, self.settings);
            }
            Tab::Palette => {
                let mut cx = Cx {
                    state: self.state,
                    settings: self.settings,
                    plugins: Some(self.plugins),
                };
                crate::views::palette::ui(ui, self.registry, &mut cx);
            }
            Tab::Constraints => self.state.constraints_ui(ui, self.settings),
            Tab::Parameters => self.panes.parameters.ui(ui, self.state),
            Tab::Shortcuts => self.panes.shortcuts.ui(ui, self.registry),
            Tab::History => self.panes.history.ui(ui, self.state),
            Tab::MachinePreview => {
                let manage = self.panes.machine.ui(ui, self.state, self.settings);
                if manage {
                    self.state.view.shell.push(crate::actions::Shell::Profiles);
                }
            }
        }
    }

    fn is_closeable(&self, tab: &Tab) -> bool {
        *tab != Tab::Canvas
    }

    fn allowed_in_windows(&self, tab: &mut Tab) -> bool {
        *tab != Tab::Canvas
    }

    /// egui_dock draws a dragged tab as a copy with its own id, so the drag
    /// is told from where the press started: on this tab's button.
    fn on_tab_button(&mut self, tab: &mut Tab, response: &egui::Response) {
        let dragging = response.ctx.input(|i| {
            i.pointer.is_decidedly_dragging()
                && i.pointer
                    .press_origin()
                    .is_some_and(|o| response.rect.contains(o))
        });
        if dragging {
            self.dragged = Some(*tab);
        }
    }

    fn context_menu(&mut self, ui: &mut egui::Ui, tab: &mut Tab, _path: egui_dock::NodePath) {
        if *tab != Tab::Canvas && ui.button("Open in its own window").clicked() {
            self.undock.push(*tab);
            ui.close();
        }
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
        #[cfg(not(target_arch = "wasm32"))]
        {
            let ctx = ui.ctx().clone();
            // eframe shows the window after the first pass; maximize it
            // then, already on screen, so it never flashes (#458).
            if self.restore_maximized && ctx.cumulative_pass_nr() >= 1 {
                self.restore_maximized = false;
                ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(true));
            }
            self.maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
        }
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
        let tk = crate::theme::tokens(ui);
        egui::Panel::top("menu")
            .frame(crate::theme::bar(ui, tk.panel_2))
            .show(ui, |ui| self.app_bar(ui));
        egui::Panel::top("ribbon").show(ui, |ui| self.ribbon(ui));
        if !self.is_shown(Tab::Palette) {
            egui::Panel::top("options").show(ui, |ui| self.options_bar(ui));
        }
        self.shell_requests(ui.ctx());
        self.state.trace_dialog(ui.ctx());
        self.sync_machine_pane();
        egui::Panel::bottom("status")
            .frame(crate::theme::bar(ui, tk.panel_2))
            .show(ui, |ui| self.status_bar(ui));
        let mut viewer = Viewer {
            registry: &self.registry,
            state: &mut self.state,
            panes: &mut self.panes,
            settings: &mut self.settings,
            status: &mut self.status,
            doc_name: self.files.doc_name(),
            plugins: &mut self.plugins,
            undock: Vec::new(),
            dragged: None,
        };
        let dock_rect = ui.available_rect_before_wrap();
        DockArea::new(&mut self.dock)
            .style(dock_style(ui.style(), &tk))
            .show_leaf_collapse_buttons(false)
            .show_inside(ui, &mut viewer);
        let undock = std::mem::take(&mut viewer.undock);
        let dragged = viewer.dragged;
        let ctx = ui.ctx().clone();
        let origin = ctx
            .input(|i| i.viewport().inner_rect)
            .map_or(egui::Vec2::ZERO, |r| r.min.to_vec2());
        for tab in undock {
            let at = egui::pos2(160.0, 140.0) + origin;
            self.undock(tab, at, egui::vec2(360.0, 480.0));
        }
        self.tab_dropped_outside(&ctx, dragged, dock_rect, origin);
        self.promote_floating(&ctx);
        self.pane_windows(&ctx);
        crate::views::machine_profile::profile_offer_window(&ctx, &mut self.state);
        let mut cx = cx!(self);
        crate::views::marking_menu::show(&ctx, &self.registry, &mut cx);
        self.search
            .show(&ctx, &self.registry, &mut cx, &self.toolbar);
        // A pane or the marking menu asked the shell for something after
        // it drained the requests: act on it next frame, not next input.
        if !self.state.view.shell.is_empty() {
            ctx.request_repaint();
        }
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        #[cfg(not(target_arch = "wasm32"))]
        main_window::save(storage, self.maximized);
        self.settings.save(storage);
        self.files.save_state(storage);
        if let Some(json) = self.toolbar.save() {
            storage.set_string(toolbar::TOOLBAR_KEY, json);
        }
        #[cfg(not(target_arch = "wasm32"))]
        self.mcp.save(storage);
        // v1 to v3 are left in place for older builds (design §7.4).
        let windows: Vec<_> = self.windows.iter().map(|w| w.saved.clone()).collect();
        if let Some(json) = layout::save(&self.dock, &windows) {
            storage.set_string(layout::LAYOUT_V4_KEY, json);
        }
    }
}

/// The dock in the mockup's colours (#360): pane headers on panel-2, the
/// open tab on the panel, hairline separators.
fn dock_style(style: &egui::Style, t: &crate::theme::Tokens) -> egui_dock::Style {
    let mut s = egui_dock::Style::from_egui(style);
    s.tab_bar.bg_fill = t.panel_2;
    s.tab_bar.height = 26.0;
    s.tab_bar.hline_color = t.line;
    s.separator.color_idle = t.line;
    s.separator.width = 1.0;
    s.tab.tab_body.bg_fill = t.panel;
    s.tab.tab_body.stroke = egui::Stroke::new(1.0, t.line);
    s.tab.active.bg_fill = t.panel;
    s.tab.active.text_color = t.ink;
    s.tab.focused.bg_fill = t.panel;
    s.tab.focused.text_color = t.accent;
    s.tab.inactive.bg_fill = t.panel_2;
    s.tab.inactive.text_color = t.muted;
    s.tab.hovered.bg_fill = t.accent_soft;
    s.tab.hovered.text_color = t.ink;
    s
}

/// Where the pointer is on a new pane window: near its top-left, as if the
/// window were held by its title bar.
const GRAB: egui::Vec2 = egui::vec2(60.0, 12.0);

/// How close to the dock's edge a lost pointer counts as leaving it.
const EDGE: f32 = 24.0;

/// Where a tab released at `pointer` (or, if the pointer left the window,
/// last seen at `last`) should open in its own window: `None` when it was
/// released over the dock, where egui_dock handles it. A pointer that
/// vanished well inside the dock is a touch release, not a drag out.
fn drop_outside(
    dock: egui::Rect,
    pointer: Option<egui::Pos2>,
    last: Option<egui::Pos2>,
) -> Option<egui::Pos2> {
    match pointer {
        Some(p) => (!dock.contains(p)).then_some(p),
        None => last.filter(|p| !dock.shrink(EDGE).contains(*p)),
    }
}

#[cfg(test)]
mod drop_tests {
    use super::drop_outside;
    use egui::{Rect, pos2};

    #[test]
    fn a_tab_released_outside_the_dock_gets_a_window_there() {
        let dock = Rect::from_min_max(pos2(0.0, 60.0), pos2(1000.0, 700.0));
        // Over the dock: egui_dock's drop.
        assert_eq!(drop_outside(dock, Some(pos2(500.0, 400.0)), None), None);
        // Outside the main window (pointer captured, coordinates beyond it).
        assert_eq!(
            drop_outside(dock, Some(pos2(1300.0, 200.0)), None),
            Some(pos2(1300.0, 200.0))
        );
        assert_eq!(
            drop_outside(dock, Some(pos2(-40.0, 300.0)), None),
            Some(pos2(-40.0, 300.0))
        );
        // Pointer gone after leaving through the right edge: last position.
        assert_eq!(
            drop_outside(dock, None, Some(pos2(995.0, 300.0))),
            Some(pos2(995.0, 300.0))
        );
        // Pointer gone mid-dock (a touch release): not a drag out.
        assert_eq!(drop_outside(dock, None, Some(pos2(500.0, 400.0))), None);
        assert_eq!(drop_outside(dock, None, None), None);
    }
}

/// A frame-time harness (#505): the whole app, headless, on a document
/// given by `LOCUS_PROFILE_FILE`, with the pointer moving over the canvas.
/// Prints the median UI and tessellation times. Run with `--ignored
/// --nocapture` in a release build.
#[cfg(all(test, not(target_arch = "wasm32")))]
#[allow(clippy::unwrap_used)]
mod frame_profile {
    use super::App;
    use egui::{Pos2, Rect, pos2, vec2};
    use std::time::{Duration, Instant};

    fn median(mut v: Vec<Duration>) -> f64 {
        v.sort();
        v[v.len() / 2].as_secs_f64() * 1000.0
    }

    #[test]
    #[ignore = "profiling harness; needs LOCUS_PROFILE_FILE"]
    fn frame_time_on_a_large_document() {
        let Some(path) = std::env::var_os("LOCUS_PROFILE_FILE") else {
            return;
        };
        let text = std::fs::read_to_string(path).unwrap();
        let t = Instant::now();
        let doc = locus_doc::file::load(&text).unwrap();
        println!("load: {:.1} ms", t.elapsed().as_secs_f64() * 1000.0);
        let ms = |t: Instant| t.elapsed().as_secs_f64() * 1000.0;
        let t = Instant::now();
        let sk = locus_doc::Sketcher::new(doc.clone());
        println!("Sketcher::new: {:.1} ms", ms(t));
        let t = Instant::now();
        let rs = locus_doc::regions::regions(&doc);
        println!("regions: {:.1} ms ({} regions)", ms(t), rs.len());
        let t = Instant::now();
        let _ = crate::scene::Scene::build_showing(
            sk.document(),
            &sk.state().fully_constrained,
            &Default::default(),
            true,
        );
        println!("Scene::build: {:.1} ms", ms(t));
        // One small edit: restyle one curve, as the Inspector does.
        let mut sk = sk;
        let id = doc
            .entities()
            .iter()
            .find(|(_, e)| !matches!(e.geom, locus_doc::Geom::Point { .. }))
            .map(|(i, _)| i)
            .unwrap();
        let t = Instant::now();
        sk.apply(&locus_doc::Command::SetStyle {
            entities: vec![id],
            patch: locus_doc::style::StylePatch {
                width: Some(0.5),
                ..Default::default()
            },
        })
        .unwrap();
        println!("one SetStyle via Sketcher::apply: {:.1} ms", ms(t));
        let ctx = egui::Context::default();
        let mut app = App::new(&eframe::CreationContext::_new_kittest(ctx.clone()));
        let t = Instant::now();
        app.state.load(doc);
        println!("state.load: {:.1} ms", t.elapsed().as_secs_f64() * 1000.0);
        let mut frame = eframe::Frame::_new_kittest();
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(1600.0, 1000.0));
        let mut clock = 0.0;
        let mut run = |app: &mut App, events: Vec<egui::Event>| {
            clock += 1.0 / 60.0;
            let input = egui::RawInput {
                screen_rect: Some(screen),
                time: Some(clock),
                events,
                ..Default::default()
            };
            let t0 = Instant::now();
            let out = ctx.run_ui(input, |ui| eframe::App::ui(app, ui, &mut frame));
            let t1 = Instant::now();
            let shapes = out.shapes.len();
            let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
            let verts: usize = prims
                .iter()
                .map(|p| match &p.primitive {
                    egui::epaint::Primitive::Mesh(m) => m.vertices.len(),
                    egui::epaint::Primitive::Callback(_) => 0,
                })
                .sum();
            (t1 - t0, t1.elapsed(), shapes, verts)
        };
        for _ in 0..5 {
            run(&mut app, Vec::new());
        }
        let (mut ui, mut tess) = (Vec::new(), Vec::new());
        let mut last = (0, 0);
        for i in 0..40 {
            let x = 500.0 + 15.0 * i as f32;
            let (u, t, s, v) = run(&mut app, vec![egui::Event::PointerMoved(pos2(x, 520.0))]);
            ui.push(u);
            tess.push(t);
            last = (s, v);
        }
        println!(
            "pointer moving: ui {:.2} ms, tessellate {:.2} ms (median of 40), {} shapes, {} vertices",
            median(ui),
            median(tess),
            last.0,
            last.1
        );
        let (mut ui, mut tess) = (Vec::new(), Vec::new());
        for _ in 0..20 {
            let (u, t, _, _) = run(&mut app, Vec::new());
            ui.push(u);
            tess.push(t);
        }
        println!(
            "idle: ui {:.2} ms, tessellate {:.2} ms (median of 20)",
            median(ui),
            median(tess)
        );
        // Everything selected: the Inspector and selection drawing see all
        // 12 500 entities each frame.
        app.state.select_all();
        let t = Instant::now();
        app.state.rebuild();
        println!(
            "rebuild with all {} selected: {:.1} ms",
            app.state.selection.items.len(),
            t.elapsed().as_secs_f64() * 1000.0
        );
        let (mut ui, mut tess) = (Vec::new(), Vec::new());
        for i in 0..20 {
            let x = 500.0 + 15.0 * i as f32;
            let (u, t, _, _) = run(&mut app, vec![egui::Event::PointerMoved(pos2(x, 520.0))]);
            ui.push(u);
            tess.push(t);
        }
        println!(
            "all selected, pointer moving: ui {:.2} ms, tessellate {:.2} ms (median of 20)",
            median(ui),
            median(tess)
        );
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

/// The main window's maximized state, kept out of eframe's hands (#458).
///
/// eframe restores a maximized window by creating it maximized but hidden
/// (it stays hidden until the first frame is painted). On Windows, winit
/// maximizes with `ShowWindow(SW_MAXIMIZE)`, which shows the window, then
/// hides it again (`SW_HIDE`) because it should be invisible; eframe shows
/// it once more after the first frame. That show, hide, show is the flash
/// at startup. So eframe is told the window was not maximized (it then
/// restores it, hidden, at the size and place it had, which is the
/// maximized one), and Locus maximizes it once it is on screen, which
/// changes nothing visible.
#[cfg(not(target_arch = "wasm32"))]
mod main_window {
    /// Locus's own record: was the main window maximized?
    pub const MAXIMIZED_KEY: &str = "locus.window.maximized";
    /// eframe's key for its window settings (`epi_integration`).
    const EFRAME_WINDOW_KEY: &str = "window";

    /// Runs after eframe has stored its window settings (eframe saves those
    /// first, then calls `App::save`).
    pub fn save(storage: &mut dyn eframe::Storage, maximized: bool) {
        storage.set_string(MAXIMIZED_KEY, maximized.to_string());
        if let Some(w) = storage.get_string(EFRAME_WINDOW_KEY) {
            let restored = unmaximized(&w);
            if restored != w {
                storage.set_string(EFRAME_WINDOW_KEY, restored);
            }
        }
    }

    /// eframe's RON window settings with `maximized` cleared.
    pub fn unmaximized(ron: &str) -> String {
        ron.replace("maximized:true", "maximized:false")
            .replace("maximized: true", "maximized: false")
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn the_saved_window_is_never_maximized_for_eframe() {
            let saved = "(inner_position_pixels:Some((x:0.0,y:23.0)),outer_position_pixels:Some((x:-8.0,y:-8.0)),fullscreen:false,maximized:true,inner_size_points:Some((x:1280.0,y:777.0)))";
            let out = unmaximized(saved);
            assert!(out.contains("maximized:false"), "{out}");
            assert_eq!(out.replace("maximized:false", "maximized:true"), saved);
            assert_eq!(unmaximized("(maximized: true)"), "(maximized: false)");
            assert_eq!(unmaximized("(maximized:false)"), "(maximized:false)");
        }
    }
}
