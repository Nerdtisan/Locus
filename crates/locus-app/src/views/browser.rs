// SPDX-License-Identifier: AGPL-3.0-or-later
//! The Browser (#326, M5 design §2): one tree on the left, as Fusion's.
//!
//! ```text
//! ▾ coaster-tray
//!   ▾ Layers          +
//!   ▾ Parameters      3
//!   ▾ Recipes         4
//!   ▸ Regions         7
//!   ▸ Constraints    38
//! ```
//!
//! Each section header shows its count (Layers shows + to add one). The
//! rows are drawn by the views that own them; this module only lays out the
//! tree, so the Parameters and Constraints panes, still available from
//! View ▸ Panes, never disagree with it.

use crate::actions::Shell;
use crate::layout::Tab;
use crate::settings::Settings;
use crate::state::AppState;
use crate::views::icons;
use crate::views::layers::LayersView;
use crate::views::parameters::trimmed;
use crate::views::recipes::RecipesView;
use egui::Ui;

/// The Browser's widget state.
#[derive(Default)]
pub struct BrowserView {
    pub layers: LayersView,
    pub recipes: RecipesView,
}

/// What a section header shows at its right end.
enum Badge {
    Count(usize),
    /// A + button, with its hover text; true when clicked.
    Add(&'static str),
}

/// A collapsible section: title, then its badge right-aligned. Returns true
/// when the badge's + was clicked.
fn section(
    ui: &mut Ui,
    key: &str,
    title: &str,
    badge: Badge,
    open: bool,
    body: impl FnOnce(&mut Ui),
) -> bool {
    let id = ui.make_persistent_id(("browser", key));
    let mut add = false;
    egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, open)
        .show_header(ui, |ui| {
            ui.label(title);
            ui.with_layout(
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| match badge {
                    Badge::Count(n) => {
                        ui.weak(n.to_string());
                    }
                    Badge::Add(tip) => {
                        let (rect, r) =
                            ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::click());
                        let c = if r.hovered() {
                            ui.visuals().strong_text_color()
                        } else {
                            ui.visuals().weak_text_color()
                        };
                        icons::paint(ui.painter(), rect.shrink(2.0), "ui.add", "+", c);
                        add = r.on_hover_text(tip).clicked();
                    }
                },
            );
        })
        .body(body);
    add
}

impl BrowserView {
    /// Draw the tree. `doc_name` is the open file's name, or "Untitled".
    pub fn ui(&mut self, ui: &mut Ui, state: &mut AppState, settings: &Settings, doc_name: &str) {
        let root = ui.make_persistent_id("browser-root");
        egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), root, true)
            .show_header(ui, |ui| {
                ui.strong(doc_name);
            })
            .body(|ui| {
                let add = section(
                    ui,
                    "layers",
                    "Layers",
                    Badge::Add("Add a layer"),
                    true,
                    |ui| self.layers.ui(ui, state, settings),
                );
                if add {
                    self.layers.request_add();
                }
                let n = state.document().parameters().len();
                section(
                    ui,
                    "parameters",
                    "Parameters",
                    Badge::Count(n),
                    true,
                    |ui| parameter_rows(ui, state),
                );
                let n = state.document().recipes().len();
                section(ui, "recipes", "Recipes", Badge::Count(n), true, |ui| {
                    self.recipes.ui(ui, state, settings.units);
                });
                let n = state.regions().len();
                section(ui, "regions", "Regions", Badge::Count(n), false, |ui| {
                    state.regions_ui(ui);
                });
                let n = state.document().constraints().len();
                section(
                    ui,
                    "constraints",
                    "Constraints",
                    Badge::Count(n),
                    false,
                    |ui| state.constraints_ui(ui, settings),
                );
            });
        ui.add_space(6.0);
        ui.weak(
            "Layers hold operation settings only. Style (colour, pen, fill) lives on each object.",
        );
    }
}

/// The parameters as `name = expression` rows (mockup); double-click or
/// "Edit parameters…" opens the Parameters pane to change them.
fn parameter_rows(ui: &mut Ui, state: &mut AppState) {
    let mut edit = false;
    let doc = state.document();
    if doc.parameters().is_empty() {
        ui.weak("Name a value once, use it in any dimension.");
    }
    for (name, p) in doc.parameters() {
        let text = egui::RichText::new(format!("{name} = {}", p.expr)).monospace();
        let r = ui
            .selectable_label(false, text)
            .on_hover_text(format!("= {}\nDouble-click to edit", trimmed(p.value)));
        edit |= r.double_clicked();
        r.context_menu(|ui| {
            if ui.button("Edit parameters…").clicked() {
                edit = true;
                ui.close();
            }
        });
    }
    if ui
        .small_button("Edit parameters…")
        .on_hover_text("Add, rename or change parameters in the Parameters pane")
        .clicked()
    {
        edit = true;
    }
    if edit {
        state.view.shell.push(Shell::ShowPane(Tab::Parameters));
    }
}
