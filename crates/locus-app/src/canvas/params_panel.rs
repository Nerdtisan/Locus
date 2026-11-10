// SPDX-License-Identifier: AGPL-3.0-or-later
//! The Parameters panel (#171; SPEC §5): named values usable in dimensions.
//!
//! Every action is a `Sketcher` command, so each is one undo step. Text is
//! held in a draft while a box has focus and committed when focus leaves it,
//! so typing is not one undo step per keystroke.

use super::Canvas;
use egui::{TextEdit, Ui};
use locus_doc::Command;
use std::collections::BTreeMap;

/// Transient panel state (never part of the document, SPEC §11).
#[derive(Debug, Default)]
pub(super) struct ParamUi {
    new_name: String,
    new_expr: String,
    /// Text being edited: (parameter, 0 = name / 1 = expression) → draft.
    drafts: BTreeMap<(String, u8), String>,
}

fn trimmed(v: f64) -> String {
    let s = format!("{v:.4}");
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}

impl Canvas {
    pub fn parameters_ui(&mut self, ui: &mut Ui) {
        let mut cmds: Vec<Command> = Vec::new();
        {
            let doc = self.sketcher.document();
            let st = &mut self.param_ui;
            ui.label("Name a value once, use it in any dimension: material * 2 + 0.2");
            ui.separator();
            if doc.parameters().is_empty() {
                ui.weak("No parameters yet.");
            }
            for (name, p) in doc.parameters() {
                ui.horizontal(|ui| {
                    let mut field = |ui: &mut Ui, which: u8, current: &str, width: f32| {
                        let key = (name.clone(), which);
                        let mut text = st
                            .drafts
                            .get(&key)
                            .cloned()
                            .unwrap_or_else(|| current.to_owned());
                        let r = ui.add(TextEdit::singleline(&mut text).desired_width(width));
                        if r.has_focus() || r.changed() {
                            st.drafts.insert(key.clone(), text.clone());
                        }
                        if r.lost_focus() {
                            st.drafts.remove(&key);
                            if text != current {
                                return Some(text);
                            }
                        }
                        None
                    };
                    if let Some(to) = field(ui, 0, name, 90.0) {
                        cmds.push(Command::RenameParameter {
                            from: name.clone(),
                            to,
                        });
                    }
                    ui.label("=");
                    if let Some(expr) = field(ui, 1, &p.expr, 140.0) {
                        cmds.push(Command::SetParameter {
                            name: name.clone(),
                            expr,
                        });
                    }
                    ui.weak(format!("→ {}", trimmed(p.value)));
                    if ui
                        .small_button("✖")
                        .on_hover_text("Delete (refused while in use)")
                        .clicked()
                    {
                        cmds.push(Command::RemoveParameter { name: name.clone() });
                    }
                });
            }
            ui.separator();
            ui.horizontal(|ui| {
                ui.add(
                    TextEdit::singleline(&mut st.new_name)
                        .hint_text("name")
                        .desired_width(90.0),
                );
                ui.label("=");
                let r = ui.add(
                    TextEdit::singleline(&mut st.new_expr)
                        .hint_text("expression")
                        .desired_width(140.0),
                );
                let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                let ready = !st.new_name.trim().is_empty() && !st.new_expr.trim().is_empty();
                if (ui.add_enabled(ready, egui::Button::new("+ Add")).clicked() || enter) && ready {
                    cmds.push(Command::SetParameter {
                        name: st.new_name.trim().to_owned(),
                        expr: st.new_expr.trim().to_owned(),
                    });
                    st.new_name.clear();
                    st.new_expr.clear();
                }
            });
        }
        for cmd in &cmds {
            self.apply_reporting(cmd);
        }
    }
}
