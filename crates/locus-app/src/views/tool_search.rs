// SPDX-License-Identifier: AGPL-3.0-or-later
//! Tool search (S), as Fusion's shortcut box (#324, M5 design §6.1).
//!
//! S, or Search in the app bar, opens a box at the pointer. Before anything
//! is typed it lists recent, then pinned, actions. Typing filters every
//! registry action by label, keywords and id, showing each one's group and
//! key; an action that can't run now is dimmed with the reason. Enter runs
//! the highlighted match, ↑↓ move, Esc or a click elsewhere closes it.
//! Running goes through the registry, so it is one command like any other.

use crate::actions::{Action, Cx, Enabled, Registry};
use crate::toolbar::Toolbar;
use crate::views::icons;
use egui::{Key, Pos2};

/// How many matches the box lists.
const SHOWN: usize = 12;
/// How many recently run actions it remembers (this session only).
const RECENT: usize = 8;
/// Its own id, never a result.
const SELF_ID: &str = "view.tool_search";

#[derive(Debug, Default)]
pub struct ToolSearch {
    /// Where the box is open, if it is.
    at: Option<Pos2>,
    query: String,
    sel: usize,
    /// Frames since it opened: focus waits a frame, so the S that opened it
    /// isn't typed into the box.
    frames: u8,
    recent: Vec<String>,
}

/// The actions to list for `query`, best first. Empty: recent, then pinned.
pub fn matches<'a, 'q>(
    reg: &'a Registry,
    query: &str,
    recent: &'q [String],
    pinned: impl Iterator<Item = &'q str>,
) -> Vec<&'a Action> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        let mut out: Vec<&Action> = Vec::new();
        let ids = recent.iter().map(String::as_str).chain(pinned);
        for a in ids.filter_map(|id| reg.get(id)) {
            if a.id.as_str() != SELF_ID && !out.iter().any(|o| o.id == a.id) {
                out.push(a);
            }
        }
        return out;
    }
    let mut scored: Vec<(u8, &Action)> = reg
        .actions()
        .iter()
        .filter(|a| a.id.as_str() != SELF_ID)
        .filter_map(|a| {
            let label = a.label.to_lowercase();
            let score = if label.starts_with(&q) {
                0
            } else if label
                .split(|c: char| !c.is_alphanumeric())
                .any(|w| w.starts_with(&q))
            {
                1
            } else if label.contains(&q) {
                2
            } else if a.keywords.iter().any(|k| k.to_lowercase().contains(&q))
                || a.id.as_str().contains(&q)
            {
                3
            } else {
                return None;
            };
            Some((score, a))
        })
        .collect();
    scored.sort_by(|(s1, a1), (s2, a2)| s1.cmp(s2).then_with(|| a1.label.cmp(&a2.label)));
    scored.into_iter().map(|(_, a)| a).collect()
}

impl ToolSearch {
    /// Open the box at `at` (screen points), empty.
    pub fn open(&mut self, at: Pos2) {
        self.at = Some(at);
        self.query.clear();
        self.sel = 0;
        self.frames = 0;
    }

    fn close(&mut self) {
        self.at = None;
    }

    /// Remember that `id` ran, most recent first.
    fn ran(&mut self, id: &str) {
        self.recent.retain(|r| r != id);
        self.recent.insert(0, id.to_owned());
        self.recent.truncate(RECENT);
    }

