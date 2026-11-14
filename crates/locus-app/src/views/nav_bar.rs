// SPDX-License-Identifier: AGPL-3.0-or-later
//! The navigation bar (M5 design §2.4, #328): bottom centre of the canvas,
//! as Fusion's. Pan · Zoom in · Zoom out · Fit · Zoom to selection |
//! Grid and snaps ▾ · Rulers | Display ▾ · Stroke width · Machine preview.
//!
//! Every button is a registry action, and the two ▾ menus list registry
//! toggles, so the bar, the View menu and the sketch palette are views of
//! the same settings and can't disagree (design §7.1).

use crate::actions::{self, Cx, DISPLAY_MENU, GRID_MENU, Item, Registry, Style};
use egui::{Align2, Color32, PopupCloseBehavior, RectAlign, Sense, Stroke, Ui, pos2, vec2};

/// The bar's layout: buttons, `None` for a divider.
const BAR: &[Option<Button>] = &[
    Some(Button::Action("view.pan")),
    Some(Button::Action("view.zoom_in")),
    Some(Button::Action("view.zoom_out")),
    Some(Button::Action("view.fit")),
    Some(Button::Action("view.zoom_selection")),
    None,
    Some(Button::Menu {
        icon: "view.grid",
        title: "Grid and snaps",
        items: GRID_MENU,
        lit: true,
    }),
    Some(Button::Action("view.rulers")),
    None,
    Some(Button::Menu {
        icon: "view.display",
        title: "Display",
        items: DISPLAY_MENU,
        lit: false,
    }),
    Some(Button::Action("view.stroke_width")),
    Some(Button::Action("view.machine_preview")),
];

#[derive(Clone, Copy)]
enum Button {
    Action(&'static str),
    /// Opens a menu of toggles. With `lit`, shown "on" while the first
    /// one is (Grid ▾ lights with the grid, as in the mockup).
    Menu {
        icon: &'static str,
        title: &'static str,
        items: &'static [Item],
        lit: bool,
    },
}

/// Draw the bar over `canvas` (screen rectangle).
pub fn show(ctx: &egui::Context, reg: &Registry, cx: &mut Cx<'_>, canvas: egui::Rect) {
    if !canvas.is_positive() {
        return;
    }
    egui::Area::new(egui::Id::new("nav-bar"))
        .order(egui::Order::Middle)
        .pivot(Align2::CENTER_BOTTOM)
        .fixed_pos(pos2(canvas.center().x, canvas.bottom() - 10.0))
        .constrain_to(canvas)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(ui.visuals().window_fill)
                .stroke(ui.visuals().window_stroke)
                .corner_radius(8.0)
                .inner_margin(3.0)
                .shadow(egui::Shadow {
                    offset: [0, 2],
                    blur: 10,
                    spread: 0,
                    color: Color32::from_black_alpha(20),
                })
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        for b in BAR {
                            match b {
                                None => divider(ui),
                                Some(Button::Action(id)) => action(ui, reg, cx, id),
                                Some(Button::Menu {
                                    icon,
                                    title,
                                    items,
                                    lit,
                                }) => {
                                    menu(ui, reg, cx, icon, title, items, *lit);
                                }
                            }
                        }
                    });
                });
        });
}

fn divider(ui: &mut Ui) {
    let (r, _) = ui.allocate_exact_size(vec2(5.0, 26.0), Sense::hover());
    ui.painter().line_segment(
        [
            pos2(r.center().x, r.top() + 3.0),
            pos2(r.center().x, r.bottom() - 3.0),
        ],
        ui.visuals().widgets.noninteractive.bg_stroke,
    );
}

/// A 28 × 26 icon button, filled while `on` (mockup `.navbar .tb`).
fn icon_button(ui: &mut Ui, icon: &str, label: &str, on: bool) -> egui::Response {
    let (r, resp) = ui.allocate_exact_size(vec2(28.0, 26.0), Sense::click());
    let v = ui.visuals();
    let (fill, ink) = if on {
        (v.selection.bg_fill, v.strong_text_color())
    } else if resp.hovered() {
        (v.widgets.hovered.weak_bg_fill, v.strong_text_color())
    } else {
        (Color32::TRANSPARENT, v.text_color())
    };
    ui.painter().rect_filled(r, 5.0, fill);
    if on {
        ui.painter().rect_stroke(
            r,
            5.0,
            Stroke::new(1.0, v.selection.stroke.color),
            egui::StrokeKind::Inside,
        );
    }
    let icon_rect = egui::Rect::from_center_size(r.center(), vec2(16.0, 16.0));
    crate::views::icons::paint(ui.painter(), icon_rect, icon, label, ink);
    resp
}

