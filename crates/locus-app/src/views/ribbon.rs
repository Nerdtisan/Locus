// SPDX-License-Identifier: AGPL-3.0-or-later
//! The ribbon: SKETCH, OUTPUT and UTILITIES tabs, with pinnable tools
//! (#299, #325, M5 design §2, §2.3).
//!
//! The tab strip picks which groups show; SKETCH ends with EXPORT at the
//! right, as Fusion's sketch ends with FINISH SKETCH. Each group shows its pinned actions as symbol buttons, then a `GROUP ▾`
//! menu listing every action in the group. Pin by dragging a menu row (or
//! its pin icon) onto the toolbar, or by clicking the pin icon; reorder by
//! dragging a button sideways, into any group; unpin by dragging a button
//! off the toolbar or right-click ▸ Unpin. The pin icon is a plain button,
//! so pinning works without dragging (keyboard, touch). Pins are UI
//! settings and never touch the document.

use crate::actions::{Action, Cx, Enabled, Group, Registry, Source};
use crate::toolbar::{RibbonTab, Toolbar, also_in};
use crate::views::icons;
use egui::{DragAndDrop, Sense};

/// What a drag carries: the action, and whether it came off the toolbar
/// (only those unpin when dropped elsewhere).
#[derive(Clone, Debug)]
struct Dragged {
    id: String,
    from_bar: bool,
}

/// A change to the pins, applied after the ribbon is drawn.
enum Edit {
    Pin(String, Group, usize),
    Unpin(String, RibbonTab),
    Reset(Group),
}

/// Draw the ribbon. Returns true when the pins changed, so the shell can
/// store them.
pub fn show(ui: &mut egui::Ui, reg: &Registry, cx: &mut Cx<'_>, bar: &mut Toolbar) -> bool {
    let ctx = ui.ctx().clone();
    let tab = tabs(ui);
    let dragging = DragAndDrop::payload::<Dragged>(&ctx);
    let mut edits = Vec::new();
    let finish = tab == RibbonTab::Sketch;
    let finish_w = if finish { 96.0 } else { 0.0 };
    // Wraps between groups on a narrow window: a group that wouldn't fit
    // in what is left of the row starts a new one, so its ▾ stays visible.
    let rect = ui
        .horizontal(|ui| {
            let w = (ui.available_width() - finish_w).max(0.0);
            let groups = ui
                .allocate_ui_with_layout(
                    egui::vec2(w, 0.0),
                    egui::Layout::left_to_right(egui::Align::Center).with_main_wrap(true),
                    |ui| row(ui, reg, cx, bar, tab, dragging.as_deref(), &mut edits),
                )
                .response
                .rect;
            if finish {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    export_end(ui, reg, cx);
                });
            }
            groups
        })
        .inner;
    finish_drag(ui, &ctx, reg, dragging.as_deref(), rect, tab, &mut edits);
    let changed = !edits.is_empty();
    for e in edits {
        match e {
            Edit::Pin(id, g, i) => bar.pin(&id, g, i),
            Edit::Unpin(id, tab) => bar.unpin(&id, tab),
            Edit::Reset(g) => bar.reset(g),
        }
    }
    changed
}

/// The tab strip; returns the tab showing. Which tab is open is view state
/// for this session, kept in egui's memory.
fn tabs(ui: &mut egui::Ui) -> RibbonTab {
    let id = egui::Id::new("locus-ribbon-tab");
    let mut tab: RibbonTab = ui.ctx().data(|d| d.get_temp(id)).unwrap_or_default();
    ui.horizontal(|ui| {
        for t in RibbonTab::ALL {
            let text = egui::RichText::new(t.title()).small().strong();
            if ui.selectable_label(tab == t, text).clicked() {
                tab = t;
            }
        }
    });
    ui.ctx().data_mut(|d| d.insert_temp(id, tab));
    tab
}

/// EXPORT at the right end of SKETCH (design §2), as Fusion's FINISH SKETCH.
fn export_end(ui: &mut egui::Ui, reg: &Registry, cx: &mut Cx<'_>) {
    let Some(a) = reg.get("output.export") else {
        return;
    };
    let tip = tooltip_plain(ui, reg, cx, a);
    let label = ui.button("EXPORT").on_hover_text(&tip);
    let icon = ui.add(egui::Button::new("").min_size(egui::vec2(26.0, 26.0)));
    let color = icon_color(ui, &icon, true);
    icons::paint(
        ui.painter(),
        icon.rect.shrink(5.0),
        a.id.as_str(),
        &a.label,
        color,
    );
    if label.clicked() || icon.on_hover_text(tip).clicked() {
        run_or_explain(reg, cx, a);
    }
}

