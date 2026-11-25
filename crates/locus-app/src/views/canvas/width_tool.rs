// SPDX-License-Identifier: AGPL-3.0-or-later
//! The Width tool (#426, M6 design §4, ADR 0019): shape a stroke's width
//! profile on the canvas, as Illustrator's width tool does.
//!
//! It works on the selected curves that share one style and join end to end
//! (the first chain, as export joins them). Each width point shows two
//! diamond handles, one on each edge of the stroke, joined across the path:
//! drag across to change the width (both sides, or only the grabbed one with
//! Alt), drag along to move the point, click an edge to add a point. Delete
//! removes the focused point, Tab cycles focus, the arrow keys nudge it.
//! Nothing is committed during a drag: the preview is built from the
//! dragged profile, and the release is one `SetStyle` (SPEC §11).
//!
//! The pure edits are free functions on [`WidthProfile`], so the rules
//! (end points stay at 0 and 1, points stay in order, factors in range, a
//! hand edit drops the Pressure seed) are tested without a canvas.

use super::strokes::segs;
use crate::render::Frame;
use crate::state::AppState;
use crate::theme::{HANDLE_HIT_PX, HANDLE_OUTLINE_PX, HANDLE_PX};
use egui::{Color32, PointerButton, Pos2, Rect};
use locus_doc::{Command, EntityId, Geom, LineStyle, StylePatch};
use locus_geom::Vec2;
use locus_stroke::{
    Chain, Limits, MAX_SIDE_FACTOR, MAX_WIDTH_POINTS, PathMap, WidthPoint, WidthProfile, chains,
    mesh,
};

/// World tolerance of the preview mesh, mm (as the Stroke width view).
const TOL: f64 = 0.005;
/// Closest two width points may sit, as a fraction of the path: keeps the
/// smoothstep between them well defined and the points distinct to grab.
const MIN_GAP: f64 = 0.002;
/// A drag shorter than this (points) hasn't chosen its direction yet.
const AXIS_PX: f64 = 4.0;
/// How far from the stroke's edge a click still adds a point, in points.
const EDGE_PX: f64 = 6.0;
/// A nudge with the arrow keys: 1 % of the path, 10 % with Shift.
const NUDGE: f64 = 0.01;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Side {
    Left,
    Right,
}

/// What a drag of a handle changes, decided once it has moved a few points.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Axis {
    /// Across the path: the width.
    Across,
    /// Along the path: where the point sits.
    Along,
}

// ───────────────────────── pure profile edits ─────────────────────────

/// The plain width: two end points at 1 / 1. What an unstyled stroke shows.
pub(crate) fn plain() -> WidthProfile {
    WidthProfile {
        points: vec![WidthPoint::even(0.0, 1.0), WidthPoint::even(1.0, 1.0)],
        seed: None,
    }
}

/// An edit's result if it is a valid profile, with the Pressure seed
/// dropped (a hand edit makes it a custom profile).
fn finish(mut p: WidthProfile) -> Option<WidthProfile> {
    p.seed = None;
    p.validate().ok()?;
    Some(p)
}

/// Point `i`'s sides set to `left` / `right` factors (`None` keeps a side),
/// clamped to the allowed range.
pub(crate) fn set_sides(
    p: &WidthProfile,
    i: usize,
    left: Option<f64>,
    right: Option<f64>,
) -> Option<WidthProfile> {
    let mut q = p.clone();
    let pt = q.points.get_mut(i)?;
    let fit = |f: f64| f.clamp(0.0, MAX_SIDE_FACTOR);
    if let Some(l) = left.filter(|f| f.is_finite()) {
        pt.left = fit(l);
    }
    if let Some(r) = right.filter(|f| f.is_finite()) {
        pt.right = fit(r);
    }
    finish(q)
}

/// A drag across the path to signed distance `dist` (mm, + left of travel)
/// from point `i`: both sides take the distance, or with `one_side` only
/// `side` does. `half` is half the stroke width.
pub(crate) fn dragged_across(
    p: &WidthProfile,
    i: usize,
    side: Side,
    dist: f64,
    half: f64,
    one_side: bool,
) -> Option<WidthProfile> {
    if half <= 0.0 || !dist.is_finite() {
        return None;
    }
    let f = dist.abs() / half;
    match (one_side, side) {
        (false, _) => set_sides(p, i, Some(f), Some(f)),
        (true, Side::Left) => set_sides(p, i, Some(f), None),
        (true, Side::Right) => set_sides(p, i, None, Some(f)),
    }
}

/// Point `i` moved to `t`, kept strictly between its neighbours. The end
/// points never move (§4).
pub(crate) fn moved_to(p: &WidthProfile, i: usize, t: f64) -> Option<WidthProfile> {
    let n = p.points.len();
    if i == 0 || i + 1 >= n || !t.is_finite() {
        return None;
    }
    let lo = p.points.get(i - 1)?.t + MIN_GAP;
    let hi = p.points.get(i + 1)?.t - MIN_GAP;
    if lo > hi {
        return None;
    }
    let mut q = p.clone();
    q.points.get_mut(i)?.t = t.clamp(lo, hi);
    finish(q)
}

