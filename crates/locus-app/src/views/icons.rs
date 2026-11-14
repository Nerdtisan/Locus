// SPDX-License-Identifier: AGPL-3.0-or-later
//! Ribbon icons drawn as vector shapes (#299, M5 design §2).
//!
//! The bundled fonts (ADR 0001) have no glyphs for most geometric symbols,
//! so text icons drew as empty boxes. Painting them keeps the ribbon
//! independent of fonts and lets the constraint symbols match Fusion's.
//! Each icon is drawn in a 16 × 16 box scaled to the target rectangle. An
//! action without a drawing (a plugin, a newly added tool) gets its label's
//! initial in a rounded box, so it is never blank.

use egui::{Color32, Pos2, Rect, Shape, Stroke, pos2};
use std::f32::consts::{FRAC_PI_2, PI, TAU};

/// A painter scaled to a 16 × 16 icon box.
struct Pen<'a> {
    p: &'a egui::Painter,
    r: Rect,
    c: Color32,
    w: f32,
}

impl Pen<'_> {
    fn at(&self, x: f32, y: f32) -> Pos2 {
        pos2(
            self.r.left() + x / 16.0 * self.r.width(),
            self.r.top() + y / 16.0 * self.r.height(),
        )
    }
    fn k(&self) -> f32 {
        self.r.width() / 16.0
    }
    fn stroke(&self) -> Stroke {
        Stroke::new(self.w, self.c)
    }
    fn line(&self, a: (f32, f32), b: (f32, f32)) {
        self.p
            .line_segment([self.at(a.0, a.1), self.at(b.0, b.1)], self.stroke());
    }
    fn poly(&self, pts: &[(f32, f32)], closed: bool) {
        let pts: Vec<Pos2> = pts.iter().map(|&(x, y)| self.at(x, y)).collect();
        self.p.add(if closed {
            Shape::closed_line(pts, self.stroke())
        } else {
            Shape::line(pts, self.stroke())
        });
    }
    fn fill(&self, pts: &[(f32, f32)]) {
        let pts: Vec<Pos2> = pts.iter().map(|&(x, y)| self.at(x, y)).collect();
        self.p.add(Shape::convex_polygon(pts, self.c, Stroke::NONE));
    }
    fn dashed(&self, a: (f32, f32), b: (f32, f32)) {
        let pts = [self.at(a.0, a.1), self.at(b.0, b.1)];
        let k = self.k();
        self.p
            .extend(Shape::dashed_line(&pts, self.stroke(), 2.0 * k, 1.6 * k));
    }
    fn circle(&self, c: (f32, f32), r: f32) {
        self.p
            .circle_stroke(self.at(c.0, c.1), r * self.k(), self.stroke());
    }
    fn dot(&self, c: (f32, f32)) {
        self.p
            .circle_filled(self.at(c.0, c.1), 1.6 * self.k(), self.c);
    }
    /// An arc from angle `a0` to `a1` (radians, y down, clockwise positive).
    fn arc(&self, c: (f32, f32), r: f32, a0: f32, a1: f32) {
        let n = 16;
        let pts: Vec<(f32, f32)> = (0..=n)
            .map(|i| {
                let t = a0 + (a1 - a0) * i as f32 / n as f32;
                (c.0 + r * t.cos(), c.1 + r * t.sin())
            })
            .collect();
        self.poly(&pts, false);
    }
    fn rect(&self, x0: f32, y0: f32, x1: f32, y1: f32) {
        self.poly(&[(x0, y0), (x1, y0), (x1, y1), (x0, y1)], true);
    }
    fn arrow_head(&self, tip: (f32, f32), dir: (f32, f32)) {
        let (dx, dy) = dir;
        let l = (dx * dx + dy * dy).sqrt().max(1e-3);
        let (ux, uy) = (dx / l, dy / l);
        let s = 3.0;
        self.fill(&[
            tip,
            (tip.0 - ux * s - uy * s * 0.6, tip.1 - uy * s + ux * s * 0.6),
            (tip.0 - ux * s + uy * s * 0.6, tip.1 - uy * s - ux * s * 0.6),
        ]);
    }
}

