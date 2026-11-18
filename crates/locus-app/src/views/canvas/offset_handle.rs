// SPDX-License-Identifier: AGPL-3.0-or-later
//! The arrow handle on an existing offset (#444, design §6.6).
//!
//! A new offset is sized by the arrow on its result (#336). Once it exists,
//! the same arrow shows on its result when the offset is picked with the
//! Offset tool or its curves are selected, and dragging it changes the
//! distance with a live preview. Releasing commits one `EditRecipe` (one
//! undo step, SPEC §11). The dialog's Distance field shows the dragged value
//! as it changes. A drag sets a plain number: an expression the distance had
//! is replaced, as typing a number would, and undo brings it back.

use super::offset_tool::{
    GRAB_PT, closest, draw_offset_arrow, handle_point, offset_shapes, plus_direction,
};
use crate::render::Frame;
use crate::scene::Shape;
use crate::state::AppState;
use crate::tools::Tool;
use egui::{PointerButton, Rect};
use locus_doc::recipe::RecipeKind;
use locus_doc::{Command, RecipeId};
use locus_geom::Vec2;

/// Where an existing offset's arrow stands: its base on the first source
/// curve and the way a positive distance goes there. Cached, since the
/// direction comes from a scratch offset; the key changes with the source.
#[derive(Clone, Debug)]
pub(crate) struct OffsetAxis {
    key: String,
    pub base: Vec2,
    pub plus: Vec2,
}

/// The arrow to draw this frame: offset, base, direction, distance and
/// handle (world mm), from [`AppState::refresh_offset_edit`].
pub(crate) type OffsetArrow = (RecipeId, Vec2, Vec2, f64, Vec2);

/// An existing offset's arrow being dragged.
#[derive(Clone, Debug)]
pub(crate) struct OffsetEdit {
    pub rid: RecipeId,
    pub base: Vec2,
    pub plus: Vec2,
    pub sharp: bool,
    /// The dragged distance (mm, signed as the recipe's).
    pub distance: f64,
    /// The result at `distance`, for the preview and the handle.
    preview: Vec<Shape>,
}

/// A point on `s` to stand the arrow on: its middle, as near as the shape
/// allows.
fn middle(s: &Shape) -> Vec2 {
    match s {
        Shape::Point(p) => *p,
        Shape::Line(l) => l.eval(0.5),
        Shape::Arc(a) => a.eval(0.5),
        Shape::Circle(c) => c.eval(std::f64::consts::FRAC_PI_2),
        Shape::Cubic(c) => c.eval(0.5),
    }
}

impl AppState {
    /// The existing offset whose arrow shows: the one picked with the
    /// Offset tool (no new offset being set up), or, with Select, the one
    /// every selected curve belongs to.
    pub(crate) fn offset_handle_target(&self) -> Option<RecipeId> {
        let doc = self.doc.sketcher.document();
        let is_offset = |rid: RecipeId| {
            matches!(
                doc.recipe(rid).map(|r| &r.kind),
                Some(RecipeKind::Offset { .. })
            )
        };
        if self.modify() == Some(crate::modify_tool::Modify::Offset) {
            return if self.tools.offset_draft.is_none() {
                self.offset_selected().filter(|r| is_offset(*r))
            } else {
                None
            };
        }
        if self.modify().is_some() || self.tool() != Tool::Select {
            return None;
        }
        let mut owners = self
            .selection
            .items
            .iter()
            .map(|id| doc.driven_by(id).filter(|r| is_offset(*r)));
        let first = owners.next()??;
        owners.all(|r| r == Some(first)).then_some(first)
    }

    /// The arrow's base and direction for offset `rid`: on the source
    /// nearest where the offset was last clicked (or the middle of its first
    /// source), so it stands where the user is looking. Cached by that
    /// source's shape, the recipe and the click.
    fn offset_axis(&mut self, rid: RecipeId) -> Option<(Vec2, Vec2)> {
        let doc = self.doc.sketcher.document();
        let r = doc.recipe(rid)?;
        let shapes: Vec<Shape> = self
            .doc
            .scene
            .items
            .iter()
            .filter(|i| r.sources.contains(&i.id))
            .map(|i| i.shape)
            .collect();
        let anchor = self.tools.offset_anchor;
        let (shape, base) = match anchor {
            Some(a) => shapes
                .iter()
                .map(|s| (*s, closest(s, a)))
                .min_by(|x, y| x.1.distance(a).total_cmp(&y.1.distance(a)))?,
            None => {
                let s = *shapes.first()?;
                (s, middle(&s))
            }
        };
        let key = format!("{}|{:?}|{:?}|{:?}", rid.get(), r.kind, shape, anchor);
        if let Some(a) = self.tools.offset_axis.as_ref().filter(|a| a.key == key) {
            return Some((a.base, a.plus));
        }
        let sources = r.sources.clone();
        let plus = plus_direction(doc, &sources, base, &shape);
        self.tools.offset_axis = Some(OffsetAxis { key, base, plus });
        Some((base, plus))
    }