/// The groups of `tab`, separated.
fn row(
    ui: &mut egui::Ui,
    reg: &Registry,
    cx: &mut Cx<'_>,
    bar: &Toolbar,
    tab: RibbonTab,
    dragging: Option<&Dragged>,
    edits: &mut Vec<Edit>,
) {
    for (n, &g) in tab.groups().iter().enumerate() {
        let step = button_size(g) + ui.spacing().item_spacing.x;
        let shown = bar
            .pinned(g)
            .iter()
            .filter(|id| reg.get(id).is_some())
            .count();
        let need = shown as f32 * step + 110.0;
        if n > 0 && need > ui.available_size_before_wrap().x {
            ui.end_row();
        } else if n > 0 {
            ui.separator();
        }
        group(ui, reg, cx, bar, g, dragging, edits);
    }
}

/// The end of a drag: unpin a button dropped off the toolbar, and draw what
/// is being dragged under the pointer.
fn finish_drag(
    ui: &egui::Ui,
    ctx: &egui::Context,
    reg: &Registry,
    dragging: Option<&Dragged>,
    rect: egui::Rect,
    tab: RibbonTab,
    edits: &mut Vec<Edit>,
) {
    if let Some(d) = dragging {
        let pointer = ctx.input(|i| i.pointer.latest_pos());
        let released = ctx.input(|i| i.pointer.any_released());
        if released {
            // A row dragged out of a group menu: the drop ends the menu too.
            egui::Popup::close_all(ctx);
        }
        // Dropped off the toolbar: a pinned button unpins (design §2.3).
        if released && d.from_bar && edits.is_empty() && !pointer.is_some_and(|p| rect.contains(p))
        {
            edits.push(Edit::Unpin(d.id.clone(), tab));
        }
        if let (Some(p), Some(a)) = (pointer, reg.get(&d.id)) {
            ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
            let painter = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Tooltip,
                egui::Id::new("ribbon-drag"),
            ));
            let at = p + egui::vec2(14.0, 10.0);
            let color = ui.visuals().strong_text_color();
            icons::paint(
                &painter,
                egui::Rect::from_min_size(at, egui::vec2(16.0, 16.0)),
                a.id.as_str(),
                &a.label,
                color,
            );
            painter.text(
                at + egui::vec2(20.0, 0.0),
                egui::Align2::LEFT_TOP,
                a.label.as_ref(),
                egui::FontId::proportional(14.0),
                ui.visuals().strong_text_color(),
            );
        }
    }
}

