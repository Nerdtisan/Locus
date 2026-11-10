// SPDX-License-Identifier: AGPL-3.0-or-later
//! Canvas drawing with the egui painter (ADR 0001, #66): sketch entities,
//! construction dashes, the fully-constrained colour, constraint glyphs and
//! dimensions.
//!
//! Everything is culled against the visible world rectangle first, and curves
//! are flattened with a screen-space tolerance, so cost follows what is on
//! screen rather than the document size.

use crate::scene::{Annot, DimKind, Item, Scene, Shape};
use crate::settings::Units;
use crate::view::View;
use egui::{Align2, Color32, FontId, Painter, Pos2, Rect, Shape as EShape, Stroke, Vec2 as EVec2};
use locus_doc::ConstraintId;
use locus_geom::{Aabb, Arc, Circle, Cubic, Vec2};
use std::collections::BTreeSet;

/// Colours for one theme. Fusion convention: under-constrained geometry in
/// blue, fully constrained in the text colour, construction dashed.
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub background: Color32,
    pub free: Color32,
    pub constrained: Color32,
    pub construction: Color32,
    pub hover: Color32,
    pub selected: Color32,
    pub dim: Color32,
    pub dim_driven: Color32,
    pub glyph_fill: Color32,
    pub glyph_text: Color32,
    /// Constraints and geometry the solver reports as conflicting (#72).
    pub conflict: Color32,
}

impl Palette {
    pub fn new(dark: bool) -> Self {
        if dark {
            Self {
                background: Color32::from_gray(24),
                free: Color32::from_rgb(96, 165, 250),
                constrained: Color32::from_gray(235),
                construction: Color32::from_rgb(200, 120, 230),
                hover: Color32::from_rgb(255, 210, 90),
                selected: Color32::from_rgb(255, 150, 40),
                dim: Color32::from_rgb(120, 200, 140),
                dim_driven: Color32::from_gray(140),
                glyph_fill: Color32::from_rgb(44, 52, 66),
                glyph_text: Color32::from_rgb(190, 215, 255),
                conflict: Color32::from_rgb(255, 85, 85),
            }
        } else {
            Self {
                background: Color32::from_gray(250),
                free: Color32::from_rgb(25, 100, 200),
                constrained: Color32::from_gray(20),
                construction: Color32::from_rgb(150, 60, 190),
                hover: Color32::from_rgb(230, 160, 0),
                selected: Color32::from_rgb(230, 100, 0),
                dim: Color32::from_rgb(20, 130, 60),
                dim_driven: Color32::from_gray(120),
                glyph_fill: Color32::from_rgb(225, 232, 245),
                glyph_text: Color32::from_rgb(30, 60, 120),
                conflict: Color32::from_rgb(210, 30, 30),
            }
        }
    }
}

/// How an item is emphasised (#67).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Emphasis {
    None,
    #[allow(dead_code)] // set by picking (#67)
    Hover,
    #[allow(dead_code)] // set by selection (#67)
    Selected,
    /// Part of a conflicting or refused constraint set (#72).
    Conflict,
}

pub struct Frame<'a> {
    pub painter: &'a Painter,
    pub view: &'a View,
    pub rect: Rect,
    pub pal: Palette,
    pub units: Units,
}

const STROKE_W: f32 = 1.6;
const DASH: (f32, f32) = (7.0, 4.0);
/// Max distance, in points, between a flattened curve and the true curve.
const FLATNESS_PT: f64 = 0.2;
const MAX_SEGMENTS: usize = 2048;

fn intersects(b: &Aabb, lo: Vec2, hi: Vec2) -> bool {
    b.max.x >= lo.x && b.min.x <= hi.x && b.max.y >= lo.y && b.min.y <= hi.y
}

