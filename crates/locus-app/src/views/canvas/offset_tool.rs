// SPDX-License-Identifier: AGPL-3.0-or-later
//! The Offset tool, Fusion style (#218, design §6.6, SPEC §6.2, ADR 0010,
//! 0011).
//!
//! Clicking a curve picks its connected chain (Chain is ticked by default).
//! The distance can then be set two ways, which are one value:
//! - drag the arrow on the canvas. Dragging across the curve switches
//!   sides.
//! - type a number or an expression in the dialog, whose field takes focus.
//!
//! A live preview shows the result, and ⇅ or Flip reverses the side.
//! Enter or OK adds the offset recipe as one command (§11). Its distance
//! stays an editable label on the canvas. Clicking an existing offset picks
//! it for editing instead.

use super::corner_dialog::field;
use crate::render::Frame;
use crate::scene::Shape;
use crate::settings::Units;
use crate::state::AppState;
use egui::{Align2, FontId, PointerButton, Pos2, Rect, Stroke, Vec2 as EVec2};
use locus_doc::recipe::RecipeKind;
use locus_doc::{Command, Document, EntityId};
use locus_geom::Vec2;

/// Screen length of the arrow at zero distance, and its grab radius.
const MIN_ARROW_PT: f32 = 36.0;
const GRAB_PT: f32 = 10.0;
/// The ⇅ flip control sits this far behind the arrow's base.
const FLIP_PT: f32 = 22.0;

/// An offset being set up: picked, not yet committed.
#[derive(Clone, Debug)]
pub struct OffsetDraft {
    /// The curve that was clicked.
    pub pick: EntityId,
    /// The curves offset: the chain through `pick`, or `pick` alone.
    pub sources: Vec<EntityId>,
    /// The point on the picked curve nearest the click: the arrow's base.
    pub base: Vec2,
    /// Unit direction a positive distance goes at `base`.
    pub plus: Vec2,
    /// The arrow is being dragged.
    pub dragging: bool,
    /// The dialog's field should take focus (just picked).
    pub focus: bool,
    /// The preview for `key`: (distance, sharp, sources).
    preview: Option<((u64, bool, usize), Vec<Shape>)>,
}

/// The point of `s` nearest `p`.
fn closest(s: &Shape, p: Vec2) -> Vec2 {
    match s {
        Shape::Point(q) => *q,
        Shape::Line(l) => l.closest_point(p),
        Shape::Circle(c) => c.closest_point(p),
        Shape::Arc(a) => a.closest_point(p),
        Shape::Cubic(c) => c.closest_point(p),
    }
}

/// The curves an offset recipe over `sources` would make, from a scratch
/// copy of the document (nothing is committed).
fn offset_shapes(doc: &Document, sources: &[EntityId], distance: f64, sharp: bool) -> Vec<Shape> {
    let cmd = Command::AddRecipe {
        kind: RecipeKind::Offset {
            distance,
            sharp,
            expr: None,
        },
        sources: sources.to_vec(),
    };
    super::pattern_tool::recipe_preview(doc, &cmd)
}

/// Which way a positive distance goes at `base`: towards the offset made
/// with a small positive distance. The recipe's own sign rule (closed
/// chains grow, open ones go left) decides, so the arrow always agrees with
/// the result.
fn plus_direction(doc: &Document, sources: &[EntityId], base: Vec2, picked: &Shape) -> Vec2 {
    let probe = (picked.bbox().max - picked.bbox().min).length().max(1.0) * 0.01;
    let shapes = offset_shapes(doc, sources, probe, true);
    shapes
        .iter()
        .map(|s| closest(s, base))
        .min_by(|a, b| a.distance(base).total_cmp(&b.distance(base)))
        .map(|q| q - base)
        .filter(|d| d.length() > f64::EPSILON)
        .map_or(Vec2::new(0.0, 1.0), |d| d / d.length())
}

