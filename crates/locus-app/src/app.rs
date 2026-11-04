// SPDX-License-Identifier: AGPL-3.0-or-later
//! App shell (#65, SPEC §12.1): egui_dock layout with the canvas in the
//! centre, properties and constraints panels, a menu bar and a status bar.
//!
//! The shell never mutates a document directly; once `locus-doc` lands every
//! edit goes through its command API (SPEC §11).

use crate::canvas::Canvas;
use crate::constrain::Kind;
use crate::file_io::FileIo;
use crate::settings::{Settings, Theme, Units};
use crate::tools::Tool;
use egui_dock::{DockArea, DockState, NodeIndex, TabViewer};
use locus_geom::Vec2;
use serde::{Deserialize, Serialize};

const LAYOUT_KEY: &str = "locus.layout.v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Tab {
    Canvas,
    Properties,
    Constraints,
}

impl Tab {
    fn title(self) -> &'static str {
        match self {
            Tab::Canvas => "Sketch",
            Tab::Properties => "Properties",
            Tab::Constraints => "Constraints",
        }
    }
}

fn default_layout() -> DockState<Tab> {
    let mut dock = DockState::new(vec![Tab::Canvas]);
    let tree = dock.main_surface_mut();
    let [_canvas, right] = tree.split_right(NodeIndex::root(), 0.76, vec![Tab::Properties]);
    tree.split_below(right, 0.55, vec![Tab::Constraints]);
    dock
}

/// A saved layout is only trusted if it parses and still holds the canvas;
/// otherwise the default is used (a corrupt file never crashes the app).
fn parse_layout(json: &str) -> DockState<Tab> {
    match serde_json::from_str::<DockState<Tab>>(json) {
        Ok(d) if d.iter_all_tabs().any(|(_, t)| *t == Tab::Canvas) => d,
        _ => {
            tracing::warn!("saved layout unusable, using default");
            default_layout()
        }
    }
}

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