/// Whether `id` has a drawn icon (otherwise the initial is shown).
#[cfg(test)]
pub(crate) fn has_icon(id: &str) -> bool {
    let p = egui::Painter::new(
        egui::Context::default(),
        egui::LayerId::background(),
        Rect::NOTHING,
    );
    draw(
        &p,
        Rect::from_min_size(Pos2::ZERO, egui::vec2(16.0, 16.0)),
        id,
        Color32::WHITE,
    )
}

/// Paint the icon of action `id` (label `label`) into `rect`.
pub fn paint(p: &egui::Painter, rect: Rect, id: &str, label: &str, color: Color32) {
    if !draw(p, rect, id, color) {
        let initial: String = label
            .chars()
            .find(|c| c.is_alphanumeric())
            .into_iter()
            .collect();
        p.rect_stroke(
            rect.shrink(1.0),
            3.0,
            Stroke::new(1.0, color),
            egui::StrokeKind::Inside,
        );
        p.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            initial,
            egui::FontId::proportional(rect.height() * 0.7),
            color,
        );
    }
}

/// Draw `id`'s icon; false if it has none.
fn draw(p: &egui::Painter, r: Rect, id: &str, c: Color32) -> bool {
    let d = Pen {
        p,
        r,
        c,
        w: (r.width() / 12.0).clamp(1.0, 2.0),
    };
    match id {
        // ── Select ──
        "select.select" => {
            d.fill(&[(4.0, 2.0), (4.0, 13.0), (7.0, 10.0)]);
            d.fill(&[(4.0, 2.0), (12.0, 10.0), (7.0, 10.0)]);
            d.line((7.0, 10.0), (10.0, 14.5));
        }
        "select.chain" => {
            d.poly(&[(2.0, 9.0), (5.0, 6.0), (8.0, 9.0), (5.0, 12.0)], true);
            d.poly(&[(8.0, 7.0), (11.0, 4.0), (14.0, 7.0), (11.0, 10.0)], true);
        }
        "edit.select_all" => {
            d.dashed((2.0, 2.0), (14.0, 2.0));
            d.dashed((14.0, 2.0), (14.0, 14.0));
            d.dashed((14.0, 14.0), (2.0, 14.0));
            d.dashed((2.0, 14.0), (2.0, 2.0));
            d.rect(5.0, 5.0, 11.0, 11.0);
        }
        "edit.deselect" => {
            d.dashed((2.0, 2.0), (14.0, 2.0));
            d.dashed((14.0, 2.0), (14.0, 14.0));
            d.dashed((14.0, 14.0), (2.0, 14.0));
            d.dashed((2.0, 14.0), (2.0, 2.0));
            d.line((5.0, 5.0), (11.0, 11.0));
            d.line((11.0, 5.0), (5.0, 11.0));
        }
        // ── Create ──
        "sketch.point" => {
            d.line((8.0, 3.0), (8.0, 13.0));
            d.line((3.0, 8.0), (13.0, 8.0));
            d.dot((8.0, 8.0));
        }
        "sketch.line" => {
            d.line((3.0, 13.0), (13.0, 3.0));
            d.dot((3.0, 13.0));
            d.dot((13.0, 3.0));
        }
        "sketch.rect_2pt" => {
            d.rect(2.0, 4.0, 14.0, 12.0);
            d.dot((2.0, 4.0));
            d.dot((14.0, 12.0));
        }
        "sketch.rect_centre" => {
            d.rect(2.0, 4.0, 14.0, 12.0);
            d.dot((8.0, 8.0));
            d.dot((14.0, 12.0));
        }
        "sketch.rect_3pt" => {
            d.poly(&[(1.5, 10.0), (9.0, 2.5), (14.5, 8.0), (7.0, 15.5)], true);
            d.dot((1.5, 10.0));
            d.dot((9.0, 2.5));
            d.dot((14.5, 8.0));
        }
        "sketch.circle_centre" => {
            d.circle((8.0, 8.0), 6.0);
            d.dot((8.0, 8.0));
        }
        "sketch.circle_2pt" => {
            d.circle((8.0, 8.0), 6.0);
            d.dot((2.0, 8.0));
            d.dot((14.0, 8.0));
        }
        "sketch.circle_3pt" => {
            d.circle((8.0, 8.0), 6.0);
            d.dot((8.0, 2.0));
            d.dot((2.8, 11.0));
            d.dot((13.2, 11.0));
        }
        "sketch.arc_3pt" => {
            d.arc((8.0, 11.0), 6.0, PI, TAU);
            d.dot((2.0, 11.0));
            d.dot((8.0, 5.0));
            d.dot((14.0, 11.0));
        }
        "sketch.arc_centre" => {
            d.arc((5.0, 12.0), 8.0, -FRAC_PI_2, 0.0);
            d.dashed((5.0, 12.0), (13.0, 12.0));
            d.dot((5.0, 12.0));
        }
        "sketch.arc_tangent" => {
            d.line((1.0, 12.0), (7.0, 12.0));
            d.arc((7.0, 7.0), 5.0, FRAC_PI_2, -FRAC_PI_2);
            d.dot((7.0, 12.0));
        }
        "sketch.polygon" => {
            let pts: Vec<(f32, f32)> = (0..6)
                .map(|i| {
                    let t = i as f32 * TAU / 6.0;
                    (8.0 + 6.5 * t.cos(), 8.0 + 6.5 * t.sin())
                })
                .collect();
            d.poly(&pts, true);
        }
        "sketch.slot" => {
            d.line((5.0, 4.0), (11.0, 4.0));
            d.line((5.0, 12.0), (11.0, 12.0));
            d.arc((5.0, 8.0), 4.0, FRAC_PI_2, 3.0 * FRAC_PI_2);
            d.arc((11.0, 8.0), 4.0, -FRAC_PI_2, FRAC_PI_2);
        }
        "sketch.pen" => {
            let pts: Vec<(f32, f32)> = (0..=16)
                .map(|i| {
                    let t = i as f32 / 16.0;
                    let u = 1.0 - t;
                    // Cubic from (2,13) via (2,3), (14,13) to (14,3).
                    (
                        u * u * u * 2.0
                            + 3.0 * u * u * t * 2.0
                            + 3.0 * u * t * t * 14.0
                            + t * t * t * 14.0,
                        u * u * u * 13.0
                            + 3.0 * u * u * t * 3.0
                            + 3.0 * u * t * t * 13.0
                            + t * t * t * 3.0,
                    )
                })
                .collect();
            d.poly(&pts, false);
            d.dashed((2.0, 13.0), (2.0, 4.0));
            d.dot((2.0, 4.0));
        }
        "sketch.text" => {
            d.poly(&[(3.0, 14.0), (8.0, 2.0), (13.0, 14.0)], false);
            d.line((5.0, 10.0), (11.0, 10.0));
        }
        "modify.offset" => {
            d.poly(&[(2.0, 14.0), (2.0, 6.0), (6.0, 2.0), (14.0, 2.0)], false);
            d.poly(&[(6.0, 14.0), (6.0, 8.0), (8.0, 6.0), (14.0, 6.0)], false);
        }
        "sketch.mirror" => {
            d.dashed((8.0, 1.0), (8.0, 15.0));
            d.poly(&[(6.0, 3.0), (1.5, 13.0), (6.0, 13.0)], true);
            d.fill(&[(10.0, 3.0), (14.5, 13.0), (10.0, 13.0)]);
        }
        "sketch.pattern_rect" => {
            for x in [3.0, 8.0, 13.0] {
                for y in [3.0, 8.0, 13.0] {
                    d.rect(x - 1.8, y - 1.8, x + 1.8, y + 1.8);
                }
            }
        }
        "sketch.pattern_circ" => {
            for i in 0..6 {
                let t = i as f32 * TAU / 6.0;
                d.circle((8.0 + 5.5 * t.cos(), 8.0 + 5.5 * t.sin()), 1.8);
            }
            d.dot((8.0, 8.0));
        }
        "sketch.linetype_construction" | "modify.construction" => {
            d.dashed((2.0, 14.0), (14.0, 2.0));
            d.dot((2.0, 14.0));
            d.dot((14.0, 2.0));
        }
        // ── Modify ──
        "modify.trim" => {
            d.line((2.0, 8.0), (8.0, 8.0));
            d.dashed((8.0, 8.0), (14.0, 8.0));
            d.line((11.0, 2.0), (11.0, 14.0));
            d.line((5.0, 5.0), (8.0, 11.0));
        }
        "modify.extend" => {
            d.line((2.0, 8.0), (7.0, 8.0));
            d.dashed((7.0, 8.0), (12.0, 8.0));
            d.arrow_head((13.0, 8.0), (1.0, 0.0));
            d.line((14.0, 2.0), (14.0, 14.0));
        }
        "modify.break" => {
            d.line((2.0, 10.0), (6.5, 10.0));
            d.line((9.5, 10.0), (14.0, 10.0));
            d.dot((6.5, 10.0));
            d.dot((9.5, 10.0));
            d.line((8.0, 2.0), (8.0, 7.0));
        }
        "modify.fillet" => {
            d.line((3.0, 2.0), (3.0, 8.0));
            d.arc((9.0, 8.0), 6.0, PI, FRAC_PI_2);
            d.line((9.0, 14.0), (14.0, 14.0));
        }
        "modify.chamfer" => {
            d.poly(&[(3.0, 2.0), (3.0, 9.0), (8.0, 14.0), (14.0, 14.0)], false);
        }
        "modify.move" => {
            d.line((8.0, 2.0), (8.0, 14.0));
            d.line((2.0, 8.0), (14.0, 8.0));
            d.arrow_head((8.0, 1.0), (0.0, -1.0));
            d.arrow_head((8.0, 15.0), (0.0, 1.0));
            d.arrow_head((1.0, 8.0), (-1.0, 0.0));
            d.arrow_head((15.0, 8.0), (1.0, 0.0));
        }
        "modify.rotate" => {
            d.arc((8.0, 8.0), 5.5, -FRAC_PI_2 + 0.4, PI + 0.4);
            d.arrow_head((7.5, 2.5), (-1.0, -0.2));
            d.dot((8.0, 8.0));
        }
        "modify.scale" => {
            d.rect(2.0, 8.0, 8.0, 14.0);
            d.rect(2.0, 2.0, 14.0, 14.0);
            d.line((6.0, 10.0), (12.0, 4.0));
            d.arrow_head((13.0, 3.0), (1.0, -1.0));
        }
        "edit.delete" => {
            d.poly(
                &[
                    (5.0, 3.0),
                    (15.0, 3.0),
                    (15.0, 13.0),
                    (5.0, 13.0),
                    (1.0, 8.0),
                ],
                true,
            );
            d.line((8.0, 6.0), (12.0, 10.0));
            d.line((12.0, 6.0), (8.0, 10.0));
        }
        // ── Constrain ──
        "constrain.dimension" => {
            d.line((2.0, 3.0), (2.0, 13.0));
            d.line((14.0, 3.0), (14.0, 13.0));
            d.line((4.0, 8.0), (12.0, 8.0));
            d.arrow_head((3.0, 8.0), (-1.0, 0.0));
            d.arrow_head((13.0, 8.0), (1.0, 0.0));
        }
        "constrain.coincident" => {
            d.circle((8.0, 8.0), 5.5);
            d.dot((8.0, 8.0));
        }
        "constrain.collinear" => {
            d.line((1.0, 13.0), (6.5, 8.5));
            d.line((9.5, 6.5), (15.0, 2.0));
            d.dashed((6.5, 8.5), (9.5, 6.5));
        }
        "constrain.concentric" => {
            d.circle((8.0, 8.0), 6.5);
            d.circle((8.0, 8.0), 3.5);
        }
        "constrain.midpoint" => {
            d.line((1.0, 11.0), (15.0, 11.0));
            d.fill(&[(8.0, 5.0), (11.0, 10.5), (5.0, 10.5)]);
        }
        "constrain.fix" => {
            d.rect(3.0, 7.5, 13.0, 14.0);
            d.arc((8.0, 7.5), 3.5, PI, TAU);
            d.dot((8.0, 10.8));
        }
        "constrain.parallel" => {
            d.line((3.0, 14.0), (8.0, 2.0));
            d.line((8.0, 14.0), (13.0, 2.0));
        }
        "constrain.perpendicular" => {
            d.line((2.0, 14.0), (14.0, 14.0));
            d.line((8.0, 2.0), (8.0, 14.0));
            d.poly(&[(8.0, 10.5), (11.5, 10.5), (11.5, 14.0)], false);
        }
        "constrain.horizontal" => {
            d.line((2.0, 8.0), (14.0, 8.0));
            d.dot((2.0, 8.0));
            d.dot((14.0, 8.0));
        }
        "constrain.vertical" => {
            d.line((8.0, 2.0), (8.0, 14.0));
            d.dot((8.0, 2.0));
            d.dot((8.0, 14.0));
        }
        "constrain.tangent" => {
            d.circle((8.0, 9.5), 4.5);
            d.line((1.0, 5.0), (15.0, 5.0));
        }
        "constrain.equal" => {
            d.line((3.0, 6.0), (13.0, 6.0));
            d.line((3.0, 10.0), (13.0, 10.0));
        }
        "constrain.symmetric" => {
            d.dashed((8.0, 1.0), (8.0, 15.0));
            d.poly(&[(2.0, 4.0), (5.5, 8.0), (2.0, 12.0)], false);
            d.poly(&[(14.0, 4.0), (10.5, 8.0), (14.0, 12.0)], false);
        }
        "constrain.auto" => {
            d.fill(&[(9.0, 1.0), (4.0, 9.0), (8.0, 9.0)]);
            d.fill(&[(8.0, 7.0), (12.0, 7.0), (7.0, 15.0)]);
        }
        // ── Inspect ──
        "view.machine_preview" => {
            d.rect(4.0, 4.0, 12.0, 12.0);
            d.dashed((2.0, 2.0), (14.0, 2.0));
            d.dashed((14.0, 2.0), (14.0, 14.0));
            d.dashed((14.0, 14.0), (2.0, 14.0));
            d.dashed((2.0, 14.0), (2.0, 2.0));
        }
        "view.stroke_width" => {
            d.line((2.0, 4.0), (14.0, 4.0));
            let thick = Pen { w: d.w * 2.5, ..d };
            thick.line((2.0, 10.0), (14.0, 10.0));
        }
        // ── The ribbon's own ──
        "ui.pinned" | "ui.unpinned" => {
            // A push pin, filled when pinned.
            let head = [(9.0, 1.5), (14.5, 7.0), (11.5, 8.0), (8.0, 4.5)];
            if id == "ui.pinned" {
                d.fill(&head);
            } else {
                d.poly(&head, true);
            }
            d.line((6.0, 5.0), (11.0, 10.0));
            d.line((9.5, 6.5), (2.0, 14.0));
        }
        // ── Browser layer rows (#326) ──
        "ui.eye" | "ui.eye_off" => {
            d.arc((8.0, 13.0), 7.0, -PI * 0.86, -PI * 0.14);
            d.arc((8.0, 3.0), 7.0, PI * 0.14, PI * 0.86);
            d.circle((8.0, 8.0), 2.0);
            if id == "ui.eye_off" {
                d.line((2.5, 13.5), (13.5, 2.5));
            }
        }
        "ui.lock" | "ui.unlock" => {
            d.rect(3.5, 7.5, 12.5, 14.0);
            if id == "ui.lock" {
                d.arc((8.0, 7.5), 3.0, PI, TAU);
            } else {
                d.arc((11.0, 5.5), 3.0, PI, TAU);
            }
        }
        "ui.export" | "ui.export_off" => {
            d.line((8.0, 10.0), (8.0, 2.5));
            d.arrow_head((8.0, 1.5), (0.0, -1.0));
            d.poly(&[(3.0, 9.0), (3.0, 14.0), (13.0, 14.0), (13.0, 9.0)], false);
            if id == "ui.export_off" {
                d.line((2.0, 2.0), (14.0, 14.0));
            }
        }
        "ui.plugin" => {
            // A plug: two prongs, a body and a lead.
            d.line((6.0, 1.5), (6.0, 5.0));
            d.line((10.0, 1.5), (10.0, 5.0));
            d.rect(3.5, 5.0, 12.5, 10.0);
            d.line((8.0, 10.0), (8.0, 14.5));
        }
        "ui.add" => {
            d.line((8.0, 3.0), (8.0, 13.0));
            d.line((3.0, 8.0), (13.0, 8.0));
        }
        // Operations (SPEC §7): what the machine does with a layer.
        "op.cut" => {
            // A blade through a line.
            d.line((1.5, 12.0), (14.5, 12.0));
            d.fill(&[(5.0, 10.0), (12.0, 2.0), (13.5, 3.5), (7.0, 11.0)]);
        }
        "op.score" => {
            d.dashed((1.5, 12.0), (14.5, 12.0));
            d.line((9.0, 2.0), (6.0, 10.0));
        }
        "op.engrave" => {
            d.rect(2.5, 2.5, 13.5, 13.5);
            d.line((2.5, 8.5), (8.5, 2.5));
            d.line((2.5, 13.5), (13.5, 2.5));
            d.line((7.5, 13.5), (13.5, 7.5));
        }
        "op.draw" => {
            // A pen nib.
            d.poly(
                &[
                    (10.0, 2.0),
                    (14.0, 6.0),
                    (6.0, 14.0),
                    (2.0, 14.0),
                    (2.0, 10.0),
                ],
                true,
            );
            d.line((8.5, 3.5), (12.5, 7.5));
        }
        "op.drag_knife" => {
            d.arc((8.0, 14.0), 6.0, -PI, -PI * 0.15);
            d.fill(&[(12.0, 2.0), (14.5, 4.5), (13.5, 8.5), (10.0, 5.0)]);
        }
        "op.reference" => {
            d.dashed((2.0, 2.0), (14.0, 2.0));
            d.dashed((14.0, 2.0), (14.0, 14.0));
            d.dashed((14.0, 14.0), (2.0, 14.0));
            d.dashed((2.0, 14.0), (2.0, 2.0));
        }
        _ => return false,
    }
    true
}