fn group(
    ui: &mut egui::Ui,
    reg: &Registry,
    cx: &mut Cx<'_>,
    bar: &Toolbar,
    g: Group,
    dragging: Option<&Dragged>,
    edits: &mut Vec<Edit>,
) {
    let pins = bar.pinned(g).to_vec();
    let mut target = None;
    let inner = ui.horizontal(|ui| {
        for (i, id) in pins.iter().enumerate() {
            // A pinned plugin action whose plugin isn't loaded stays pinned
            // but hidden (design §7.1).
            let Some(a) = reg.get(id) else { continue };
            // Stable ids: auto ids would shift as pins come and go, and a
            // menu's open state would jump to another widget.
            let r = ui
                .push_id(("ribbon-pin", id.as_str()), |ui| {
                    pin_button(ui, reg, cx, a, button_size(g))
                })
                .inner;
            r.dnd_set_drag_payload(Dragged {
                id: id.clone(),
                from_bar: true,
            });
            if dragging.is_some()
                && let Some(p) = r.hover_pos().or_else(|| pointer_in(ui, r.rect))
            {
                let before = p.x < r.rect.center().x;
                target = Some(if before { i } else { i + 1 });
                let x = if before {
                    r.rect.left()
                } else {
                    r.rect.right()
                } + if before { -2.0 } else { 2.0 };
                insertion_line(ui, x, r.rect);
            }
            if r.clicked() {
                run_or_explain(reg, cx, a);
            }
            r.context_menu(|ui| {
                if ui.button("Unpin").clicked() {
                    edits.push(Edit::Unpin(id.clone(), RibbonTab::of(g)));
                    ui.close();
                }
            });
        }
        // "GROUP ▾": the fonts have no ▾, so the triangle is painted.
        let m = ui
            .push_id(("ribbon-menu", g.title()), |ui| {
                ui.menu_button(format!("{}    ", g.title()), |ui| {
                    menu(ui, reg, cx, bar, g, edits);
                })
            })
            .inner;
        let r = m.response.rect;
        let c = ui.visuals().text_color();
        let (x, y) = (r.right() - 11.0, r.center().y);
        ui.painter().add(egui::Shape::convex_polygon(
            vec![
                egui::pos2(x - 3.5, y - 2.0),
                egui::pos2(x + 3.5, y - 2.0),
                egui::pos2(x, y + 2.5),
            ],
            c,
            egui::Stroke::NONE,
        ));
    });
    let rect = inner.response.rect;
    if let Some(d) = dragging
        && pointer_in(ui, rect).is_some()
    {
        let at = target.unwrap_or(pins.len());
        if target.is_none() {
            insertion_line(ui, rect.left() - 2.0, rect);
        }
        if ui.input(|i| i.pointer.any_released()) {
            edits.push(Edit::Pin(d.id.clone(), g, at));
        }
    }
}

/// Where the pointer is, if inside `rect` (hover state is off while
/// something else is being dragged).
fn pointer_in(ui: &egui::Ui, rect: egui::Rect) -> Option<egui::Pos2> {
    ui.input(|i| i.pointer.latest_pos())
        .filter(|p| rect.contains(*p))
}

fn insertion_line(ui: &egui::Ui, x: f32, rect: egui::Rect) {
    let c = ui.visuals().selection.stroke.color;
    ui.painter()
        .vline(x, rect.y_range(), egui::Stroke::new(2.0, c));
}

/// One pinned symbol button. It is draggable, so it can't be greyed out
/// with `add_enabled`; a disabled action is drawn weak and explains itself
/// when clicked.
/// Pinned buttons' size. The constraint symbols are smaller, as in Fusion
/// and the mockup, so all twelve fit (design §2).
fn button_size(g: Group) -> f32 {
    if g == Group::Constrain { 22.0 } else { 26.0 }
}

fn pin_button(
    ui: &mut egui::Ui,
    reg: &Registry,
    cx: &Cx<'_>,
    a: &Action,
    size: f32,
) -> egui::Response {
    let enabled = reg.enabled(a, cx.state);
    let on = reg.is_on(a, cx.state, cx.settings);
    let r = ui.add(
        egui::Button::selectable(on, "")
            .min_size(egui::vec2(size, size))
            .sense(Sense::click_and_drag()),
    );
    let color = icon_color(ui, &r, enabled.yes());
    icons::paint(
        ui.painter(),
        r.rect.shrink(size * 0.19),
        a.id.as_str(),
        &a.label,
        color,
    );
    r.on_hover_text(tooltip(ui, reg, cx, a, &enabled))
}

fn tooltip(ui: &egui::Ui, reg: &Registry, cx: &Cx<'_>, a: &Action, enabled: &Enabled) -> String {
    let mut t = reg.label(a, cx.state).into_owned();
    if let Some(k) = reg.shortcut(a) {
        t += &format!(" ({})", crate::actions::key_text(ui.ctx(), &k));
    }
    if let Source::Plugin(_) = a.source {
        t += "  · PLUGIN";
    }
    match enabled {
        Enabled::No(why) => t += &format!("\n{why}"),
        Enabled::Yes if !a.hint.is_empty() => t += &format!("\n{}", a.hint),
        Enabled::Yes => {}
    }
    t + "\nDrag to reorder, drag off the toolbar or right-click to unpin"
}

/// A tooltip for a button that isn't a pin: label, key and hint.
fn tooltip_plain(ui: &egui::Ui, reg: &Registry, cx: &Cx<'_>, a: &Action) -> String {
    let mut t = reg.label(a, cx.state).into_owned();
    if let Some(k) = reg.shortcut(a) {
        t += &format!(" ({})", crate::actions::key_text(ui.ctx(), &k));
    }
    if !a.hint.is_empty() {
        t += &format!(
            "
{}",
            a.hint
        );
    }
    t
}