pub struct App {
    dock: DockState<Tab>,
    settings: Settings,
    canvas: Canvas,
    status: Status,
    files: FileIo,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        crate::fonts::install(&cc.egui_ctx);
        let settings = Settings::load(cc.storage);
        cc.egui_ctx.set_theme(settings.theme.preference());
        let dock = cc
            .storage
            .and_then(|s| s.get_string(LAYOUT_KEY))
            .map_or_else(default_layout, |j| parse_layout(&j));
        Self {
            dock,
            settings,
            canvas: Canvas::default(),
            status: Status::default(),
            files: FileIo::new(&cc.egui_ctx, cc.storage),
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn load_stress(&mut self) {
        self.canvas.load(crate::demo::stress(5000));
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
                self.files.file_menu(ui, &mut self.canvas);
                ui.separator();
                ui.menu_button("Samples", |ui| {
                    if ui.button("Demo sketch").clicked() {
                        let ctx = ui.ctx().clone();
                        self.files
                            .open_sample(&ctx, &mut self.canvas, crate::demo::sketch());
                        ui.close();
                    }
                    if ui.button("Stress scene (5 000 curves)").clicked() {
                        let ctx = ui.ctx().clone();
                        self.files
                            .open_sample(&ctx, &mut self.canvas, crate::demo::stress(5000));
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
                let undo = history_label("Undo", self.canvas.undo_label());
                if ui
                    .add_enabled(
                        self.canvas.undo_label().is_some(),
                        egui::Button::new(undo).shortcut_text(ui.ctx().format_shortcut(&UNDO)),
                    )
                    .clicked()
                {
                    self.canvas.undo();
                    ui.close();
                }
                let redo = history_label("Redo", self.canvas.redo_label());
                if ui
                    .add_enabled(
                        self.canvas.redo_label().is_some(),
                        egui::Button::new(redo).shortcut_text(ui.ctx().format_shortcut(&REDO)),
                    )
                    .clicked()
                {
                    self.canvas.redo();
                    ui.close();
                }
                ui.separator();
                if ui.button("Select all").clicked() {
                    self.canvas.select_all();
                    ui.close();
                }
                if ui.button("Deselect").clicked() {
                    self.canvas.clear_selection();
                    ui.close();
                }
            });
            ui.menu_button("View", |ui| {
                if ui.button("Fit to content").clicked() {
                    self.canvas.request_fit();
                    ui.close();
                }
                ui.separator();
                for tab in [Tab::Properties, Tab::Constraints] {
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
                ui.menu_button("Units", |ui| {
                    for u in Units::ALL {
                        ui.selectable_value(&mut self.settings.units, u, u.label());
                    }
                });
            });
            ui.menu_button("Sketch", |ui| {
                for tool in Tool::DRAWING {
                    let label = match tool.key() {
                        Some(k) => format!("{}  ({k})", tool.label()),
                        None => tool.label().to_owned(),
                    };
                    if ui
                        .selectable_label(self.canvas.tool() == tool, label)
                        .clicked()
                    {
                        self.canvas.set_tool(tool);
                        ui.close();
                    }
                }
                stub(ui, "Rectangle");
            });
            ui.menu_button("Constrain", |ui| {
                for kind in Kind::ALL {
                    let enabled = self.canvas.can_constrain(kind);
                    let b = egui::Button::new(kind.label()).shortcut_text(kind.key().to_string());
                    if ui
                        .add_enabled(enabled, b)
                        .on_disabled_hover_text(format!("Select {}", kind.needs()))
                        .clicked()
                    {
                        self.canvas.constrain(kind);
                        ui.close();
                    }
                }
                ui.separator();
                let b = egui::Button::new("Dimension").shortcut_text("D");
                if ui
                    .add_enabled(self.canvas.can_dimension(), b)
                    .on_disabled_hover_text("Select a line, two points, a circle or arc…")
                    .clicked()
                {
                    self.canvas.start_dimension(self.settings.units);
                    ui.close();
                }
                ui.separator();
                ui.checkbox(
                    &mut self.settings.auto_constrain,
                    "Auto-constrain while drawing (hold Alt to suppress)",
                );
            });
            ui.menu_button("Help", |ui| {
                ui.hyperlink_to("Locus on GitHub", "https://github.com/Nerdtisan/Locus");
                ui.label(format!("Locus v{}", env!("CARGO_PKG_VERSION")));
            });
        });
    }

    /// The tool strip under the menu (#68): Select plus the drawing tools.
    fn tool_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            for tool in [Tool::Select].into_iter().chain(Tool::DRAWING) {
                let text = match tool.key() {
                    Some(k) => format!("{} ({k})", tool.label()),
                    None => tool.label().to_owned(),
                };
                if ui
                    .selectable_label(self.canvas.tool() == tool, text)
                    .clicked()
                {
                    self.canvas.set_tool(tool);
                }
            }
            ui.separator();
            ui.weak(self.canvas.hint());
        });
    }

    /// One button per constraint, enabled when the selection supports it (#70).
    fn constraint_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            for kind in Kind::ALL {
                let enabled = self.canvas.can_constrain(kind);
                let b = ui.add_enabled(enabled, egui::Button::new(kind.label()));
                let tip = format!("{} ({}): {}", kind.label(), kind.key(), kind.needs());
                if b.on_hover_text(tip).clicked() {
                    self.canvas.constrain(kind);
                }
            }
            ui.separator();
            let b = ui.add_enabled(self.canvas.can_dimension(), egui::Button::new("Dimension"));
            if b.on_hover_text("Dimension (D)").clicked() {
                self.canvas.start_dimension(self.settings.units);
            }
        });
    }

    /// Not while a text field has focus: it owns its own undo.
    fn history_shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        // The shift variant first: a plain-Z match ignores extra shift.
        let (redo, undo) = ctx.input_mut(|i| {
            (
                i.consume_shortcut(&REDO) || i.consume_shortcut(&REDO_ALT),
                i.consume_shortcut(&UNDO),
            )
        });
        if redo {
            self.canvas.redo();
        } else if undo {
            self.canvas.undo();
        }
    }

    fn status_bar(&self, ui: &mut egui::Ui) {
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
            ui.label(self.canvas.summary());
            ui.separator();
            ui.label(format!("CPU {:.1} ms/frame", self.status.frame_ms));
            ui.separator();
            ui.label(self.canvas.solve_summary());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(if self.settings.auto_constrain {
                    "Auto-constrain on"
                } else {
                    "Auto-constrain off"
                });
            });
        });
    }
}