    /// The handle for offset `rid` (world mm): on its result at its
    /// distance, or on the drag's preview.
    fn offset_handle_at(&mut self, rid: RecipeId) -> Option<(Vec2, Vec2, f64, Vec2)> {
        if let Some(e) = self.tools.offset_edit.as_ref().filter(|e| e.rid == rid) {
            let tip = handle_point(e.base, e.plus, e.distance, Some(&e.preview));
            return Some((e.base, e.plus, e.distance, tip));
        }
        let (base, plus) = self.offset_axis(rid)?;
        let (distance, _) = self.offset_params(rid)?;
        let doc = self.doc.sketcher.document();
        let result: Vec<Shape> = self
            .doc
            .scene
            .items
            .iter()
            .filter(|i| doc.driven_by(i.id) == Some(rid))
            .filter(|i| !matches!(i.shape, Shape::Point(_)))
            .map(|i| i.shape)
            .collect();
        let tip = handle_point(base, plus, distance, Some(&result));
        Some((base, plus, distance, tip))
    }

    /// Start dragging offset `rid`'s arrow.
    pub(crate) fn offset_edit_begin(&mut self, rid: RecipeId) -> bool {
        let Some((base, plus, distance, _)) = self.offset_handle_at(rid) else {
            return false;
        };
        let Some((_, sharp)) = self.offset_params(rid) else {
            return false;
        };
        self.tools.offset_edit = Some(OffsetEdit {
            rid,
            base,
            plus,
            sharp,
            distance,
            preview: Vec::new(),
        });
        true
    }

    /// The drag at world point `w`: the distance along the arrow, snapped
    /// to 0.1 mm (1 mm with `coarse`), never zero; the preview follows.
    pub(crate) fn offset_edit_drag_to(&mut self, w: Vec2, coarse: bool) {
        let step = if coarse { 1.0 } else { 0.1 };
        let Some(e) = self.tools.offset_edit.as_ref() else {
            return;
        };
        let mm = ((w - e.base).dot(e.plus) / step).round() * step;
        if mm == 0.0 || mm == e.distance {
            return;
        }
        let doc = self.doc.sketcher.document();
        let sources = doc
            .recipe(e.rid)
            .map(|r| r.sources.clone())
            .unwrap_or_default();
        let preview = offset_shapes(doc, &sources, mm, e.sharp);
        if let Some(e) = self.tools.offset_edit.as_mut() {
            e.distance = mm;
            e.preview = preview;
        }
    }

    /// Release: commit the dragged distance as one undo step (a plain
    /// number, replacing an expression). Nothing happens if it didn't move.
    pub(crate) fn offset_edit_release(&mut self) -> bool {
        let Some(e) = self.tools.offset_edit.take() else {
            return false;
        };
        let doc = self.doc.sketcher.document();
        let Some(RecipeKind::Offset { distance, expr, .. }) =
            doc.recipe(e.rid).map(|r| r.kind.clone())
        else {
            return false;
        };
        let now = locus_doc::offset::distance(
            doc,
            &RecipeKind::Offset {
                distance,
                sharp: e.sharp,
                expr: expr.clone(),
            },
        );
        if now == Some(e.distance) {
            return false;
        }
        self.apply_reporting(&Command::EditRecipe {
            id: e.rid,
            kind: RecipeKind::Offset {
                distance: e.distance,
                sharp: e.sharp,
                expr: None,
            },
        })
    }