impl AppState {
    /// A click with the Offset tool on curve `id` at `at`: pick an existing
    /// offset, or start a draft on the curve's chain.
    pub(crate) fn offset_click(&mut self, id: EntityId, at: Vec2, units: Units) {
        let doc = self.doc.sketcher.document();
        if let Some(rid) = doc.driven_by(id)
            && matches!(
                doc.recipe(rid).map(|r| &r.kind),
                Some(RecipeKind::Offset { .. })
            )
        {
            self.tools.offset_draft = None;
            self.tools.offset_pick = Some(rid);
            self.edit_offset(rid, units);
            return;
        }
        let sources = self.offset_sources(id);
        if sources.is_empty() {
            self.doc.notice = Some("That curve can't be offset".to_owned());
            return;
        }
        let Some(item) = self.doc.scene.items.iter().find(|i| i.id == id) else {
            return;
        };
        let base = closest(&item.shape, at);
        let plus = plus_direction(doc, &sources, base, &item.shape);
        self.selection.items.set_all(sources.iter().copied());
        self.tools.offset_pick = None;
        self.tools.offset_draft = Some(OffsetDraft {
            pick: id,
            sources,
            base,
            plus,
            dragging: false,
            focus: true,
            preview: None,
        });
        self.doc.notice = None;
    }

    /// The chain through `id`, or `id` alone with Chain unticked.
    fn offset_sources(&self, id: EntityId) -> Vec<EntityId> {
        let doc = self.doc.sketcher.document();
        let chain = locus_doc::offset::chain_through(doc, id);
        if self.tools.corner.offset_chain || chain.is_empty() {
            chain
        } else {
            vec![id]
        }
    }

    /// The draft's distance (mm) and expression, from the dialog's field.
    pub(crate) fn offset_value(&self, units: Units) -> Result<(f64, Option<String>), String> {
        let (d, expr) = field(
            &self.tools.corner.text.offset,
            false,
            units,
            self.doc.sketcher.document(),
        )?;
        if d == 0.0 || !d.is_finite() {
            return Err("The distance can't be zero".to_owned());
        }
        Ok((d, expr))
    }

    /// Set the distance from the arrow or Flip, as a plain number.
    fn set_offset_distance(&mut self, mm: f64, units: Units) {
        self.tools.corner.text.offset = crate::state::format_value(mm, false, units);
    }

    /// Enter / OK: add the offset as one undo step and pick it, so its
    /// distance can be edited at once. The tool stays for the next one.
    pub(crate) fn offset_ok(&mut self, units: Units) -> bool {
        let Some(draft) = self.tools.offset_draft.clone() else {
            return false;
        };
        let (distance, expr) = match self.offset_value(units) {
            Ok(v) => v,
            Err(why) => {
                self.doc.notice = Some(why);
                return false;
            }
        };
        let cmd = Command::AddRecipe {
            kind: RecipeKind::Offset {
                distance,
                sharp: self.tools.corner.sharp,
                expr,
            },
            sources: draft.sources,
        };
        let ok = self.apply_reporting(&cmd);
        if ok {
            self.tools.offset_draft = None;
            self.selection.items.clear();
            self.tools.offset_pick = self
                .doc
                .sketcher
                .document()
                .recipes()
                .keys()
                .last()
                .copied();
        }
        ok
    }

    /// Esc: drop the draft (or the picked offset). Returns whether there was
    /// one, so a second Esc leaves the tool.
    pub(crate) fn offset_esc(&mut self) -> bool {
        let had =
            self.tools.offset_draft.take().is_some() || self.tools.offset_pick.take().is_some();
        if had {
            self.selection.items.clear();
            self.doc.notice = None;
        }
        had
    }