/// Undo (Cmd/Ctrl+Z) and redo (Cmd+Shift+Z, Ctrl+Y) shortcuts (#73).
const UNDO: egui::KeyboardShortcut =
    egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::Z);
const REDO: egui::KeyboardShortcut = egui::KeyboardShortcut::new(
    egui::Modifiers::COMMAND.plus(egui::Modifiers::SHIFT),
    egui::Key::Z,
);
const REDO_ALT: egui::KeyboardShortcut =
    egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::Y);

/// "Undo Add Line" / plain "Undo" when there is nothing to undo.
fn history_label(verb: &str, label: Option<&str>) -> String {
    label.map_or_else(|| verb.to_owned(), |l| format!("{verb} {l}"))
}

fn stub(ui: &mut egui::Ui, label: &str) {
    ui.add_enabled(false, egui::Button::new(label))
        .on_disabled_hover_text("Not implemented yet");
}

struct Viewer<'a> {
    canvas: &'a mut Canvas,
    settings: &'a Settings,
    status: &'a mut Status,
}

impl TabViewer for Viewer<'_> {
    type Tab = Tab;

    fn title(&mut self, tab: &mut Tab) -> egui::WidgetText {
        tab.title().into()
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Tab) {
        match tab {
            Tab::Canvas => self.canvas.ui(ui, self.settings, self.status),
            Tab::Properties => self.canvas.properties_ui(ui, self.settings),
            Tab::Constraints => self.canvas.constraints_ui(ui, self.settings),
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
        self.history_shortcuts(ui.ctx());
        self.files.update(ui.ctx(), &mut self.canvas);
        egui::Panel::top("menu").show(ui, |ui| self.menu_bar(ui));
        egui::Panel::top("tools").show(ui, |ui| self.tool_bar(ui));
        egui::Panel::top("constraints").show(ui, |ui| self.constraint_bar(ui));
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));
        let mut viewer = Viewer {
            canvas: &mut self.canvas,
            settings: &self.settings,
            status: &mut self.status,
        };
        DockArea::new(&mut self.dock)
            .show_leaf_collapse_buttons(false)
            .show_inside(ui, &mut viewer);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        self.settings.save(storage);
        self.files.save_state(storage);
        if let Ok(json) = serde_json::to_string(&self.dock) {
            storage.set_string(LAYOUT_KEY, json);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_labels() {
        assert_eq!(history_label("Undo", None), "Undo");
        assert_eq!(history_label("Redo", Some("Add Line")), "Redo Add Line");
    }

    #[test]
    fn layout_round_trips() {
        let json = serde_json::to_string(&default_layout()).unwrap_or_default();
        assert_eq!(parse_layout(&json).iter_all_tabs().count(), 3);
    }

    #[test]
    fn corrupt_layout_falls_back() {
        for bad in ["", "{", "null", "{\"surfaces\":[]}", "42"] {
            let d = parse_layout(bad);
            assert!(d.iter_all_tabs().any(|(_, t)| *t == Tab::Canvas), "{bad:?}");
        }
    }

    #[test]
    fn layout_without_canvas_falls_back() {
        let json =
            serde_json::to_string(&DockState::new(vec![Tab::Properties])).unwrap_or_default();
        let back = parse_layout(&json);
        assert!(back.iter_all_tabs().any(|(_, t)| *t == Tab::Canvas));
    }
}