/// A point added at `u` with the widths the profile has there. Returns the
/// new profile and the new point's index; `None` when `u` is on or too near
/// an existing point or the profile is full.
pub(crate) fn inserted_at(p: &WidthProfile, u: f64) -> Option<(WidthProfile, usize)> {
    if p.points.len() >= MAX_WIDTH_POINTS || !(u > 0.0 && u < 1.0) {
        return None;
    }
    let k = p.points.iter().position(|pt| pt.t > u)?;
    let (a, b) = (p.points.get(k.checked_sub(1)?)?, p.points.get(k)?);
    if u - a.t < MIN_GAP || b.t - u < MIN_GAP {
        return None;
    }
    let (left, right) = p.at(u);
    let mut q = p.clone();
    q.points.insert(k, WidthPoint::new(u, left, right));
    Some((finish(q)?, k))
}

/// Point `i` removed. The two end points stay (§4), as do at least two points.
pub(crate) fn removed(p: &WidthProfile, i: usize) -> Option<WidthProfile> {
    if i == 0 || i + 1 >= p.points.len() {
        return None;
    }
    let mut q = p.clone();
    q.points.remove(i);
    finish(q)
}

/// Which way a drag goes, from how far it has moved along and across the
/// path (points). `None` until it has moved a few points. An end point only
/// ever changes width.
pub(crate) fn axis_of(along: f64, across: f64, end_point: bool) -> Option<Axis> {
    if along.hypot(across) < AXIS_PX {
        return None;
    }
    Some(if !end_point && along.abs() > across.abs() {
        Axis::Along
    } else {
        Axis::Across
    })
}

/// The hover read-out for a point at `t` with side factors `left`, `right`
/// of a stroke `width` mm wide: `t 42 % · W 1.20 mm (L 0.60 / R 0.60)`.
pub(crate) fn readout(t: f64, left: f64, right: f64, width: f64) -> String {
    let (l, r) = (left * width / 2.0, right * width / 2.0);
    format!(
        "t {:.0} % \u{b7} W {:.2} mm (L {l:.2} / R {r:.2})",
        t * 100.0,
        l + r
    )
}

// ───────────────────────────── canvas state ─────────────────────────────

/// What the tool edits: the chain, its width, and the profile it has now.
pub(crate) struct WidthTarget {
    /// The scene and selection this was built for; a change rebuilds it.
    stamp: u64,
    selection: Vec<EntityId>,
    /// The entities of the chain: a commit styles every one.
    ids: Vec<EntityId>,
    map: PathMap,
    chain: Chain,
    /// Stroke width, mm; the factors are × half of it.
    width: f64,
    line: LineStyle,
    /// The committed profile (plain when the object has none).
    profile: WidthProfile,
    /// Its outline, world triangles.
    tris: Vec<[Vec2; 3]>,
}

/// A handle being dragged.
struct Drag {
    index: usize,
    side: Side,
    /// Where the press landed (world mm), to find the direction.
    press: Vec2,
    axis: Option<Axis>,
    base: WidthProfile,
}

/// The Width tool's state: its target, the focused point and the drag.
#[derive(Default)]
pub(crate) struct WidthDraft {
    target: Option<WidthTarget>,
    focus: Option<usize>,
    drag: Option<Drag>,
    /// The dragged profile and its outline, until release.
    live: Option<(WidthProfile, Vec<[Vec2; 3]>)>,
    /// A just-committed profile that is uniform (extra points all at the
    /// plain width). The document stores such a profile as none (§4), so
    /// the tool remembers its points, for this scene and selection only,
    /// and they show until the first real width change.
    local: Option<(u64, Vec<EntityId>, WidthProfile)>,
}

impl WidthDraft {
    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }

    pub(crate) fn set_target(&mut self, t: Option<WidthTarget>) {
        self.target = t;
    }

    /// The profile shown now: the drag's, or the committed one.
    fn shown(&self) -> Option<(&WidthProfile, &[[Vec2; 3]])> {
        match (&self.live, &self.target) {
            (Some((p, t)), _) => Some((p, t)),
            (None, Some(t)) => Some((&t.profile, &t.tris)),
            _ => None,
        }
    }
}

/// The triangles of `chain` stroked at `width` with `profile`: its outline,
/// whatever the line style's dashes and extra strokes (the tool edits the
/// one profile). Empty past the limits.
fn outline_tris(
    chain: &Chain,
    width: f64,
    line: &LineStyle,
    profile: &WidthProfile,
) -> Vec<[Vec2; 3]> {
    let mut style = line.clone();
    style.profile = Some(profile.clone());
    style.dash = None;
    style.extra.clear();
    let mut tris = Vec::new();
    if mesh(chain, width, &style, TOL, &Limits::default(), &mut tris).is_err() {
        tris.clear();
    }
    tris
}

/// Does `item` (a curve's pieces) lie on `chain`? `chains` keeps each
/// piece's end points, possibly swapped, so matching them is enough.
fn on_chain(chain: &Chain, item: &[locus_region::Seg]) -> bool {
    let same = |a: Vec2, b: Vec2| a.distance(b) < 1e-6;
    item.iter().all(|s| {
        chain.segs.iter().any(|c| {
            (same(s.start(), c.start()) && same(s.end(), c.end()))
                || (same(s.start(), c.end()) && same(s.end(), c.start()))
        })
    })
}

/// A side's handle position: `half` × `factor` from the path point `p`
/// along the normal `n` (left of travel), on the left or right.
fn handle_pos(p: Vec2, n: Vec2, factor: f64, half: f64, side: Side) -> Vec2 {
    match side {
        Side::Left => p + n * (factor * half),
        Side::Right => p - n * (factor * half),
    }
}