    /// Flip the side without changing the size (⇅, design §6.6).
    pub(crate) fn offset_flip(&mut self, units: Units) {
        let text = self.tools.corner.text.offset.trim().to_owned();
        if let Some(v) = crate::state::parse_number(&text) {
            let mm = if units == Units::Inch { v * 25.4 } else { v };
            self.set_offset_distance(-mm, units);
        } else if let Some(inner) = text.strip_prefix("-(").and_then(|t| t.strip_suffix(')')) {
            self.tools.corner.text.offset = inner.to_owned();
        } else {
            self.tools.corner.text.offset = format!("-({text})");
        }
    }

    /// Screen positions of the arrow's base, tip and the ⇅ control.
    fn offset_arrow(&self, d: &OffsetDraft, distance: f64, rect: Rect) -> (Pos2, Pos2, Pos2) {
        let cam = &self.view.camera;
        let base = cam.to_screen(d.base, rect);
        let ahead = cam.to_screen(d.base + d.plus, rect) - base;
        let dir = if ahead.length() > 0.0 {
            ahead.normalized()
        } else {
            EVec2::Y
        };
        let len = (distance.abs() as f32 * cam.pt_per_mm as f32).max(MIN_ARROW_PT);
        let sign = if distance < 0.0 { -1.0 } else { 1.0 };
        let tip = base + dir * len * sign;
        let flip = base - dir * FLIP_PT * sign;
        (base, tip, flip)
    }