/// Run `a`, or say in the status bar why it can't run now.
fn run_or_explain(reg: &Registry, cx: &mut Cx<'_>, a: &Action) {
    match reg.enabled(a, cx.state) {
        Enabled::Yes => reg.run(a, cx),
        Enabled::No(why) => cx.state.doc.notice = Some(why.into_owned()),
    }
}

fn icon_color(ui: &egui::Ui, r: &egui::Response, enabled: bool) -> egui::Color32 {
    if enabled {
        ui.style().interact(r).fg_stroke.color
    } else {
        ui.visuals().weak_text_color()
    }
}

/// A group's ▾ menu: every action in the group, each with a pin icon, and
/// a reset at the end (design §2.3).
fn menu(
    ui: &mut egui::Ui,
    reg: &Registry,
    cx: &mut Cx<'_>,
    bar: &Toolbar,
    g: Group,
    edits: &mut Vec<Edit>,
) {
    let all: Vec<&Action> = also_in(g)
        .iter()
        .filter_map(|id| reg.get(id))
        .chain(reg.actions().iter().filter(|a| a.group == g))
        .collect();
    if all.is_empty() {
        ui.weak(match g {
            Group::Insert => "Generator plugins add their commands here",
            _ => "Nothing here yet",
        });
    }
    for a in all {
        let id = a.id.as_str();
        let pinned = bar.is_pinned(id, RibbonTab::of(g));
        ui.horizontal(|ui| {
            let pin = ui.add(
                egui::Button::selectable(pinned, "")
                    .min_size(egui::vec2(18.0, 18.0))
                    .sense(Sense::click_and_drag()),
            );
            let pc = icon_color(ui, &pin, true);
            icons::paint(
                ui.painter(),
                pin.rect.shrink(2.0),
                if pinned { "ui.pinned" } else { "ui.unpinned" },
                "",
                pc,
            );
            let pin = pin.on_hover_text(if pinned {
                "Unpin from the toolbar"
            } else {
                "Pin to the toolbar (or drag this row onto it)"
            });
            pin.dnd_set_drag_payload(Dragged {
                id: id.to_owned(),
                from_bar: false,
            });
            if pin.clicked() {
                edits.push(if pinned {
                    Edit::Unpin(id.to_owned(), RibbonTab::of(g))
                } else {
                    Edit::Pin(id.to_owned(), g, usize::MAX)
                });
            }
            let enabled = reg.enabled(a, cx.state);
            let (slot, _) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), Sense::hover());
            let ic = icon_color(ui, &pin, enabled.yes());
            icons::paint(ui.painter(), slot, id, &a.label, ic);
            let mut label = egui::RichText::new(reg.label(a, cx.state));
            if !enabled.yes() {
                label = label.color(ui.visuals().weak_text_color());
            }
            let mut b = egui::Button::selectable(reg.is_on(a, cx.state, cx.settings), label)
                .sense(Sense::click_and_drag());
            if let Some(k) = reg.shortcut(a) {
                b = b.shortcut_text(crate::actions::key_text(ui.ctx(), &k));
            }
            let row = ui.add(b);
            row.dnd_set_drag_payload(Dragged {
                id: id.to_owned(),
                from_bar: false,
            });
            if a.source != Source::Builtin {
                ui.weak("PLUGIN");
            }
            let row = match &enabled {
                Enabled::No(why) => row.on_hover_text(why.as_ref()),
                Enabled::Yes if !a.hint.is_empty() => row.on_hover_text(a.hint.as_ref()),
                Enabled::Yes => row,
            };
            if row.clicked() {
                run_or_explain(reg, cx, a);
                ui.close();
            }
        });
    }
    ui.separator();
    if ui
        .button(format!("Reset {} toolbar", title_case(g.title())))
        .clicked()
    {
        edits.push(Edit::Reset(g));
        ui.close();
    }
}

fn title_case(s: &str) -> String {
    let lower = s.to_lowercase();
    let mut c = lower.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_titles_read_as_words() {
        assert_eq!(title_case("CONSTRAIN"), "Constrain");
        assert_eq!(title_case(""), "");
    }
}
