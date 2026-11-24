// SPDX-License-Identifier: AGPL-3.0-or-later
//! Fillet and chamfer drag handles (#418, design §3 step 2, §6.6–6.8).
//!
//! Like the offset arrow (#336) and the pattern handles (#337), each size in
//! the Fillet / Chamfer dialog has a handle at the corner:
//! - a fillet's handle sits on the middle of its arc and slides along the
//!   corner's bisector: towards the sharp corner shrinks the radius;
//! - a chamfer has one at each end of the bevel, sliding along its line
//!   (one in Equal mode moves both; Distance and angle has only the first).
//!
//! Handle and field are one value: a drag writes the field (snapped to
//! 0.1 mm, 1 mm with Shift) and the handle follows what is typed. Nothing
//! changes the document until OK or Enter, which commits one command
//! (SPEC §11). The handles work on a corner being picked, and on an existing
//! fillet or chamfer clicked with the tool to resize it.

use crate::modify_tool::{ChamferMode, Modify};
use crate::render::Frame;
use crate::scene::Shape;
use crate::settings::Units;
use crate::state::AppState;
use egui::{Align2, FontId, PointerButton, Rect, Stroke};
use locus_doc::corner::{chamfer_dims, fillet_dims};
use locus_doc::{Command, ConstraintId, Document, EntityId, Geom, Sketcher};
use locus_geom::Vec2;

/// A handle's grab radius (screen points), as the offset arrow's.
const GRAB_PT: f32 = 12.0;
/// Below this a fillet is nearly a half circle: its handle then moves
/// radially instead (the bisector mapping would blow up).
const MIN_HALF_COS: f64 = 0.05;

/// Which dialog field a handle drives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SizeField {
    Radius,
    /// A chamfer's first set-back.
    First,
    /// A chamfer's second set-back.
    Second,
    /// Both set-backs (Equal distance): the first field.
    Both,
}

/// One handle: where it is, and how a pointer position maps to its value:
/// `value = (p − origin) · dir / scale` (world mm).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CornerHandle {
    pub at: Vec2,
    pub field: SizeField,
    pub origin: Vec2,
    pub dir: Vec2,
    pub scale: f64,
}

impl CornerHandle {
    /// The size for a pointer at `w`, snapped to `step` and never below it.
    pub(crate) fn value_at(&self, w: Vec2, step: f64) -> f64 {
        let v = (w - self.origin).dot(self.dir) / self.scale;
        ((v / step).round() * step).max(step)
    }
}

/// The handle of a fillet arc: on its middle, sliding along the bisector.
/// Between two lines the arc's middle is `r · (1 / cos(θ/2) − 1)` from the
/// sharp corner (θ its sweep), so that distance over the factor is the
/// radius; between curved sides the same mapping holds near the arc.
fn fillet_handle(arc: locus_geom::Arc) -> Option<CornerHandle> {
    let (c, r) = (arc.center, arc.radius());
    let at = arc.eval(0.5);
    let u = (at - c).normalized(0.0)?;
    let half_cos = (arc.sweep() / 2.0).cos();
    let (origin, dir, scale) = if half_cos > MIN_HALF_COS {
        let k = 1.0 / half_cos - 1.0;
        (c + u * (r / half_cos), -u, k)
    } else {
        (c, u, 1.0)
    };
    (scale > 1e-9).then_some(CornerHandle {
        at,
        field: SizeField::Radius,
        origin,
        dir,
        scale,
    })
}

