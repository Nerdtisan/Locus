// SPDX-License-Identifier: AGPL-3.0-or-later
//! Regions and booleans in the canvas (SPEC §6.3; #122, #124).
//!
//! Regions (the faces `locus-region` finds after each solve) are shaded on
//! hover and selected by clicking inside them, holes respected, alongside
//! entity selection. Booleans are made with a Fusion-style "combine": the
//! selected regions become the targets, the user clicks tool regions, sees
//! the result previewed, and applies it as one `AddBoolean` command. Recipe
//! results are drawn filled and outlined in their own colour, and the Regions
//! panel lists recipes to edit or delete.

use crate::hit::Mods;
use crate::state::AppState;
use crate::view::View;
use egui::{Color32, Mesh, Painter, Pos2, Rect, Stroke, Ui};
use locus_doc::{Boolean, BooleanId, Command, EntityId};
use locus_geom::Vec2;
use locus_region::{Area, BoolOp, Loop, Region, RegionKey};
use std::collections::BTreeSet;

/// Screen-space flatness for fills and outlines, in points.
const FLAT_PT: f64 = 0.25;

/// A combine in progress: targets fixed, tools being picked.
#[derive(Debug, Clone)]
pub(crate) struct Combine {
    pub op: BoolOp,
    pub targets: Vec<RegionKey>,
    pub tools: BTreeSet<RegionKey>,
    /// The recipe being edited, if this combine edits one.
    pub editing: Option<BooleanId>,
}

/// Region selection and boolean tool state. UI only, never in the document.
#[derive(Debug, Default)]
pub(crate) struct RegionUi {
    pub selected: BTreeSet<RegionKey>,
    pub hover: Option<RegionKey>,
    pub combine: Option<Combine>,
    /// The recipe highlighted from the panel.
    pub focus: Option<BooleanId>,
}

impl RegionUi {
    /// A click on a region (or empty space): the entity-selection rules.
    pub fn click(&mut self, hit: Option<&RegionKey>, mods: Mods) {
        match (hit, mods) {
            (None, m) if m.add || m.toggle => {}
            (None, _) => self.selected.clear(),
            (Some(k), m) if m.toggle => {
                if !self.selected.remove(k) {
                    self.selected.insert(k.clone());
                }
            }
            (Some(k), m) if m.add => {
                self.selected.insert(k.clone());
            }
            (Some(k), _) => {
                self.selected.clear();
                self.selected.insert(k.clone());
            }
        }
    }
}

/// The op's name for buttons and labels.
pub(crate) fn op_name(op: BoolOp) -> &'static str {
    match op {
        BoolOp::Union => "Union",
        BoolOp::Subtract => "Subtract",
        BoolOp::Intersect => "Intersect",
    }
}

fn with_alpha(c: Color32, a: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a)
}

/// Colours of the region overlays, by theme.
struct Tints {
    hover: Color32,
    selected: Color32,
    target: Color32,
    tool: Color32,
    result: Color32,
}

impl Tints {
    fn new(dark: bool) -> Self {
        let result = if dark {
            Color32::from_rgb(45, 200, 190)
        } else {
            Color32::from_rgb(0, 140, 135)
        };
        Self {
            hover: Color32::from_rgb(240, 180, 40),
            selected: Color32::from_rgb(240, 120, 30),
            target: Color32::from_rgb(70, 130, 240),
            tool: Color32::from_rgb(230, 70, 70),
            result,
        }
    }
}

/// Fill loops (non-zero rule, so holes stay open) with one scanline quad per
/// screen point row. egui only fills convex paths, and a sketch region is
/// rarely convex; this keeps the fill exact to a point without a
/// tessellation dependency.
fn fill(painter: &Painter, view: &View, rect: Rect, loops: &[Loop], colour: Color32) {
    fill_with(painter, view, rect, loops, &|_| colour, false);
}

