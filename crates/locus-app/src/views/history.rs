// SPDX-License-Identifier: AGPL-3.0-or-later
//! The History pane (#446, M6 design §7, ADR 0019 decision 5): two tabs.
//!
//! - **History:** every undo step's label, oldest at the top, the current
//!   state highlighted. Clicking a row goes back (or forward) to just after
//!   it as one batch with one re-solve. Later rows dim but stay until the
//!   next edit, which drops them: the history is linear, as in Photoshop.
//!   ↑ / ↓ move the selection and Enter jumps to it. Session only, capped at
//!   [`locus_doc::MAX_HISTORY`] steps.
//! - **Timeline:** the document's features (patterns, mirrors, offsets,
//!   plugin objects) in creation order. Hovering one highlights its
//!   geometry, clicking selects it, and the right-click menu explodes or
//!   deletes it, each one command (SPEC §11).

use crate::state::AppState;
use crate::views::icons;
use egui::{Key, Ui};
use locus_doc::{Command, EntityId, RecipeId};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Page {
    #[default]
    History,
    Timeline,
}

/// Transient pane state (never part of the document, SPEC §11).
#[derive(Debug, Default)]
pub struct HistoryView {
    page: Page,
    /// The row the arrow keys move, as a history step (0 = the start).
    focus: Option<usize>,
}

impl HistoryView {
    pub fn ui(&mut self, ui: &mut Ui, app: &mut AppState) {
        ui.horizontal(|ui| {
            if let Some(p) = crate::theme::segmented(
                ui,
                self.page,
                &[(Page::History, "History"), (Page::Timeline, "Timeline")],
            ) {
                self.page = p;
            }
        });
        ui.separator();
        if self.page != Page::Timeline {
            app.selection.highlight.clear();
        }
        match self.page {
            Page::History => self.history(ui, app),
            Page::Timeline => timeline(ui, app),
        }
    }

    fn history(&mut self, ui: &mut Ui, app: &mut AppState) {
        let h = app.doc.sketcher.editor().history();
        let mut jump = None;
        let focus = self.focus.unwrap_or(h.current).min(h.steps.len());
        egui::ScrollArea::vertical()
            .auto_shrink([false, true])
            .max_height(ui.available_height() - 24.0)
            .show(ui, |ui| {
                // Row 0 is the document before the first step.
                let rows = std::iter::once("Start").chain(h.steps.iter().map(String::as_str));
                for (step, label) in rows.enumerate() {
                    let later = step > h.current;
                    let text = if later {
                        egui::RichText::new(label).weak()
                    } else {
                        egui::RichText::new(label)
                    };
                    let r = ui
                        .add(egui::Button::selectable(step == h.current, text).truncate())
                        .on_hover_text(if later {
                            "Redo to here (dropped when you change the document)"
                        } else {
                            "Go back to here"
                        });
                    // The arrow keys' row shows as highlighted.
                    let r = if step == focus && self.focus.is_some() {
                        r.highlight()
                    } else {
                        r
                    };
                    if r.clicked() {
                        jump = Some(step);
                    }
                }
            });
        // Arrow keys and Enter, while the pointer is over the pane.
        if ui.ui_contains_pointer() && !ui.ctx().egui_wants_keyboard_input() {
            let (up, down, enter) = ui.input_mut(|i| {
                (
                    i.consume_key(egui::Modifiers::NONE, Key::ArrowUp),
                    i.consume_key(egui::Modifiers::NONE, Key::ArrowDown),
                    i.consume_key(egui::Modifiers::NONE, Key::Enter),
                )
            });
            if up {
                self.focus = Some(focus.saturating_sub(1));
            } else if down {
                self.focus = Some((focus + 1).min(h.steps.len()));
            } else if enter {
                jump = Some(focus);
            }
        }
        ui.separator();
        ui.weak(format!(
            "Session only · {} steps",
            group_thousands(locus_doc::MAX_HISTORY)
        ));
        if let Some(step) = jump {
            self.focus = None;
            app.history_go(step);
        }
    }
}

/// `1000` as "1 000", as the design writes it.
fn group_thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

