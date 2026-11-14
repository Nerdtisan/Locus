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

    /// Is `tab` showing, docked or in its own window?
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
                let builder = egui::ViewportBuilder::default()
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
    fn shell_requests(&mut self) {
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
                for tab in Tab::PANES {
                    let mut shown = self.is_shown(tab);
                    if ui.checkbox(&mut shown, tab.title()).changed() {
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
                {
                    let mut cx = cx!(self);
                    actions::show(ui, &self.registry, &mut cx, actions::HELP_MENU, Style::Menu);
                }
                ui.separator();
                ui.hyperlink_to("Locus on GitHub", "https://github.com/Nerdtisan/Locus");
                ui.label(format!("Locus v{}", env!("CARGO_PKG_VERSION")));
            });
        });
    }

    /// The SKETCH ribbon with the user's pinned tools (#299, design §2.3).
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
            ui.label(egui::RichText::new(self.state.hint()).strong())
                .on_hover_text("What the next click does");
            self.state.touch_modifiers_ui(ui);
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
                        let (x, y) = match u {
                            Units::Mm => (format!("{:.2}", p.x), format!("{:.2}", p.y)),
                            Units::Inch => {
                                (format!("{:.3}", p.x / 25.4), format!("{:.3}", p.y / 25.4))
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

    fn title(&mut self, tab: &mut Tab) -> egui::WidgetText {
        tab.title().into()
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
        egui::Panel::top("ribbon").show(ui, |ui| self.ribbon(ui));
        if !self.is_shown(Tab::Palette) {
            egui::Panel::top("options").show(ui, |ui| self.options_bar(ui));
        }
        self.shell_requests();
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));
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
        let mut cx = cx!(self);
        crate::views::marking_menu::show(&ctx, &self.registry, &mut cx);
        // A pane or the marking menu asked the shell for something after
        // it drained the requests: act on it next frame, not next input.
        if !self.state.view.shell.is_empty() {
            ctx.request_repaint();
        }
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
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