/// [`fill`] with a colour per point (a gradient, #436): each span is cut
/// into pieces a few points wide, each in the colour at its middle.
fn fill_with(
    painter: &Painter,
    view: &View,
    rect: Rect,
    loops: &[Loop],
    colour_at: &dyn Fn(Vec2) -> Color32,
    varies: bool,
) {
    const PIECE_PT: f32 = 3.0;
    // Rows are found in world space (f64) against the exact line and arc
    // edges, so the fill stays exact at any zoom without flattening (#333,
    // #338); only the clipped spans are narrowed to screen points.
    let Some(bb) = loops
        .iter()
        .filter_map(Loop::bbox)
        .reduce(locus_geom::Aabb::union)
    else {
        return;
    };
    let top = (view.to_screen(Vec2::new(0.0, bb.max.y), rect).y).max(rect.top());
    let bottom = (view.to_screen(Vec2::new(0.0, bb.min.y), rect).y).min(rect.bottom());
    if top.is_nan() || bottom.is_nan() || top >= bottom {
        return;
    }
    let cx = f64::from(rect.center().x);
    let to_x = |wx: f64| (cx + (wx - view.center.x) * view.pt_per_mm) as f32;
    let mut mesh = Mesh::default();
    let mut xs: Vec<(f64, i32)> = Vec::new();
    let mut y = top.floor() + 0.5;
    while y < bottom {
        let wy = view.to_world(Pos2::new(rect.center().x, y), rect).y;
        xs.clear();
        for l in loops {
            for e in &l.edges {
                crossings(&e.seg, wy, &mut xs);
            }
        }
        xs.sort_by(|p, q| p.0.total_cmp(&q.0));
        let (mut w, mut start) = (0, 0.0);
        for &(x, dw) in &xs {
            if w == 0 {
                start = x;
            }
            w += dw;
            if w == 0 {
                let l = to_x(start).max(rect.left());
                let r = to_x(x).min(rect.right());
                if r > l && !varies {
                    mesh.add_colored_rect(
                        Rect::from_min_max(Pos2::new(l, y - 0.5), Pos2::new(r, y + 0.5)),
                        colour_at(Vec2::ZERO),
                    );
                } else if r > l {
                    let mut a = l;
                    while a < r {
                        let b = (a + PIECE_PT).min(r);
                        let mid = view.to_world(Pos2::new((a + b) / 2.0, y), rect);
                        mesh.add_colored_rect(
                            Rect::from_min_max(Pos2::new(a, y - 0.5), Pos2::new(b, y + 0.5)),
                            colour_at(mid),
                        );
                        a = b;
                    }
                }
            }
        }
        y += 1.0;
    }
    painter.add(mesh);
}

/// Where `seg` crosses the horizontal line at world `y`: x and the
/// direction it crosses (+1 upwards, -1 downwards), for non-zero filling.
/// Ends are half-open, as for polygon edges, so shared vertices count once.
fn crossings(seg: &locus_region::Seg, y: f64, out: &mut Vec<(f64, i32)>) {
    use locus_region::Seg;
    use std::f64::consts::TAU;
    match *seg {
        Seg::Line { a, b } => {
            if (a.y <= y) != (b.y <= y) {
                let x = a.x + (y - a.y) / (b.y - a.y) * (b.x - a.x);
                out.push((x, if b.y > a.y { 1 } else { -1 }));
            }
        }
        Seg::Arc {
            center,
            radius,
            start,
            sweep,
        } => {
            let dy = y - center.y;
            if dy.abs() >= radius || sweep == 0.0 {
                return;
            }
            let dx = (radius * radius - dy * dy).sqrt();
            for x in [center.x + dx, center.x - dx] {
                let a = dy.atan2(x - center.x);
                let rel = if sweep > 0.0 {
                    (a - start).rem_euclid(TAU)
                } else {
                    (start - a).rem_euclid(TAU)
                };
                if rel < sweep.abs() {
                    // y grows with the angle where cos > 0; reversed for a
                    // clockwise sweep.
                    let up = (x - center.x) * sweep;
                    if up != 0.0 {
                        out.push((x, if up > 0.0 { 1 } else { -1 }));
                    }
                }
            }
        }
    }
}

fn outline(painter: &Painter, view: &View, rect: Rect, loops: &[Loop], stroke: Stroke) {
    let sag = FLAT_PT / view.pt_per_mm;
    for l in loops {
        let mut pts: Vec<Pos2> = l
            .flatten(sag)
            .into_iter()
            .map(|p| view.to_screen(p, rect))
            .collect();
        if let Some(&first) = pts.first() {
            pts.push(first);
            painter.add(egui::Shape::line(pts, stroke));
        }
    }
}

