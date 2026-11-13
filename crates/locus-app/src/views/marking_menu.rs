// SPDX-License-Identifier: AGPL-3.0-or-later
//! The marking menu (design §4, #290): right-click (or a long press on
//! touch) opens a ring of eight actions around the pointer with a list
//! beneath it. Both come from the action registry (design §7.1).
//!
//! * The ring never changes ([`MARKING_RING`]); actions that can't run are
//!   dimmed, not moved.
//! * **Flick:** press the right button, move toward a slot and release, and
//!   it runs; the menu isn't drawn while a flick is under way.
//! * A plain right-click leaves the menu open until a choice, a press
//!   outside it or Esc.

use crate::actions::{Cx, Item, MARKING_RING, Registry, Style, constraints_for, marking_list};
use egui::{Pos2, Vec2};

/// Distance from the centre to each ring button's inner edge, points.
const RING_PT: f32 = 56.0;
/// Releasing at least this far from the centre is a flick.
const FLICK_PT: f32 = 24.0;
/// A held menu that hasn't moved this far is drawn after this long, s.
const SHOW_AFTER_S: f64 = 0.25;
const BUTTON: Vec2 = Vec2::new(96.0, 26.0);

/// How slot `k`'s button hangs off its anchor: it grows away from the
/// centre, so long labels never cover a neighbour.
fn pivot(k: usize) -> egui::Align2 {
    use egui::Align2 as A;
    [
        A::CENTER_BOTTOM,
        A::LEFT_BOTTOM,
        A::LEFT_CENTER,
        A::LEFT_TOP,
        A::CENTER_TOP,
        A::RIGHT_TOP,
        A::RIGHT_CENTER,
        A::RIGHT_BOTTOM,
    ][k % 8]
}

/// Which ring slot a pointer offset points at: 0 = N, clockwise (screen Y
/// grows downwards).
#[must_use]
pub fn slot(d: Vec2) -> usize {
    let a = d.x.atan2(-d.y).to_degrees().rem_euclid(360.0);
    ((a / 45.0).round() as usize) % 8
}

/// Where slot `k`'s button is centred, relative to the ring's centre.
fn offset(k: usize) -> Vec2 {
    let a = (k as f32 * 45.0).to_radians();
    Vec2::new(a.sin(), -a.cos()) * RING_PT
}

/// Draw and run the marking menu, if one is open.
pub fn show(ctx: &egui::Context, reg: &Registry, cx: &mut Cx<'_>) {
    let Some((m, rect)) = cx.state.marking() else {
        return;
    };
    let (pointer, released, now) = ctx.input(|i| {
        (
            i.pointer.latest_pos(),
            i.pointer.button_released(egui::PointerButton::Secondary),
            i.time,
        )
    });
    let moved = pointer.map_or(Vec2::ZERO, |p| p - m.at);
    if m.held && released {
        if moved.length() >= FLICK_PT {
            cx.state.close_marking();
            if let Some(a) = reg.get(MARKING_RING[slot(moved)])
                && reg.enabled(a, cx.state).yes()
            {
                reg.run(a, cx);
            }
            return;
        }
        // A plain right-click: the menu stays open.
        cx.state.set_marking_held(false);
    }
    let flicking = m.held && (moved.length() >= FLICK_PT || now - m.since < SHOW_AFTER_S);
    if flicking {
        ctx.request_repaint();
        return;
    }
    let aim = (m.held && moved.length() >= FLICK_PT).then(|| slot(moved));
    let mut close = false;
    let mut areas = Vec::new();
    // The ring: one small area per slot, so the canvas stays clickable
    // between the buttons.
    for (k, id) in MARKING_RING.iter().enumerate() {
        let Some(a) = reg.get(id) else { continue };
        let centre = m.at + offset(k);
        let r = egui::Area::new(egui::Id::new(("locus-marking", k)))
            .order(egui::Order::Foreground)
            .pivot(pivot(k))
            .fixed_pos(centre)
            .constrain_to(rect)
            .show(ctx, |ui| {
                let enabled = reg.enabled(a, cx.state);
                // Labels only: the web build's font has no glyphs for the icons.
                let label = reg.label(a, cx.state).into_owned();
                let b = egui::Button::new(label)
                    .min_size(BUTTON)
                    .selected(aim == Some(k));
                let mut resp = ui.add_enabled(enabled.yes(), b);
                if let crate::actions::Enabled::No(why) = &enabled {
                    resp = resp.on_disabled_hover_text(why.as_ref());
                }
                if resp.clicked() {
                    reg.run(a, cx);
                    close = true;
                }
            });
        areas.push(r.response.rect);
    }
    // The list beneath the ring.
    let selected = !cx.state.selection.items.is_empty();
    let list = egui::Area::new(egui::Id::new("locus-marking-list"))
        .order(egui::Order::Foreground)
        .pivot(egui::Align2::CENTER_TOP)
        .fixed_pos(m.at + Vec2::new(0.0, RING_PT + 2.0 * BUTTON.y))
        .constrain_to(rect)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_min_width(BUTTON.x);
                if selected {
                    let fits = constraints_for(reg, cx.state);
                    ui.add_enabled_ui(!fits.is_empty(), |ui| {
                        ui.menu_button("Constrain", |ui| {
                            for a in fits {
                                if crate::actions::show_one(ui, reg, cx, a, Style::Menu) {
                                    close = true;
                                }
                            }
                        })
                        .response
                        .on_disabled_hover_text("No constraint fits the selection");
                    });
                }
                let items: &[Item] = marking_list(selected);
                if crate::actions::show(ui, reg, cx, items, Style::Menu) {
                    close = true;
                }
            });
        });
    areas.push(list.response.rect);
    let outside = ctx.input(|i| {
        i.pointer.any_pressed()
            && i.pointer
                .press_origin()
                .is_some_and(|p: Pos2| !areas.iter().any(|a| a.contains(p)))
    }) && !egui::Popup::is_any_open(ctx);
    if close || (outside && !m.held) || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        cx.state.close_marking();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actions::Registry;

    #[test]
    fn directions_map_to_the_eight_slots() {
        let r = 50.0;
        for k in 0..8 {
            assert_eq!(slot(offset(k)), k, "slot {k}");
            // A little either side still picks the same slot.
            let a = (k as f32 * 45.0 + 20.0).to_radians();
            assert_eq!(slot(Vec2::new(a.sin(), -a.cos()) * r), k);
        }
        assert_eq!(slot(Vec2::new(0.0, -r)), 0, "up is N");
        assert_eq!(slot(Vec2::new(r, 0.0)), 2, "right is E");
        assert_eq!(slot(Vec2::new(0.0, r)), 4, "down is S");
        assert_eq!(slot(Vec2::new(-r, -r)), 7, "up-left is NW");
    }

    /// The ring is the design's (§4) and every slot is a registry action.
    #[test]
    fn the_ring_is_the_designs() {
        let reg = Registry::default();
        let labels: Vec<&str> = MARKING_RING
            .iter()
            .map(|id| reg.get(id).map(|a| a.id.as_str()).unwrap_or("missing"))
            .collect();
        assert_eq!(
            labels,
            [
                "edit.repeat",
                "edit.undo",
                "modify.move",
                "modify.offset",
                "edit.delete",
                "modify.construction",
                "modify.trim",
                "constrain.dimension",
            ]
        );
    }
}
