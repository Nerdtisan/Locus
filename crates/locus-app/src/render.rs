// SPDX-License-Identifier: AGPL-3.0-or-later
//! Canvas drawing with the egui painter (ADR 0001, #66): sketch entities,
//! construction dashes, the fully-constrained colour, constraint glyphs and
//! dimensions.
//!
//! Everything is culled against the visible world rectangle first, and curves
//! are flattened with a screen-space tolerance, so cost follows what is on
//! screen rather than the document size.

use crate::scene::{Annot, DimKind, Handle, Item, Scene, Shape, Symbol};
use crate::settings::Units;
use crate::view::View;
use egui::{Align2, Color32, FontId, Painter, Pos2, Rect, Shape as EShape, Stroke, Vec2 as EVec2};
use locus_doc::{ConstraintId, Edge};
use locus_geom::{Aabb, Arc, Circle, Cubic, Vec2};
use std::collections::BTreeSet;

/// A constraint symbol's badge size, its offset from the anchor and the
/// spacing of several symbols on one anchor (screen points; mockup: 16).
const GLYPH_SIZE: f32 = 16.0;
const GLYPH_GAP: f32 = 12.0;
const GLYPH_PITCH: f32 = 18.0;

/// Which annotations to draw (Display ▾, M5 design §2.4, #328).
#[derive(Clone, Copy, Debug)]
pub struct Show {
    pub dimensions: bool,
    pub symbols: bool,
}

/// Where the annotations landed this frame (screen points).
#[derive(Debug, Default)]
pub struct AnnotRects {
    pub dims: Vec<(ConstraintId, Rect)>,
    pub glyphs: Vec<(ConstraintId, Symbol, Rect)>,
}

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
    /// Outline of a constraint symbol's badge (#329).
    pub glyph_line: Color32,
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
                // The mockup's panel, line and muted ink (M5 design §2, #329).
                glyph_fill: Color32::from_rgb(42, 47, 56),
                glyph_line: Color32::from_rgb(62, 69, 82),
                glyph_text: Color32::from_rgb(154, 163, 179),
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
                glyph_fill: Color32::from_rgb(246, 247, 249),
                glyph_line: Color32::from_rgb(201, 207, 216),
                glyph_text: Color32::from_rgb(95, 104, 120),
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
    /// Not drawn (Display ▾ Points off, #328).
    Hidden,
}

pub struct Frame<'a> {
    pub painter: &'a Painter,
    pub view: &'a View,
    pub rect: Rect,
    pub pal: Palette,
    pub units: Units,
}

const STROKE_W: f32 = 1.6;

/// One canvas handle (#470, M6 design §12): `HANDLE_PX` across, filled
/// with `fill` and outlined in `outline` (the canvas colour) so it reads
/// over fills and strokes. Anchors are squares, control points circles.
pub fn handle_mark(painter: &Painter, at: Pos2, kind: Handle, fill: Color32, outline: Color32) {
    use crate::theme::{HANDLE_OUTLINE_PX, HANDLE_PX};
    let stroke = Stroke::new(HANDLE_OUTLINE_PX, outline);
    match kind {
        Handle::Anchor => {
            let r = Rect::from_center_size(at, EVec2::splat(HANDLE_PX));
            painter.rect(r, 0.0, fill, stroke, egui::StrokeKind::Outside);
        }
        Handle::Control => {
            painter.circle(at, HANDLE_PX / 2.0, fill, stroke);
        }
    }
}
const DASH: (f32, f32) = (7.0, 4.0);
/// Max distance, in points, between a flattened curve and the true curve.
const FLATNESS_PT: f64 = 0.2;
const MAX_SEGMENTS: usize = 2048;

fn intersects(b: &Aabb, lo: Vec2, hi: Vec2) -> bool {
    b.max.x >= lo.x && b.min.x <= hi.x && b.max.y >= lo.y && b.min.y <= hi.y
}

