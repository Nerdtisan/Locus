// SPDX-License-Identifier: AGPL-3.0-or-later
//! The Browser's Recipes rows (#168, #169, #326; SPEC §6.2.1, M5 design §2):
//! every pattern, mirror, offset and plugin object, one row each. Clicking a
//! row selects its geometry; right-clicking also opens the marking menu.
//! Expanding a row edits a pattern's count / spacing / angle as expressions
//! over the parameters, suppresses single copies, and explodes or removes
//! any recipe. Patterns are made with Create ▸ Rectangular / Circular
//! Pattern (#221). Every action is one `Sketcher` command, so one undo step.
//! Text is held in a draft while a box has focus and committed when focus
//! leaves it.

use crate::settings::Units;
use crate::state::{AppState, to_display, to_stored};
use crate::views::icons;
use egui::{TextEdit, Ui};
use locus_doc::{Command, EntityId, Recipe, RecipeId, RecipeKind};
use std::collections::BTreeMap;

/// How many suppress checkboxes one pattern shows (the rest stay usable via
/// the count).
const SHOWN_COPIES: usize = 48;

/// Transient state (never part of the document, SPEC §11).
#[derive(Debug, Default)]
pub struct RecipesView {
    drafts: BTreeMap<(RecipeId, u8), String>,
}

/// The editable expression boxes of a pattern: (label, current text).
fn fields(kind: &RecipeKind) -> Vec<(&'static str, String)> {
    match kind {
        RecipeKind::Linear { dir1, dir2 } => {
            let mut v = vec![
                ("count 1", dir1.count.clone()),
                ("spacing 1", dir1.spacing.clone()),
            ];
            if let Some(d) = dir2 {
                v.push(("count 2", d.count.clone()));
                v.push(("spacing 2", d.spacing.clone()));
            }
            v
        }
        RecipeKind::Circular { count, angle, .. } => {
            vec![("count", count.clone()), ("angle °", angle.clone())]
        }
        _ => Vec::new(),
    }
}

/// `kind` with expression box `idx` (as numbered by [`fields`]) set to `text`.
pub(crate) fn with_field(kind: &RecipeKind, idx: usize, text: String) -> RecipeKind {
    let mut k = kind.clone();
    match &mut k {
        RecipeKind::Linear { dir1, dir2 } => match idx {
            0 => dir1.count = text,
            1 => dir1.spacing = text,
            2 => {
                if let Some(d) = dir2 {
                    d.count = text;
                }
            }
            _ => {
                if let Some(d) = dir2 {
                    d.spacing = text;
                }
            }
        },
        RecipeKind::Circular { count, angle, .. } => {
            if idx == 0 {
                *count = text;
            } else {
                *angle = text;
            }
        }
        _ => {}
    }
    k
}

/// A row's short summary, right-aligned as in the mockup ("2×2", "−3 mm").
fn detail(r: &Recipe, units: Units) -> String {
    match &r.kind {
        RecipeKind::Linear { dir1, dir2 } => match dir2 {
            Some(d) => format!("{}×{}", dir1.count.trim(), d.count.trim()),
            None => format!("{}×", dir1.count.trim()),
        },
        RecipeKind::Circular { count, .. } => format!("{}×", count.trim()),
        RecipeKind::Offset { distance, expr, .. } => match expr {
            Some(e) => e.clone(),
            None => units.format_len(*distance),
        },
        RecipeKind::Mirror { .. } => format!("{} copied", r.sources.len()),
        RecipeKind::Generated { .. } | RecipeKind::Modified { .. } => "plugin".to_owned(),
    }
}

/// Everything a recipe is made of and makes: its sources and its copies.
fn geometry(r: &Recipe) -> Vec<EntityId> {
    r.sources.iter().copied().chain(r.instances()).collect()
}

