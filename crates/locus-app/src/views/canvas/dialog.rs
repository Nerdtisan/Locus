// SPDX-License-Identifier: AGPL-3.0-or-later
//! The command dialog (M5 design §2, §6.6–6.9; #330): one Fusion-style
//! panel docked to the canvas's top-right corner for every dialog tool —
//! Offset, Move, Rotate, Scale, Pattern, Mirror, Text, Fillet and Chamfer.
//!
//! Every dialog is built from the same parts, matching the approved mockup:
//! a header with the tool's ribbon icon and its name in capitals (plus an
//! optional switch, e.g. Rectangular | Circular), label-column rows,
//! selection boxes ("4 curves ✕") and a footer with the Enter/Esc hint,
//! Cancel and a primary OK. It floats over the canvas, so the pointer over
//! it never reaches the canvas.

use egui::{Align, Align2, Color32, Layout, Rect, RichText, Sense, Stroke, Ui, vec2};

/// The mockup's dialog width (`.dlg{width:268px}`); Text is 320.
pub(crate) const WIDTH: f32 = 268.0;
pub(crate) const WIDE: f32 = 320.0;
/// The label column of a dialog row (`.row .lb{flex:0 0 92px}`).
const LABEL_W: f32 = 92.0;

/// The mockup's accent and the ink drawn on it (`--accent`, `--accent-ink`).
pub(crate) fn accent(ui: &Ui) -> (Color32, Color32) {
    if ui.visuals().dark_mode {
        (
            Color32::from_rgb(76, 147, 236),
            Color32::from_rgb(13, 20, 32),
        )
    } else {
        (Color32::from_rgb(31, 111, 209), Color32::WHITE)
    }
}

/// What a dialog shows in its header.
pub(crate) struct Header<'a> {
    /// The ribbon action whose icon heads the dialog (`views::icons`).
    pub icon: &'static str,
    pub title: &'a str,
    pub width: f32,
}

impl<'a> Header<'a> {
    pub(crate) fn new(icon: &'static str, title: &'a str) -> Self {
        Self {
            icon,
            title,
            width: WIDTH,
        }
    }
    pub(crate) fn wide(mut self) -> Self {
        self.width = WIDE;
        self
    }
}

/// Draw a command dialog: `header`, then `extra` at the header's right end
/// (a mode switch, or nothing), then `add`'s body.
pub(crate) fn command_dialog<R>(
    ui: &Ui,
    rect: Rect,
    header: Header<'_>,
    extra: impl FnOnce(&mut Ui),
    add: impl FnOnce(&mut Ui) -> R,
) -> R {
    egui::Area::new(egui::Id::new("command-dialog"))
        .order(egui::Order::Foreground)
        .anchor(
            Align2::RIGHT_TOP,
            rect.right_top().to_vec2() - ui.ctx().content_rect().right_top().to_vec2()
                + vec2(-12.0, 12.0),
        )
        .show(ui.ctx(), |ui| {
            egui::Frame::window(ui.style())
                .inner_margin(egui::Margin::symmetric(10, 7))
                .show(ui, |ui| {
                    // A dialog, not a pane: fixed width, so wide content
                    // (the font list) wraps inside it.
                    ui.set_width(header.width);
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
                        let ink = ui.visuals().strong_text_color();
                        crate::views::icons::paint(ui.painter(), r, header.icon, header.title, ink);
                        ui.label(RichText::new(header.title.to_uppercase()).strong());
                        ui.with_layout(Layout::right_to_left(Align::Center), extra);
                    });
                    ui.separator();
                    ui.spacing_mut().item_spacing.y = 4.0;
                    add(ui)
                })
                .inner
        })
        .inner
}

/// One dialog row: a muted label in the label column, then `add`.
pub(crate) fn row<R>(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(
            vec2(LABEL_W, ui.spacing().interact_size.y),
            Layout::left_to_right(Align::Center),
            |ui| {
                ui.set_min_width(LABEL_W);
                ui.weak(label);
            },
        );
        add(ui)
    })
    .inner
}