fn region_loops(r: &Region) -> Vec<Loop> {
    let mut v = vec![r.outer.clone()];
    v.extend(r.holes.iter().cloned());
    v
}

impl AppState {
    pub(crate) fn regions(&self) -> &[Region] {
        self.doc.sketcher.regions()
    }

    /// The region under a world point. Regions are per layer (#457), so
    /// layers' regions may overlap: the active layer's wins, then the
    /// innermost.
    pub(crate) fn region_at(&self, w: Vec2) -> Option<RegionKey> {
        let doc = self.doc.sketcher.document();
        let active = self.doc.active_layer;
        self.regions()
            .iter()
            .filter(|r| r.contains(w))
            .min_by(|a, b| {
                let off = |r: &Region| locus_doc::regions::layer_of(doc, r) != Some(active);
                off(a)
                    .cmp(&off(b))
                    .then(a.area().abs().total_cmp(&b.area().abs()))
            })
            .map(|r| r.key.clone())
    }

    /// A region's fill as resolved for the canvas, Inspector and export
    /// alike (#457): stored, or kept from a region an edit split.
    pub(crate) fn resolved_fill(
        &self,
        k: &RegionKey,
    ) -> Option<locus_doc::region_fill::RegionFill> {
        let rs = self.regions();
        let i = rs.binary_search_by(|r| r.key.cmp(k)).ok()?;
        self.doc.sketcher.region_fills().get(i).cloned().flatten()
    }

    /// A region's gradient (#436), its frame on the page and the fill's
    /// opacity, when its fill is on and has one.
    pub(crate) fn region_gradient(
        &self,
        r: &Region,
    ) -> Option<(
        locus_doc::gradient::Gradient,
        locus_doc::gradient::Frame,
        f64,
    )> {
        let f = self.resolved_fill(&r.key)?;
        f.painted()?;
        let g = *f.gradient?;
        let frame = locus_doc::gradient::region_frame(r, &g)?;
        Some((g, frame, f.opacity))
    }

    /// A region's own fill (#451), at its opacity.
    pub(crate) fn region_fill(&self, r: &Region) -> Option<Color32> {
        let f = self.resolved_fill(&r.key)?;
        let [red, g, b] = f.painted()?;
        let a = (f.opacity.clamp(0.0, 1.0) * 255.0).round() as u8;
        Some(Color32::from_rgba_unmultiplied(red, g, b, a))
    }

    /// The colour a region's outline is drawn in: the layer colour of its
    /// first curve, the swatch a new fill starts from.
    pub(crate) fn region_layer_colour(&self, k: &RegionKey) -> [u8; 3] {
        let doc = self.doc.sketcher.document();
        self.region(k)
            .and_then(|r| r.outer.edges.first())
            .and_then(|e| doc.entity(EntityId::new(e.source)))
            .and_then(|e| doc.layer(e.layer))
            .map_or([128, 128, 128], |l| l.colour)
    }

    pub(crate) fn region(&self, k: &RegionKey) -> Option<&Region> {
        let rs = self.regions();
        rs.binary_search_by(|r| r.key.cmp(k)).ok().map(|i| &rs[i])
    }

    /// Drop selected or picked regions that no longer exist.
    pub(crate) fn retain_regions(&mut self) {
        let keys: BTreeSet<RegionKey> = self.regions().iter().map(|r| r.key.clone()).collect();
        let ui = &mut self.selection.regions;
        ui.selected.retain(|k| keys.contains(k));
        if ui.hover.as_ref().is_some_and(|k| !keys.contains(k)) {
            ui.hover = None;
        }
        if let Some(c) = &mut ui.combine {
            c.tools.retain(|k| keys.contains(k));
        }
        if ui
            .focus
            .is_some_and(|id| self.doc.sketcher.document().boolean(id).is_none())
        {
            ui.focus = None;
        }
    }

    /// A click on the canvas in combine mode: toggle a tool region.
    pub(crate) fn combine_click(&mut self, hit: Option<RegionKey>) {
        if let (Some(c), Some(k)) = (&mut self.selection.regions.combine, hit)
            && !c.tools.remove(&k)
        {
            c.tools.insert(k);
        }
    }