/// The hover text: the label, its first key and its hint.
fn tooltip(ctx: &egui::Context, a: &actions::Action) -> String {
    let mut t = a.label.to_string();
    if let Some(k) = a.keys.first() {
        t.push_str(&format!(" ({})", actions::key_text(ctx, k)));
    }
    if !a.hint.is_empty() {
        t.push('\n');
        t.push_str(&a.hint);
    }
    t
}

fn action(ui: &mut Ui, reg: &Registry, cx: &mut Cx<'_>, id: &str) {
    let Some(a) = reg.get(id) else {
        return;
    };
    let on = a.op.kind() == actions::ActionKind::Toggle && reg.is_on(a, cx.state, cx.settings);
    let tip = tooltip(ui.ctx(), a);
    if icon_button(ui, id, &a.label, on)
        .on_hover_text(tip)
        .clicked()
    {
        reg.run(a, cx);
    }
}

#[allow(clippy::too_many_arguments)]
fn menu(
    ui: &mut Ui,
    reg: &Registry,
    cx: &mut Cx<'_>,
    icon: &str,
    title: &str,
    items: &'static [Item],
    lit: bool,
) {
    let on = match items.first().filter(|_| lit) {
        Some(Item::Action(id)) => reg
            .get(id)
            .is_some_and(|a| reg.is_on(a, cx.state, cx.settings)),
        _ => false,
    };
    let resp = icon_button(ui, icon, title, on).on_hover_text(format!("{title} ▾"));
    egui::Popup::menu(&resp)
        .align(RectAlign::TOP_END)
        .gap(6.0)
        .close_behavior(PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            ui.set_min_width(190.0);
            ui.weak(title.to_uppercase());
            actions::show(ui, reg, cx, items, Style::Pane);
        });
}

#[cfg(test)]
#[allow(clippy::expect_used)] // a missing action should fail loudly
mod tests {
    use super::*;

    /// Every bar button and ▾ item is a registered action with a drawn
    /// icon where it shows one (design §2.4).
    #[test]
    fn every_button_is_registered_and_drawn() {
        let reg = Registry::default();
        for b in BAR.iter().flatten() {
            match *b {
                Button::Action(id) => {
                    assert!(reg.get(id).is_some(), "{id}");
                    assert!(crate::views::icons::has_icon(id), "{id} icon");
                }
                Button::Menu { icon, items, .. } => {
                    assert!(crate::views::icons::has_icon(icon), "{icon} icon");
                    for i in items {
                        if let Item::Action(id) = i {
                            assert!(
                                reg.get(id)
                                    .is_some_and(|a| a.op.kind() == actions::ActionKind::Toggle),
                                "{id}"
                            );
                        }
                    }
                }
            }
        }
    }

    /// The bar's actions change the shared settings and the camera
    /// (design §2.4): a toggle flips `Settings::display`, which the View
    /// menu and the palette read too; zoom steps about the centre.
    #[test]
    fn actions_drive_settings_and_camera() {
        let reg = Registry::default();
        let mut state = crate::state::AppState::default();
        state.view.rect = egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(800.0, 600.0));
        let mut settings = crate::settings::Settings::default();
        let mut cx = Cx {
            state: &mut state,
            settings: &mut settings,
            plugins: None,
        };
        let run = |cx: &mut Cx<'_>, id: &str| {
            let a = reg.get(id).expect("registered");
            reg.run(a, cx);
        };
        assert!(!cx.settings.display.rulers);
        run(&mut cx, "view.rulers");
        assert!(cx.settings.display.rulers);
        run(&mut cx, "view.constraints");
        assert!(!cx.settings.display.constraints);
        let before = cx.state.view.camera.pt_per_mm;
        run(&mut cx, "view.zoom_in");
        assert!(cx.state.view.camera.pt_per_mm > before);
        run(&mut cx, "view.zoom_out");
        assert!((cx.state.view.camera.pt_per_mm - before).abs() < 1e-9);
        // Nothing selected: Zoom to selection says so and leaves the view.
        run(&mut cx, "view.zoom_selection");
        assert!((cx.state.view.camera.pt_per_mm - before).abs() < 1e-9);
        assert!(cx.state.doc.notice.is_some());
        run(&mut cx, "view.pan");
        assert!(cx.state.view.pan_mode);
    }
}