/// The features in creation order: a strip of icons with names.
fn timeline(ui: &mut Ui, app: &mut AppState) {
    let mut cmds = Vec::new();
    let mut select: Option<Vec<EntityId>> = None;
    let mut hover: Option<Vec<EntityId>> = None;
    let mut edit: Option<RecipeId> = None;
    {
        let doc = app.doc.sketcher.document();
        if doc.recipes().is_empty() {
            ui.weak("Patterns, mirrors, offsets and plugin objects appear here, in the order they were made.");
        }
        let failures = app.doc.sketcher.editor().recipe_failures();
        let names = feature_names(doc);
        ui.horizontal_wrapped(|ui| {
            // Timeline order (#446): reorderable where nothing depends on it.
            let order = doc.recipe_order();
            let count = order.len();
            for (pos, rid) in order.iter().enumerate() {
                let Some(r) = doc.recipe(*rid) else { continue };
                let name = names.get(rid).cloned().unwrap_or_default();
                let failed = failures.get(rid);
                let geometry: Vec<EntityId> =
                    r.sources.iter().copied().chain(r.instances()).collect();
                let resp = ui
                    .vertical(|ui| {
                        let (slot, resp) =
                            ui.allocate_exact_size(egui::vec2(28.0, 28.0), egui::Sense::click());
                        if resp.hovered() {
                            ui.painter().rect_filled(
                                slot,
                                4.0,
                                ui.visuals().widgets.hovered.weak_bg_fill,
                            );
                        }
                        // Suppressed features are dim and struck through.
                        let off = r.feature_suppressed;
                        // A failed feature shows red with its reason (#446).
                        let c = if failed.is_some() {
                            ui.visuals().error_fg_color
                        } else if off {
                            ui.visuals().weak_text_color()
                        } else {
                            ui.visuals().text_color()
                        };
                        icons::paint(
                            ui.painter(),
                            slot.shrink(4.0),
                            icons::recipe(&r.kind),
                            "",
                            c,
                        );
                        let label = egui::RichText::new(&name).small();
                        let label = if failed.is_some() {
                            label.color(ui.visuals().error_fg_color)
                        } else {
                            label
                        };
                        ui.label(if off { label.strikethrough().weak() } else { label });
                        resp
                    })
                    .inner
                    .on_hover_text(match failed {
                        Some(why) => format!(
                            "{name} failed: {why}. It keeps its last good copies; edit it or its inputs to fix it."
                        ),
                        None => format!(
                            "{name}: click to select its geometry, double-click to edit it; right-click for more"
                        ),
                    });
                if resp.hovered() {
                    hover = Some(geometry.clone());
                }
                if resp.clicked() {
                    select = Some(geometry.clone());
                }
                if resp.double_clicked() {
                    edit = Some(*rid);
                }
                let off = r.feature_suppressed;
                resp.context_menu(|ui| {
                    menu(ui, *rid, off, &mut cmds, &mut edit);
                    ui.separator();
                    for (label, to, ok) in [
                        ("Move earlier", pos.saturating_sub(1), pos > 0),
                        ("Move later", pos + 1, pos + 1 < count),
                    ] {
                        if ui
                            .add_enabled(ok, egui::Button::new(label))
                            .on_hover_text("Change its place in the timeline; refused if it would come before a feature it uses")
                            .clicked()
                        {
                            cmds.push(Command::ReorderRecipe {
                                id: *rid,
                                to: u32::try_from(to).unwrap_or(u32::MAX),
                            });
                            ui.close();
                        }
                    }
                });
            }
        });
    }
    if let Some(ids) = select {
        app.selection.items.set_all(ids);
    }
    // Edit: the feature's own settings, in its Browser row, opened.
    if let Some(rid) = edit {
        let id = egui::Id::new(("browser-recipe", rid));
        let ctx = ui.ctx().clone();
        let mut state =
            egui::collapsing_header::CollapsingState::load_with_default_open(&ctx, id, false);
        state.set_open(true);
        state.store(&ctx);
        app.view
            .shell
            .push(crate::actions::Shell::ShowPane(crate::layout::Tab::Browser));
    }
    // Hovering a feature highlights its geometry, as hovering a curve does.
    app.selection.highlight = hover.unwrap_or_default().into_iter().collect();
    for cmd in &cmds {
        app.apply_reporting(cmd);
    }
}

/// Each feature's Timeline name: its kind, numbered within the kind in
/// creation order, so a reordered feature keeps its name and the move shows
/// (#514: numbering by position made a swap of two offsets look like
/// nothing happened).
fn feature_names(doc: &locus_doc::Document) -> std::collections::BTreeMap<RecipeId, String> {
    let mut seen = std::collections::BTreeMap::<&str, usize>::new();
    doc.recipes()
        .iter()
        .map(|(id, r)| {
            let n = seen.entry(r.kind.name()).or_default();
            *n += 1;
            (*id, format!("{} {n}", r.kind.name()))
        })
        .collect()
}