/// How far outside the canvas geometry is still drawn, in points: enough
/// for stroke joins and dashes, small enough that every coordinate given
/// to the f32 painter stays near the canvas (#333).
const CLIP_PAD_PT: f64 = 64.0;
/// Above this screen radius a circle or arc is sampled only where it can
/// be seen, and a Bézier is split until its pieces are this small.
const HUGE_PT: f64 = 20_000.0;

/// The part of segment `a`–`b` inside the box `lo`–`hi` (Liang–Barsky), in
/// world mm (f64), or `None` when it misses the box.
fn clip(a: Vec2, b: Vec2, lo: Vec2, hi: Vec2) -> Option<(Vec2, Vec2)> {
    let d = b - a;
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    for (p, q) in [
        (-d.x, a.x - lo.x),
        (d.x, hi.x - a.x),
        (-d.y, a.y - lo.y),
        (d.y, hi.y - a.y),
    ] {
        if p == 0.0 {
            if q < 0.0 {
                return None;
            }
        } else {
            let r = q / p;
            if p < 0.0 {
                t0 = t0.max(r);
            } else {
                t1 = t1.min(r);
            }
            if t0 > t1 {
                return None;
            }
        }
    }
    Some((a + d * t0, a + d * t1))
}

/// The angle intervals of an arc from `a0` sweeping `sweep` (radians, ≥ 0)
/// that fall inside the window `w0`..`w1` (any turn).
fn arc_window(a0: f64, sweep: f64, w0: f64, w1: f64) -> Vec<(f64, f64)> {
    use std::f64::consts::TAU;
    let shift = ((a0 - w0) / TAU).floor() * TAU;
    let mut out = Vec::new();
    for k in -1..=2 {
        let off = shift + f64::from(k) * TAU;
        let lo = a0.max(w0 + off);
        let hi = (a0 + sweep).min(w1 + off);
        if hi > lo {
            out.push((lo, hi));
        }
    }
    out
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
            Emphasis::Hidden => Color32::TRANSPARENT,
            Emphasis::None if item.construction => self.pal.construction,
            Emphasis::None if item.constrained => self.pal.constrained,
            Emphasis::None => self.pal.free,
        }
    }

    /// Draw the geometry, calling `emphasis` for each visible item. Bézier
    /// handles are left to [`Frame::draw_handles`], so they go on top of
    /// their guide lines.
    pub fn draw_items(&self, scene: &Scene, emphasis: impl Fn(&Item) -> Emphasis) -> usize {
        let (lo, hi) = self.visible(4.0);
        let mut drawn = 0;
        for item in &scene.items {
            if item.handle.is_some() || !intersects(&item.bbox, lo, hi) {
                continue;
            }
            let e = emphasis(item);
            if e == Emphasis::Hidden {
                continue;
            }
            drawn += 1;
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
                Shape::Line(l) => self.world_polyline(&[l.a, l.b], stroke, item.construction),
                Shape::Circle(c) => self.circle(c, stroke, item.construction),
                Shape::Arc(a) => self.arc(a, stroke, item.construction),
                Shape::Cubic(c) => self.cubic(&c, stroke, item.construction),
            }
        }
        drawn
    }

    /// Filled triangles given in world mm (the stroke preview, #428); those
    /// wholly outside the view are skipped.
    pub fn draw_triangles(&self, tris: &[[Vec2; 3]], colour: Color32) {
        let (lo, hi) = self.visible(4.0);
        let mut mesh = egui::Mesh::default();
        for t in tris {
            let all = |f: &dyn Fn(&Vec2) -> bool| t.iter().all(f);
            if all(&|p| p.x < lo.x)
                || all(&|p| p.x > hi.x)
                || all(&|p| p.y < lo.y)
                || all(&|p| p.y > hi.y)
            {
                continue;
            }
            let Ok(i) = u32::try_from(mesh.vertices.len()) else {
                break;
            };
            for p in t {
                mesh.colored_vertex(self.s(*p), colour);
            }
            mesh.add_triangle(i, i + 1, i + 2);
        }
        self.painter.add(mesh);
    }

    /// A polyline given in world mm: each segment is clipped to the padded
    /// view in f64 first, and the visible runs are drawn (#333).
    fn world_polyline(&self, pts: &[Vec2], stroke: Stroke, dashed: bool) {
        let (lo, hi) = self.visible(CLIP_PAD_PT);
        let mut run: Vec<Pos2> = Vec::new();
        let mut last: Option<Vec2> = None;
        for w in pts.windows(2) {
            match clip(w[0], w[1], lo, hi) {
                Some((p, q)) => {
                    if last != Some(p) && run.len() > 1 {
                        self.polyline(std::mem::take(&mut run), stroke, dashed);
                    }
                    if last != Some(p) {
                        run.clear();
                        run.push(self.s(p));
                    }
                    run.push(self.s(q));
                    last = Some(q);
                }
                None => {
                    if run.len() > 1 {
                        self.polyline(std::mem::take(&mut run), stroke, dashed);
                    }
                    run.clear();
                    last = None;
                }
            }
        }
        if run.len() > 1 {
            self.polyline(run, stroke, dashed);
        }
    }

    /// The angle window (radians) of a huge circle of `r` mm about `c` that
    /// can be seen, or `None` when none of it can. Small ones are seen whole.
    fn visible_angles(&self, c: Vec2, r: f64) -> Option<(f64, f64)> {
        use std::f64::consts::PI;
        if r * self.view.pt_per_mm < HUGE_PT {
            return Some((0.0, 2.0 * PI));
        }
        let (lo, hi) = self.visible(CLIP_PAD_PT);
        let half_diag = (hi - lo).length() / 2.0;
        let mid = (lo + hi) * 0.5;
        let d = mid - c;
        let dist = d.length();
        if (dist - r).abs() > half_diag {
            return None;
        }
        let around = d.y.atan2(d.x);
        // Twice the angle the view subtends, so the run reaches past it.
        let h = (2.0 * half_diag / r).min(PI);
        Some((around - h, around + h))
    }

    /// Sample `c + r·(cos, sin)` over `a0`..`a1` finely enough on screen.
    fn arc_points(&self, c: Vec2, r: f64, a0: f64, a1: f64) -> Vec<Vec2> {
        let n = Self::segments(r * self.view.pt_per_mm, a1 - a0);
        (0..=n)
            .map(|i| c + Vec2::from_angle(a0 + (a1 - a0) * i as f64 / n as f64) * r)
            .collect()
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
        // Huge on screen (#333): only the part that can be seen.
        if let Some((a0, a1)) = self.visible_angles(c.center, c.radius) {
            let pts = self.arc_points(c.center, c.radius, a0, a1);
            self.world_polyline(&pts, stroke, dashed);
        }
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
        let Some((w0, w1)) = self.visible_angles(a.center, a.radius()) else {
            return;
        };
        for (lo, hi) in arc_window(a0, sweep, w0, w1) {
            let pts = self.arc_points(a.center, a.radius(), lo, hi);
            self.world_polyline(&pts, stroke, dashed);
        }
    }

    /// A Bézier as a polyline whose chord error stays under [`FLATNESS_PT`].
    fn cubic(&self, c: &Cubic, stroke: Stroke, dashed: bool) {
        let tol = FLATNESS_PT / self.view.pt_per_mm;
        let (lo, hi) = self.visible(CLIP_PAD_PT);
        let mut pieces = Vec::new();
        self.visible_pieces(*c, lo, hi, 0, &mut pieces);
        for piece in pieces {
            self.world_polyline(&piece.flatten(tol), stroke, dashed);
        }
    }

    /// Split a Bézier that is huge on screen until its pieces are small,
    /// keeping only those near the view (#333): flattening a whole curve
    /// at a nanometre tolerance would take millions of points.
    fn visible_pieces(&self, c: Cubic, lo: Vec2, hi: Vec2, depth: u32, out: &mut Vec<Cubic>) {
        let b = c.bbox();
        if !intersects(&b, lo, hi) {
            return;
        }
        let size_pt = (b.max - b.min).length() * self.view.pt_per_mm;
        if size_pt < HUGE_PT || depth >= 48 {
            out.push(c);
            return;
        }
        let (l, r) = c.split(0.5);
        self.visible_pieces(l, lo, hi, depth + 1, out);
        self.visible_pieces(r, lo, hi, depth + 1, out);
    }

    /// Rubber-band shapes of the active drawing tool (#68).
    pub fn draw_preview(&self, shapes: &[Shape]) {
        let stroke = Stroke::new(STROKE_W, self.pal.hover);
        for shape in shapes {
            match *shape {
                Shape::Point(p) => {
                    self.painter.circle_filled(self.s(p), 3.4, stroke.color);
                }
                Shape::Line(l) => self.world_polyline(&[l.a, l.b], stroke, true),
                Shape::Circle(c) => self.circle(c, stroke, true),
                Shape::Arc(a) => self.arc(a, stroke, true),
                Shape::Cubic(c) => self.cubic(&c, stroke, true),
            }
        }
    }

    /// A small constraint symbol beside the cursor: what inference will add
    /// (#69). Drawn with the CONSTRAIN group's icon, like the canvas glyphs.
    pub fn draw_hint(&self, at: Vec2, symbol: Symbol) {
        let c = self.s(at) + EVec2::new(-14.0, -14.0);
        let r = Rect::from_center_size(c, EVec2::splat(16.0));
        self.painter.rect_filled(r, 3.0, self.pal.glyph_fill);
        self.painter.rect_stroke(
            r,
            3.0,
            Stroke::new(1.0, self.pal.hover),
            egui::StrokeKind::Inside,
        );
        crate::views::icons::paint(
            self.painter,
            r.shrink(2.0),
            symbol.action,
            symbol.name,
            self.pal.glyph_text,
        );
    }

    /// The shown Béziers' anchors (squares) and control points (circles)
    /// at the theme's fixed handle size (#470, M6 design §12), with a ring
    /// in the selection colour on the hovered one.
    pub fn draw_handles(&self, scene: &Scene, emphasis: impl Fn(&Item) -> Emphasis) {
        let pad = f64::from(crate::theme::HANDLE_HIT_PX);
        let (lo, hi) = self.visible(pad);
        for item in &scene.items {
            let (Some(kind), Shape::Point(p)) = (item.handle, item.shape) else {
                continue;
            };
            if !intersects(&item.bbox, lo, hi) {
                continue;
            }
            let e = emphasis(item);
            if e == Emphasis::Hidden {
                continue;
            }
            let at = self.s(p);
            handle_mark(
                self.painter,
                at,
                kind,
                self.colour(item, e),
                self.pal.background,
            );
            if e == Emphasis::Hover {
                let r = crate::theme::HANDLE_PX / 2.0 + 3.0;
                self.painter
                    .circle_stroke(at, r, Stroke::new(1.5, self.pal.selected));
            }
        }
    }

    /// Lines from a selected Bézier's end points to their handles (#160).
    pub fn draw_handle_guides(&self, guides: &[(Vec2, Vec2)]) {
        let stroke = Stroke::new(1.0, self.pal.construction);
        for (a, b) in guides {
            self.world_polyline(&[*a, *b], stroke, false);
        }
    }

    /// Ring marking the existing point a tool will snap to.
    pub fn draw_snap(&self, p: Vec2) {
        self.painter
            .circle_stroke(self.s(p), 7.0, Stroke::new(1.6, self.pal.selected));
    }

    // ── Annotations ──────────────────────────────────────────────────────

    /// Draw constraint symbols and dimensions. Returns where each dimension
    /// label and each symbol landed (screen points) so the canvas can
    /// hit-test them: editing a dimension (#70), naming or deleting a
    /// constraint from its symbol (#329). `hot` is the symbol under the
    /// pointer, drawn highlighted.
    pub fn draw_annotations(
        &self,
        scene: &Scene,
        flagged: &BTreeSet<ConstraintId>,
        hot: Option<ConstraintId>,
        show: Show,
    ) -> AnnotRects {
        let (lo, hi) = self.visible(80.0);
        let mut out = AnnotRects::default();
        for a in &scene.annots {
            match a {
                Annot::Glyph {
                    id,
                    at,
                    slot,
                    symbol,
                } => {
                    if show.symbols && intersects(&Aabb::from_point(*at), lo, hi) {
                        let r =
                            self.glyph(*at, *slot, *symbol, flagged.contains(id), hot == Some(*id));
                        out.glyphs.push((*id, *symbol, r));
                    }
                }
                Annot::Dim {
                    id,
                    kind,
                    value,
                    driven,
                    edge,
                } => {
                    let bad = flagged.contains(id);
                    if !show.dimensions {
                        continue;
                    }
                    if let Some(r) = self.dimension(*kind, *value, *driven, *edge, bad, lo, hi) {
                        out.dims.push((*id, r));
                    }
                }
            }
        }
        out
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

    /// One constraint symbol (#329): a 16-point badge in the mockup's panel
    /// colours with the CONSTRAIN group's icon, beside its anchor.
    fn glyph(&self, at: Vec2, slot: u8, symbol: Symbol, bad: bool, hot: bool) -> Rect {
        let ink = if bad {
            self.pal.conflict
        } else if hot {
            self.pal.hover
        } else {
            self.pal.glyph_text
        };
        let line = if bad || hot { ink } else { self.pal.glyph_line };
        let c = self.s(at) + EVec2::new(GLYPH_GAP + f32::from(slot) * GLYPH_PITCH, -GLYPH_GAP);
        let rect = Rect::from_center_size(c, EVec2::splat(GLYPH_SIZE));
        self.painter.rect(
            rect,
            3.0,
            self.pal.glyph_fill,
            Stroke::new(if bad || hot { 1.6 } else { 1.0 }, line),
            egui::StrokeKind::Inside,
        );
        crate::views::icons::paint(
            self.painter,
            rect.shrink(2.0),
            symbol.action,
            symbol.name,
            ink,
        );
        rect
    }

    #[allow(clippy::too_many_arguments)]
    fn dimension(
        &self,
        kind: DimKind,
        value: f64,
        driven: bool,
        edge: Edge,
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
        let suffix = match edge {
            Edge::Centre => "",
            Edge::Inside => " in",
            Edge::Outside => " out",
        };
        let text = |v: f64| {
            if driven {
                format!("({}){suffix}", units.format_len(v))
            } else {
                format!("{}{suffix}", units.format_len(v))
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
    #[allow(clippy::expect_used)] // a miss here should fail loudly
    fn clipping_keeps_the_inside_and_drops_the_outside() {
        let (lo, hi) = (Vec2::new(0.0, 0.0), Vec2::new(10.0, 10.0));
        let (a, b) = clip(Vec2::new(-1e12, 5.0), Vec2::new(1e12, 5.0), lo, hi).expect("crosses");
        assert!((a.x - 0.0).abs() < 1e-3 && (b.x - 10.0).abs() < 1e-3 && a.y == 5.0);
        assert!(clip(Vec2::new(-5.0, -5.0), Vec2::new(-1.0, 20.0), lo, hi).is_none());
        let inside = clip(Vec2::new(1.0, 1.0), Vec2::new(2.0, 3.0), lo, hi);
        assert_eq!(inside, Some((Vec2::new(1.0, 1.0), Vec2::new(2.0, 3.0))));
        // An arc's sweep cut to a window, across the ±π seam.
        let w = arc_window(3.0, 1.0, 3.5, 4.5);
        assert_eq!(w.len(), 1);
        assert!((w[0].0 - 3.5).abs() < 1e-12 && (w[0].1 - 4.0).abs() < 1e-12);
        let w = arc_window(-0.5, 1.0, 6.0, 7.0);
        assert!(!w.is_empty(), "the window wraps onto the sweep");
        assert!(arc_window(0.0, 1.0, 2.0, 3.0).is_empty());
    }

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