    /// Pointer input while Offset is active: drag the arrow, click ⇅, or
    /// pick a curve. Returns true when the pointer was used here.
    pub(crate) fn offset_pointer(
        &mut self,
        ui: &egui::Ui,
        resp: &egui::Response,
        rect: Rect,
        units: Units,
    ) -> bool {
        let Some(d) = self.tools.offset_draft.clone() else {
            return false;
        };
        let distance = self.offset_value(units).map_or(0.0, |v| v.0);
        let (_, tip, flip) = self.offset_arrow(&d, distance, rect);
        let press = super::press_pos(ui, resp);
        if resp.drag_started_by(PointerButton::Primary)
            && press.is_some_and(|p| p.distance(tip) <= GRAB_PT)
            && let Some(draft) = self.tools.offset_draft.as_mut()
        {
            draft.dragging = true;
        }
        if d.dragging || self.tools.offset_draft.as_ref().is_some_and(|x| x.dragging) {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            if let Some(p) = resp.interact_pointer_pos() {
                let w = self.view.camera.to_world(p, rect);
                let step = if ui.input(|i| i.modifiers.shift) {
                    1.0
                } else {
                    0.1
                };
                let mm = ((w - d.base).dot(d.plus) / step).round() * step;
                if mm != 0.0 {
                    self.set_offset_distance(mm, units);
                }
            }
            if !resp.dragged_by(PointerButton::Primary)
                && let Some(draft) = self.tools.offset_draft.as_mut()
            {
                draft.dragging = false;
            }
            return true;
        }
        if let Some(h) = resp.hover_pos()
            && (h.distance(tip) <= GRAB_PT || h.distance(flip) <= GRAB_PT)
        {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if resp.clicked_by(PointerButton::Primary)
            && press.is_some_and(|p| p.distance(flip) <= GRAB_PT)
        {
            self.offset_flip(units);
            return true;
        }
        // A click on the arrow itself isn't a new pick.
        press.is_some_and(|p| p.distance(tip) <= GRAB_PT) && resp.clicked_by(PointerButton::Primary)
    }

    /// Recompute the preview when the distance, corners or chain changed:
    /// it comes from a scratch copy of the document, so not every frame.
    pub(crate) fn refresh_offset_preview(&mut self, units: Units) {
        let Some(d) = self.tools.offset_draft.as_ref() else {
            return;
        };
        let Ok((mm, _)) = self.offset_value(units) else {
            if let Some(draft) = self.tools.offset_draft.as_mut() {
                draft.preview = None;
            }
            return;
        };
        let key = (mm.to_bits(), self.tools.corner.sharp, d.sources.len());
        if d.preview.as_ref().is_some_and(|(k, _)| *k == key) {
            return;
        }
        let shapes = offset_shapes(
            self.doc.sketcher.document(),
            &d.sources,
            mm,
            self.tools.corner.sharp,
        );
        if let Some(draft) = self.tools.offset_draft.as_mut() {
            draft.preview = Some((key, shapes));
        }
    }

    /// The preview, the arrow, ⇅ and the heads-up distance.
    pub(crate) fn draw_offset_draft(&self, frame: &Frame<'_>) {
        let units = frame.units;
        let Some(d) = self.tools.offset_draft.as_ref() else {
            return;
        };
        let distance = self.offset_value(units).map_or(0.0, |v| v.0);
        if let Some((_, shapes)) = d.preview.as_ref() {
            frame.draw_preview(shapes);
        }
        let (base, tip, flip) = self.offset_arrow(d, distance, frame.rect);
        let c = frame.pal.selected;
        let p = frame.painter;
        let w = if d.dragging { 3.0 } else { 2.0 };
        p.line_segment([base, tip], Stroke::new(w, c));
        let dir = (tip - base).normalized();
        let side = EVec2::new(-dir.y, dir.x) * 6.0;
        p.add(egui::Shape::convex_polygon(
            vec![tip + dir * 12.0, tip + side, tip - side],
            c,
            Stroke::NONE,
        ));
        p.circle_filled(base, 3.0, c);
        // ⇅ drawn as two opposed arrowheads along the arrow's axis (the
        // default fonts have no such glyph).
        p.circle_stroke(flip, 9.0, Stroke::new(1.5, c));
        let half = EVec2::new(-dir.y, dir.x) * 2.5;
        for s in [1.0, -1.0] {
            let tip2 = flip + dir * 6.0 * s;
            p.add(egui::Shape::convex_polygon(
                vec![
                    tip2,
                    flip + dir * 1.5 * s + half,
                    flip + dir * 1.5 * s - half,
                ],
                c,
                Stroke::NONE,
            ));
        }
        // The read-out beyond the tip, on the side the arrow points to.
        let anchor = if dir.x < -0.3 {
            Align2::RIGHT_CENTER
        } else {
            Align2::LEFT_CENTER
        };
        p.text(
            tip + dir * 18.0,
            anchor,
            format!("offset {}", units.format_len(distance)),
            FontId::monospace(13.0),
            frame.pal.hover,
        );
    }

    /// The Offset command dialog: the draft's fields, or the picked offset's.
    pub(crate) fn offset_dialog(&mut self, ui: &egui::Ui, rect: Rect, units: Units) {
        let draft = self
            .tools
            .offset_draft
            .as_ref()
            .map(|d| (d.sources.len(), d.focus, d.pick));
        if let Some(d) = self.tools.offset_draft.as_mut() {
            d.focus = false;
        }
        let picked = self.offset_selected();
        let error = draft.and_then(|_| self.offset_value(units).err());
        let (mut ok, mut cancel, mut flip, mut rechain) = (false, false, false, false);
        let mut explode = None;
        let mut remove = None;
        let mut edited = None;
        let unit = units.label();
        let params = picked.and_then(|rid| self.offset_params(rid));
        super::dialog::command_dialog(ui, rect, "Offset", |ui| {
            let c = &mut self.tools.corner;
            match draft {
                Some((n, focus, _)) => {
                    ui.label(format!("Curves: {n} picked"));
                    rechain = ui
                        .checkbox(&mut c.offset_chain, "Chain")
                        .on_hover_text("Offset every curve joined end to end with the picked one")
                        .changed();
                    ui.horizontal(|ui| {
                        ui.label("Distance");
                        let out = egui::TextEdit::singleline(&mut c.text.offset)
                            .desired_width(90.0)
                            .show(ui);
                        let r = out.response.response.clone();
                        if focus {
                            super::corner_dialog::focus_all(ui, &r, out.state, &c.text.offset);
                        }
                        if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            ok = true;
                        }
                        r.on_hover_text("A number, or an expression over parameters; + grows a closed chain or goes left");
                        ui.weak(unit);
                        flip = ui
                            .button("Flip")
                            .on_hover_text("Offset to the other side, keeping the distance")
                            .clicked();
                    });
                    ui.horizontal(|ui| {
                        ui.label("Corners");
                        ui.selectable_value(&mut c.sharp, false, "Round");
                        ui.selectable_value(&mut c.sharp, true, "Sharp");
                    });
                    if let Some(why) = &error {
                        ui.colored_label(ui.visuals().error_fg_color, why);
                    }
                    ui.horizontal(|ui| {
                        ok |= ui
                            .add_enabled(error.is_none(), egui::Button::new("OK"))
                            .clicked();
                        cancel = ui.button("Cancel").clicked();
                    });
                }
                None => match (picked, params) {
                    (Some(rid), Some((mut d, mut sharp))) => {
                        ui.label("Picked offset");
                        ui.horizontal(|ui| {
                            ui.label("Distance");
                            // Typed, not dragged: each change is one undo step.
                            let r1 = ui
                                .add(egui::DragValue::new(&mut d).speed(0.0).max_decimals(3).suffix(" mm"))
                                .on_hover_text("Click to type, or double-click the label on the canvas for an expression");
                            let r2 = ui.checkbox(&mut sharp, "Sharp");
                            if (r1.changed() || r2.changed()) && d != 0.0 {
                                edited = Some((rid, d, sharp));
                            }
                        });
                        ui.horizontal(|ui| {
                            if ui
                                .button("Explode")
                                .on_hover_text("Turn the offset into ordinary geometry")
                                .clicked()
                            {
                                explode = Some(rid);
                            }
                            if ui.button("Remove").clicked() {
                                remove = Some(rid);
                            }
                            cancel = ui.button("Close").clicked();
                        });
                    }
                    _ => {
                        ui.label("Click a curve to offset its chain, or an offset to edit it");
                        cancel = ui.button("Cancel").clicked();
                    }
                },
            }
        });
        if let Some((rid, d, sharp)) = edited {
            self.set_offset_params(rid, d, sharp);
        }
        if let Some(rid) = explode {
            self.finish_offset(rid, true);
        }
        if let Some(rid) = remove {
            self.finish_offset(rid, false);
        }
        if rechain && let Some((_, _, pick)) = draft {
            let sources = self.offset_sources(pick);
            self.selection.items.set_all(sources.iter().copied());
            if let Some(d) = self.tools.offset_draft.as_mut() {
                d.sources = sources;
                d.preview = None;
            }
        }
        if flip {
            self.offset_flip(units);
        }
        if ok {
            self.offset_ok(units);
        } else if cancel && !self.offset_esc() {
            self.set_tool(crate::tools::Tool::Select);
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)] // a missing fixture should fail loudly
mod tests {
    use super::*;
    use crate::modify_tool::Modify;
    use crate::tool::ToolEvent;
    use crate::tools::{Pick, Tool};

