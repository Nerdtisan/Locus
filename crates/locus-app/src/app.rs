// SPDX-License-Identifier: AGPL-3.0-or-later
//! App shell (#65, SPEC §12.1): egui_dock layout with the canvas in the
//! centre, properties and constraints panels, a menu bar and a status bar.
//!
//! The shell never mutates a document directly; once `locus-doc` lands every
//! edit goes through its command API (SPEC §11).

use crate::canvas::Canvas;
use crate::constrain::Kind;
use crate::file_io::FileIo;
use crate::modify_tool::{ChamferMode, Modify, TransformMode};
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
    Layers,
    Constraints,
    /// Regions and boolean recipes (#124).
    Regions,
    /// Named parameters for dimension expressions (#171).
    Parameters,
    /// Linear and circular patterns (#168, #169).
    Patterns,
}

impl Tab {
    fn title(self) -> &'static str {
        match self {
            Tab::Canvas => "Sketch",
            Tab::Properties => "Properties",
            Tab::Layers => "Layers",
            Tab::Constraints => "Constraints",
            Tab::Regions => "Regions",
            Tab::Parameters => "Parameters",
            Tab::Patterns => "Patterns",
        }
    }
}

fn default_layout() -> DockState<Tab> {
    let mut dock = DockState::new(vec![Tab::Canvas]);
    let tree = dock.main_surface_mut();
    let [_canvas, right] = tree.split_right(
        NodeIndex::root(),
        0.76,
        vec![
            Tab::Properties,
            Tab::Layers,
            Tab::Regions,
            Tab::Parameters,
            Tab::Patterns,
        ],
    );
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
    #[cfg(not(target_arch = "wasm32"))]
    plugins: crate::plugins::Plugins,
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
        #[allow(unused_mut)] // mutated only on desktop
        let mut app = Self {
            dock,
            settings,
            canvas: {
                let mut c = Canvas::default();
                c.install_fonts(&cc.egui_ctx);
                c
            },
            status: Status::default(),
            files: FileIo::new(&cc.egui_ctx, cc.storage),
            #[cfg(not(target_arch = "wasm32"))]
            plugins: crate::plugins::Plugins::default(),
        };
        #[cfg(not(target_arch = "wasm32"))]
        app.plugins.attach(&mut app.canvas, &mut app.files);
        app
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
                self.files.file_menu(ui, &mut self.canvas, &self.settings);
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
                let mut machine = self.canvas.machine_preview();
                if ui
                    .checkbox(&mut machine, "Machine preview")
                    .on_hover_text(
                        "Show kerf- and blade-compensated paths and hatch fills, as they will be exported",
                    )
                    .changed()
                {
                    self.canvas.set_machine_preview(machine);
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
            ui.menu_button("Plugins", |ui| {
                #[cfg(not(target_arch = "wasm32"))]
                self.plugins.menu(ui, &self.canvas);
                #[cfg(target_arch = "wasm32")]
                ui.weak("Plugins run in the desktop app.");
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
            for m in Modify::ALL {
                if ui
                    .selectable_label(self.canvas.modify() == Some(m), m.label())
                    .clicked()
                {
                    self.canvas.set_modify(m);
                }
            }
            ui.separator();
            self.corner_sizes(ui);
            if self.canvas.tool() == Tool::Text {
                self.canvas.text_tool_ui(ui);
                ui.separator();
            }
            if self.canvas.tool() == Tool::Polygon {
                let (mut sides, mut outside) = self.canvas.polygon();
                let changed = ui
                    .add(
                        egui::DragValue::new(&mut sides)
                            .range(3..=64)
                            .suffix(" sides"),
                    )
                    .changed()
                    | ui.checkbox(&mut outside, "Circumscribed").changed();
                if changed {
                    self.canvas.set_polygon(sides, outside);
                }
                ui.separator();
            }
            ui.weak(self.canvas.hint());
        });
    }

    /// Fillet radius and chamfer size, while those tools are active (mm and
    /// degrees; SPEC §6.2).
    fn corner_sizes(&mut self, ui: &mut egui::Ui) {
        fn len(v: &mut f64) -> egui::DragValue<'_> {
            egui::DragValue::new(v)
                .range(0.001..=10_000.0)
                .speed(0.1)
                .suffix(" mm")
        }
        let m = self.canvas.modify();
        let c = &mut self.canvas.corner;
        match m {
            Some(Modify::Fillet) => {
                ui.label("Radius");
                ui.add(len(&mut c.radius));
                ui.separator();
            }
            Some(Modify::Chamfer) => {
                egui::ComboBox::from_id_salt("chamfer-mode")
                    .selected_text(c.mode.label())
                    .show_ui(ui, |ui| {
                        for m in ChamferMode::ALL {
                            ui.selectable_value(&mut c.mode, m, m.label());
                        }
                    });
                ui.add(len(&mut c.d1));
                match c.mode {
                    ChamferMode::Equal => {}
                    ChamferMode::TwoDistances => {
                        ui.add(len(&mut c.d2));
                    }
                    ChamferMode::DistanceAngle => {
                        ui.add(
                            egui::DragValue::new(&mut c.angle_deg)
                                .range(0.1..=179.9)
                                .suffix("°"),
                        );
                    }
                }
                ui.separator();
            }
            Some(Modify::Offset) => {
                ui.label("Distance");
                ui.add(egui::DragValue::new(&mut c.offset).speed(0.1).suffix(" mm"));
                ui.checkbox(&mut c.sharp, "Sharp corners");
                if let Some(rid) = self.canvas.offset_selected()
                    && let Some((mut d, mut sharp)) = self.canvas.offset_params(rid)
                {
                    ui.separator();
                    ui.label("Picked offset");
                    // Typed, not dragged: each change is one undo step.
                    let r1 = ui
                        .add(
                            egui::DragValue::new(&mut d)
                                .speed(0.0)
                                .max_decimals(3)
                                .suffix(" mm"),
                        )
                        .on_hover_text("Click to type a new distance");
                    let r2 = ui.checkbox(&mut sharp, "Sharp");
                    if (r1.changed() || r2.changed()) && d != 0.0 {
                        self.canvas.set_offset_params(rid, d, sharp);
                    }
                    if ui
                        .button("Explode")
                        .on_hover_text("Turn the offset into ordinary geometry")
                        .clicked()
                    {
                        self.canvas.finish_offset(rid, true);
                    }
                    if ui.button("Remove").clicked() {
                        self.canvas.finish_offset(rid, false);
                    }
                }
                ui.separator();
            }
            Some(Modify::Transform) => {
                let t = &mut self.canvas.transform;
                egui::ComboBox::from_id_salt("transform-mode")
                    .selected_text(t.mode.label())
                    .show_ui(ui, |ui| {
                        for m in TransformMode::ALL {
                            ui.selectable_value(&mut t.mode, m, m.label());
                        }
                    });
                match t.mode {
                    TransformMode::Move => {
                        ui.label("dx");
                        ui.add(egui::DragValue::new(&mut t.dx).speed(0.1).suffix(" mm"));
                        ui.label("dy");
                        ui.add(egui::DragValue::new(&mut t.dy).speed(0.1).suffix(" mm"));
                    }
                    TransformMode::Rotate => {
                        ui.add(
                            egui::DragValue::new(&mut t.angle_deg)
                                .speed(1.0)
                                .suffix("°"),
                        );
                    }
                    TransformMode::Scale => {
                        ui.add(
                            egui::DragValue::new(&mut t.factor)
                                .range(0.001..=1000.0)
                                .speed(0.01)
                                .prefix("×"),
                        );
                    }
                }
                ui.checkbox(&mut t.copy, "Copy");
                let ok = self.canvas.can_transform();
                if ui
                    .add_enabled(ok, egui::Button::new("Apply"))
                    .on_disabled_hover_text("Select geometry first")
                    .clicked()
                {
                    self.canvas.apply_transform(self.settings.units);
                }
                ui.separator();
            }
            _ => {}
        }
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

struct Viewer<'a> {
    canvas: &'a mut Canvas,
    settings: &'a Settings,
    status: &'a mut Status,
    #[cfg(not(target_arch = "wasm32"))]
    plugins: &'a mut crate::plugins::Plugins,
}