/// The handles of a chamfer line: one at each set-back point, sliding
/// along its side away from the corner.
fn chamfer_handles(doc: &Document, line: EntityId, mode: ChamferMode) -> Vec<CornerHandle> {
    let (Some(dims), Some(&Geom::Line { start, end })) =
        (chamfer_dims(doc, line), doc.entity(line).map(|e| &e.geom))
    else {
        return Vec::new();
    };
    let Some(v) = doc.point_pos(dims.corner) else {
        return Vec::new();
    };
    let equal = mode == ChamferMode::Equal && !dims.angle;
    let mut ends = vec![(
        start,
        if equal {
            SizeField::Both
        } else {
            SizeField::First
        },
    )];
    if !dims.angle {
        ends.push((
            end,
            if equal {
                SizeField::Both
            } else {
                SizeField::Second
            },
        ));
    }
    ends.into_iter()
        .filter_map(|(p, field)| {
            let at = doc.point_pos(p)?;
            let dir = (at - v).normalized(0.0)?;
            Some(CornerHandle {
                at,
                field,
                origin: v,
                dir,
                scale: 1.0,
            })
        })
        .collect()
}

/// The handles of fillet arc or chamfer line `curve` in `doc`.
fn handles_of(doc: &Document, curve: EntityId, m: Modify, mode: ChamferMode) -> Vec<CornerHandle> {
    match m {
        Modify::Fillet => doc
            .arc(curve)
            .filter(|_| fillet_dims(doc, curve).is_some())
            .and_then(fillet_handle)
            .into_iter()
            .collect(),
        Modify::Chamfer => chamfer_handles(doc, curve, mode),
        _ => Vec::new(),
    }
}

/// A curve's shape from its geometry in `doc`, for previews.
pub(crate) fn curve_shape(doc: &Document, id: EntityId) -> Option<Shape> {
    match doc.entity(id)?.geom {
        Geom::Line { .. } => doc.line(id).map(Shape::Line),
        Geom::Arc { .. } => doc.arc(id).map(Shape::Arc),
        _ => None,
    }
}

/// An existing fillet or chamfer resized in a scratch copy of the document:
/// the curves that change, and the handles where they end up. Cached by the
/// dialog's values so the copy is solved only when they change.
#[derive(Clone, Debug, Default)]
pub(crate) struct ResizePreview {
    key: String,
    pub shapes: Vec<Shape>,
    pub handles: Vec<CornerHandle>,
}

impl AppState {
    /// The handles to show: on the picked corner's preview, or on the
    /// fillet or chamfer being resized.
    pub(crate) fn corner_handles(&self, m: Modify, units: Units) -> Vec<CornerHandle> {
        let mode = self.tools.corner.mode;
        if self.tools.corner_edit.is_some() {
            return self.tools.corner_resize.handles.clone();
        }
        let Some((doc, first_new)) = self.corner_draft_after(m, units) else {
            return Vec::new();
        };
        let want = if m == Modify::Fillet {
            locus_doc::EntityKind::Arc
        } else {
            locus_doc::EntityKind::Line
        };
        doc.entities()
            .iter()
            .filter(|(id, e)| id.get() >= first_new && e.geom.kind() == want)
            .flat_map(|(id, _)| handles_of(&doc, id, m, mode))
            .collect()
    }

    /// The picked corner as the command would leave it (written positions,
    /// before re-solving), and the first ID it creates.
    pub(crate) fn corner_draft_after(&self, m: Modify, units: Units) -> Option<(Document, u64)> {
        self.tools.corner_pick?;
        let plan = self.corner_plan(m, units).ok()?;
        let doc = self.doc.sketcher.document();
        let first_new = doc.next_id();
        let mut ed = locus_doc::Editor::new(doc.clone());
        ed.apply(&plan.command).ok()?;
        Some((ed.into_document(), first_new))
    }