    /// A blank editor with a 20 × 10 rectangle and the Offset tool picking
    /// its bottom edge at (10, 0).
    fn drafted() -> (AppState, EntityId) {
        let mut c = AppState::default();
        c.load(Document::new());
        c.set_tool(Tool::RectCorner);
        for p in [Vec2::ZERO, Vec2::new(20.0, 10.0)] {
            c.tool_event(ToolEvent::Click(Pick::free(p)), None, Units::Mm);
        }
        c.set_modify(Modify::Offset);
        let doc = c.doc.sketcher.document();
        let bottom = doc
            .entities()
            .iter()
            .find(|(id, _)| {
                doc.line(*id)
                    .is_some_and(|l| l.a.y.abs() < 1e-9 && l.b.y.abs() < 1e-9)
            })
            .map(|(id, _)| id)
            .expect("the bottom edge");
        let at = Vec2::new(10.0, 0.0);
        c.tool_event(ToolEvent::Click(Pick::free(at)), Some(bottom), Units::Mm);
        (c, bottom)
    }

    fn recipes(c: &AppState) -> usize {
        c.doc.sketcher.document().recipes().len()
    }

    /// Owner hand-check (#218): a closed profile takes one pick, one value
    /// and Enter.
    #[test]
    fn one_pick_one_value_enter() {
        let (mut c, _) = drafted();
        let d = c
            .tools
            .offset_draft
            .clone()
            .expect("a draft, nothing committed yet");
        assert_eq!(recipes(&c), 0);
        assert_eq!(d.sources.len(), 4, "the whole chain");
        // + grows a closed chain, so it points down (outside) at the bottom.
        assert!(d.plus.y < -0.99, "{:?}", d.plus);
        c.tools.corner.text.offset = "3".to_owned();
        assert!(c.offset_ok(Units::Mm));
        assert_eq!(recipes(&c), 1);
        assert_eq!(c.undo_label(), Some("Add offset"));
        assert!(c.offset_selected().is_some(), "picked for editing");
        assert!(c.tools.offset_draft.is_none());
        assert_eq!(c.modify(), Some(Modify::Offset), "ready for the next one");
    }