impl AppState {
    /// The chain the tool would edit for the current selection, or why not.
    pub(crate) fn width_build(&self) -> Result<WidthTarget, String> {
        let none = || "Width tool: select a stroked curve first".to_owned();
        let doc = self.doc.sketcher.document();
        let mut construction = false;
        // The first usable curve fixes the style; the others must share it.
        let mut key: Option<(locus_doc::LayerId, f64, LineStyle)> = None;
        let mut items: Vec<(EntityId, Vec<locus_region::Seg>)> = Vec::new();
        for id in self.selection.items.iter() {
            let Some(e) = doc.entity(id) else { continue };
            if !matches!(
                e.geom,
                Geom::Line { .. } | Geom::Arc { .. } | Geom::Circle { .. } | Geom::Bezier { .. }
            ) {
                continue;
            }
            if e.construction {
                construction = true;
                continue;
            }
            let mut w = doc.stroke_width(id);
            if w <= 0.0 {
                // A layer that draws no stroke still keeps the profile.
                w = e.style.width;
            }
            if w <= 0.0 {
                continue;
            }
            let this = (e.layer, w, e.style.line.clone());
            if key.as_ref().is_some_and(|k| *k != this) {
                continue;
            }
            for it in self
                .doc
                .scene
                .items
                .iter()
                .filter(|i| i.id == id && !i.text)
            {
                let s = segs(&it.shape);
                if !s.is_empty() {
                    key.get_or_insert_with(|| this.clone());
                    items.push((id, s));
                }
            }
        }
        let Some((_, width, line)) = key else {
            return Err(if construction {
                "Construction lines aren't styled".to_owned()
            } else {
                none()
            });
        };
        let Some(chain) = chains(items.iter().map(|(_, s)| s.clone()).collect())
            .into_iter()
            .next()
        else {
            return Err(none());
        };
        let mut ids: Vec<EntityId> = items
            .iter()
            .filter(|(_, s)| on_chain(&chain, s))
            .map(|(id, _)| *id)
            .collect();
        ids.dedup();
        let map = PathMap::new(&chain);
        if map.length() <= 0.0 {
            return Err(none());
        }
        let selection: Vec<EntityId> = self.selection.items.iter().collect();
        let mut profile = line.profile.clone().unwrap_or_else(plain);
        if profile.is_uniform()
            && let Some((_, _, kept)) = self
                .tools
                .width
                .local
                .as_ref()
                .filter(|(stamp, sel, _)| *stamp == self.doc.scene_stamp && *sel == selection)
        {
            profile = kept.clone();
        }
        let tris = outline_tris(&chain, width, &line, &profile);
        Ok(WidthTarget {
            stamp: self.doc.scene_stamp,
            selection,
            ids,
            map,
            chain,
            width,
            line,
            profile,
            tris,
        })
    }

    /// Keep the target current: rebuilt when the scene or the selection
    /// changed (an undo, a commit), and the tool left when nothing suitable
    /// remains.
    fn width_sync(&mut self) {
        let sel: Vec<EntityId> = self.selection.items.iter().collect();
        let fresh = self
            .tools
            .width
            .target
            .as_ref()
            .is_some_and(|t| t.stamp == self.doc.scene_stamp && t.selection == sel);
        if fresh {
            return;
        }
        match self.width_build() {
            Ok(t) => {
                let n = t.profile.points.len();
                self.tools.width.target = Some(t);
                if self.tools.width.focus.is_some_and(|f| f >= n) {
                    self.tools.width.focus = None;
                }
            }
            Err(why) => {
                self.set_tool(crate::tools::Tool::Select);
                self.doc.notice = Some(why);
            }
        }
    }

    /// Commit `p` as one undo step on every entity of the chain.
    fn width_commit(&mut self, p: WidthProfile) -> bool {
        let Some(t) = self.tools.width.target.as_ref() else {
            return false;
        };
        if let Err(e) = p.validate() {
            self.doc.notice = Some(format!("Width profile: {e}"));
            return false;
        }
        if p == t.profile {
            return false;
        }
        let cmd = Command::SetStyle {
            entities: t.ids.clone(),
            patch: StylePatch {
                profile: Some(Some(p.clone())),
                ..StylePatch::default()
            },
        };
        let ok = self.apply_reporting(&cmd);
        if ok {
            let selection = self.selection.items.iter().collect();
            self.tools.width.local = p
                .is_uniform()
                .then_some((self.doc.scene_stamp, selection, p));
        }
        ok
    }

    /// Handle positions of `profile`: point index, side, world position.
    fn width_handles(&self, profile: &WidthProfile) -> Vec<(usize, Side, Vec2)> {
        let Some(t) = self.tools.width.target.as_ref() else {
            return Vec::new();
        };
        let half = t.width / 2.0;
        let mut out = Vec::new();
        for (i, pt) in profile.points.iter().enumerate() {
            let Some((p, tan)) = t.map.at(pt.t) else {
                continue;
            };
            let n = tan.perp();
            out.push((i, Side::Left, handle_pos(p, n, pt.left, half, Side::Left)));
            out.push((
                i,
                Side::Right,
                handle_pos(p, n, pt.right, half, Side::Right),
            ));
        }
        out
    }

    /// The handle under screen point `at`, nearest first.
    fn width_handle_at(&self, at: Pos2, rect: Rect) -> Option<(usize, Side)> {
        let (shown, _) = self.tools.width.shown()?;
        let cam = self.view.camera;
        let reach = HANDLE_HIT_PX / 2.0;
        self.width_handles(shown)
            .into_iter()
            .map(|(i, s, w)| (i, s, cam.to_screen(w, rect).distance(at)))
            .filter(|(_, _, d)| *d <= reach)
            .min_by(|a, b| a.2.total_cmp(&b.2))
            .map(|(i, s, _)| (i, s))
    }