/// A selection box (`.selbox`): filled in the accent colour with a ✕ while
/// something is picked, dashed and muted while waiting for a pick. `active`
/// marks the box the next canvas click fills. Returns (box clicked, ✕
/// clicked): clicking the box makes it the one being picked, ✕ clears it.
pub(crate) fn selection(ui: &mut Ui, text: &str, filled: bool, active: bool) -> (bool, bool) {
    let (accent, accent_ink) = accent(ui);
    let font = egui::FontId::proportional(11.5);
    let on = if filled {
        accent_ink
    } else {
        ui.visuals().weak_text_color()
    };
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, on);
    let x_w = if filled { 14.0 } else { 0.0 };
    let size = vec2(galley.size().x + 16.0 + x_w, 22.0);
    let (r, resp) = ui.allocate_exact_size(size, Sense::click());
    let p = ui.painter();
    if filled {
        p.rect_filled(r, 4.0, accent);
    } else {
        p.rect_filled(r, 4.0, ui.visuals().extreme_bg_color);
        let stroke = Stroke::new(1.0, if active { accent } else { on });
        p.extend(egui::Shape::dashed_line(
            &[
                r.left_top(),
                r.right_top(),
                r.right_bottom(),
                r.left_bottom(),
                r.left_top(),
            ],
            stroke,
            3.0,
            2.0,
        ));
    }
    if active && filled {
        p.rect_stroke(
            r.expand(1.5),
            5.0,
            Stroke::new(1.5, accent),
            egui::StrokeKind::Outside,
        );
    }
    p.galley(
        r.left_center() + vec2(8.0, -galley.size().y / 2.0),
        galley,
        on,
    );
    let mut cleared = false;
    if filled {
        let xr = Rect::from_center_size(r.right_center() - vec2(10.0, 0.0), vec2(14.0, 14.0));
        // Painted, not a ✕ glyph: the bundled fonts lack it (tofu box).
        let k = xr.shrink(4.0);
        let x = Stroke::new(1.4, on);
        p.line_segment([k.left_top(), k.right_bottom()], x);
        p.line_segment([k.right_top(), k.left_bottom()], x);
        let hit = resp.clicked()
            && resp
                .interact_pointer_pos()
                .is_some_and(|q| xr.expand(2.0).contains(q));
        cleared = hit;
    }
    let resp = resp.on_hover_text(if filled {
        "Click to pick again; the cross clears"
    } else {
        "Pick on the canvas"
    });
    (resp.clicked() && !cleared, cleared)
}

/// A segmented switch (`.seg`): one button per option, the chosen one
/// filled. Returns the option clicked, if it differs from `current`.
pub(crate) fn segmented<T: Copy + PartialEq>(
    ui: &mut Ui,
    current: T,
    options: &[(T, &str)],
) -> Option<T> {
    let mut picked = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        let (accent, ink) = accent(ui);
        for &(v, label) in options {
            let on = v == current;
            let text = if on {
                RichText::new(label).color(ink)
            } else {
                RichText::new(label)
            };
            let fill = if on {
                accent
            } else {
                ui.visuals().extreme_bg_color
            };
            let b = egui::Button::new(text).fill(fill).corner_radius(0.0);
            if ui.add(b).clicked() && !on {
                picked = Some(v);
            }
        }
    });
    picked
}

/// The footer (`.df`): "Enter OK · Esc cancel", Cancel, then the primary
/// OK. `ok` is `Err(why)` while OK can't run yet (shown on hover). Returns
/// (OK clicked, Cancel clicked).
pub(crate) fn footer(ui: &mut Ui, ok: Result<(), &str>) -> (bool, bool) {
    ui.separator();
    ui.horizontal(|ui| {
        ui.weak(RichText::new("Enter OK · Esc cancel").size(10.5));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let (accent, ink) = accent(ui);
            let btn = egui::Button::new(RichText::new("OK").color(ink)).fill(accent);
            let r = ui.add_enabled(ok.is_ok(), btn);
            let ok_clicked = match ok {
                Ok(()) => r.clicked(),
                Err(why) => {
                    r.on_disabled_hover_text(why);
                    false
                }
            };
            let cancel = ui.button("Cancel").clicked();
            (ok_clicked, cancel)
        })
        .inner
    })
    .inner
}

/// A footer with a single Close: for dialogs whose changes apply as you go
/// (editing placed text, a picked offset).
pub(crate) fn close_footer(ui: &mut Ui) -> bool {
    ui.separator();
    ui.horizontal(|ui| {
        ui.weak(RichText::new("Esc closes").size(10.5));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.button("Close").clicked()
        })
        .inner
    })
    .inner
}

/// "N curves" / "1 curve": a selection box's text (mockup: "4 curves").
pub(crate) fn count(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_read_naturally() {
        assert_eq!(count(1, "curve", "curves"), "1 curve");
        assert_eq!(count(4, "curve", "curves"), "4 curves");
        assert_eq!(count(0, "curve", "curves"), "0 curves");
    }

    /// Every dialog tool's header icon is a drawn ribbon icon, not a
    /// fallback initial (#330).
    #[test]
    fn dialog_icons_are_drawn() {
        for id in DIALOG_ICONS {
            assert!(crate::views::icons::has_icon(id), "{id} has no icon");
        }
    }

    const DIALOG_ICONS: [&str; 10] = [
        "modify.offset",
        "modify.move",
        "modify.rotate",
        "modify.scale",
        "sketch.pattern_rect",
        "sketch.pattern_circ",
        "sketch.mirror",
        "sketch.text",
        "modify.fillet",
        "modify.chamfer",
    ];
}