    /// Start resizing the existing fillet or chamfer `curve` (#418): the
    /// dialog takes its dimensions, as numbers or their expressions.
    pub(crate) fn corner_edit_begin(&mut self, curve: EntityId, units: Units) {
        let doc = self.doc.sketcher.document();
        let text = |id: ConstraintId, angle: bool| {
            doc.dimension_expr(id).map_or_else(
                || {
                    let v = doc
                        .constraint(id)
                        .and_then(locus_doc::Constraint::dimension)
                        .map_or(0.0, |d| d.value);
                    crate::state::format_value(v, angle, units)
                },
                |e| {
                    if angle {
                        e.to_owned()
                    } else {
                        crate::state::to_display(e, units)
                    }
                },
            )
        };
        let c = &mut self.tools.corner;
        if let Some(r) = fillet_dims(doc, curve) {
            c.text.radius = text(r, false);
        } else if let Some(d) = chamfer_dims(doc, curve) {
            c.text.d1 = text(d.first, false);
            if d.angle {
                c.mode = ChamferMode::DistanceAngle;
                c.text.angle = text(d.second, true);
            } else {
                c.text.d2 = text(d.second, false);
                c.mode = if c.text.d1 == c.text.d2 {
                    ChamferMode::Equal
                } else {
                    ChamferMode::TwoDistances
                };
            }
        } else {
            return;
        }
        self.tools.corner_pick = None;
        self.tools.corner_edit = Some(curve);
        self.tools.corner_resize = ResizePreview::default();
        self.tools.corner_focus = true;
        self.selection.items.set_all([curve]);
        self.doc.notice = None;
    }

    /// Start the resize of a fillet or chamfer that was selected when its
    /// tool was picked (#445).
    pub(crate) fn start_pending_corner_edit(&mut self, units: Units) {
        if let Some(id) = self.tools.corner_edit_pending.take() {
            self.corner_edit_begin(id, units);
        }
    }

    /// The command that resizes the fillet or chamfer being edited to the
    /// dialog's values: one compound, one undo step (SPEC §11). `None` when
    /// nothing changes.
    pub(crate) fn corner_edit_command(
        &self,
        m: Modify,
        units: Units,
    ) -> Result<Option<Command>, String> {
        let curve = self
            .tools
            .corner_edit
            .ok_or_else(|| "Pick a corner first".to_owned())?;
        let doc = self.doc.sketcher.document();
        let t = &self.tools.corner.text;
        let gone = || "That corner is no longer there".to_owned();
        let sizes: Vec<(ConstraintId, &str, bool)> = match m {
            Modify::Fillet => vec![(fillet_dims(doc, curve).ok_or_else(gone)?, &t.radius, false)],
            _ => {
                let d = chamfer_dims(doc, curve).ok_or_else(gone)?;
                let second = match (d.angle, self.tools.corner.mode) {
                    (true, _) => (&t.angle, true),
                    (false, ChamferMode::Equal) => (&t.d1, false),
                    (false, _) => (&t.d2, false),
                };
                vec![(d.first, &t.d1, false), (d.second, second.0, second.1)]
            }
        };
        let mut commands = Vec::new();
        for (id, text, angle) in sizes {
            let (v, expr) = super::corner_dialog::field(text, angle, units, doc)?;
            if !angle && v <= 0.0 {
                return Err("A size must be more than zero".to_owned());
            }
            let value = if angle { v.to_radians() } else { v };
            let old = doc.dimension_expr(id);
            let current = doc
                .constraint(id)
                .and_then(locus_doc::Constraint::dimension)
                .map_or(f64::NAN, |d| d.value);
            match expr {
                Some(e) => {
                    if old != Some(e.as_str()) {
                        commands.push(Command::SetDimensionExpr { id, expr: Some(e) });
                    }
                }
                None => {
                    if old.is_some() {
                        commands.push(Command::SetDimensionExpr { id, expr: None });
                    }
                    if (current - value).abs() > 1e-12 {
                        commands.push(Command::SetDimension { id, value });
                    }
                }
            }
        }
        Ok((!commands.is_empty()).then(|| Command::Compound {
            label: format!("Resize {}", m.label().to_lowercase()),
            commands,
        }))
    }