impl RecipesView {
    /// Draw the rows. `ui` is the Recipes section of the Browser.
    pub fn ui(&mut self, ui: &mut Ui, app: &mut AppState, units: Units) {
        let mut cmds: Vec<Command> = Vec::new();
        let mut select: Option<Vec<EntityId>> = None;
        let mut marking: Option<egui::Pos2> = None;
        {
            let doc = app.doc.sketcher.document();
            let st = &mut *self;
            if doc.recipes().is_empty() {
                ui.weak("Patterns, mirrors, offsets and plugin objects appear here.");
            }
            // Numbered per kind, as Fusion names browser features.
            let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
            for (rid, r) in doc.recipes() {
                let n = seen.entry(r.kind.name()).or_default();
                *n += 1;
                let id = ui.make_persistent_id(("browser-recipe", *rid));
                egui::collapsing_header::CollapsingState::load_with_default_open(
                    ui.ctx(),
                    id,
                    false,
                )
                .show_header(ui, |ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    let (slot, _) =
                        ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
                    let c = ui.visuals().text_color();
                    icons::paint(ui.painter(), slot, icons::recipe(&r.kind), "", c);
                    let row = ui
                        .selectable_label(false, format!("{} {n}", r.kind.name()))
                        .on_hover_text(
                            "Click to select its geometry; right-click for the marking menu",
                        );
                    if row.clicked() {
                        select = Some(geometry(r));
                    }
                    if row.secondary_clicked() {
                        select = Some(geometry(r));
                        marking = row.interact_pointer_pos();
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.weak(detail(r, units));
                    });
                })
                .body(|ui| st.editor(ui, *rid, r, units, &mut cmds));
            }
        }
        if let Some(ids) = select {
            app.selection.items.set_all(ids);
        }
        if let Some(at) = marking {
            let now = ui.input(|i| i.time);
            app.view.marking = Some(crate::state::Marking {
                at,
                held: false,
                since: now,
            });
        }
        for cmd in &cmds {
            app.apply_reporting(cmd);
        }
    }

    /// One recipe's settings: a pattern's expressions and copies, then
    /// Explode and Remove for every kind.
    fn editor(
        &mut self,
        ui: &mut Ui,
        rid: RecipeId,
        r: &Recipe,
        units: Units,
        cmds: &mut Vec<Command>,
    ) {
        for (i, (label, current)) in fields(&r.kind).into_iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(label);
                let is_len = label.starts_with("spacing");
                let current = if is_len {
                    to_display(&current, units)
                } else {
                    current
                };
                let key = (rid, u8::try_from(i).unwrap_or(0));
                let mut t = self
                    .drafts
                    .get(&key)
                    .cloned()
                    .unwrap_or_else(|| current.clone());
                let resp = ui.add(TextEdit::singleline(&mut t).desired_width(110.0));
                if resp.has_focus() || resp.changed() {
                    self.drafts.insert(key, t.clone());
                }
                if resp.lost_focus() {
                    self.drafts.remove(&key);
                    if t != current {
                        cmds.push(Command::EditRecipe {
                            id: rid,
                            kind: with_field(
                                &r.kind,
                                i,
                                if is_len { to_stored(&t, units) } else { t },
                            ),
                        });
                    }
                }
            });
        }
        if let RecipeKind::Circular {
            symmetric, rotate, ..
        } = &r.kind
        {
            let (mut s, mut ro) = (*symmetric, *rotate);
            let a = ui.checkbox(&mut s, "symmetric").changed();
            let b = ui.checkbox(&mut ro, "rotate copies").changed();
            if a || b {
                let mut k = r.kind.clone();
                if let RecipeKind::Circular {
                    symmetric, rotate, ..
                } = &mut k
                {
                    *symmetric = s;
                    *rotate = ro;
                }
                cmds.push(Command::EditRecipe { id: rid, kind: k });
            }
        }
        if r.kind.is_pattern() {
            ui.label(format!("{} copies (untick to suppress)", r.active));
            ui.horizontal_wrapped(|ui| {
                for k in 0..r.active.min(SHOWN_COPIES) {
                    let mut on = !r.suppressed.contains(&k);
                    if ui.checkbox(&mut on, (k + 1).to_string()).changed() {
                        cmds.push(Command::SetInstanceSuppressed {
                            id: rid,
                            index: k,
                            suppressed: !on,
                        });
                    }
                }
            });
        }
        ui.horizontal(|ui| {
            if ui
                .button("Explode")
                .on_hover_text("Turn the copies into ordinary geometry")
                .clicked()
            {
                cmds.push(Command::ExplodeRecipe { id: rid });
            }
            if ui
                .button("Remove")
                .on_hover_text("Delete the copies; the sources stay")
                .clicked()
            {
                cmds.push(Command::RemoveRecipe { id: rid });
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use locus_doc::{Axis, LinearDir};

    fn recipe(kind: RecipeKind) -> Recipe {
        Recipe {
            kind,
            sources: Vec::new(),
            pool: Vec::new(),
            active: 0,
            suppressed: Default::default(),
            constraints: Vec::new(),
        }
    }

    fn dir(count: &str) -> LinearDir {
        LinearDir {
            axis: Axis::X,
            count: count.to_owned(),
            spacing: "10".to_owned(),
            symmetric: false,
        }
    }

    /// The mockup's right-hand summaries: "2×2", "−3 mm".
    #[test]
    fn rows_summarise_their_recipe() {
        let grid = recipe(RecipeKind::Linear {
            dir1: dir("2"),
            dir2: Some(dir(" 2 ")),
        });
        assert_eq!(detail(&grid, Units::Mm), "2×2");
        let offset = recipe(RecipeKind::Offset {
            distance: -3.0,
            sharp: false,
            expr: None,
        });
        assert_eq!(detail(&offset, Units::Mm), Units::Mm.format_len(-3.0));
        let named = recipe(RecipeKind::Offset {
            distance: -3.0,
            sharp: false,
            expr: Some("-kerf".to_owned()),
        });
        assert_eq!(detail(&named, Units::Mm), "-kerf");
    }
}
