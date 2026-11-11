// SPDX-License-Identifier: AGPL-3.0-or-later
//! The Patterns panel (#168, #169; SPEC §6.2.1): make a linear or circular
//! pattern from the selection, edit its count / spacing / angle as
//! expressions over the parameters, suppress single copies, explode or remove
//! it. Every action is one `Sketcher` command, so one undo step. Text is held
//! in a draft while a box has focus and committed when focus leaves it.

use super::Canvas;
use egui::{TextEdit, Ui};
use locus_doc::{Axis, Command, EntityId, LinearDir, RecipeId, RecipeKind};
use std::collections::BTreeMap;

/// How many suppress checkboxes one pattern shows (the rest stay usable via
/// the count).
const SHOWN_COPIES: usize = 48;

/// Transient panel state (never part of the document, SPEC §11).
#[derive(Debug)]
pub(super) struct PatternUi {
    circular: bool,
    second: bool,
    axis1: Axis,
    axis2: Axis,
    count1: String,
    spacing1: String,
    count2: String,
    spacing2: String,
    symmetric: bool,
    centre: Option<EntityId>,
    c_count: String,
    c_angle: String,
    rotate: bool,
    drafts: BTreeMap<(RecipeId, u8), String>,
}

impl Default for PatternUi {
    fn default() -> Self {
        Self {
            circular: false,
            second: false,
            axis1: Axis::X,
            axis2: Axis::Y,
            count1: "3".to_owned(),
            spacing1: "10".to_owned(),
            count2: "2".to_owned(),
            spacing2: "10".to_owned(),
            symmetric: false,
            centre: None,
            c_count: "6".to_owned(),
            c_angle: "360".to_owned(),
            rotate: true,
            drafts: BTreeMap::new(),
        }
    }
}

