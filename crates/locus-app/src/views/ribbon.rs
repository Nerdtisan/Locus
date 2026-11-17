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
use crate::toolbar::{RibbonTab, SEPARATOR_BEFORE, Toolbar, also_in};
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
    let tk = crate::theme::tokens(ui);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        for t in RibbonTab::ALL {
            let on = tab == t;
            // The mockup's tabs: muted capitals, the open one on the panel
            // in the accent colour.
            let text = egui::RichText::new(t.title())
                .size(11.0)
                .strong()
                .color(if on { tk.accent } else { tk.muted });
            let b = egui::Button::new(text)
                .fill(if on {
                    tk.panel
                } else {
                    egui::Color32::TRANSPARENT
                })
                .stroke(if on {
                    egui::Stroke::new(1.0, tk.line)
                } else {
                    egui::Stroke::NONE
                })
                .corner_radius(egui::CornerRadius {
                    nw: 6,
                    ne: 6,
                    sw: 0,
                    se: 0,
                })
                .min_size(egui::vec2(0.0, 22.0));
            if ui.add(b).clicked() {
                tab = t;
            }
        }
    });
    ui.ctx().data_mut(|d| d.insert_temp(id, tab));
    tab
}

/// EXPORT at the right end of SKETCH (design §2), as Fusion's FINISH SKETCH:
/// one button, its icon then its label (#359: an icon button beside a
/// text button read as two Export buttons).
fn export_end(ui: &mut egui::Ui, reg: &Registry, cx: &mut Cx<'_>) {
    let Some(a) = reg.get("output.export") else {
        return;
    };
    let tip = tooltip_plain(ui, reg, cx, a);
    // The mockup's finish group (#360): the icon above "EXPORT", as one
    // button (#359).
    let (rect, r) = ui.allocate_exact_size(egui::vec2(56.0, 50.0), egui::Sense::click());
    r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "EXPORT"));
    let tk = crate::theme::tokens(ui);
    if r.hovered() {
        ui.painter().rect_filled(rect, 5.0, tk.accent_soft);
    }
    let color = icon_color(ui, &r, true);
    let icon = egui::Rect::from_center_size(
        egui::pos2(rect.center().x, rect.top() + 18.0),
        egui::vec2(20.0, 20.0),
    );
    icons::paint(ui.painter(), icon, a.id.as_str(), &a.label, color);
    ui.painter().text(
        egui::pos2(rect.center().x, rect.bottom() - 8.0),
        egui::Align2::CENTER_CENTER,
        "EXPORT",
        egui::FontId::proportional(10.0),
        if r.hovered() { tk.accent } else { tk.muted },
    );
    let r = r.on_hover_text(tip);
    if r.clicked() {
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
        let step = button_width(g) + 1.0;
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
    // The mockup's group (#360): its buttons in a row, its label centred
    // under them. The group is as wide as the wider of the two, so the
    // label never wraps (#387: one button under "AUTOMATION").
    let shown = pins
        .iter()
        .filter(|id| reg.get(id).is_some())
        .count()
        .max(1);
    let buttons_w = shown as f32 * (button_width(g) + 1.0) - 1.0;
    let label_w = label_width(ui, g);
    let width = buttons_w.max(label_w);
    let inner = ui.vertical(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(1.0, 0.0);
        let row = ui.horizontal(|ui| {
            ui.add_space((width - buttons_w) / 2.0);
            for (i, id) in pins.iter().enumerate() {
                // A pinned plugin action whose plugin isn't loaded stays pinned
                // but hidden (design §7.1).
                let Some(a) = reg.get(id) else { continue };
                // Stable ids: auto ids would shift as pins come and go, and a
                // menu's open state would jump to another widget.
                let r = ui
                    .push_id(("ribbon-pin", id.as_str()), |ui| {
                        pin_button(ui, reg, cx, a, button_width(g))
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
            if pins.iter().all(|id| reg.get(id).is_none()) {
                // An empty group keeps its place (and takes drops).
                ui.add_space(button_width(g));
            }
        });
        // "GROUP ▾" under the buttons: small, muted, spaced capitals. The
        // fonts have no ▾, so the triangle is painted.
        let muted = crate::theme::tokens(ui).muted;
        let label = egui::RichText::new(label_text(g))
            .size(LABEL_SIZE)
            .strong()
            .color(muted);
        let width = row.response.rect.width().max(width);
        let m = ui
            .allocate_ui_with_layout(
                egui::vec2(width, 16.0),
                egui::Layout::top_down(egui::Align::Center),
                |ui| {
                    // Never wrap, whatever the measured width (#387).
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                    ui.push_id(("ribbon-menu", g.title()), |ui| {
                        ui.style_mut().visuals.widgets.inactive.weak_bg_fill =
                            egui::Color32::TRANSPARENT;
                        ui.menu_button(label, |ui| {
                            menu(ui, reg, cx, bar, g, edits);
                        })
                    })
                    .inner
                },
            )
            .inner;
        let r = m.response.rect;
        let c = muted;
        let (x, y) = (r.right() - 10.0, r.center().y);
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
/// Pinned buttons' width, as the mockup's `.tb` (34 × 32). The constraint
/// symbols are narrower (`.tb.sm`), as in Fusion, so all of them fit
/// (design §2).
fn button_width(g: Group) -> f32 {
    if g == Group::Constrain { 27.0 } else { 34.0 }
}

const BUTTON_HEIGHT: f32 = 32.0;

/// The group label's size (the mockup's `.glabel`, 10 px capitals).
const LABEL_SIZE: f32 = 10.0;

/// "GROUP" with room after it for the painted ▾.
fn label_text(g: Group) -> String {
    format!("{}      ", g.title())
}

/// The width the group's label button needs on one line: the text, unwrapped,
/// plus the button's padding.
fn label_width(ui: &egui::Ui, g: Group) -> f32 {
    let text = ui.fonts_mut(|f| {
        f.layout_no_wrap(
            label_text(g),
            egui::FontId::proportional(LABEL_SIZE),
            egui::Color32::WHITE,
        )
        .size()
        .x
    });
    text + 2.0 * ui.spacing().button_padding.x
}

fn pin_button(
    ui: &mut egui::Ui,
    reg: &Registry,
    cx: &Cx<'_>,
    a: &Action,
    width: f32,
) -> egui::Response {
    let enabled = reg.enabled(a, cx.state);
    let on = reg.is_on(a, cx.state, cx.settings);
    let r = ui.add(
        egui::Button::selectable(on, "")
            .min_size(egui::vec2(width, BUTTON_HEIGHT))
            .sense(Sense::click_and_drag()),
    );
    let color = icon_color(ui, &r, enabled.yes());
    icons::paint(
        ui.painter(),
        egui::Rect::from_center_size(
            r.rect.center(),
            egui::Vec2::splat(if width < 30.0 { 17.0 } else { 20.0 }),
        ),
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
    let rows = menu_rows(reg, g);
    if rows.is_empty() {
        ui.weak("Nothing here yet");
    }
    for row in rows {
        let a = match row {
            MenuRow::Separator => {
                ui.separator();
                continue;
            }
            MenuRow::Header(h) => {
                ui.label(egui::RichText::new(h).size(10.0).strong().weak());
                continue;
            }
            MenuRow::Action(a) => a,
        };
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

/// One row of a group's ▾ menu.
#[derive(Debug)]
enum MenuRow<'a> {
    Action(&'a Action),
    Separator,
    /// A small capitals heading (the plugins' block).
    Header(&'static str),
}

impl PartialEq for MenuRow<'_> {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (MenuRow::Action(a), MenuRow::Action(b)) => a.id == b.id,
            (MenuRow::Separator, MenuRow::Separator) => true,
            (MenuRow::Header(a), MenuRow::Header(b)) => a == b,
            _ => false,
        }
    }
}

/// A group's ▾ menu, as the mockup lays it out (design §2, #386): the
/// group's built-in actions in registry order, with separators between
/// their blocks; its plugin actions under a PLUGINS heading (before Delete
/// in MODIFY, last elsewhere); then what it lists from other groups
/// (INSPECT's Validate for export).
fn menu_rows(reg: &Registry, g: Group) -> Vec<MenuRow<'_>> {
    let own = reg.actions().iter().filter(|a| a.group == g);
    let (plugins, builtins): (Vec<&Action>, Vec<&Action>) =
        own.partition(|a| a.source != Source::Builtin);
    fn plugin_block<'a>(rows: &mut Vec<MenuRow<'a>>, plugins: &[&'a Action]) {
        if !plugins.is_empty() {
            if !rows.is_empty() {
                rows.push(MenuRow::Separator);
            }
            rows.push(MenuRow::Header("PLUGINS"));
            rows.extend(plugins.iter().map(|a| MenuRow::Action(a)));
        }
    }
    let mut rows = Vec::new();
    let mut plugins_shown = false;
    for a in builtins {
        let id = a.id.as_str();
        if id == "edit.delete" && !plugins_shown {
            plugin_block(&mut rows, &plugins);
            plugins_shown = true;
        }
        if !rows.is_empty() && SEPARATOR_BEFORE.contains(&id) {
            rows.push(MenuRow::Separator);
        }
        rows.push(MenuRow::Action(a));
    }
    if !plugins_shown {
        plugin_block(&mut rows, &plugins);
    }
    rows.extend(
        also_in(g)
            .iter()
            .filter_map(|id| reg.get(id))
            .map(MenuRow::Action),
    );
    rows
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

    /// #359: the SKETCH tab ends in exactly one Export button.
    #[test]
    #[allow(clippy::expect_used)]
    fn sketch_ends_in_one_export_button() {
        use egui::accesskit::Role;
        let reg = Registry::default();
        let mut state = crate::state::AppState::default();
        let mut settings = crate::settings::Settings::default();
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut run = || {
            ctx.run_ui(egui::RawInput::default(), |ui| {
                let mut cx = Cx {
                    state: &mut state,
                    settings: &mut settings,
                    plugins: None,
                };
                export_end(ui, &reg, &mut cx);
            })
        };
        let _ = run();
        let out = run();
        let tree = out
            .platform_output
            .accesskit_update
            .expect("accesskit tree");
        let buttons: Vec<_> = tree
            .nodes
            .iter()
            .filter(|(_, n)| n.role() == Role::Button)
            .collect();
        assert_eq!(buttons.len(), 1, "one button, icon and label together");
        assert!(buttons[0].1.label().is_some_and(|l| l.contains("EXPORT")));
    }

    /// #386: every SKETCH group's ▾ menu lists the mockup's tools in the
    /// mockup's order (design §2). Window select and Selection filters are
    /// follow-ups; Smooth (G2) is hidden until the spec has it.
    #[test]
    fn group_menus_match_the_mockup() {
        let reg = Registry::default();
        let ids = |g| {
            menu_rows(&reg, g)
                .into_iter()
                .filter_map(|r| match r {
                    MenuRow::Action(a) => Some(a.id.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(
            ids(Group::Create),
            [
                "sketch.line",
                "sketch.rect_2pt",
                "sketch.rect_centre",
                "sketch.rect_3pt",
                "sketch.circle_centre",
                "sketch.circle_2pt",
                "sketch.circle_3pt",
                "sketch.arc_3pt",
                "sketch.arc_centre",
                "sketch.arc_tangent",
                "sketch.polygon",
                "sketch.polygon_edge",
                "sketch.slot",
                "sketch.pen",
                "sketch.point",
                "sketch.text",
                "sketch.mirror",
                "sketch.pattern_circ",
                "sketch.pattern_rect",
                "modify.offset",
            ]
        );
        assert_eq!(
            ids(Group::Modify),
            [
                "modify.fillet",
                "modify.chamfer",
                "modify.trim",
                "modify.extend",
                "modify.break",
                "modify.move",
                "modify.rotate",
                "modify.scale",
                "modify.construction",
                "modify.explode",
                "modify.style",
                "edit.delete",
            ]
        );
        assert_eq!(
            ids(Group::Constrain),
            [
                "constrain.dimension",
                "constrain.coincident",
                "constrain.collinear",
                "constrain.concentric",
                "constrain.midpoint",
                "constrain.fix",
                "constrain.parallel",
                "constrain.perpendicular",
                "constrain.horizontal",
                "constrain.vertical",
                "constrain.tangent",
                "constrain.equal",
                "constrain.symmetric",
                "constrain.auto",
            ]
        );
        assert_eq!(
            ids(Group::Inspect),
            [
                "inspect.measure",
                "view.machine_preview",
                "view.stroke_width",
                "output.validate",
            ]
        );
        assert_eq!(ids(Group::Insert), ["insert.svg", "insert.dxf"]);
        assert_eq!(
            ids(Group::Select),
            [
                "select.select",
                "select.chain",
                "select.layer",
                "edit.select_all",
                "edit.deselect",
            ]
        );
        // The mockup's names (Fusion's).
        let label = |id| reg.get(id).map(|a| a.label.to_string());
        assert_eq!(label("inspect.measure").as_deref(), Some("Measure"));
        assert_eq!(
            label("sketch.circle_centre").as_deref(),
            Some("Center Diameter Circle")
        );
        assert_eq!(
            label("constrain.dimension").as_deref(),
            Some("Sketch Dimension")
        );
        assert_eq!(
            reg.get("inspect.measure")
                .and_then(|a| reg.shortcut(a))
                .map(|k| k.logical_key),
            Some(egui::Key::I)
        );
    }

    /// The mockup's separators, and a plugin's actions under PLUGINS before
    /// Delete in MODIFY.
    #[test]
    fn menus_have_the_mockups_separators_and_plugin_block() {
        let mut reg = Registry::default();
        reg.sync_plugins(vec![crate::actions::PluginAction {
            plugin: "dogbone".into(),
            name: "Dogbone".into(),
            kind: crate::actions::PluginKind::Modifier,
        }]);
        let rows = menu_rows(&reg, Group::Modify);
        let at = |id: &str| {
            rows.iter()
                .position(|r| matches!(r, MenuRow::Action(a) if a.id.as_str() == id))
        };
        let (Some(plugin), Some(delete), Some(mv)) = (
            at("plugin.dogbone.apply"),
            at("edit.delete"),
            at("modify.move"),
        ) else {
            panic!("rows missing: {rows:?}");
        };
        assert_eq!(rows[plugin - 1], MenuRow::Header("PLUGINS"));
        assert!(plugin < delete);
        assert_eq!(rows[delete - 1], MenuRow::Separator);
        assert_eq!(rows[mv - 1], MenuRow::Separator);
        assert!(!matches!(rows[0], MenuRow::Separator));
    }

    /// #387: no group label wraps, on any tab, however few buttons it has
    /// (UTILITIES ▸ AUTOMATION has one under a long word).
    #[test]
    #[allow(clippy::expect_used)]
    fn group_labels_never_wrap() {
        use egui::accesskit::Role;
        let reg = Registry::default();
        let mut state = crate::state::AppState::default();
        let mut settings = crate::settings::Settings::default();
        let mut bar = Toolbar::default();
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx);
        ctx.enable_accesskit();
        for tab in RibbonTab::ALL {
            ctx.data_mut(|d| d.insert_temp(egui::Id::new("locus-ribbon-tab"), tab));
            let mut run = || {
                ctx.run_ui(egui::RawInput::default(), |ui| {
                    let mut cx = Cx {
                        state: &mut state,
                        settings: &mut settings,
                        plugins: None,
                    };
                    show(ui, &reg, &mut cx, &mut bar);
                })
            };
            let _ = run();
            let tree = run()
                .platform_output
                .accesskit_update
                .expect("accesskit tree");
            for &g in tab.groups() {
                let (_, node) = tree
                    .nodes
                    .iter()
                    .filter(|(_, n)| n.role() == Role::Button)
                    .find(|(_, n)| n.label().is_some_and(|l| l.starts_with(g.title())))
                    .unwrap_or_else(|| panic!("{} label", g.title()));
                let b = node.bounds().expect("bounds");
                assert!(
                    b.height() < 20.0,
                    "{} wraps: {:.0} px tall",
                    g.title(),
                    b.height()
                );
            }
        }
    }

    #[test]
    fn group_titles_read_as_words() {
        assert_eq!(title_case("CONSTRAIN"), "Constrain");
        assert_eq!(title_case(""), "");
    }
}