    /// Pointer input on an existing offset's arrow, with Select or the
    /// Offset tool. Returns true when the pointer was used here.
    pub(crate) fn offset_edit_pointer(
        &mut self,
        ui: &egui::Ui,
        resp: &egui::Response,
        rect: Rect,
    ) -> bool {
        let cam = self.view.camera;
        if self.tools.offset_edit.is_some() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            if let Some(p) = ui.input(|i| i.pointer.latest_pos()) {
                let coarse = ui.input(|i| i.modifiers.shift);
                self.offset_edit_drag_to(cam.to_world(p, rect), coarse);
            }
            if !ui.input(|i| i.pointer.primary_down()) {
                self.offset_edit_release();
            }
            return true;
        }
        let Some(rid) = self.offset_handle_target() else {
            return false;
        };
        let Some((_, _, _, tip)) = self.offset_handle_at(rid) else {
            return false;
        };
        let tip = cam.to_screen(tip, rect);
        let on = |p: egui::Pos2| p.distance(tip) <= GRAB_PT;
        // Taken on the press: with Select, a press on a curve would
        // otherwise start dragging the geometry under the arrow.
        if resp.hovered()
            && ui.input(|i| i.pointer.primary_pressed())
            && ui.input(|i| i.pointer.press_origin()).is_some_and(on)
        {
            self.offset_edit_begin(rid);
            return true;
        }
        if resp.hover_pos().is_some_and(on) {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        false
    }

    /// Remember where a click landed, so an offset picked or selected by it
    /// gets its arrow there. Called for every click the arrow doesn't take.
    pub(crate) fn offset_note_click(&mut self, resp: &egui::Response, rect: Rect) {
        if resp.clicked_by(PointerButton::Primary)
            && let Some(p) = resp.interact_pointer_pos()
        {
            self.tools.offset_anchor = Some(self.view.camera.to_world(p, rect));
        }
    }

    /// Work out this frame's arrow (after input, before drawing): drawing
    /// borrows the view, so it reads this instead of solving anything.
    pub(crate) fn refresh_offset_edit(&mut self) {
        let rid = self
            .tools
            .offset_edit
            .as_ref()
            .map(|e| e.rid)
            .or_else(|| self.offset_handle_target());
        self.tools.offset_arrow = rid.and_then(|rid| {
            let (base, plus, d, tip) = self.offset_handle_at(rid)?;
            Some((rid, base, plus, d, tip))
        });
    }

    /// The arrow on the target offset, and the drag's preview.
    pub(crate) fn draw_offset_edit(&self, frame: &Frame<'_>) {
        if let Some(e) = self.tools.offset_edit.as_ref() {
            frame.draw_preview(&e.preview);
        }
        let Some((_, base, plus, distance, tip)) = self.tools.offset_arrow else {
            return;
        };
        let s = |w| frame.view.to_screen(w, frame.rect);
        let dragging = self.tools.offset_edit.is_some();
        draw_offset_arrow(frame, base, plus, distance, s(base), s(tip), dragging);
    }
}