    /// Start a boolean from the selected regions. A union with no further
    /// picks applies straight away; subtract and intersect wait for tools.
    pub(crate) fn start_combine(&mut self, op: BoolOp) {
        let targets: Vec<RegionKey> = self.selection.regions.selected.iter().cloned().collect();
        if targets.is_empty() {
            self.doc.notice = Some("Select one or more regions first".to_owned());
            return;
        }
        if op == BoolOp::Union {
            self.apply_boolean(
                None,
                Boolean {
                    op,
                    targets,
                    tools: Vec::new(),
                },
            );
            return;
        }
        self.selection.regions.combine = Some(Combine {
            op,
            targets,
            tools: BTreeSet::new(),
            editing: None,
        });
        self.selection.regions.selected.clear();
    }

    /// Re-open a recipe in combine mode (Enter applies the edit).
    pub(crate) fn edit_recipe(&mut self, id: BooleanId) {
        if let Some(b) = self.doc.sketcher.document().boolean(id) {
            self.selection.regions.combine = Some(Combine {
                op: b.op,
                targets: b.targets.clone(),
                tools: b.tools.iter().cloned().collect(),
                editing: Some(id),
            });
        }
    }

    /// The recipe the current combine would create.
    pub(crate) fn combine_recipe(&self) -> Option<(Option<BooleanId>, Boolean)> {
        let c = self.selection.regions.combine.as_ref()?;
        Some((
            c.editing,
            Boolean {
                op: c.op,
                targets: c.targets.clone(),
                tools: c.tools.iter().cloned().collect(),
            },
        ))
    }

    pub(crate) fn finish_combine(&mut self) {
        if let Some((editing, b)) = self.combine_recipe() {
            if b.validate().is_err() {
                self.doc.notice = Some(format!(
                    "{}: click the regions to take away or keep",
                    op_name(b.op)
                ));
                return;
            }
            self.selection.regions.combine = None;
            self.apply_boolean(editing, b);
        }
    }

    pub(crate) fn cancel_combine(&mut self) {
        self.selection.regions.combine = None;
    }

    pub(crate) fn apply_boolean(&mut self, editing: Option<BooleanId>, boolean: Boolean) {
        let cmd = match editing {
            Some(id) => Command::EditBoolean { id, boolean },
            None => Command::AddBoolean { boolean },
        };
        match self.doc.sketcher.apply(&cmd) {
            Ok(applied) => {
                self.selection.regions.focus = applied.booleans.first().copied().or(editing);
                self.selection.regions.selected.clear();
                self.doc.notice = None;
                self.rebuild();
            }
            Err(e) => self.doc.notice = Some(e.to_string()),
        }
    }