    /// Recompute the resize preview when the dialog's values changed: the
    /// change is applied to a solved scratch copy of the document.
    pub(crate) fn refresh_corner_resize(&mut self, m: Modify, units: Units) {
        let Some(curve) = self.tools.corner_edit else {
            return;
        };
        let key = format!(
            "{}|{:?}|{:?}|{}",
            curve.get(),
            self.tools.corner.text,
            self.tools.corner.mode,
            self.doc.sketcher.editor().undo_label().unwrap_or_default()
        );
        if self.tools.corner_resize.key == key && !key.is_empty() {
            return;
        }
        let mode = self.tools.corner.mode;
        let doc = self.doc.sketcher.document();
        let mut preview = ResizePreview {
            key,
            ..ResizePreview::default()
        };
        match self.corner_edit_command(m, units) {
            Ok(Some(cmd)) => {
                let mut sk = Sketcher::new(doc.clone());
                if sk.apply(&cmd).is_ok() {
                    let after = sk.document();
                    preview.shapes = after
                        .entities()
                        .iter()
                        .filter_map(|(id, _)| {
                            let s = curve_shape(after, id)?;
                            (curve_shape(doc, id) != Some(s)).then_some(s)
                        })
                        .collect();
                    preview.handles = handles_of(after, curve, m, mode);
                }
            }
            Ok(None) => preview.handles = handles_of(doc, curve, m, mode),
            Err(_) => {}
        }
        self.tools.corner_resize = preview;
    }

    /// Write `field` from a drag (mm), as a plain number.
    fn set_corner_size(&mut self, field: SizeField, mm: f64, units: Units) {
        let s = crate::state::format_value(mm, false, units);
        let t = &mut self.tools.corner.text;
        match field {
            SizeField::Radius => t.radius = s,
            SizeField::First | SizeField::Both => t.d1 = s,
            SizeField::Second => t.d2 = s,
        }
    }

    /// A drag of `h` to world point `w`: the field takes the new size.
    pub(crate) fn corner_drag_to(&mut self, h: CornerHandle, w: Vec2, coarse: bool, units: Units) {
        let step = if coarse { 1.0 } else { 0.1 };
        self.set_corner_size(h.field, h.value_at(w, step), units);
    }