impl Frame<'_> {
    fn s(&self, p: Vec2) -> Pos2 {
        self.view.to_screen(p, self.rect)
    }

    /// World rectangle currently visible, padded by `pad_pt` points.
    fn visible(&self, pad_pt: f64) -> (Vec2, Vec2) {
        let pad = pad_pt / self.view.pt_per_mm;
        let lo = self.view.to_world(self.rect.left_bottom(), self.rect);
        let hi = self.view.to_world(self.rect.right_top(), self.rect);
        (lo - Vec2::new(pad, pad), hi + Vec2::new(pad, pad))
    }

    fn colour(&self, item: &Item, emphasis: Emphasis) -> Color32 {
        match emphasis {
            Emphasis::Conflict => self.pal.conflict,
            Emphasis::Selected => self.pal.selected,
            Emphasis::Hover => self.pal.hover,
            Emphasis::None if item.construction => self.pal.construction,
            Emphasis::None if item.constrained => self.pal.constrained,
            Emphasis::None => self.pal.free,
        }
    }

    /// Draw the geometry, calling `emphasis` for each visible item.
    pub fn draw_items(&self, scene: &Scene, emphasis: impl Fn(&Item) -> Emphasis) -> usize {
        let (lo, hi) = self.visible(4.0);
        let mut drawn = 0;
        for item in &scene.items {
            if !intersects(&item.bbox, lo, hi) {
                continue;
            }
            drawn += 1;
            let e = emphasis(item);
            let colour = self.colour(item, e);
            let width = if e == Emphasis::None {
                STROKE_W
            } else {
                STROKE_W + 1.2
            };
            let stroke = Stroke::new(width, colour);
            match item.shape {
                Shape::Point(p) if item.owned_point && e == Emphasis::None => {
                    // Endpoints and centres are by far the most numerous
                    // items; a square is two triangles, a circle is a fan.
                    let r = Rect::from_center_size(self.s(p), EVec2::splat(3.2));
                    self.painter.rect_filled(r, 0.0, colour);
                }
                Shape::Point(p) => {
                    self.painter.circle_filled(self.s(p), 3.4, colour);
                }
                Shape::Line(l) if !item.construction => {
                    self.painter
                        .line_segment([self.s(l.a), self.s(l.b)], stroke);
                }
                Shape::Line(l) => {
                    self.polyline(vec![self.s(l.a), self.s(l.b)], stroke, true);
                }
                Shape::Circle(c) => self.circle(c, stroke, item.construction),
                Shape::Arc(a) => self.arc(a, stroke, item.construction),
                Shape::Cubic(c) => self.cubic(&c, stroke, item.construction),
            }
        }
        drawn
    }

    fn polyline(&self, pts: Vec<Pos2>, stroke: Stroke, dashed: bool) {
        if dashed {
            self.painter
                .extend(EShape::dashed_line(&pts, stroke, DASH.0, DASH.1));
        } else {
            self.painter.add(EShape::line(pts, stroke));
        }
    }

    /// Segment count so the chord error stays under [`FLATNESS_PT`].
    fn segments(radius_pt: f64, sweep: f64) -> usize {
        let r = radius_pt.max(FLATNESS_PT * 2.0);
        let step = 2.0 * (1.0 - FLATNESS_PT / r).clamp(-1.0, 1.0).acos();
        ((sweep / step.max(1e-4)).ceil() as usize).clamp(4, MAX_SEGMENTS)
    }

    fn circle(&self, c: Circle, stroke: Stroke, dashed: bool) {
        let r_pt = c.radius * self.view.pt_per_mm;
        if r_pt < 0.5 {
            self.painter
                .circle_filled(self.s(c.center), stroke.width * 0.6, stroke.color);
            return;
        }
        if !dashed && r_pt < 4000.0 {
            self.painter
                .circle_stroke(self.s(c.center), r_pt as f32, stroke);
            return;
        }
        let n = Self::segments(r_pt, std::f64::consts::TAU);
        let pts = (0..=n)
            .map(|i| self.s(c.eval(std::f64::consts::TAU * i as f64 / n as f64)))
            .collect();
        self.polyline(pts, stroke, dashed);
    }

    fn arc(&self, a: Arc, stroke: Stroke, dashed: bool) {
        let r_pt = a.radius() * self.view.pt_per_mm;
        let sweep = a.sweep();
        if r_pt < 0.5 || sweep <= 0.0 {
            self.painter
                .circle_filled(self.s(a.start), stroke.width * 0.6, stroke.color);
            return;
        }
        let a0 = a.start_angle();
        let n = Self::segments(r_pt, sweep);
        let pts = (0..=n)
            .map(|i| {
                let ang = a0 + sweep * i as f64 / n as f64;
                self.s(a.center + Vec2::from_angle(ang) * a.radius())
            })
            .collect();
        self.polyline(pts, stroke, dashed);
    }

    /// A Bézier as a polyline whose chord error stays under [`FLATNESS_PT`].
    fn cubic(&self, c: &Cubic, stroke: Stroke, dashed: bool) {
        let tol = FLATNESS_PT / self.view.pt_per_mm;
        let pts = c.flatten(tol).into_iter().map(|p| self.s(p)).collect();
        self.polyline(pts, stroke, dashed);
    }

    /// Rubber-band shapes of the active drawing tool (#68).
    pub fn draw_preview(&self, shapes: &[Shape]) {
        let stroke = Stroke::new(STROKE_W, self.pal.hover);
        for shape in shapes {
            match *shape {
                Shape::Point(p) => {
                    self.painter.circle_filled(self.s(p), 3.4, stroke.color);
                }
                Shape::Line(l) => self.polyline(vec![self.s(l.a), self.s(l.b)], stroke, true),
                Shape::Circle(c) => self.circle(c, stroke, true),
                Shape::Arc(a) => self.arc(a, stroke, true),
                Shape::Cubic(c) => self.cubic(&c, stroke, true),
            }
        }
    }

    /// A small constraint glyph beside the cursor: what inference will add.
    pub fn draw_hint(&self, at: Vec2, label: &str) {
        let c = self.s(at) + EVec2::new(-14.0, -14.0);
        let r = Rect::from_center_size(c, EVec2::splat(16.0));
        self.painter.rect_filled(r, 3.0, self.pal.glyph_fill);
        self.painter.rect_stroke(
            r,
            3.0,
            Stroke::new(1.0, self.pal.hover),
            egui::StrokeKind::Inside,
        );
        self.painter.text(
            c,
            Align2::CENTER_CENTER,
            label,
            FontId::monospace(11.0),
            self.pal.glyph_text,
        );
    }

    /// Ring marking the existing point a tool will snap to.
    pub fn draw_snap(&self, p: Vec2) {
        self.painter
            .circle_stroke(self.s(p), 7.0, Stroke::new(1.6, self.pal.selected));
    }

    // ── Annotations ──────────────────────────────────────────────────────

    /// Draw glyphs and dimensions. Returns each dimension's label rectangle
    /// (screen points) so the canvas can hit-test it for editing (#70).
    pub fn draw_annotations(
        &self,
        scene: &Scene,
        flagged: &BTreeSet<ConstraintId>,
    ) -> Vec<(ConstraintId, Rect)> {
        let (lo, hi) = self.visible(80.0);
        let mut labels = Vec::new();
        for a in &scene.annots {
            match a {
                Annot::Glyph {
                    id,
                    at,
                    slot,
                    label,
                } => {
                    if intersects(&Aabb::from_point(*at), lo, hi) {
                        self.glyph(*at, *slot, label, flagged.contains(id));
                    }
                }
                Annot::Dim {
                    id,
                    kind,
                    value,
                    driven,
                } => {
                    let bad = flagged.contains(id);
                    if let Some(r) = self.dimension(*kind, *value, *driven, bad, lo, hi) {
                        labels.push((*id, r));
                    }
                }
            }
        }
        labels
    }

    fn label(&self, at: Pos2, text: &str, colour: Color32) -> Rect {
        let font = FontId::proportional(12.0);
        let galley = self.painter.layout_no_wrap(text.to_owned(), font, colour);
        let rect = Align2::CENTER_CENTER
            .anchor_size(at, galley.size())
            .expand(2.0);
        self.painter.rect_filled(rect, 3.0, self.pal.background);
        self.painter.galley(rect.shrink(2.0).min, galley, colour);
        rect
    }

    fn glyph(&self, at: Vec2, slot: u8, label: &str, bad: bool) {
        let ink = if bad {
            self.pal.conflict
        } else {
            self.pal.glyph_text
        };
        let c = self.s(at) + EVec2::new(10.0 + f32::from(slot) * 20.0, -12.0);
        let rect = Rect::from_center_size(c, EVec2::new(18.0, 15.0));
        self.painter.rect(
            rect,
            3.0,
            self.pal.glyph_fill,
            Stroke::new(if bad { 1.6 } else { 1.0 }, ink),
            egui::StrokeKind::Inside,
        );
        self.painter.text(
            c,
            Align2::CENTER_CENTER,
            label,
            FontId::monospace(10.0),
            ink,
        );
    }

    fn dimension(
        &self,
        kind: DimKind,
        value: f64,
        driven: bool,
        bad: bool,
        lo: Vec2,
        hi: Vec2,
    ) -> Option<Rect> {
        let colour = if bad {
            self.pal.conflict
        } else if driven {
            self.pal.dim_driven
        } else {
            self.pal.dim
        };
        let stroke = Stroke::new(1.0, colour);
        let units = self.units;
        let text = |v: f64| {
            if driven {
                format!("({})", units.format_len(v))
            } else {
                units.format_len(v)
            }
        };
        const OFF: f32 = 26.0;
        match kind {
            DimKind::Distance { a, b }
            | DimKind::Horizontal { a, b }
            | DimKind::Vertical { a, b } => {
                if !intersects(&Aabb::from_points(a, b), lo, hi) {
                    return None;
                }
                let (sa, sb) = (self.s(a), self.s(b));
                let (qa, qb) = match kind {
                    DimKind::Horizontal { .. } => {
                        let y = sa.y.min(sb.y) - OFF;
                        (Pos2::new(sa.x, y), Pos2::new(sb.x, y))
                    }
                    DimKind::Vertical { .. } => {
                        let x = sa.x.max(sb.x) + OFF;
                        (Pos2::new(x, sa.y), Pos2::new(x, sb.y))
                    }
                    _ => {
                        let d = sb - sa;
                        let n = if d.length() > 0.0 {
                            EVec2::new(d.y, -d.x).normalized() * OFF
                        } else {
                            EVec2::new(0.0, -OFF)
                        };
                        (sa + n, sb + n)
                    }
                };
                self.dimension_line(sa, sb, qa, qb, stroke);
                Some(self.label(qa + (qb - qa) * 0.5, &text(value), colour))
            }
            DimKind::Radius { center, radius } | DimKind::Diameter { center, radius } => {
                if !intersects(
                    &Aabb::from_point(center),
                    lo - Vec2::new(radius, radius),
                    hi + Vec2::new(radius, radius),
                ) {
                    return None;
                }
                let dia = matches!(kind, DimKind::Diameter { .. });
                let c = self.s(center);
                let r = (radius * self.view.pt_per_mm) as f32;
                let dir = EVec2::new(
                    std::f32::consts::FRAC_1_SQRT_2,
                    -std::f32::consts::FRAC_1_SQRT_2,
                );
                let tip = c + dir * r;
                let start = if dia { c - dir * r } else { c };
                self.painter.line_segment([start, tip + dir * 14.0], stroke);
                self.arrow(tip, -dir, colour);
                if dia {
                    self.arrow(start, dir, colour);
                }
                let prefix = if dia { "Ø " } else { "R " };
                Some(self.label(
                    tip + dir * 34.0,
                    &format!("{prefix}{}", text(value)),
                    colour,
                ))
            }
            DimKind::Angle { at } => {
                if !intersects(&Aabb::from_point(at), lo, hi) {
                    return None;
                }
                {
                    let deg = value.to_degrees();
                    let shown = if driven {
                        format!("({deg:.2}°)")
                    } else {
                        format!("{deg:.2}°")
                    };
                    Some(self.label(self.s(at) + EVec2::new(24.0, -18.0), &shown, colour))
                }
            }
        }
    }

    fn dimension_line(&self, pa: Pos2, pb: Pos2, qa: Pos2, qb: Pos2, stroke: Stroke) {
        for (p, q) in [(pa, qa), (pb, qb)] {
            let d = (q - p).normalized();
            let d = if d.is_finite() { d } else { EVec2::ZERO };
            self.painter
                .line_segment([p + d * 3.0, q + d * 4.0], stroke);
        }
        self.painter.line_segment([qa, qb], stroke);
        let along = (qb - qa).normalized();
        if along.is_finite() {
            self.arrow(qa, along, stroke.color);
            self.arrow(qb, -along, stroke.color);
        }
    }

    /// A small filled arrowhead at `tip`, pointing along `dir`'s opposite
    /// (`dir` runs from the tip back along the dimension line).
    fn arrow(&self, tip: Pos2, dir: EVec2, colour: Color32) {
        let n = EVec2::new(-dir.y, dir.x);
        self.painter.add(EShape::convex_polygon(
            vec![tip, tip + dir * 9.0 + n * 3.2, tip + dir * 9.0 - n * 3.2],
            colour,
            Stroke::NONE,
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flattening_stays_bounded_and_grows_with_radius() {
        let tau = std::f64::consts::TAU;
        let small = Frame::segments(10.0, tau);
        let big = Frame::segments(1000.0, tau);
        assert!(small >= 4 && big > small);
        assert_eq!(Frame::segments(1e9, tau), MAX_SEGMENTS);
        assert!(Frame::segments(0.0, 0.0) >= 4);
    }
}