    /// Region shading, the combine preview and recipe results, under the
    /// geometry. Hidden while dragging: regions update when the drag ends.
    pub(crate) fn draw_regions(&self, painter: &Painter, rect: Rect, dark: bool) {
        if self.tools.dragging {
            return;
        }
        let t = Tints::new(dark);
        let view = &self.view.camera;
        let shade = |k: &RegionKey, c: Color32| {
            if let Some(r) = self.region(k) {
                fill(painter, view, rect, &region_loops(r), c);
            }
        };
        // Style fills (#338): each region in its objects' fill colour, as it
        // exports, while Show fills is on.
        if self.view.display.fills {
            // In layer order (#462): earlier layers' fills underneath.
            let doc = self.doc.sketcher.document();
            let rank: std::collections::HashMap<_, _> = doc
                .layer_order()
                .into_iter()
                .enumerate()
                .map(|(i, l)| (l, i))
                .collect();
            let mut filled: Vec<&Region> = self.regions().iter().collect();
            filled.sort_by_key(|r| {
                locus_doc::regions::layer_of(doc, r).and_then(|l| rank.get(&l).copied())
            });
            for r in filled {
                // A gradient (#436), sampled per piece of each span.
                if let Some((g, frame, opacity)) = self.region_gradient(r) {
                    let at = |w: Vec2| {
                        let (c, a) = locus_doc::gradient::paint_at(&g, &frame, opacity, w);
                        Color32::from_rgba_unmultiplied(
                            c[0].round() as u8,
                            c[1].round() as u8,
                            c[2].round() as u8,
                            (a * 255.0).round() as u8,
                        )
                    };
                    fill_with(painter, view, rect, &region_loops(r), &at, true);
                } else if let Some(c) = self.region_fill(r) {
                    fill(painter, view, rect, &region_loops(r), c);
                }
            }
        }
        // Display ▾ Profiles (design §2.4, #328): every closed profile shaded
        // faintly, as Fusion shades sketch profiles.
        if self.view.display.profiles {
            for r in self.regions() {
                fill(
                    painter,
                    view,
                    rect,
                    &region_loops(r),
                    with_alpha(t.hover, 22),
                );
            }
        }
        for (id, res) in self.doc.sketcher.boolean_results() {
            if let Ok(area) = res {
                let focused = self.selection.regions.focus == Some(*id);
                fill(
                    painter,
                    view,
                    rect,
                    &area.loops,
                    with_alpha(t.result, if focused { 90 } else { 45 }),
                );
                let w = if focused { 3.5 } else { 2.5 };
                outline(painter, view, rect, &area.loops, Stroke::new(w, t.result));
            }
        }
        if let Some(c) = &self.selection.regions.combine {
            for k in &c.targets {
                shade(k, with_alpha(t.target, 70));
            }
            for k in &c.tools {
                shade(k, with_alpha(t.tool, 70));
            }
            if let Some((_, b)) = self.combine_recipe()
                && b.validate().is_ok()
                && let Ok(area) = b.evaluate(self.regions())
            {
                draw_preview(painter, view, rect, &area, t.result);
            }
        } else {
            for k in &self.selection.regions.selected {
                shade(k, with_alpha(t.selected, 80));
            }
        }
        if let Some(k) = &self.selection.regions.hover {
            shade(k, with_alpha(t.hover, 50));
        }
    }

    /// The Regions panel: boolean buttons, the combine in progress, and the
    /// recipe list with edit and delete.
    pub fn regions_ui(&mut self, ui: &mut Ui) {
        let n = self.regions().len();
        let selected = self.selection.regions.selected.len();
        ui.label(format!("{n} regions · {selected} selected"));
        if let Some(c) = &self.selection.regions.combine {
            let (op, picked) = (c.op, c.tools.len());
            let verb = if c.editing.is_some() {
                "Editing"
            } else {
                "New"
            };
            ui.separator();
            ui.strong(format!("{verb} {}", op_name(op)));
            ui.label(format!(
                "{} target region(s) in blue. Click tool regions ({picked} picked, red).",
                c.targets.len()
            ));
            ui.horizontal(|ui| {
                if ui.button("Apply  ⏎").clicked() {
                    self.finish_combine();
                }
                if ui.button("Cancel  Esc").clicked() {
                    self.cancel_combine();
                }
            });
        } else {
            ui.horizontal(|ui| {
                for op in [BoolOp::Union, BoolOp::Subtract, BoolOp::Intersect] {
                    let tip = match op {
                        BoolOp::Union => "Merge the selected regions into one outline",
                        BoolOp::Subtract => "Take tool regions away from the selected regions",
                        BoolOp::Intersect => {
                            "Keep where the selected regions meet the tool regions"
                        }
                    };
                    if ui
                        .add_enabled(selected > 0, egui::Button::new(op_name(op)))
                        .on_hover_text(tip)
                        .clicked()
                    {
                        self.start_combine(op);
                    }
                }
            });
        }
        ui.separator();
        ui.strong("Booleans");
        let rows: Vec<(BooleanId, String, Option<String>)> = self
            .doc
            .sketcher
            .document()
            .booleans()
            .iter()
            .map(|(id, b)| {
                let label = format!(
                    "{} · {} target(s), {} tool(s)",
                    op_name(b.op),
                    b.targets.len(),
                    b.tools.len()
                );
                let err = match self.doc.sketcher.boolean_results().get(id) {
                    Some(Err(e)) => Some(e.to_string()),
                    _ => None,
                };
                (*id, label, err)
            })
            .collect();
        if rows.is_empty() {
            ui.weak("None yet. Select regions, then Union, Subtract or Intersect.");
        }
        let mut want: Option<(BooleanId, bool)> = None;
        for (id, label, err) in rows {
            ui.horizontal(|ui| {
                let focused = self.selection.regions.focus == Some(id);
                if ui.selectable_label(focused, label).clicked() {
                    self.selection.regions.focus = if focused { None } else { Some(id) };
                }
                if ui.small_button("Edit").clicked() {
                    want = Some((id, false));
                }
                if ui.small_button("Delete").clicked() {
                    want = Some((id, true));
                }
            });
            if let Some(e) = err {
                ui.colored_label(ui.visuals().error_fg_color, format!("  ⚠ {e}"));
            }
        }
        match want {
            Some((id, true)) => {
                if let Err(e) = self.doc.sketcher.apply(&Command::RemoveBoolean { id }) {
                    self.doc.notice = Some(e.to_string());
                }
                self.rebuild();
            }
            Some((id, false)) => self.edit_recipe(id),
            None => {}
        }
    }