    /// Start dragging a handle (press at world point `press`).
    pub(crate) fn width_begin_drag(&mut self, index: usize, side: Side, press: Vec2) {
        let Some(t) = self.tools.width.target.as_ref() else {
            return;
        };
        let base = self
            .tools
            .width
            .shown()
            .map_or_else(|| t.profile.clone(), |(p, _)| p.clone());
        self.tools.width.focus = Some(index);
        self.tools.width.drag = Some(Drag {
            index,
            side,
            press,
            axis: None,
            base,
        });
    }

    /// The drag at world point `w`: the preview follows; nothing commits.
    /// `alt` changes only the grabbed side.
    pub(crate) fn width_drag_to(&mut self, w: Vec2, alt: bool) {
        let per_mm = self.view.camera.pt_per_mm;
        let (Some(d), Some(t)) = (
            self.tools.width.drag.as_ref(),
            self.tools.width.target.as_ref(),
        ) else {
            return;
        };
        let end = d.index == 0 || d.index + 1 >= d.base.points.len();
        let (tan, at) = d
            .base
            .points
            .get(d.index)
            .and_then(|pt| t.map.at(pt.t))
            .map_or((Vec2::ZERO, Vec2::ZERO), |(p, tan)| (tan, p));
        let axis = d.axis.or_else(|| {
            let delta = w - d.press;
            axis_of(delta.dot(tan) * per_mm, delta.dot(tan.perp()) * per_mm, end)
        });
        let next = match axis {
            None => None,
            Some(Axis::Across) => dragged_across(
                &d.base,
                d.index,
                d.side,
                (w - at).dot(tan.perp()),
                t.width / 2.0,
                alt,
            ),
            Some(Axis::Along) => t
                .map
                .project(w)
                .and_then(|(u, _)| moved_to(&d.base, d.index, u)),
        };
        let (line, width, chain) = (t.line.clone(), t.width, t.chain.clone());
        if let Some(d) = self.tools.width.drag.as_mut() {
            d.axis = axis;
        }
        if let Some(p) = next
            && self.tools.width.live.as_ref().is_none_or(|(l, _)| *l != p)
        {
            let tris = outline_tris(&chain, width, &line, &p);
            self.tools.width.live = Some((p, tris));
        }
    }

    /// Release: commit the dragged profile as one undo step; nothing if it
    /// didn't change.
    pub(crate) fn width_release(&mut self) -> bool {
        self.tools.width.drag = None;
        match self.tools.width.live.take() {
            Some((p, _)) => self.width_commit(p),
            None => false,
        }
    }

    /// A click at world point `w`: add a point if it is on the stroke's
    /// edge (the new point becomes the focus).
    pub(crate) fn width_click(&mut self, w: Vec2) -> bool {
        let per_mm = self.view.camera.pt_per_mm;
        let Some(t) = self.tools.width.target.as_ref() else {
            return false;
        };
        let Some((u, d)) = t.map.project(w) else {
            return false;
        };
        let (l, r) = t.profile.at(u);
        let edge = if d >= 0.0 { l } else { r } * t.width / 2.0;
        if d.abs() > edge + EDGE_PX / per_mm {
            self.tools.width.focus = None;
            return false;
        }
        match inserted_at(&t.profile, u) {
            Some((p, k)) => {
                let ok = self.width_commit(p);
                if ok {
                    self.tools.width.focus = Some(k);
                }
                ok
            }
            None if t.profile.points.len() >= MAX_WIDTH_POINTS => {
                self.doc.notice = Some(format!("A profile has at most {MAX_WIDTH_POINTS} points"));
                false
            }
            None => false,
        }
    }

    /// Delete / Backspace in the tool: remove the focused point (the end
    /// points stay). Never deletes the curves.
    pub(crate) fn width_delete_focus(&mut self) -> bool {
        let (Some(t), Some(i)) = (self.tools.width.target.as_ref(), self.tools.width.focus) else {
            return false;
        };
        match removed(&t.profile, i) {
            Some(p) => {
                self.tools.width.focus = None;
                self.width_commit(p)
            }
            None => {
                self.doc.notice = Some("The end points of a width profile stay".to_owned());
                false
            }
        }
    }

    /// Tab: focus the next (or with `back` the previous) point.
    fn width_cycle_focus(&mut self, back: bool) {
        let n = self
            .tools
            .width
            .target
            .as_ref()
            .map_or(0, |t| t.profile.points.len());
        if n == 0 {
            return;
        }
        let at = self
            .tools
            .width
            .focus
            .unwrap_or(if back { 0 } else { n - 1 });
        self.tools.width.focus = Some(if back { (at + n - 1) % n } else { (at + 1) % n });
    }

    /// An arrow key: move the focused point by `dt` of the path.
    fn width_nudge(&mut self, dt: f64) -> bool {
        let (Some(t), Some(i)) = (self.tools.width.target.as_ref(), self.tools.width.focus) else {
            return false;
        };
        let Some(now) = t.profile.points.get(i).map(|p| p.t) else {
            return false;
        };
        match moved_to(&t.profile, i, now + dt) {
            Some(p) => self.width_commit(p),
            None => false,
        }
    }

