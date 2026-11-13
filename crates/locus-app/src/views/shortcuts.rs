// SPDX-License-Identifier: AGPL-3.0-or-later
//! The shortcuts pane (#299, M5 design §5, §5.1).
//!
//! A pane, not a dialog, so it can stay open while working; `?` toggles it.
//! Every key row comes from the action registry, so this pane, the menus,
//! the ribbon tooltips and the marking menu can't disagree. Constraints
//! have no letter keys (owner decision); they are listed by symbol.

use crate::actions::{Op, Registry};

/// One row of the map.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub key: String,
    pub action: String,
    /// Where a key changed from M4 Locus, or how the action is reached.
    pub note: &'static str,
    /// For a constraint: the action id whose symbol is drawn as the key.
    pub symbol: Option<String>,
}

/// Keys that changed meaning in M5 (design §5, "Today" column).
const CHANGED: &[(&str, &str)] = &[
    ("sketch.rect_2pt", "was B; R was Center Arc"),
    ("modify.trim", "T was Tangent"),
    ("modify.offset", "O was Concentric"),
    ("modify.move", "M was Midpoint"),
    ("modify.construction", "X was Collinear"),
    ("edit.delete", "new in M5"),
];

/// Input that isn't an action: the tool host and canvas handle it the same
/// way for every tool (design §3, §5).
const FIXED: &[(&str, &str)] = &[
    ("Esc", "Cancel the step; again: back to Select"),
    ("Enter", "Commit"),
    ("Tab", "Next typed field"),
    ("Alt (hold)", "Suppress auto-constraints"),
    ("Space + drag", "Pan"),
    ("Middle-drag", "Pan"),
    ("Wheel", "Zoom about the cursor"),
    ("Right-click", "Marking menu"),
    ("Double-click", "Select chain"),
];

/// The map's rows: every keyed action in registry order, then the
/// constraint symbols, then the fixed input.
pub fn rows(reg: &Registry, fmt: impl Fn(&egui::KeyboardShortcut) -> String) -> Vec<Row> {
    let note = |id: &str| {
        CHANGED
            .iter()
            .find(|(c, _)| *c == id)
            .map_or("", |(_, n)| *n)
    };
    let keyed = reg
        .actions()
        .iter()
        .filter(|a| !a.keys.is_empty())
        .map(|a| Row {
            key: a.keys.iter().map(&fmt).collect::<Vec<_>>().join(" / "),
            action: a.label.to_string(),
            note: note(a.id.as_str()),
            symbol: None,
        });
    let symbols = reg
        .actions()
        .iter()
        .filter(|a| matches!(a.op, Op::Constrain(_)))
        .map(|a| Row {
            key: String::new(),
            action: a.label.to_string(),
            note: "symbol only, no key",
            symbol: Some(a.id.as_str().to_owned()),
        });
    let fixed = FIXED.iter().map(|(k, a)| Row {
        key: (*k).to_owned(),
        action: (*a).to_owned(),
        note: "",
        symbol: None,
    });
    keyed.chain(symbols).chain(fixed).collect()
}

/// Case-insensitive match on key or action name (design §5.1).
fn matches(r: &Row, filter: &str) -> bool {
    let f = filter.trim().to_lowercase();
    f.is_empty() || r.key.to_lowercase().contains(&f) || r.action.to_lowercase().contains(&f)
}

/// The pane's transient widget state.
#[derive(Default)]
pub struct ShortcutsView {
    filter: String,
}

impl ShortcutsView {
    pub fn ui(&mut self, ui: &mut egui::Ui, reg: &Registry) {
        ui.horizontal(|ui| {
            ui.label("Filter");
            ui.add(
                egui::TextEdit::singleline(&mut self.filter)
                    .hint_text("key or action")
                    .desired_width(160.0),
            );
        });
        ui.weak("? shows or hides this pane. Drag its tab to dock it anywhere.");
        ui.separator();
        let ctx = ui.ctx().clone();
        let rows = rows(reg, |k| crate::actions::key_text(&ctx, k));
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("shortcut-map")
                .striped(true)
                .num_columns(3)
                .show(ui, |ui| {
                    for r in rows.iter().filter(|r| matches(r, &self.filter)) {
                        match &r.symbol {
                            Some(id) => {
                                let (rect, _) = ui.allocate_exact_size(
                                    egui::vec2(16.0, 16.0),
                                    egui::Sense::hover(),
                                );
                                let c = ui.visuals().text_color();
                                crate::views::icons::paint(ui.painter(), rect, id, &r.action, c);
                            }
                            None => {
                                ui.monospace(&r.key);
                            }
                        }
                        ui.label(&r.action);
                        ui.weak(r.note);
                        ui.end_row();
                    }
                });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all() -> Vec<Row> {
        rows(&Registry::default(), |k| format!("{:?}", k.logical_key))
    }

    #[test]
    fn every_keyed_action_and_constraint_is_listed() {
        let reg = Registry::default();
        let rows = all();
        for a in reg.actions() {
            if !a.keys.is_empty() || matches!(a.op, Op::Constrain(_)) {
                assert!(
                    rows.iter().any(|r| r.action == a.label),
                    "{}",
                    a.id.as_str()
                );
            }
        }
        assert!(
            rows.iter()
                .any(|r| r.key == "Questionmark" && r.action == "Keyboard shortcuts")
        );
        assert!(rows.iter().any(|r| r.key == "Esc"));
    }

    #[test]
    fn constraints_show_symbols_not_letters() {
        for r in all().iter().filter(|r| r.note == "symbol only, no key") {
            assert!(r.key.is_empty() && r.symbol.is_some(), "{r:?}");
        }
    }

    #[test]
    fn changed_keys_name_a_registered_action() {
        let reg = Registry::default();
        for (id, _) in CHANGED {
            assert!(reg.get(id).is_some(), "{id}");
        }
        let rows = all();
        let trim = rows.iter().find(|r| r.action == "Trim").map(|r| r.note);
        assert_eq!(trim, Some("T was Tangent"));
    }

    #[test]
    fn punctuation_keys_read_as_themselves() {
        use egui::{Key, KeyboardShortcut, Modifiers};
        let ctx = egui::Context::default();
        let k = |m, k| crate::actions::key_text(&ctx, &KeyboardShortcut::new(m, k));
        assert_eq!(k(Modifiers::NONE, Key::Questionmark), "?");
        assert_eq!(k(Modifiers::NONE, Key::L), "L");
        assert_eq!(k(Modifiers::NONE, Key::Delete), "Delete");
        assert!(k(Modifiers::COMMAND, Key::Z).ends_with('Z'));
    }

    #[test]
    fn filter_matches_key_or_name() {
        let rows = all();
        let hits = |f: &str| rows.iter().filter(|r| matches(r, f)).count();
        assert_eq!(hits(""), rows.len());
        assert!(hits("  LINE ") >= 1);
        assert!(hits("esc") >= 1);
        assert_eq!(hits("no such thing"), 0);
    }
}