    /// Lines for the Properties panel when regions are selected.
    pub(crate) fn region_summary(&self) -> Option<String> {
        if self.selection.regions.selected.is_empty() {
            return None;
        }
        let area: f64 = self
            .selection
            .regions
            .selected
            .iter()
            .filter_map(|k| self.region(k))
            .map(Region::area)
            .sum();
        Some(format!(
            "{} region(s) · area {area:.3} mm²",
            self.selection.regions.selected.len()
        ))
    }
}

/// The combine result: dashed outline over a light fill.
fn draw_preview(painter: &Painter, view: &View, rect: Rect, area: &Area, colour: Color32) {
    fill(painter, view, rect, &area.loops, with_alpha(colour, 60));
    let sag = FLAT_PT / view.pt_per_mm;
    for l in &area.loops {
        let mut pts: Vec<Pos2> = l
            .flatten(sag)
            .into_iter()
            .map(|p| view.to_screen(p, rect))
            .collect();
        if let Some(&first) = pts.first() {
            pts.push(first);
            painter.extend(egui::Shape::dashed_line(
                &pts,
                Stroke::new(2.5, colour),
                8.0,
                5.0,
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(n: u64) -> RegionKey {
        RegionKey {
            bounds: vec![n],
            index: 0,
        }
    }

    /// Headless frames of the Regions panel.
    fn panel(c: &mut AppState, n: usize) {
        let ctx = egui::Context::default();
        for _ in 0..n {
            let _ = ctx.run_ui(egui::RawInput::default(), |ui| c.regions_ui(ui));
        }
    }

    #[test]
    #[allow(clippy::unwrap_used)]
    fn combine_flow_is_one_undo_step_and_previews() {
        let mut c = AppState::default();
        let rs: Vec<RegionKey> = c.regions().iter().map(|r| r.key.clone()).collect();
        assert!(rs.len() >= 2, "the demo sketch has regions");
        panel(&mut c, 1);
        // Subtract: target = the first region, tool = the second.
        c.selection.regions.click(Some(&rs[0]), Mods::default());
        c.start_combine(BoolOp::Subtract);
        assert!(c.selection.regions.combine.is_some() && c.selection.regions.selected.is_empty());
        c.finish_combine(); // no tools yet: refused with a hint, still open
        assert!(c.selection.regions.combine.is_some() && c.doc.notice.is_some());
        c.combine_click(Some(rs[1].clone()));
        panel(&mut c, 1);
        c.finish_combine();
        assert!(c.selection.regions.combine.is_none());
        let (id, b) = c
            .document()
            .booleans()
            .iter()
            .next()
            .map(|(i, b)| (*i, b.clone()))
            .unwrap();
        assert_eq!(
            (b.op, b.targets.len(), b.tools.len()),
            (BoolOp::Subtract, 1, 1)
        );
        assert_eq!(c.selection.regions.focus, Some(id));
        assert!(c.doc.sketcher.boolean_results().get(&id).unwrap().is_ok());
        panel(&mut c, 1);
        assert_eq!(c.undo().as_deref(), Some("Add boolean"));
        assert!(c.document().booleans().is_empty());
        assert_eq!(c.selection.regions.focus, None);
        // Union applies straight from the selection.
        c.selection.regions.click(Some(&rs[0]), Mods::default());
        c.selection.regions.click(
            Some(&rs[1]),
            Mods {
                add: true,
                toggle: false,
            },
        );
        c.start_combine(BoolOp::Union);
        assert_eq!(c.document().booleans().len(), 1);
        // Edit re-opens it; cancel leaves it unchanged.
        let id = *c.document().booleans().keys().next().unwrap();
        c.edit_recipe(id);
        c.cancel_combine();
        assert_eq!(c.document().boolean(id).unwrap().op, BoolOp::Union);
    }

    #[test]
    fn region_click_semantics_match_entities() {
        let mut ui = RegionUi::default();
        let none = Mods::default();
        let shift = Mods {
            add: true,
            toggle: false,
        };
        let ctrl = Mods {
            add: false,
            toggle: true,
        };
        ui.click(Some(&k(1)), none);
        ui.click(Some(&k(2)), shift);
        assert_eq!(ui.selected.len(), 2);
        ui.click(Some(&k(1)), ctrl);
        assert_eq!(ui.selected.iter().collect::<Vec<_>>(), vec![&k(2)]);
        ui.click(None, shift);
        assert_eq!(ui.selected.len(), 1);
        ui.click(None, none);
        assert!(ui.selected.is_empty());
    }

    /// #338: horizontal crossings are exact for lines and for arcs either
    /// way round, with the direction that makes non-zero filling work.
    #[test]
    fn crossings_are_exact_for_lines_and_arcs() {
        use locus_region::Seg;
        let mut xs = Vec::new();
        let ccw = Seg::Arc {
            center: Vec2::new(0.0, 0.0),
            radius: 2.0,
            start: 0.0,
            sweep: std::f64::consts::TAU,
        };
        crossings(&ccw, 1.0, &mut xs);
        xs.sort_by(|a, b| a.0.total_cmp(&b.0));
        let dx = 3f64.sqrt();
        assert_eq!(xs.len(), 2);
        assert!((xs[0].0 + dx).abs() < 1e-12 && xs[0].1 == -1);
        assert!((xs[1].0 - dx).abs() < 1e-12 && xs[1].1 == 1);
        // The upper half only, clockwise: the right crossing goes down.
        xs.clear();
        let cw = Seg::Arc {
            center: Vec2::new(0.0, 0.0),
            radius: 2.0,
            start: std::f64::consts::PI,
            sweep: -std::f64::consts::PI,
        };
        crossings(&cw, 1.0, &mut xs);
        xs.sort_by(|a, b| a.0.total_cmp(&b.0));
        assert_eq!(xs.iter().map(|x| x.1).collect::<Vec<_>>(), vec![1, -1]);
        xs.clear();
        crossings(&cw, -1.0, &mut xs);
        assert!(xs.is_empty(), "below the upper half");
        let line = Seg::Line {
            a: Vec2::new(5.0, 0.0),
            b: Vec2::new(5.0, 4.0),
        };
        xs.clear();
        crossings(&line, 2.0, &mut xs);
        assert_eq!(xs, vec![(5.0, 1)]);
    }

    /// #451: a region has its own fill and opacity; without one it has none.
    #[test]
    #[allow(clippy::expect_used)]
    fn a_region_has_its_own_fill() {
        use crate::settings::Units;
        use crate::tool::ToolEvent;
        use crate::tools::{Pick, Tool};
        let mut c = AppState::default();
        c.load(locus_doc::Document::new());
        c.set_tool(Tool::RectCorner);
        for p in [Vec2::ZERO, Vec2::new(20.0, 10.0)] {
            c.tool_event(ToolEvent::Click(Pick::free(p)), None, Units::Mm);
        }
        let r = c.regions().first().cloned().expect("a region");
        assert_eq!(c.region_fill(&r), None);
        assert!(c.apply_reporting(&Command::SetRegionFill {
            regions: vec![r.key.clone()],
            fill: Some(locus_doc::region_fill::RegionFill {
                fill: locus_doc::Fill::Colour([10, 200, 30]),
                opacity: 0.5,
                gradient: None,
            }),
        }));
        let r = c.regions().first().cloned().expect("a region");
        assert_eq!(
            c.region_fill(&r),
            Some(Color32::from_rgba_unmultiplied(10, 200, 30, 128))
        );
    }
}