    /// Pointer input on the handles while Fillet or Chamfer is active.
    /// Returns true when the pointer was used here.
    pub(crate) fn corner_pointer(
        &mut self,
        ui: &egui::Ui,
        resp: &egui::Response,
        rect: Rect,
        m: Modify,
        units: Units,
    ) -> bool {
        let cam = self.view.camera;
        let press = super::press_pos(ui, resp);
        if self.tools.corner_drag.is_none()
            && resp.drag_started_by(PointerButton::Primary)
            && let Some(p) = press
        {
            let near = self
                .corner_handles(m, units)
                .into_iter()
                .map(|h| (cam.to_screen(h.at, rect).distance(p), h))
                .filter(|(d, _)| *d <= GRAB_PT)
                .min_by(|a, b| a.0.total_cmp(&b.0));
            self.tools.corner_drag = near.map(|(_, h)| h);
        }
        if let Some(h) = self.tools.corner_drag {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            if let Some(p) = resp.interact_pointer_pos() {
                let coarse = ui.input(|i| i.modifiers.shift);
                self.corner_drag_to(h, cam.to_world(p, rect), coarse, units);
            }
            if !resp.dragged_by(PointerButton::Primary) {
                self.tools.corner_drag = None;
            }
            return true;
        }
        let handles = self.corner_handles(m, units);
        let over = |p: egui::Pos2| {
            handles
                .iter()
                .any(|h| cam.to_screen(h.at, rect).distance(p) <= GRAB_PT)
        };
        if resp.hover_pos().is_some_and(over) {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        // A click on a handle focuses the size field; it isn't a new pick.
        let on_handle = resp.clicked_by(PointerButton::Primary) && press.is_some_and(over);
        if on_handle {
            self.tools.corner_focus = true;
        }
        on_handle
    }

    /// The resize preview and every handle with its read-out.
    pub(crate) fn draw_corner_handles(&self, frame: &Frame<'_>, m: Modify) {
        if self.tools.corner_edit.is_some() {
            frame.draw_preview(&self.tools.corner_resize.shapes);
        }
        let handles = self.corner_handles(m, frame.units);
        let (p, c) = (frame.painter, frame.pal.selected);
        let t = &self.tools.corner.text;
        for h in &handles {
            let at = frame.view.to_screen(h.at, frame.rect);
            let ahead = frame.view.to_screen(h.at + h.dir, frame.rect) - at;
            let dir = if ahead.length() > 0.0 {
                ahead.normalized()
            } else {
                egui::Vec2::X
            };
            let dragging = self.tools.corner_drag.is_some_and(|d| d.field == h.field);
            // A short double arrow along the way it slides.
            p.line_segment([at - dir * 14.0, at + dir * 14.0], Stroke::new(1.5, c));
            let side = egui::Vec2::new(-dir.y, dir.x) * 4.0;
            for s in [1.0, -1.0] {
                let tip = at + dir * 18.0 * s;
                p.add(egui::Shape::convex_polygon(
                    vec![tip, tip - dir * 7.0 * s + side, tip - dir * 7.0 * s - side],
                    c,
                    Stroke::NONE,
                ));
            }
            crate::render::handle_mark(
                p,
                at,
                crate::scene::Handle::Control,
                c,
                frame.pal.background,
            );
            if dragging {
                p.circle_stroke(at, crate::theme::HANDLE_PX / 2.0 + 3.0, Stroke::new(1.5, c));
            }
            p.circle_stroke(at, GRAB_PT - 2.0, Stroke::new(1.0, c.gamma_multiply(0.5)));
            let text = match h.field {
                SizeField::Radius => format!("R {}", t.radius),
                SizeField::First | SizeField::Both => t.d1.clone(),
                SizeField::Second => t.d2.clone(),
            };
            let normal = egui::Vec2::new(-dir.y, dir.x);
            p.text(
                at + normal * 16.0,
                if normal.x < 0.0 {
                    Align2::RIGHT_CENTER
                } else {
                    Align2::LEFT_CENTER
                },
                format!("{text} {}", frame.units.label()),
                FontId::monospace(13.0),
                frame.pal.hover,
            );
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // fixtures built in the test
mod tests {
    use super::*;
    use crate::tool::ToolEvent;
    use crate::tools::{Pick, Tool};
    use locus_doc::Constraint;

    /// A 20 × 10 rectangle from the Rectangle tool with `m` picking its
    /// corner at the origin.
    fn picked(m: Modify) -> AppState {
        let mut c = AppState::default();
        c.load(Document::new());
        c.set_tool(Tool::RectCorner);
        for p in [Vec2::ZERO, Vec2::new(20.0, 10.0)] {
            c.tool_event(ToolEvent::Click(Pick::free(p)), None, Units::Mm);
        }
        c.set_modify(m);
        let doc = c.doc.sketcher.document();
        let at_origin: Vec<EntityId> = doc
            .entities()
            .iter()
            .filter_map(|(id, _)| doc.line(id).map(|l| (id, l)))
            .filter(|(_, l)| l.a.length() < 1e-9 || l.b.length() < 1e-9)
            .map(|(id, _)| id)
            .collect();
        for id in at_origin {
            click(&mut c, id, Vec2::ZERO);
        }
        assert!(c.tools.corner_pick.is_some(), "the corner is picked");
        c
    }

    fn dims(c: &AppState) -> Vec<f64> {
        c.doc
            .sketcher
            .document()
            .constraints()
            .values()
            .filter_map(|k| match k {
                Constraint::Radius { dim, .. } | Constraint::Distance { dim, .. } => {
                    Some(dim.value)
                }
                _ => None,
            })
            .collect()
    }

    fn near(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    /// The fillet's handle sits on its arc's middle; dragging it along the
    /// bisector sets the radius field, and OK commits that radius.
    #[test]
    fn dragging_the_fillet_handle_sets_the_radius() {
        let mut c = picked(Modify::Fillet);
        let hs = c.corner_handles(Modify::Fillet, Units::Mm);
        assert_eq!(hs.len(), 1);
        let h = hs[0];
        // Default 2 mm at a right angle: centre (2, 2), middle 2 mm from it
        // towards the corner.
        let s = 2.0 - 2.0_f64.sqrt();
        assert!(h.at.distance(Vec2::new(s, s)) < 1e-6, "{:?}", h.at);
        assert!(h.origin.length() < 1e-6, "slides from the sharp corner");
        // The pointer where a 5 mm arc's middle would be.
        let k = 2.0_f64.sqrt() - 1.0;
        let w = h.origin + h.dir * (5.0 * k);
        c.corner_drag_to(h, w, false, Units::Mm);
        assert_eq!(c.tools.corner.text.radius, "5");
        // The handle follows the field.
        let h5 = c.corner_handles(Modify::Fillet, Units::Mm)[0];
        let s5 = 5.0 - 5.0 * std::f64::consts::FRAC_1_SQRT_2;
        assert!(h5.at.distance(Vec2::new(s5, s5)) < 1e-6, "{:?}", h5.at);
        // Shift snaps to whole millimetres; it never goes to zero.
        c.corner_drag_to(h, h.origin + h.dir * (3.4 * k), true, Units::Mm);
        assert_eq!(c.tools.corner.text.radius, "3");
        c.corner_drag_to(h, h.origin - h.dir, false, Units::Mm);
        assert_eq!(c.tools.corner.text.radius, "0.1");
        c.corner_drag_to(h, w, false, Units::Mm);
        assert!(c.corner_ok(Modify::Fillet, Units::Mm));
        assert_eq!(c.undo_label(), Some("Fillet"));
        assert!(dims(&c).iter().any(|v| near(*v, 5.0)), "{:?}", dims(&c));
    }

    /// Two distances: a handle at each end of the bevel, each setting its
    /// own field; Equal has two that both set the one distance.
    #[test]
    fn dragging_chamfer_handles_sets_each_distance() {
        let mut c = picked(Modify::Chamfer);
        let hs = c.corner_handles(Modify::Chamfer, Units::Mm);
        assert_eq!(hs.len(), 2);
        assert!(hs.iter().all(|h| h.field == SizeField::Both));
        c.tools.corner.mode = ChamferMode::TwoDistances;
        let hs = c.corner_handles(Modify::Chamfer, Units::Mm);
        let second = *hs.iter().find(|h| h.field == SizeField::Second).unwrap();
        assert!(near(second.at.distance(second.origin), 2.0));
        c.corner_drag_to(second, second.origin + second.dir * 4.0, false, Units::Mm);
        assert_eq!(c.tools.corner.text.d2, "4");
        let moved = c.corner_handles(Modify::Chamfer, Units::Mm);
        let s2 = moved.iter().find(|h| h.field == SizeField::Second).unwrap();
        assert!(near(s2.at.distance(s2.origin), 4.0), "the handle follows");
        assert!(c.corner_ok(Modify::Chamfer, Units::Mm));
        let mut v = dims(&c);
        v.sort_by(f64::total_cmp);
        assert!(near(v[0], 2.0) && near(v[1], 4.0), "{v:?}");
        // Distance and angle: one handle, on the first distance.
        let mut c = picked(Modify::Chamfer);
        c.tools.corner.mode = ChamferMode::DistanceAngle;
        let hs = c.corner_handles(Modify::Chamfer, Units::Mm);
        assert_eq!(hs.len(), 1);
        assert_eq!(hs[0].field, SizeField::First);
    }

    /// The curve a corner tool made, picked by clicking it.
    fn click(c: &mut AppState, id: EntityId, at: Vec2) {
        c.tool_event(ToolEvent::Click(Pick::free(at)), Some(id), Units::Mm);
    }

    /// An existing fillet clicked with the Fillet tool is resized: its
    /// radius fills the dialog, the handle drags it, and OK is one undo
    /// step that the solver honours.
    #[test]
    fn an_existing_fillet_is_resized_by_its_handle() {
        let mut c = picked(Modify::Fillet);
        assert!(c.corner_ok(Modify::Fillet, Units::Mm));
        let doc = c.doc.sketcher.document();
        let (arc, a) = doc
            .entities()
            .iter()
            .find_map(|(id, _)| doc.arc(id).map(|a| (id, a)))
            .unwrap();
        c.tools.corner.text.radius = "9".to_owned();
        click(&mut c, arc, a.eval(0.5));
        assert_eq!(c.tools.corner_edit, Some(arc));
        assert_eq!(c.tools.corner.text.radius, "2", "the fillet's own radius");
        c.refresh_corner_resize(Modify::Fillet, Units::Mm);
        let h = c.corner_handles(Modify::Fillet, Units::Mm)[0];
        assert!(h.at.distance(a.eval(0.5)) < 1e-6);
        let k = 2.0_f64.sqrt() - 1.0;
        c.corner_drag_to(h, h.origin + h.dir * (3.0 * k), false, Units::Mm);
        assert_eq!(c.tools.corner.text.radius, "3");
        c.refresh_corner_resize(Modify::Fillet, Units::Mm);
        assert!(!c.tools.corner_resize.shapes.is_empty(), "a live preview");
        // The handle moves to the middle of the re-solved 3 mm arc.
        let moved = c.corner_handles(Modify::Fillet, Units::Mm)[0];
        let solved = c
            .tools
            .corner_resize
            .shapes
            .iter()
            .find_map(|s| match s {
                Shape::Arc(a) => Some(*a),
                _ => None,
            })
            .unwrap();
        assert!(near(solved.radius(), 3.0), "{}", solved.radius());
        assert!(moved.at.distance(solved.eval(0.5)) < 1e-6, "{:?}", moved.at);
        let before = c.doc.sketcher.document().clone();
        assert!(c.corner_ok(Modify::Fillet, Units::Mm));
        assert_eq!(c.undo_label(), Some("Resize fillet"));
        assert!(c.tools.corner_edit.is_none());
        let r = c.doc.sketcher.document().arc(arc).unwrap().radius();
        assert!(near(r, 3.0), "{r}");
        c.undo();
        assert_eq!(c.doc.sketcher.document(), &before, "one undo step");
    }

    /// An existing chamfer keeps its two distances apart, and an expression
    /// typed in the dialog stays an expression.
    #[test]
    fn an_existing_chamfer_is_resized_by_its_handles() {
        let mut c = picked(Modify::Chamfer);
        c.tools.corner.mode = ChamferMode::TwoDistances;
        c.tools.corner.text.d2 = "3".to_owned();
        assert!(c.corner_ok(Modify::Chamfer, Units::Mm));
        let doc = c.doc.sketcher.document();
        let line = doc
            .entities()
            .iter()
            .map(|(id, _)| id)
            .find(|id| chamfer_dims(doc, *id).is_some())
            .unwrap();
        let mid = doc.line(line).unwrap().eval(0.5);
        c.tools.corner.mode = ChamferMode::Equal;
        click(&mut c, line, mid);
        assert_eq!(c.tools.corner_edit, Some(line));
        assert_eq!(c.tools.corner.mode, ChamferMode::TwoDistances);
        assert_eq!(
            (
                c.tools.corner.text.d1.as_str(),
                c.tools.corner.text.d2.as_str()
            ),
            ("2", "3")
        );
        c.refresh_corner_resize(Modify::Chamfer, Units::Mm);
        let first = *c
            .corner_handles(Modify::Chamfer, Units::Mm)
            .iter()
            .find(|h| h.field == SizeField::First)
            .unwrap();
        c.corner_drag_to(first, first.origin + first.dir * 1.5, false, Units::Mm);
        assert!(c.apply_reporting(&Command::SetParameter {
            name: "bevel".to_owned(),
            expr: "2.5".to_owned(),
        }));
        c.tools.corner.text.d2 = "bevel".to_owned();
        assert!(c.corner_ok(Modify::Chamfer, Units::Mm));
        assert_eq!(c.undo_label(), Some("Resize chamfer"));
        let doc = c.doc.sketcher.document();
        let d = chamfer_dims(doc, line).unwrap();
        let v = |id| {
            doc.constraint(id)
                .and_then(Constraint::dimension)
                .unwrap()
                .value
        };
        assert!(near(v(d.first), 1.5) && near(v(d.second), 2.5));
        assert_eq!(doc.dimension_expr(d.second), Some("bevel"));
    }

    /// The fillet a picked corner made: its arc, and the arc itself.
    fn filleted() -> (AppState, EntityId, locus_geom::Arc) {
        let mut c = picked(Modify::Fillet);
        assert!(c.corner_ok(Modify::Fillet, Units::Mm));
        let doc = c.doc.sketcher.document();
        let (arc, a) = doc
            .entities()
            .iter()
            .find_map(|(id, _)| doc.arc(id).map(|a| (id, a)))
            .unwrap();
        (c, arc, a)
    }

    /// Regression (#445): at normal zoom a 2 mm fillet is a pixel or two,
    /// so clicks on it land on its end points, which the line and the arc
    /// share. That picked a new corner (line + arc, "too large") instead
    /// of resizing the fillet. Its end points, its curve and the line next
    /// to it at an end point now all start the resize.
    #[test]
    fn a_small_fillet_is_resized_from_its_end_points_or_its_curve() {
        let (c0, arc, a) = filleted();
        let doc = c0.doc.sketcher.document();
        let neighbour = doc
            .entities()
            .iter()
            .map(|(id, _)| id)
            .find(|id| {
                doc.line(*id)
                    .is_some_and(|l| l.a.distance(a.start) < 1e-6 || l.b.distance(a.start) < 1e-6)
            })
            .unwrap();
        for (at, hover) in [
            (a.start, Some(neighbour)),
            (a.end, Some(arc)),
            (a.eval(0.5), Some(arc)),
            (a.start, None),
        ] {
            let (mut c, ..) = filleted();
            // The default view: the pick radius is a few millimetres.
            c.tool_event(ToolEvent::Click(Pick::free(at)), hover, Units::Mm);
            assert_eq!(c.tools.corner_edit, Some(arc), "{at:?} {hover:?}");
            assert!(c.tools.corner_pick.is_none(), "not a new corner");
            assert!(c.hint().starts_with("Resizing the fillet"), "{}", c.hint());
        }
    }

    /// #445: selecting a fillet, then picking the Fillet tool, resizes it;
    /// hovering one with the tool says a click resizes it.
    #[test]
    fn a_selected_fillet_is_resized_when_the_tool_starts() {
        let (mut c, arc, _) = filleted();
        c.set_tool(Tool::Select);
        c.selection.items.set_all([arc]);
        c.set_modify(Modify::Fillet);
        c.start_pending_corner_edit(Units::Mm);
        assert_eq!(c.tools.corner_edit, Some(arc));
        assert_eq!(c.tools.corner.text.radius, "2");
        assert!(c.corner_esc());
        c.selection.hover = Some(arc);
        assert_eq!(c.hint(), "Click to resize this fillet");
        // Chamfer ignores a selected fillet.
        c.selection.items.set_all([arc]);
        c.set_modify(Modify::Chamfer);
        c.start_pending_corner_edit(Units::Mm);
        assert!(c.tools.corner_edit.is_none());
    }
}