    /// Draw the box if open, and run what the user picks.
    pub fn show(&mut self, ctx: &egui::Context, reg: &Registry, cx: &mut Cx<'_>, bar: &Toolbar) {
        let Some(at) = self.at else {
            return;
        };
        let hits = matches(reg, &self.query, &self.recent, bar.all_pins());
        let shown = hits.len().min(SHOWN);
        self.sel = self.sel.min(shown.saturating_sub(1));
        let (down, up, enter, esc) = ctx.input(|i| {
            (
                i.key_pressed(Key::ArrowDown),
                i.key_pressed(Key::ArrowUp),
                i.key_pressed(Key::Enter),
                i.key_pressed(Key::Escape),
            )
        });
        if down && self.sel + 1 < shown {
            self.sel += 1;
        }
        if up {
            self.sel = self.sel.saturating_sub(1);
        }
        let mut run: Option<&Action> = None;
        let area = egui::Area::new(egui::Id::new("locus-tool-search"))
            .order(egui::Order::Foreground)
            .fixed_pos(at)
            .constrain(true)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_width(380.0);
                    let edit = ui.add(
                        egui::TextEdit::singleline(&mut self.query)
                            .hint_text("Search tools and commands")
                            .desired_width(f32::INFINITY),
                    );
                    if self.frames == 1 {
                        edit.request_focus();
                    }
                    if edit.changed() {
                        self.sel = 0;
                    }
                    ui.separator();
                    if self.query.trim().is_empty() {
                        ui.weak(if hits.is_empty() {
                            "Type to search every command"
                        } else {
                            "Recent and pinned"
                        });
                    } else if hits.is_empty() {
                        ui.weak("No command matches");
                    }
                    for (i, a) in hits.iter().take(SHOWN).enumerate() {
                        if row(ui, reg, cx, a, i == self.sel).clicked() {
                            run = Some(a);
                        }
                    }
                });
            });
        if enter && let Some(a) = hits.get(self.sel) {
            run = Some(a);
        }
        self.frames = self.frames.saturating_add(1);
        if let Some(a) = run {
            match reg.enabled(a, cx.state) {
                Enabled::Yes => {
                    self.ran(a.id.as_str());
                    self.close();
                    reg.run(a, cx);
                }
                // Stay open, so the reason is read where it was asked.
                Enabled::No(why) => cx.state.doc.notice = Some(why.into_owned()),
            }
        } else if esc || (self.frames > 2 && area.response.clicked_elsewhere()) {
            self.close();
        }
    }
}

/// One result: icon, label, group, key; dimmed with its reason if it
/// can't run now.
fn row(ui: &mut egui::Ui, reg: &Registry, cx: &Cx<'_>, a: &Action, on: bool) -> egui::Response {
    let enabled = reg.enabled(a, cx.state);
    let weak = ui.visuals().weak_text_color();
    let inner = ui.horizontal(|ui| {
        let (slot, _) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
        let c = if enabled.yes() {
            ui.visuals().text_color()
        } else {
            weak
        };
        icons::paint(ui.painter(), slot, a.id.as_str(), &a.label, c);
        let mut text = egui::RichText::new(reg.label(a, cx.state));
        if !enabled.yes() {
            text = text.color(weak);
        }
        let mut b = egui::Button::selectable(on, text);
        if let Some(k) = reg.shortcut(a) {
            b = b.shortcut_text(crate::actions::key_text(ui.ctx(), &k));
        }
        let r = ui.add(b);
        ui.weak(group_name(a));
        r
    });
    let r = inner.inner;
    match enabled {
        Enabled::No(why) => r.on_hover_text(why.as_ref()),
        Enabled::Yes if !a.hint.is_empty() => r.on_hover_text(a.hint.as_ref()),
        Enabled::Yes => r,
    }
}

fn group_name(a: &Action) -> String {
    let t = a.group.title().to_lowercase();
    let mut c = t.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(v: &[&Action]) -> Vec<String> {
        v.iter().map(|a| a.id.as_str().to_owned()).collect()
    }

    #[test]
    fn typing_ranks_label_prefixes_first() {
        let reg = Registry::default();
        let hits = ids(&matches(&reg, "line", &[], std::iter::empty()));
        assert_eq!(hits.first().map(String::as_str), Some("sketch.line"));
        // Keywords count too: "laser" finds Export.
        let hits = ids(&matches(&reg, "laser", &[], std::iter::empty()));
        assert!(hits.contains(&"output.export".to_owned()));
        // Constraints have no keys but are found by name (design §2).
        let hits = ids(&matches(&reg, "tangent", &[], std::iter::empty()));
        assert!(hits.contains(&"constrain.tangent".to_owned()));
        assert!(matches(&reg, "zzzz", &[], std::iter::empty()).is_empty());
        // Never itself.
        assert!(
            !ids(&matches(&reg, "search", &[], std::iter::empty())).contains(&SELF_ID.to_owned())
        );
    }

    #[test]
    fn empty_lists_recent_then_pinned_once() {
        let reg = Registry::default();
        let recent = vec!["modify.trim".to_owned(), "gone.action".to_owned()];
        let pins = ["sketch.line", "modify.trim", SELF_ID];
        let hits = ids(&matches(&reg, "  ", &recent, pins.into_iter()));
        assert_eq!(hits, ["modify.trim", "sketch.line"]);
    }

    #[test]
    fn recent_keeps_the_latest_first_and_is_bounded() {
        let mut s = ToolSearch::default();
        for i in 0..20 {
            s.ran(&format!("a.{i}"));
        }
        s.ran("a.5");
        assert_eq!(s.recent.len(), RECENT);
        assert_eq!(s.recent[0], "a.5");
    }
}