/// The icon id for an operation (SPEC §7).
pub fn operation(kind: locus_doc::OperationKind) -> &'static str {
    use locus_doc::OperationKind as K;
    match kind {
        K::Cut => "op.cut",
        K::Score => "op.score",
        K::Engrave => "op.engrave",
        K::Draw => "op.draw",
        K::DragKnife => "op.drag_knife",
        K::Reference => "op.reference",
    }
}

/// A recipe's icon: the tool that makes it.
pub fn recipe(kind: &locus_doc::RecipeKind) -> &'static str {
    use locus_doc::RecipeKind as R;
    match kind {
        R::Linear { .. } => "sketch.pattern_rect",
        R::Circular { .. } => "sketch.pattern_circ",
        R::Mirror { .. } => "sketch.mirror",
        R::Offset { .. } => "modify.offset",
        R::Generated { .. } | R::Modified { .. } => "ui.plugin",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actions::Registry;
    use crate::toolbar::RIBBON;

    /// Every built-in ribbon action has a drawn icon; only plugins and
    /// not-yet-drawn tools fall back to their initial.
    #[test]
    fn every_builtin_ribbon_action_has_an_icon() {
        let reg = Registry::default();
        for a in reg.actions().iter().filter(|a| RIBBON.contains(&a.group)) {
            assert!(has_icon(a.id.as_str()), "{} has no icon", a.id.as_str());
        }
        assert!(!has_icon("plugin.gear.insert"));
    }

    /// Browser rows (#326): every operation and recipe kind is drawn.
    #[test]
    fn every_operation_and_layer_toggle_has_an_icon() {
        for k in locus_doc::OperationKind::ALL {
            assert!(has_icon(operation(k)), "{k:?}");
        }
        for id in [
            "ui.eye",
            "ui.eye_off",
            "ui.lock",
            "ui.unlock",
            "ui.export",
            "ui.export_off",
            "ui.add",
            "ui.plugin",
        ] {
            assert!(has_icon(id), "{id}");
        }
        let mirror = locus_doc::RecipeKind::Mirror {
            axis: locus_doc::EntityId::new(1),
        };
        assert!(has_icon(recipe(&mirror)));
    }
}