fn menu(
    ui: &mut Ui,
    id: RecipeId,
    off: bool,
    cmds: &mut Vec<Command>,
    edit: &mut Option<RecipeId>,
) {
    if ui
        .button("Edit")
        .on_hover_text("Open its settings in the Browser")
        .clicked()
    {
        *edit = Some(id);
        ui.close();
    }
    // Mirrors too (#516): their copies and their Symmetric/Equal
    // constraints go and come back under the same IDs (#499).
    let (label, tip) = if off {
        ("Unsuppress", "Bring its copies back")
    } else {
        (
            "Suppress",
            "Hide its copies but keep the feature (one undo step); refused, with the reason, while something else uses them",
        )
    };
    if ui.button(label).on_hover_text(tip).clicked() {
        cmds.push(Command::SetRecipeSuppressed {
            id,
            suppressed: !off,
        });
        ui.close();
    }
    if ui
        .button("Explode")
        .on_hover_text("Keep the copies as plain geometry and drop the feature")
        .clicked()
    {
        cmds.push(Command::ExplodeRecipe { id });
        ui.close();
    }
    if ui
        .button("Delete")
        .on_hover_text("Remove the feature and its copies")
        .clicked()
    {
        cmds.push(Command::RemoveRecipe { id });
        ui.close();
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// #514: names follow creation order, not the timeline position, so
    /// moving a feature visibly moves its name.
    #[test]
    fn a_moved_feature_keeps_its_name() {
        use locus_doc::{Document, Editor, Geom, RecipeKind};
        let mut ed = Editor::new(Document::new());
        let l = Document::default_layer();
        let mut seg = |y: f64| {
            let mut pt = |x: f64| {
                ed.apply(&Command::AddEntity {
                    layer: l,
                    construction: false,
                    style: Default::default(),
                    geom: Geom::Point {
                        pos: locus_geom::Vec2::new(x, y),
                    },
                })
                .unwrap()
                .entities[0]
            };
            let (p, q) = (pt(0.0), pt(5.0));
            ed.apply(&Command::AddEntity {
                layer: l,
                construction: false,
                style: Default::default(),
                geom: Geom::Line { start: p, end: q },
            })
            .unwrap()
            .entities[0]
        };
        let (s1, s2) = (seg(0.0), seg(50.0));
        let lin = || RecipeKind::Linear {
            dir1: locus_doc::recipe::LinearDir {
                axis: locus_doc::recipe::Axis::X,
                count: "2".into(),
                spacing: "10".into(),
                symmetric: false,
            },
            dir2: None,
        };
        let mut add = |s| {
            ed.apply(&Command::AddRecipe {
                kind: lin(),
                sources: vec![s],
            })
            .unwrap()
            .recipes[0]
        };
        let (a, b) = (add(s1), add(s2));
        let names = |ed: &Editor| {
            let n = feature_names(ed.document());
            ed.document()
                .recipe_order()
                .iter()
                .map(|r| n[r].clone())
                .collect::<Vec<_>>()
        };
        let first = names(&ed);
        ed.apply(&Command::ReorderRecipe { id: b, to: 0 }).unwrap();
        let moved = names(&ed);
        assert_ne!(first, moved);
        assert_eq!(moved[0], feature_names(ed.document())[&b]);
        assert_ne!(moved[0], feature_names(ed.document())[&a]);
    }

    #[test]
    fn the_cap_reads_as_the_design_writes_it() {
        assert_eq!(group_thousands(1000), "1 000");
        assert_eq!(group_thousands(12), "12");
        assert_eq!(group_thousands(1_234_567), "1 234 567");
    }

    #[test]
    fn jumping_back_is_one_step_and_an_edit_drops_the_rest() {
        let mut c = AppState::default();
        c.load(locus_doc::Document::new());
        for x in 0..3 {
            c.apply(&Command::AddEntity {
                layer: locus_doc::Document::default_layer(),
                construction: false,
                style: Default::default(),
                geom: locus_doc::Geom::Point {
                    pos: locus_geom::Vec2::new(f64::from(x), 0.0),
                },
            })
            .unwrap();
        }
        c.history_go(1);
        assert_eq!(c.document().entities().len(), 1);
        let h = c.doc.sketcher.editor().history();
        assert_eq!((h.steps.len(), h.current), (3, 1));
        c.history_go(3);
        assert_eq!(c.document().entities().len(), 3);
    }
}