impl TabViewer for Viewer<'_> {
    type Tab = Tab;

    fn title(&mut self, tab: &mut Tab) -> egui::WidgetText {
        tab.title().into()
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Tab) {
        match tab {
            Tab::Canvas => self.canvas.ui(ui, self.settings, self.status),
            Tab::Properties => {
                #[cfg(not(target_arch = "wasm32"))]
                self.plugins.properties_ui(ui, self.canvas);
                #[cfg(target_arch = "wasm32")]
                if let Some((rid, plugin, modifier)) = self.canvas.selected_plugin_object() {
                    ui.label(egui::RichText::new("Plugin object").strong());
                    if modifier && self.canvas.plugin_object_stale(rid) {
                        ui.colored_label(ui.visuals().warn_fg_color, "⚠ Out of date");
                    }
                    self.canvas.plugin_object_ui(ui, &plugin);
                    ui.weak("Plugins run in the desktop app; this one is shown from its saved geometry.");
                    ui.separator();
                }
                self.canvas.properties_ui(ui, self.settings);
            }
            Tab::Layers => self.canvas.layers_ui(ui, self.settings),
            Tab::Constraints => self.canvas.constraints_ui(ui, self.settings),
            Tab::Regions => self.canvas.regions_ui(ui),
            Tab::Parameters => self.canvas.parameters_ui(ui),
            Tab::Patterns => self.canvas.patterns_ui(ui, self.settings.units),
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
        self.files
            .update(ui.ctx(), &mut self.canvas, &mut self.settings);
        #[cfg(not(target_arch = "wasm32"))]
        self.plugins
            .windows(ui.ctx(), &mut self.canvas, &mut self.files);
        egui::Panel::top("menu").show(ui, |ui| self.menu_bar(ui));
        egui::Panel::top("tools").show(ui, |ui| self.tool_bar(ui));
        egui::Panel::top("constraints").show(ui, |ui| self.constraint_bar(ui));
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));
        let mut viewer = Viewer {
            canvas: &mut self.canvas,
            settings: &self.settings,
            status: &mut self.status,
            #[cfg(not(target_arch = "wasm32"))]
            plugins: &mut self.plugins,
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
        assert_eq!(parse_layout(&json).iter_all_tabs().count(), 7);
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