fn axis_name(a: &Axis) -> &'static str {
    match a {
        Axis::X => "X",
        Axis::Y => "Y",
        Axis::Line(_) => "line",
    }
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
fn with_field(kind: &RecipeKind, idx: usize, text: String) -> RecipeKind {
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

fn dir(axis: &Axis, count: &str, spacing: &str, symmetric: bool) -> LinearDir {
    LinearDir {
        axis: axis.clone(),
        count: count.trim().to_owned(),
        spacing: spacing.trim().to_owned(),
        symmetric,
    }
}

fn axis_combo(ui: &mut Ui, id: &str, axis: &mut Axis) {
    egui::ComboBox::from_id_salt(id)
        .width(50.0)
        .selected_text(axis_name(axis))
        .show_ui(ui, |ui| {
            ui.selectable_value(axis, Axis::X, "X");
            ui.selectable_value(axis, Axis::Y, "Y");
        });
}

fn text(ui: &mut Ui, s: &mut String, w: f32) {
    ui.add(TextEdit::singleline(s).desired_width(w));
}

impl Canvas {
    pub fn patterns_ui(&mut self, ui: &mut Ui) {
        let mut cmds: Vec<Command> = Vec::new();
        let selection: Vec<EntityId> = self.selected();
        {
            let doc = self.sketcher.document();
            let st = &mut self.pattern_ui;
            ui.label(
                "Copies of the selection, updating with it. Counts and spacing take parameters.",
            );
            ui.horizontal(|ui| {
                ui.selectable_value(&mut st.circular, false, "Linear");
                ui.selectable_value(&mut st.circular, true, "Circular");
            });
            if st.circular {
                ui.horizontal(|ui| {
                    ui.label("centre");
                    ui.weak(
                        st.centre
                            .map_or("none".to_owned(), |c| format!("point {c}")),
                    );
                    let one_point = selection.len() == 1
                        && doc
                            .entity(selection[0])
                            .is_some_and(|e| matches!(e.geom, locus_doc::Geom::Point { .. }));
                    if ui
                        .add_enabled(one_point, egui::Button::new("Use selected point"))
                        .clicked()
                    {
                        st.centre = selection.first().copied();
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("count");
                    text(ui, &mut st.c_count, 60.0);
                    ui.label("angle °");
                    text(ui, &mut st.c_angle, 60.0);
                });
                ui.horizontal(|ui| {
                    ui.checkbox(&mut st.symmetric, "symmetric");
                    ui.checkbox(&mut st.rotate, "rotate copies");
                });
            } else {
                ui.horizontal(|ui| {
                    ui.label("dir 1");
                    axis_combo(ui, "pat-axis1", &mut st.axis1);
                    ui.label("count");
                    text(ui, &mut st.count1, 50.0);
                    ui.label("spacing");
                    text(ui, &mut st.spacing1, 60.0);
                });
                ui.checkbox(&mut st.second, "second direction");
                if st.second {
                    ui.horizontal(|ui| {
                        ui.label("dir 2");
                        axis_combo(ui, "pat-axis2", &mut st.axis2);
                        ui.label("count");
                        text(ui, &mut st.count2, 50.0);
                        ui.label("spacing");
                        text(ui, &mut st.spacing2, 60.0);
                    });
                }
                ui.checkbox(&mut st.symmetric, "symmetric (both sides of the source)");
            }
            let ready = !selection.is_empty() && (!st.circular || st.centre.is_some());
            if ui
                .add_enabled(
                    ready,
                    egui::Button::new(format!("Create from selection ({})", selection.len())),
                )
                .clicked()
            {
                let kind = if st.circular {
                    RecipeKind::Circular {
                        centre: st.centre.unwrap_or(selection[0]),
                        count: st.c_count.trim().to_owned(),
                        angle: st.c_angle.trim().to_owned(),
                        symmetric: st.symmetric,
                        rotate: st.rotate,
                    }
                } else {
                    RecipeKind::Linear {
                        dir1: dir(&st.axis1, &st.count1, &st.spacing1, st.symmetric),
                        dir2: st
                            .second
                            .then(|| dir(&st.axis2, &st.count2, &st.spacing2, false)),
                    }
                };
                cmds.push(Command::AddRecipe {
                    kind,
                    sources: selection.clone(),
                });
            }
            ui.separator();
            let mut any = false;
            for (rid, r) in doc.recipes() {
                if !r.kind.is_pattern() {
                    continue;
                }
                any = true;
                ui.collapsing(format!("{} #{rid}", r.kind.name()), |ui| {
                    for (i, (label, current)) in fields(&r.kind).into_iter().enumerate() {
                        ui.horizontal(|ui| {
                            ui.label(label);
                            let key = (*rid, u8::try_from(i).unwrap_or(0));
                            let mut t = st
                                .drafts
                                .get(&key)
                                .cloned()
                                .unwrap_or_else(|| current.clone());
                            let resp = ui.add(TextEdit::singleline(&mut t).desired_width(120.0));
                            if resp.has_focus() || resp.changed() {
                                st.drafts.insert(key, t.clone());
                            }
                            if resp.lost_focus() {
                                st.drafts.remove(&key);
                                if t != current {
                                    cmds.push(Command::EditRecipe {
                                        id: *rid,
                                        kind: with_field(&r.kind, i, t),
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
                            cmds.push(Command::EditRecipe { id: *rid, kind: k });
                        }
                    }
                    ui.label(format!("{} copies (untick to suppress)", r.active));
                    ui.horizontal_wrapped(|ui| {
                        for k in 0..r.active.min(SHOWN_COPIES) {
                            let mut on = !r.suppressed.contains(&k);
                            if ui.checkbox(&mut on, (k + 1).to_string()).changed() {
                                cmds.push(Command::SetInstanceSuppressed {
                                    id: *rid,
                                    index: k,
                                    suppressed: !on,
                                });
                            }
                        }
                    });
                    ui.horizontal(|ui| {
                        if ui.button("Explode").clicked() {
                            cmds.push(Command::ExplodeRecipe { id: *rid });
                        }
                        if ui.button("Remove").clicked() {
                            cmds.push(Command::RemoveRecipe { id: *rid });
                        }
                    });
                });
            }
            if !any {
                ui.weak("No patterns yet.");
            }
        }
        for cmd in &cmds {
            self.apply_reporting(cmd);
        }
    }
}