    /// Pointer and key input while the Width tool is active.
    pub(crate) fn width_input(&mut self, ui: &egui::Ui, resp: &egui::Response, rect: Rect) {
        use egui::{Key, Modifiers};
        self.width_sync();
        if self.tools.width.target.is_none() {
            return;
        }
        let cam = self.view.camera;
        let ctx = ui.ctx();
        // Esc cancels a drag, then leaves the tool.
        if resp.hovered() && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            if self.tools.width.drag.take().is_some() {
                self.tools.width.live = None;
            } else {
                self.set_tool(crate::tools::Tool::Select);
            }
            return;
        }
        if self.tools.width.drag.is_some() {
            ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
            if let Some(p) = ui.input(|i| i.pointer.latest_pos()) {
                let alt = ui.input(|i| i.modifiers.alt);
                self.width_drag_to(cam.to_world(p, rect), alt);
            }
            if !ui.input(|i| i.pointer.primary_down()) {
                self.width_release();
                self.width_sync();
            }
            return;
        }
        if resp.hovered() {
            let (tab, back) = ctx.input_mut(|i| {
                (
                    i.consume_key(Modifiers::SHIFT, Key::Tab)
                        || i.consume_key(Modifiers::NONE, Key::Tab),
                    i.modifiers.shift,
                )
            });
            if tab {
                self.width_cycle_focus(back);
            }
            if self.tools.width.focus.is_some() {
                let step = ctx.input_mut(|i| {
                    let (shift, none) = (Modifiers::SHIFT, Modifiers::NONE);
                    [
                        (shift, Key::ArrowLeft, -10.0 * NUDGE),
                        (shift, Key::ArrowRight, 10.0 * NUDGE),
                        (none, Key::ArrowLeft, -NUDGE),
                        (none, Key::ArrowRight, NUDGE),
                    ]
                    .into_iter()
                    .find(|(m, k, _)| i.consume_key(*m, *k))
                    .map(|(_, _, dt)| dt)
                });
                if let Some(dt) = step {
                    self.width_nudge(dt);
                    self.width_sync();
                }
            }
        }
        let over = resp.hover_pos().and_then(|p| self.width_handle_at(p, rect));
        if over.is_some() {
            ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        // Taken on the press, so the click that ends a drag adds no point.
        if resp.hovered()
            && ui.input(|i| i.pointer.primary_pressed())
            && let Some(press) = ui.input(|i| i.pointer.press_origin())
            && let Some((i, side)) = self.width_handle_at(press, rect)
        {
            self.width_begin_drag(i, side, cam.to_world(press, rect));
            return;
        }
        if resp.clicked_by(PointerButton::Primary)
            && let Some(p) = resp.interact_pointer_pos()
        {
            self.width_click(cam.to_world(p, rect));
            self.width_sync();
        }
    }