/// The offset curve nearest `p` among `shapes`, for tests.
#[cfg(test)]
fn nearest(shapes: &[Shape], p: Vec2) -> f64 {
    shapes
        .iter()
        .map(|s| closest(s, p).distance(p))
        .fold(f64::INFINITY, f64::min)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // fixtures built in the test
mod tests {
    use super::*;
    use crate::modify_tool::Modify;
    use crate::settings::Units;
    use crate::tool::ToolEvent;
    use crate::tools::Pick;
    use locus_doc::{Document, EntityId};

    /// A 20 × 10 rectangle with a 2 mm sharp offset outward, made with the
    /// Offset tool; returns the editor and the offset.
    fn offset() -> (AppState, RecipeId) {
        let mut c = AppState::default();
        c.load(Document::new());
        c.set_tool(Tool::RectCorner);
        for p in [Vec2::ZERO, Vec2::new(20.0, 10.0)] {
            c.tool_event(ToolEvent::Click(Pick::free(p)), None, Units::Mm);
        }
        let doc = c.doc.sketcher.document();
        let bottom = doc
            .entities()
            .iter()
            .map(|(id, _)| id)
            .find(|id| {
                doc.line(*id)
                    .is_some_and(|l| l.a.y.abs() < 1e-9 && l.b.y.abs() < 1e-9)
            })
            .unwrap();
        c.set_modify(Modify::Offset);
        c.tool_event(
            ToolEvent::Click(Pick::free(Vec2::new(10.0, 0.0))),
            Some(bottom),
            Units::Mm,
        );
        c.tools.corner.text.offset = "2".to_owned();
        assert!(c.offset_ok(Units::Mm));
        let rid = *c.doc.sketcher.document().recipes().keys().last().unwrap();
        (c, rid)
    }

    fn result(c: &AppState, rid: RecipeId) -> Vec<Shape> {
        let doc = c.doc.sketcher.document();
        c.doc
            .scene
            .items
            .iter()
            .filter(|i| doc.driven_by(i.id) == Some(rid) && !matches!(i.shape, Shape::Point(_)))
            .map(|i| i.shape)
            .collect()
    }

    /// #444: a picked offset shows the arrow on its result; dragging it
    /// previews the new distance without touching the document, and the
    /// release is one undo step.
    #[test]
    fn dragging_an_existing_offsets_arrow_sets_its_distance() {
        let (mut c, rid) = offset();
        assert_eq!(c.tools.offset_pick, Some(rid), "the new offset is picked");
        assert_eq!(c.offset_handle_target(), Some(rid));
        let (base, plus, d, tip) = c.offset_handle_at(rid).unwrap();
        assert_eq!(d, 2.0);
        assert!(
            base.distance(Vec2::new(10.0, 0.0)) < 1e-6,
            "where the draft's arrow was, not under the dialog: {base:?}"
        );
        assert!(nearest(&result(&c, rid), tip) < 1e-6, "on the result");
        assert!((tip.distance(base) - 2.0).abs() < 1e-6, "2 mm out");
        let before = c.doc.sketcher.document().clone();
        assert!(c.offset_edit_begin(rid));
        c.offset_edit_drag_to(base + plus * 5.04, false);
        assert_eq!(c.doc.sketcher.document(), &before, "nothing committed yet");
        let (_, _, d, tip) = c.offset_handle_at(rid).unwrap();
        assert!((d - 5.0).abs() < 1e-9, "{d}");
        assert!(
            (tip.distance(base) - 5.0).abs() < 1e-6,
            "the handle follows"
        );
        assert!(c.offset_edit_release());
        assert_eq!(c.offset_params(rid).map(|p| p.0), Some(5.0));
        assert!(c.tools.offset_edit.is_none());
        c.undo();
        assert_eq!(
            c.doc.sketcher.document().recipes(),
            before.recipes(),
            "one step"
        );
    }

    /// #444: selecting an offset's curves with Select shows the arrow too;
    /// a drag replaces an expression with the number, and undo restores it.
    #[test]
    fn a_selected_offset_is_dragged_and_its_expression_replaced() {
        let (mut c, rid) = offset();
        assert!(c.apply_reporting(&Command::SetParameter {
            name: "gap".to_owned(),
            expr: "3".to_owned(),
        }));
        assert!(c.apply_reporting(&Command::EditRecipe {
            id: rid,
            kind: RecipeKind::Offset {
                distance: 3.0,
                sharp: true,
                expr: Some("gap".to_owned()),
            },
        }));
        c.set_tool(Tool::Select);
        assert_eq!(c.offset_handle_target(), None, "nothing selected");
        let curves: Vec<EntityId> = c
            .doc
            .sketcher
            .document()
            .recipe(rid)
            .unwrap()
            .instances()
            .collect();
        c.selection.items.set_all(curves);
        assert_eq!(c.offset_handle_target(), Some(rid));
        // Selected by a click near the right-hand side: the arrow is there.
        c.tools.offset_anchor = Some(Vec2::new(22.0, 5.0));
        let (base, ..) = c.offset_handle_at(rid).unwrap();
        assert!(base.distance(Vec2::new(20.0, 5.0)) < 1e-6, "{base:?}");
        let (base, plus, d, _) = c.offset_handle_at(rid).unwrap();
        assert_eq!(d, 3.0);
        assert!(c.offset_edit_begin(rid));
        c.offset_edit_drag_to(base + plus * 1.0, true);
        assert!(c.offset_edit_release());
        let kind = |c: &AppState| c.doc.sketcher.document().recipe(rid).unwrap().kind.clone();
        assert!(matches!(
            kind(&c),
            RecipeKind::Offset { distance, expr: None, .. } if distance == 1.0
        ));
        c.undo();
        assert!(matches!(
            kind(&c),
            RecipeKind::Offset { expr: Some(e), .. } if e == "gap"
        ));
        // A drag that ends where it started commits nothing.
        assert!(c.offset_edit_begin(rid));
        assert!(!c.offset_edit_release());
    }
}