    #[test]
    fn flip_reverses_the_side_and_keeps_the_size() {
        let (mut c, _) = drafted();
        c.tools.corner.text.offset = "2.5".to_owned();
        c.offset_flip(Units::Mm);
        assert_eq!(c.offset_value(Units::Mm).ok().map(|v| v.0), Some(-2.5));
        c.offset_flip(Units::Mm);
        assert_eq!(c.offset_value(Units::Mm).ok().map(|v| v.0), Some(2.5));
        // An expression flips by wrapping it.
        assert!(c.apply_reporting(&Command::SetParameter {
            name: "kerf".to_owned(),
            expr: "0.2".to_owned(),
        }));
        c.tools.corner.text.offset = "kerf".to_owned();
        c.offset_flip(Units::Mm);
        assert_eq!(c.tools.corner.text.offset, "-(kerf)");
        let (v, e) = c.offset_value(Units::Mm).expect("evaluates");
        assert!((v + 0.2).abs() < 1e-12 && e.as_deref() == Some("-(kerf)"));
        c.offset_flip(Units::Mm);
        assert_eq!(c.tools.corner.text.offset, "kerf");
        // An expression distance is kept on the recipe.
        assert!(c.offset_ok(Units::Mm));
        let doc = c.doc.sketcher.document();
        assert!(doc.recipes().values().any(|r| matches!(
            &r.kind,
            RecipeKind::Offset { expr: Some(e), .. } if e == "kerf"
        )));
    }

    #[test]
    fn the_preview_matches_the_commit_and_esc_cancels() {
        let (c, _) = drafted();
        let d = c.tools.offset_draft.clone().expect("draft");
        let shapes = offset_shapes(c.doc.sketcher.document(), &d.sources, 2.0, true);
        assert_eq!(
            shapes.len(),
            4,
            "a sharp offset of a rectangle is four lines"
        );
        let (mut c, bottom) = drafted();
        // Chain unticked: only the picked edge.
        c.tools.corner.offset_chain = false;
        assert_eq!(c.offset_sources(bottom), vec![bottom]);
        assert!(c.offset_esc());
        assert!(!c.offset_esc());
        assert_eq!(recipes(&c), 0);
        // Zero is refused with a reason.
        let (mut c, _) = drafted();
        c.tools.corner.text.offset = "0".to_owned();
        assert!(!c.offset_ok(Units::Mm));
        assert!(c.doc.notice.is_some());
    }
}
