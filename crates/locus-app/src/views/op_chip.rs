// SPDX-License-Identifier: AGPL-3.0-or-later
//! Layer operations at a glance (#420, SPEC §7): one colour and one short
//! label per operation, shared by the Browser's layer chips, the Machine
//! preview tint and its legend, so a colour means the same thing everywhere.
//!
//! The colours follow the LightBurn convention people already read (red
//! cuts, blue scores), shifted so each reads on both themes. They are the
//! editor's own: export colours stay the machine profile's (SPEC §7).

use egui::{Color32, Response, Sense, Ui};
use locus_doc::OperationKind;

/// The operation's colour on the canvas and in the Browser.
#[must_use]
pub fn colour(kind: OperationKind, dark: bool) -> Color32 {
    let (d, l) = match kind {
        OperationKind::Cut => ([255, 92, 92], [210, 30, 40]),
        OperationKind::Score => ([90, 160, 255], [20, 90, 220]),
        OperationKind::Engrave => ([255, 170, 60], [200, 110, 0]),
        OperationKind::Draw => ([90, 205, 120], [20, 135, 60]),
        OperationKind::DragKnife => ([235, 110, 235], [170, 40, 170]),
        OperationKind::Reference => ([150, 150, 160], [120, 120, 130]),
    };
    let [r, g, b] = if dark { d } else { l };
    Color32::from_rgb(r, g, b)
}

/// The chip's label: short enough for a narrow Browser.
#[must_use]
pub const fn short(kind: OperationKind) -> &'static str {
    match kind {
        OperationKind::Cut => "CUT",
        OperationKind::Score => "SCORE",
        OperationKind::Engrave => "ENGRAVE",
        OperationKind::Draw => "DRAW",
        OperationKind::DragKnife => "KNIFE",
        OperationKind::Reference => "REF",
    }
}

/// A rounded chip in the operation's colour: tinted fill, the operation's
/// icon and its short name.
/// `marked` adds a warning outline (the machine can't run this operation).
pub fn chip(ui: &mut Ui, kind: OperationKind, marked: bool) -> Response {
    let dark = ui.visuals().dark_mode;
    let c = colour(kind, dark);
    let font = egui::FontId::proportional(10.0);
    let galley = ui.painter().layout_no_wrap(short(kind).to_owned(), font, c);
    let size = egui::vec2(galley.size().x + 24.0, 16.0);
    let (rect, r) = ui.allocate_exact_size(size, Sense::click());
    let fill = c.gamma_multiply(if r.hovered() { 0.35 } else { 0.18 });
    ui.painter().rect_filled(rect, 8.0, fill);
    let stroke = if marked {
        egui::Stroke::new(1.5, ui.visuals().warn_fg_color)
    } else {
        egui::Stroke::new(1.0, c.gamma_multiply(0.6))
    };
    ui.painter()
        .rect_stroke(rect, 8.0, stroke, egui::StrokeKind::Inside);
    let icon = egui::Rect::from_center_size(
        egui::pos2(rect.left() + 10.0, rect.center().y),
        egui::vec2(11.0, 11.0),
    );
    crate::views::icons::paint(
        ui.painter(),
        icon,
        crate::views::icons::operation(kind),
        "",
        c,
    );
    let at = egui::pos2(rect.left() + 18.0, rect.center().y - galley.size().y / 2.0);
    ui.painter().galley(at, galley, c);
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every operation has its own colour on each theme, and its own label.
    #[test]
    fn operations_are_told_apart() {
        for dark in [false, true] {
            let cs: Vec<Color32> = OperationKind::ALL
                .iter()
                .map(|k| colour(*k, dark))
                .collect();
            for (i, a) in cs.iter().enumerate() {
                assert!(!cs[i + 1..].contains(a), "{i} {dark}");
            }
        }
        let mut labels: Vec<&str> = OperationKind::ALL.iter().map(|k| short(*k)).collect();
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(labels.len(), OperationKind::ALL.len());
    }
}