    /// The edited stroke's outline, the width points' handles and the hover
    /// read-out. `hover` is the pointer, if over the canvas.
    pub(crate) fn draw_width(&self, frame: &Frame<'_>, hover: Option<Pos2>) {
        let (Some(t), Some((shown, tris))) =
            (self.tools.width.target.as_ref(), self.tools.width.shown())
        else {
            return;
        };
        let pal = frame.pal;
        // Whatever the Stroke width view says: the tool shows its stroke.
        frame.draw_triangles(tris, pal.selected.gamma_multiply(0.3));
        let s = |w: Vec2| frame.view.to_screen(w, frame.rect);
        let hs = self.width_handles(shown);
        let line = egui::Stroke::new(1.0, pal.selected);
        for pair in hs.chunks(2) {
            if let [(_, _, l), (_, _, r)] = pair {
                frame.painter.line_segment([s(*l), s(*r)], line);
            }
        }
        let focus = self.tools.width.focus;
        let r = HANDLE_PX * 0.7;
        for (i, _, w) in &hs {
            let c = s(*w);
            let pts = vec![
                c + egui::vec2(0.0, -r),
                c + egui::vec2(r, 0.0),
                c + egui::vec2(0.0, r),
                c + egui::vec2(-r, 0.0),
            ];
            let fill = if focus == Some(*i) {
                Color32::WHITE
            } else {
                pal.selected
            };
            frame.painter.add(egui::Shape::convex_polygon(
                pts,
                fill,
                egui::Stroke::new(HANDLE_OUTLINE_PX, pal.background),
            ));
        }
        // The read-out: the dragged point's, or where the pointer is over
        // the stroke.
        let text = if let Some(d) = self.tools.width.drag.as_ref() {
            shown
                .points
                .get(d.index)
                .map(|p| readout(p.t, p.left, p.right, t.width))
        } else {
            let w = hover.map(|p| frame.view.to_world(p, frame.rect));
            w.and_then(|w| t.map.project(w)).and_then(|(u, d)| {
                let (l, r) = shown.at(u);
                let edge = l.max(r) * t.width / 2.0;
                (d.abs() <= edge + EDGE_PX / frame.view.pt_per_mm)
                    .then(|| readout(u, l, r, t.width))
            })
        };
        if let (Some(text), Some(at)) = (text, hover) {
            let galley = frame.painter.layout_no_wrap(
                text,
                egui::FontId::proportional(12.0),
                pal.glyph_text,
            );
            let pos = at + egui::vec2(14.0, 16.0);
            frame.painter.rect_filled(
                Rect::from_min_size(pos, galley.size()).expand(3.0),
                3.0,
                pal.glyph_fill,
            );
            frame.painter.galley(pos, galley, pal.glyph_text);
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // fixtures built in the test
mod tests {
    use super::*;
    use locus_doc::{Document, Editor, Style};

    fn p(points: &[(f64, f64, f64)]) -> WidthProfile {
        WidthProfile {
            points: points
                .iter()
                .map(|&(t, l, r)| WidthPoint::new(t, l, r))
                .collect(),
            seed: Some(7),
        }
    }

    fn three() -> WidthProfile {
        p(&[(0.0, 1.0, 1.0), (0.5, 2.0, 2.0), (1.0, 1.0, 1.0)])
    }

    #[test]
    fn a_drag_across_sets_both_sides_or_only_the_grabbed_one() {
        // Half width 0.5 mm: a handle 1 mm out is factor 2.
        let both = dragged_across(&three(), 1, Side::Left, 1.0, 0.5, false).unwrap();
        assert_eq!(both.points[1], WidthPoint::new(0.5, 2.0, 2.0));
        // Either side of the path counts: the distance is absolute.
        let both = dragged_across(&three(), 1, Side::Right, -0.25, 0.5, false).unwrap();
        assert_eq!(both.points[1], WidthPoint::new(0.5, 0.5, 0.5));
        let left = dragged_across(&three(), 1, Side::Left, 0.25, 0.5, true).unwrap();
        assert_eq!(left.points[1], WidthPoint::new(0.5, 0.5, 2.0));
        let right = dragged_across(&three(), 1, Side::Right, 0.25, 0.5, true).unwrap();
        assert_eq!(right.points[1], WidthPoint::new(0.5, 2.0, 0.5));
        // Clamped to the factor range, never negative.
        let huge = dragged_across(&three(), 1, Side::Left, 1e9, 0.5, false).unwrap();
        assert_eq!(huge.points[1].left, MAX_SIDE_FACTOR);
        assert!(dragged_across(&three(), 1, Side::Left, f64::NAN, 0.5, false).is_none());
        assert!(dragged_across(&three(), 1, Side::Left, 1.0, 0.0, false).is_none());
    }

    #[test]
    fn any_edit_clears_the_pressure_seed() {
        assert_eq!(three().seed, Some(7));
        assert_eq!(set_sides(&three(), 0, Some(0.0), None).unwrap().seed, None);
        assert_eq!(moved_to(&three(), 1, 0.4).unwrap().seed, None);
        assert_eq!(inserted_at(&three(), 0.25).unwrap().0.seed, None);
        let four = inserted_at(&three(), 0.25).unwrap().0;
        assert_eq!(removed(&four, 1).unwrap().seed, None);
    }

    #[test]
    fn the_end_points_never_move_along_or_go() {
        let q = three();
        assert!(moved_to(&q, 0, 0.1).is_none());
        assert!(moved_to(&q, 2, 0.9).is_none());
        assert!(removed(&q, 0).is_none());
        assert!(removed(&q, 2).is_none());
        // Their width still changes.
        let w = set_sides(&q, 0, Some(0.0), Some(0.0)).unwrap();
        assert_eq!((w.points[0].t, w.points[2].t), (0.0, 1.0));
        assert_eq!(w.points[0].left, 0.0);
    }

    #[test]
    fn a_moved_point_stays_strictly_between_its_neighbours() {
        let q = inserted_at(&three(), 0.25).unwrap().0;
        // Points at 0, 0.25, 0.5, 1: the one at 0.25 can't reach 0.5 or 0.
        let up = moved_to(&q, 1, 0.9).unwrap();
        assert!(up.points[1].t < 0.5 && up.points[1].t > 0.49);
        let down = moved_to(&q, 1, -3.0).unwrap();
        assert!(down.points[1].t > 0.0 && down.points[1].t < 0.01);
        assert!(down.validate().is_ok() && up.validate().is_ok());
        assert!(moved_to(&q, 1, f64::NAN).is_none());
    }

    #[test]
    fn adding_a_point_keeps_the_width_there_and_the_profile_valid() {
        let q = three();
        let (with, k) = inserted_at(&q, 0.25).unwrap();
        assert_eq!(k, 1);
        assert_eq!(with.points.len(), 4);
        let (l, r) = q.at(0.25);
        assert_eq!(with.points[1], WidthPoint::new(0.25, l, r));
        assert!(with.validate().is_ok());
        // The curve through the new point is unchanged there.
        assert_eq!(with.at(0.25), q.at(0.25));
        // On an existing point, too close to one, or at the ends: refused.
        assert!(inserted_at(&q, 0.5).is_none());
        assert!(inserted_at(&q, 0.5 + MIN_GAP / 2.0).is_none());
        assert!(inserted_at(&q, 0.0).is_none());
        assert!(inserted_at(&q, 1.0).is_none());
        assert!(inserted_at(&q, f64::NAN).is_none());
    }

    #[test]
    fn a_profile_holds_at_most_the_maximum_points() {
        let mut q = plain();
        for k in 1..(MAX_WIDTH_POINTS - 1) {
            let u = k as f64 / (MAX_WIDTH_POINTS - 1) as f64;
            q = inserted_at(&q, u).unwrap().0;
        }
        assert_eq!(q.points.len(), MAX_WIDTH_POINTS);
        assert!(inserted_at(&q, 0.5 / (MAX_WIDTH_POINTS - 1) as f64).is_none());
    }

    #[test]
    fn removing_a_point_keeps_the_ends() {
        let q = removed(&three(), 1).unwrap();
        assert_eq!(q.points.len(), 2);
        assert!(removed(&q, 1).is_none(), "two points stay");
    }

    #[test]
    fn a_drag_picks_its_direction_after_a_few_points() {
        assert_eq!(axis_of(1.0, 1.0, false), None);
        assert_eq!(axis_of(8.0, 2.0, false), Some(Axis::Along));
        assert_eq!(axis_of(2.0, -8.0, false), Some(Axis::Across));
        // An end point only changes width, whichever way it is pulled.
        assert_eq!(axis_of(8.0, 1.0, true), Some(Axis::Across));
    }

    #[test]
    fn the_readout_names_position_and_widths() {
        assert_eq!(
            readout(0.42, 1.0, 1.0, 1.2),
            "t 42 % \u{b7} W 1.20 mm (L 0.60 / R 0.60)"
        );
        assert_eq!(
            readout(1.0, 0.0, 2.0, 1.0),
            "t 100 % \u{b7} W 1.00 mm (L 0.00 / R 1.00)"
        );
    }

    // ── on the canvas state ──

    /// A 20 mm line, 1 mm wide, selected, with the Width tool on.
    fn line_tool() -> (AppState, EntityId) {
        let mut c = AppState::default();
        let mut ed = Editor::new(Document::new());
        let layer = Document::default_layer();
        let mut pt = |x: f64| {
            ed.apply(&Command::AddEntity {
                layer,
                construction: false,
                style: Style::default(),
                geom: Geom::Point {
                    pos: Vec2::new(x, 0.0),
                },
            })
            .unwrap()
            .entities[0]
        };
        let (a, b) = (pt(0.0), pt(20.0));
        let style = Style {
            width: 1.0,
            ..Style::default()
        };
        let id = ed
            .apply(&Command::AddEntity {
                layer,
                construction: false,
                style,
                geom: Geom::Line { start: a, end: b },
            })
            .unwrap()
            .entities[0];
        c.load(ed.document().clone());
        // Zoomed in, so a 1 mm drag is well past the direction threshold.
        c.view.camera.pt_per_mm = 50.0;
        c.selection.items.set_all([id]);
        c.set_modify(crate::modify_tool::Modify::Width);
        (c, id)
    }

    fn profile_of(c: &AppState, id: EntityId) -> Option<WidthProfile> {
        c.doc
            .sketcher
            .document()
            .entity(id)
            .unwrap()
            .style
            .line
            .profile
            .clone()
    }

    #[test]
    fn the_tool_needs_a_stroked_curve() {
        let mut c = AppState::default();
        c.set_modify(crate::modify_tool::Modify::Width);
        assert_eq!(c.modify(), None, "nothing selected");
        assert_eq!(
            c.doc.notice.as_deref(),
            Some("Width tool: select a stroked curve first")
        );
        let (mut c, id) = line_tool();
        assert_eq!(c.modify(), Some(crate::modify_tool::Modify::Width));
        // A construction curve is refused.
        c.set_tool(crate::tools::Tool::Select);
        assert!(c.apply_reporting(&Command::SetConstruction {
            entities: vec![id],
            construction: true,
        }));
        c.selection.items.set_all([id]);
        c.set_modify(crate::modify_tool::Modify::Width);
        assert_eq!(c.modify(), None);
        assert_eq!(
            c.doc.notice.as_deref(),
            Some("Construction lines aren't styled")
        );
    }

    #[test]
    fn an_unstyled_stroke_shows_the_two_end_points() {
        let (c, _) = line_tool();
        let t = c.tools.width.target.as_ref().unwrap();
        assert_eq!(t.profile, plain());
        let hs = c.width_handles(&t.profile);
        assert_eq!(hs.len(), 4, "two points, two handles each");
        // Left of travel (+x) is +y: 0.5 mm out at both ends.
        let at = |i, s| hs.iter().find(|h| h.0 == i && h.1 == s).unwrap().2;
        assert!(at(0, Side::Left).distance(Vec2::new(0.0, 0.5)) < 1e-9);
        assert!(at(1, Side::Right).distance(Vec2::new(20.0, -0.5)) < 1e-9);
        assert!(!t.tris.is_empty());
    }

    #[test]
    fn a_width_drag_previews_then_commits_one_undo_step() {
        let (mut c, id) = line_tool();
        // Add a middle point by clicking the edge at x = 10.
        assert!(c.width_click(Vec2::new(10.0, 0.5)));
        c.width_sync();
        let mid = c.tools.width.focus.unwrap();
        assert_eq!(mid, 1);
        assert_eq!(profile_of(&c, id), None, "a uniform profile stores nothing");
        let before = c.doc.sketcher.document().clone();
        // Drag it across to 1.5 mm out: factor 3.
        c.width_begin_drag(1, Side::Left, Vec2::new(10.0, 0.5));
        c.width_drag_to(Vec2::new(10.0, 1.5), false);
        assert_eq!(c.doc.sketcher.document(), &before, "nothing committed yet");
        let live = c.tools.width.live.as_ref().unwrap();
        assert_eq!(live.0.points[1].left, 3.0);
        assert!(!live.1.is_empty(), "a preview outline");
        assert!(c.width_release());
        c.width_sync();
        let got = profile_of(&c, id).unwrap();
        assert_eq!(got.points[1], WidthPoint::new(0.5, 3.0, 3.0));
        assert!(c.tools.width.drag.is_none() && c.tools.width.live.is_none());
        c.undo();
        assert_eq!(profile_of(&c, id), None, "one step");
    }

    #[test]
    fn alt_changes_one_side_and_a_sideways_drag_moves_the_point() {
        let (mut c, id) = line_tool();
        assert!(c.width_click(Vec2::new(10.0, 0.5)));
        c.width_sync();
        c.width_begin_drag(1, Side::Left, Vec2::new(10.0, 0.5));
        c.width_drag_to(Vec2::new(10.0, 1.0), true);
        assert!(c.width_release());
        c.width_sync();
        assert_eq!(
            profile_of(&c, id).unwrap().points[1],
            WidthPoint::new(0.5, 2.0, 1.0)
        );
        // Along the path: the point moves, its widths stay.
        c.width_begin_drag(1, Side::Left, Vec2::new(10.0, 1.0));
        c.width_drag_to(Vec2::new(15.0, 1.0), false);
        assert!(c.width_release());
        c.width_sync();
        let got = profile_of(&c, id).unwrap().points[1];
        assert!((got.t - 0.75).abs() < 1e-6, "{got:?}");
        assert_eq!((got.left, got.right), (2.0, 1.0));
        // A drag that goes nowhere commits nothing.
        let before = c.doc.sketcher.document().clone();
        c.width_begin_drag(1, Side::Left, Vec2::new(15.0, 1.0));
        c.width_drag_to(Vec2::new(15.0, 1.0), false);
        assert!(!c.width_release());
        assert_eq!(c.doc.sketcher.document(), &before);
    }

    #[test]
    fn delete_removes_the_focused_point_but_never_the_curve() {
        let (mut c, id) = line_tool();
        assert!(c.width_click(Vec2::new(10.0, 0.5)));
        c.width_sync();
        c.width_begin_drag(1, Side::Left, Vec2::new(10.0, 0.5));
        c.width_drag_to(Vec2::new(10.0, 1.5), false);
        assert!(c.width_release());
        c.width_sync();
        assert!(c.width_delete_focus());
        c.width_sync();
        assert_eq!(profile_of(&c, id), None, "back to two plain end points");
        assert!(c.doc.sketcher.document().entity(id).is_some());
        // An end point stays.
        c.tools.width.focus = Some(0);
        assert!(!c.width_delete_focus());
        assert!(c.doc.sketcher.document().entity(id).is_some());
    }

    #[test]
    fn tab_cycles_and_arrows_nudge_the_focused_point() {
        let (mut c, id) = line_tool();
        assert!(c.width_click(Vec2::new(10.0, 0.5)));
        c.width_sync();
        c.width_begin_drag(1, Side::Left, Vec2::new(10.0, 0.5));
        c.width_drag_to(Vec2::new(10.0, 1.5), false);
        assert!(c.width_release());
        c.width_sync();
        c.tools.width.focus = None;
        c.width_cycle_focus(false);
        assert_eq!(c.tools.width.focus, Some(0));
        c.width_cycle_focus(false);
        c.width_cycle_focus(false);
        assert_eq!(c.tools.width.focus, Some(2));
        c.width_cycle_focus(false);
        assert_eq!(c.tools.width.focus, Some(0), "wraps");
        c.width_cycle_focus(true);
        assert_eq!(c.tools.width.focus, Some(2), "and back");
        c.tools.width.focus = Some(1);
        assert!(c.width_nudge(NUDGE));
        c.width_sync();
        assert!((profile_of(&c, id).unwrap().points[1].t - 0.51).abs() < 1e-9);
        assert!(c.width_nudge(-10.0 * NUDGE));
        c.width_sync();
        assert!((profile_of(&c, id).unwrap().points[1].t - 0.41).abs() < 1e-9);
        // The end points don't nudge.
        c.tools.width.focus = Some(0);
        assert!(!c.width_nudge(NUDGE));
    }

    #[test]
    fn a_click_away_from_the_stroke_adds_nothing() {
        let (mut c, id) = line_tool();
        assert!(!c.width_click(Vec2::new(10.0, 5.0)));
        assert_eq!(profile_of(&c, id), None);
        assert!(c.doc.sketcher.document().entity(id).is_some());
    }

    #[test]
    fn a_commit_styles_every_curve_of_the_chain() {
        let mut c = AppState::default();
        let mut ed = Editor::new(Document::new());
        let layer = Document::default_layer();
        let mut pt = |x: f64, y: f64| {
            ed.apply(&Command::AddEntity {
                layer,
                construction: false,
                style: Style::default(),
                geom: Geom::Point {
                    pos: Vec2::new(x, y),
                },
            })
            .unwrap()
            .entities[0]
        };
        let (a, b, d) = (pt(0.0, 0.0), pt(10.0, 0.0), pt(10.0, 10.0));
        let style = Style {
            width: 1.0,
            ..Style::default()
        };
        let mut line = |s, e| {
            ed.apply(&Command::AddEntity {
                layer,
                construction: false,
                style: style.clone(),
                geom: Geom::Line { start: s, end: e },
            })
            .unwrap()
            .entities[0]
        };
        let (l1, l2) = (line(a, b), line(b, d));
        c.load(ed.document().clone());
        c.view.camera.pt_per_mm = 50.0;
        c.selection.items.set_all([l1, l2]);
        c.set_modify(crate::modify_tool::Modify::Width);
        assert_eq!(c.modify(), Some(crate::modify_tool::Modify::Width));
        let t = c.tools.width.target.as_ref().unwrap();
        assert_eq!(t.ids.len(), 2, "both lines are one chain");
        // The chain is 20 mm long: x = 5 along the first line is t 0.25.
        assert!(c.width_click(Vec2::new(5.0, 0.5)));
        c.width_sync();
        c.width_begin_drag(1, Side::Left, Vec2::new(5.0, 0.5));
        c.width_drag_to(Vec2::new(5.0, 1.5), false);
        assert!(c.width_release());
        assert_eq!(profile_of(&c, l1), profile_of(&c, l2));
        assert!(profile_of(&c, l1).is_some());
        c.undo();
        assert_eq!(profile_of(&c, l1), None);
        assert_eq!(profile_of(&c, l2), None);
    }
}
