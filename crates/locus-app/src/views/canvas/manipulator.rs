// SPDX-License-Identifier: AGPL-3.0-or-later
//! Move/Copy, Rotate and Sketch Scale on the canvas (#219, design §6.7).
//!
//! Each tool works on the selection. Before anything is selected the canvas
//! selects as usual, and once something is selected a manipulator sits on it:
//! - Move has X and Y arrows and a free-drag centre.
//! - Rotate has a ring with a knob and a pivot square.
//! - Scale has a corner handle and a base-point square.
//!
//! Dragging a grip, typing on the canvas or editing the command dialog
//! changes the same values (two views of one value), and a ghost of the
//! result follows. Enter or OK applies the change as one command (SPEC §11).
//! Esc resets the values, and a second Esc returns to Select.

use super::dialog;
use crate::hit;
use crate::modify_tool::{Grip, Modify, TransformMode};
use crate::render::Frame;
use crate::scene::Shape;
use crate::settings::Units;
use crate::state::AppState;
use crate::tools::Tool;
use egui::{Align2, Color32, FontId, PointerButton, Pos2, Rect, Stroke, Vec2 as EVec2};
use locus_doc::transform::Transform;
use locus_geom::{Arc, Circle, Cubic, Line, Vec2};

/// Arrow length, ring radius and handle sizes, in screen points.
const ARROW_PT: f32 = 70.0;
const RING_PT: f32 = 60.0;
const KNOB_PT: f32 = 7.0;
const SQUARE_PT: f32 = 7.0;
const GRAB_PT: f32 = 7.0;

/// The shape `t` makes of `s`. Rotations and positive scales keep arcs
/// counter-clockwise (SPEC §4).
pub(crate) fn moved_shape(s: &Shape, t: &Transform) -> Shape {
    let p = |v: Vec2| t.point(v);
    match *s {
        Shape::Point(v) => Shape::Point(p(v)),
        Shape::Line(l) => Shape::Line(Line::new(p(l.a), p(l.b))),
        Shape::Circle(c) => Shape::Circle(Circle {
            center: p(c.center),
            radius: t.length(c.radius),
        }),
        Shape::Arc(a) => Shape::Arc(Arc {
            center: p(a.center),
            start: p(a.start),
            end: p(a.end),
        }),
        Shape::Cubic(c) => Shape::Cubic(Cubic {
            p0: p(c.p0),
            c0: p(c.c0),
            c1: p(c.c1),
            p1: p(c.p1),
        }),
    }
}

/// Where the manipulator's parts are on screen.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Gizmo {
    pub mode: TransformMode,
    /// The centre square: the moved centre in Move, the pivot otherwise.
    pub origin: Pos2,
    pub angle_deg: f64,
    pub factor: f64,
}

impl Gizmo {
    fn knob(&self) -> Pos2 {
        let a = self.angle_deg.to_radians() as f32;
        self.origin + EVec2::new(a.cos(), -a.sin()) * RING_PT
    }

    /// The scale handle runs out along the diagonal with the factor,
    /// clamped so it stays on screen and grabbable.
    fn corner(&self) -> Pos2 {
        let f = self.factor.clamp(0.25, 3.0) as f32;
        self.origin + EVec2::new(1.0, -1.0).normalized() * RING_PT * f
    }

    /// The grip under the screen point `p`, if any.
    pub fn grip_at(&self, p: Pos2) -> Option<Grip> {
        let near_square = (p.x - self.origin.x).abs() <= SQUARE_PT + 2.0
            && (p.y - self.origin.y).abs() <= SQUARE_PT + 2.0;
        match self.mode {
            TransformMode::Move => {
                if near_square {
                    return Some(Grip::MoveFree);
                }
                let along = |dir: EVec2| {
                    let d = p - self.origin;
                    let t = d.dot(dir);
                    (0.0..=ARROW_PT + 6.0).contains(&t) && (d - dir * t).length() <= GRAB_PT
                };
                if along(EVec2::X) {
                    Some(Grip::MoveX)
                } else if along(-EVec2::Y) {
                    Some(Grip::MoveY)
                } else {
                    None
                }
            }
            TransformMode::Rotate => {
                if p.distance(self.knob()) <= KNOB_PT + 3.0 {
                    Some(Grip::Ring)
                } else if near_square {
                    Some(Grip::Pivot)
                } else if (p.distance(self.origin) - RING_PT).abs() <= GRAB_PT {
                    Some(Grip::Ring)
                } else {
                    None
                }
            }
            TransformMode::Scale => {
                if p.distance(self.corner()) <= KNOB_PT + 3.0 {
                    Some(Grip::Factor)
                } else if near_square {
                    Some(Grip::Pivot)
                } else {
                    None
                }
            }
        }
    }

    fn draw(&self, painter: &egui::Painter, hot: Option<Grip>, colors: [Color32; 3]) {
        let [xc, yc, accent] = colors;
        let width = |g: Grip| if hot == Some(g) { 3.0 } else { 2.0 };
        let o = self.origin;
        let square = Rect::from_center_size(o, EVec2::splat(SQUARE_PT * 2.0));
        let sq_grip = match self.mode {
            TransformMode::Move => Grip::MoveFree,
            _ => Grip::Pivot,
        };
        match self.mode {
            TransformMode::Move => {
                for (dir, c, g) in [(EVec2::X, xc, Grip::MoveX), (-EVec2::Y, yc, Grip::MoveY)] {
                    let tip = o + dir * ARROW_PT;
                    painter.line_segment([o + dir * SQUARE_PT, tip], Stroke::new(width(g), c));
                    let side = EVec2::new(-dir.y, dir.x) * 6.0;
                    painter.add(egui::Shape::convex_polygon(
                        vec![tip + dir * 12.0, tip + side, tip - side],
                        c,
                        Stroke::NONE,
                    ));
                }
            }
            TransformMode::Rotate => {
                painter.circle_stroke(o, RING_PT, Stroke::new(width(Grip::Ring), accent));
                painter.line_segment([o, self.knob()], Stroke::new(1.0, accent));
                painter.circle_filled(self.knob(), KNOB_PT, accent);
            }
            TransformMode::Scale => {
                let c = self.corner();
                painter.line_segment([o, c], Stroke::new(width(Grip::Factor), accent));
                let r = Rect::from_center_size(c, EVec2::splat(KNOB_PT * 2.0));
                painter.rect_filled(r, 2.0, accent);
            }
        }
        painter.rect_stroke(
            square,
            2.0,
            Stroke::new(width(sq_grip), accent),
            egui::StrokeKind::Middle,
        );
    }
}

impl AppState {
    /// The transform tool's mode, while one is active.
    pub(crate) fn transform_mode(&self) -> Option<TransformMode> {
        match self.modify() {
            Some(Modify::Transform(m)) => Some(m),
            _ => None,
        }
    }

    /// The manipulator for `mode` on the current selection, if there is one.
    pub(crate) fn gizmo(&self, mode: TransformMode, rect: Rect) -> Option<Gizmo> {
        let center = self.selection_center()?;
        let t = &self.tools.transform;
        let at = match mode {
            TransformMode::Move => center + Vec2::new(t.dx, t.dy),
            _ => t.pivot.unwrap_or(center),
        };
        Some(Gizmo {
            mode,
            origin: self.view.camera.to_screen(at, rect),
            angle_deg: t.angle_deg,
            factor: t.factor,
        })
    }

    /// Pointer and keys while Move, Rotate or Scale is active.
    pub(crate) fn transform_input(
        &mut self,
        ui: &egui::Ui,
        resp: &egui::Response,
        rect: Rect,
        units: Units,
        mode: TransformMode,
    ) {
        use egui::{Key, Modifiers};
        let over = resp.hovered();
        let selected = self.can_transform();
        // Keys first, consumed, so Select's own Esc / Enter don't see them.
        if over || !self.tools.typed.is_empty() || self.tools.transform.grip.is_some() {
            let (esc, enter, text, back) = ui.ctx().input_mut(|i| {
                let text: String = i
                    .events
                    .iter()
                    .filter_map(|e| match e {
                        egui::Event::Text(t) => Some(t.clone()),
                        _ => None,
                    })
                    .collect();
                (
                    i.consume_key(Modifiers::NONE, Key::Escape),
                    i.consume_key(Modifiers::NONE, Key::Enter),
                    text,
                    i.key_pressed(Key::Backspace),
                )
            });
            if selected {
                for c in text.chars() {
                    let first = self.tools.typed.is_empty();
                    if c.is_ascii_digit() || c == '.' || (c == '-' && first) {
                        self.tools.typed.push(c);
                    }
                }
            }
            if back {
                self.tools.typed.pop();
            }
            if esc {
                self.transform_esc();
                return;
            }
            if enter {
                self.transform_enter(mode, units);
                return;
            }
        }
        let Some(gizmo) = self.gizmo(mode, rect).filter(|_| selected) else {
            // Nothing selected yet: select as usual, without dragging geometry.
            self.pick_and_select(ui, resp, rect, units, false);
            return;
        };
        let center = self.selection_center().unwrap_or(Vec2::ZERO);
        let world = |p: Pos2| self.view.camera.to_world(p, rect);
        if resp.drag_started_by(PointerButton::Primary)
            && let Some(o) = super::press_pos(ui, resp)
        {
            let tol = self.view.tol_pt / self.view.camera.pt_per_mm;
            let grip = gizmo.grip_at(o).or_else(|| {
                // In Move, dragging the selection itself moves it freely.
                let on = hit::pick(&self.doc.scene.items, world(o), tol);
                (mode == TransformMode::Move
                    && on.is_some_and(|id| self.selection.items.contains(id)))
                .then_some(Grip::MoveFree)
            });
            if let Some(g) = grip {
                let values = self.tools.transform.values();
                self.tools.transform.grip = Some((g, world(o), values));
                self.tools.typed.clear();
            }
        }
        if let Some((grip, _, _)) = self.tools.transform.grip {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            if let Some(p) = resp.interact_pointer_pos() {
                let mut to = world(p);
                if grip == Grip::Pivot {
                    // The pivot snaps to points (#69), so it can sit on a corner.
                    to = self.designate(to, false).pos;
                }
                let coarse = ui.input(|i| i.modifiers.shift);
                self.tools.transform.drag(to, center, coarse);
            }
            if !resp.dragged_by(PointerButton::Primary) {
                self.tools.transform.grip = None;
            }
            return;
        }
        let hot = resp.hover_pos().and_then(|p| gizmo.grip_at(p));
        if hot.is_some() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
        // A click on a grip is not a selection click.
        let on_grip = super::press_pos(ui, resp)
            .and_then(|o| gizmo.grip_at(o))
            .is_some();
        if !on_grip {
            self.pick_and_select(ui, resp, rect, units, false);
        }
    }

    /// Esc: drop a grip drag, else reset the values, else back to Select
    /// with the selection kept.
    fn transform_esc(&mut self) {
        let mode = self.transform_mode();
        let t = &mut self.tools.transform;
        if let Some((_, _, v)) = t.grip.take() {
            (t.dx, t.dy, t.angle_deg, t.factor, t.pivot) =
                (v.dx, v.dy, v.angle_deg, v.factor, v.pivot);
        } else if !self.tools.typed.is_empty() {
            self.tools.typed.clear();
        } else if mode.is_some_and(|m| !t.is_identity(m)) || t.pivot.is_some() {
            t.reset();
        } else {
            self.set_tool(Tool::Select);
        }
        self.doc.notice = None;
    }

    /// Enter: take a typed value, then apply.
    fn transform_enter(&mut self, mode: TransformMode, units: Units) {
        let typed = std::mem::take(&mut self.tools.typed);
        if !typed.is_empty() {
            match typed.parse::<f64>() {
                Ok(v) => {
                    let v = match (mode, units) {
                        (TransformMode::Move, Units::Inch) => v * 25.4,
                        _ => v,
                    };
                    self.tools.transform.set_typed(mode, v);
                }
                Err(_) => {
                    self.doc.notice = Some(format!("\"{typed}\" is not a number"));
                    return;
                }
            }
        }
        self.transform_ok(mode, units);
    }

    /// OK / Enter: apply as one undo step, then back to Select with the
    /// result still selected (Fusion closes the dialog on OK).
    pub(crate) fn transform_ok(&mut self, mode: TransformMode, units: Units) {
        if !self.can_transform() {
            self.doc.notice = Some("Select geometry first".to_owned());
            return;
        }
        if self.tools.transform.is_identity(mode) {
            self.doc.notice = Some(match mode {
                TransformMode::Move => "Drag an arrow or type a distance first".to_owned(),
                TransformMode::Rotate => "Drag the ring's knob or type an angle first".to_owned(),
                TransformMode::Scale => "Drag the corner handle or type a factor first".to_owned(),
            });
            return;
        }
        if self.apply_transform(mode, units) {
            let notice = self.doc.notice.take();
            self.tools.transform.reset();
            self.set_tool(Tool::Select);
            self.doc.notice = notice;
        }
    }

    /// The ghost of the result, the manipulator and its read-out.
    pub(crate) fn draw_manipulator(&self, frame: &Frame<'_>, mode: TransformMode) {
        let Some(g) = self.gizmo(mode, frame.rect) else {
            return;
        };
        let center = self.selection_center().unwrap_or(Vec2::ZERO);
        let t = &self.tools.transform;
        if !t.is_identity(mode) {
            let xf = t.transform(mode, center);
            let ghost: Vec<Shape> = self
                .doc
                .scene
                .items
                .iter()
                .filter(|i| {
                    self.selection.items.contains(i.id) && !matches!(i.shape, Shape::Point(_))
                })
                .map(|i| moved_shape(&i.shape, &xf))
                .collect();
            frame.draw_preview(&ghost);
        }
        let hot = t.grip.map(|(g, _, _)| g);
        let colors = [
            Color32::from_rgb(230, 80, 80),
            Color32::from_rgb(80, 190, 90),
            frame.pal.selected,
        ];
        g.draw(frame.painter, hot, colors);
        let units = frame.units;
        let label = if self.tools.typed.is_empty() {
            match mode {
                TransformMode::Move => format!(
                    "dx {}  dy {}",
                    units.format_len(t.dx),
                    units.format_len(t.dy)
                ),
                TransformMode::Rotate => format!("{:.1}°", t.angle_deg),
                TransformMode::Scale => format!("×{:.3}", t.factor),
            }
        } else {
            let what = match mode {
                TransformMode::Move if t.along_y => "dy",
                TransformMode::Move => "dx",
                TransformMode::Rotate => "Angle",
                TransformMode::Scale => "Factor",
            };
            format!("{what} {}▏", self.tools.typed)
        };
        frame.painter.text(
            g.origin + EVec2::new(SQUARE_PT + 6.0, SQUARE_PT + 6.0),
            Align2::LEFT_TOP,
            label,
            FontId::monospace(13.0),
            frame.pal.hover,
        );
    }

    /// The command dialog (design §2, §6.7): the mode switch, the fields,
    /// Copy, OK and Cancel. The fields and the manipulator are one value.
    pub(crate) fn transform_dialog(
        &mut self,
        ui: &egui::Ui,
        rect: Rect,
        mode: TransformMode,
        units: Units,
    ) {
        let mut next = None;
        let mut ok = false;
        let mut cancel = false;
        let selected = self.selection.items.iter().count();
        let mut clear = false;
        let icon = match mode {
            TransformMode::Move => "modify.move",
            TransformMode::Rotate => "modify.rotate",
            TransformMode::Scale => "modify.scale",
        };
        let header = dialog::Header::new(icon, mode.label());
        dialog::command_dialog(
            ui,
            rect,
            header,
            |_| {},
            |ui| {
                // Three tools, one dialog: switching keeps the selection (§6.7).
                next = dialog::segmented(
                    ui,
                    mode,
                    &[
                        (TransformMode::Move, "Move (M)"),
                        (TransformMode::Rotate, "Rotate"),
                        (TransformMode::Scale, "Scale"),
                    ],
                );
                dialog::row(ui, "Objects", |ui| {
                    let text = if selected == 0 {
                        "Select geometry".to_owned()
                    } else {
                        dialog::count(selected, "curve", "curves")
                    };
                    clear = dialog::selection(ui, &text, selected > 0, selected == 0).1;
                });
                let t = &mut self.tools.transform;
                let (scale, suffix) = match units {
                    Units::Mm => (1.0, " mm"),
                    Units::Inch => (25.4, " in"),
                };
                match mode {
                    TransformMode::Move => {
                        for (name, v) in [("X distance", &mut t.dx), ("Y distance", &mut t.dy)] {
                            dialog::row(ui, name, |ui| {
                                let mut shown = *v / scale;
                                if ui
                                    .add(
                                        egui::DragValue::new(&mut shown)
                                            .speed(0.1)
                                            .max_decimals(4)
                                            .suffix(suffix),
                                    )
                                    .changed()
                                {
                                    *v = shown * scale;
                                }
                            });
                        }
                    }
                    TransformMode::Rotate | TransformMode::Scale => {
                        let pivot = if mode == TransformMode::Rotate {
                            "Pivot"
                        } else {
                            "Base point"
                        };
                        dialog::row(ui, pivot, |ui| {
                            let text = match t.pivot {
                                None => "Selection centre".to_owned(),
                                Some(p) => {
                                    format!("{}, {}", units.format_len(p.x), units.format_len(p.y))
                                }
                            };
                            // ✕ goes back to the selection centre.
                            if dialog::selection(ui, &text, true, false).1 {
                                t.pivot = None;
                            }
                        });
                        if mode == TransformMode::Rotate {
                            dialog::row(ui, "Angle", |ui| {
                                ui.add(
                                    egui::DragValue::new(&mut t.angle_deg)
                                        .speed(1.0)
                                        .max_decimals(3)
                                        .suffix("°"),
                                );
                            });
                        } else {
                            dialog::row(ui, "Scale factor", |ui| {
                                ui.add(
                                    egui::DragValue::new(&mut t.factor)
                                        .range(0.001..=1000.0)
                                        .speed(0.01)
                                        .max_decimals(4),
                                );
                            });
                        }
                    }
                }
                dialog::row(ui, "", |ui| ui.checkbox(&mut t.copy, "Create copy"));
                (ok, cancel) = dialog::footer(
                    ui,
                    if selected > 0 {
                        Ok(())
                    } else {
                        Err("Select geometry first")
                    },
                );
            },
        );
        if clear {
            self.clear_selection();
        }
        if let Some(m) = next {
            self.set_modify(Modify::Transform(m));
        } else if ok {
            self.transform_ok(mode, units);
        } else if cancel {
            self.tools.transform.reset();
            self.set_tool(Tool::Select);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::ToolEvent;
    use crate::tools::Pick;
    use locus_doc::{Document, Geom};

    /// A blank editor with one rectangle drawn by the Rectangle tool, all of
    /// it selected.
    fn rectangle() -> AppState {
        let mut c = AppState::default();
        c.load(Document::new());
        c.set_tool(Tool::RectCorner);
        for p in [Vec2::ZERO, Vec2::new(20.0, 10.0)] {
            c.tool_event(ToolEvent::Click(Pick::free(p)), None, Units::Mm);
        }
        let lines: Vec<_> = c
            .doc
            .sketcher
            .document()
            .entities()
            .iter()
            .filter(|(_, e)| matches!(e.geom, Geom::Line { .. }))
            .map(|(id, _)| id)
            .collect();
        assert_eq!(lines.len(), 4);
        c.selection.items.set_all(lines);
        c
    }

    fn bottom_angle(c: &AppState) -> f64 {
        let doc = c.doc.sketcher.document();
        let (lo, hi) = doc
            .entities()
            .iter()
            .filter_map(|(id, _)| doc.line(id))
            .map(|l| (l.a, l.b))
            .min_by(|a, b| (a.0.y + a.1.y).total_cmp(&(b.0.y + b.1.y)))
            .unwrap_or((Vec2::ZERO, Vec2::ZERO));
        let d = hi - lo;
        // Modulo a quarter turn: the lowest edge may be a side once rotated.
        d.y.atan2(d.x).to_degrees().rem_euclid(90.0)
    }

    /// Regression (#219): the owner couldn't rotate anything. A rectangle's
    /// own horizontal and vertical constraints refused every rotation.
    #[test]
    fn the_rotate_tool_rotates_a_drawn_rectangle_in_one_undo_step() {
        let mut c = rectangle();
        let before = c.doc.sketcher.document().constraints().len();
        assert!(before > 0, "the rectangle tool constrains its sides");
        c.set_modify(Modify::Transform(TransformMode::Rotate));
        assert!(c.can_transform(), "the selection is kept");
        c.tools.transform.angle_deg = 30.0;
        c.transform_ok(TransformMode::Rotate, Units::Mm);
        assert!(
            (bottom_angle(&c) - 30.0).abs() < 1e-6,
            "{}",
            bottom_angle(&c)
        );
        assert!(
            c.doc
                .notice
                .as_deref()
                .is_some_and(|n| n.contains("removed")),
            "the dropped constraints are named: {:?}",
            c.doc.notice
        );
        assert_eq!(c.tool(), Tool::Select);
        c.undo();
        assert!(bottom_angle(&c).abs() < 1e-9);
        assert_eq!(c.doc.sketcher.document().constraints().len(), before);
    }

    #[test]
    fn move_and_scale_tools_apply_typed_values_and_copy() {
        let mut c = rectangle();
        c.set_modify(Modify::Transform(TransformMode::Move));
        c.tools.typed = "5".to_owned();
        c.transform_enter(TransformMode::Move, Units::Mm);
        let xs: Vec<f64> = c
            .doc
            .sketcher
            .document()
            .entities()
            .iter()
            .filter_map(|(_, e)| match e.geom {
                Geom::Point { pos } => Some(pos.x),
                _ => None,
            })
            .collect();
        assert!(xs.iter().all(|x| *x >= 5.0 - 1e-9), "{xs:?}");
        // Scale ×2 as a copy keeps the original.
        let n = c.doc.sketcher.document().entities().len();
        c.set_modify(Modify::Transform(TransformMode::Scale));
        c.tools.transform.factor = 2.0;
        c.tools.transform.copy = true;
        c.transform_ok(TransformMode::Scale, Units::Mm);
        assert_eq!(c.doc.sketcher.document().entities().len(), n * 2);
        // Nothing to apply is refused with a hint, not an empty undo step.
        let mut c = rectangle();
        c.set_modify(Modify::Transform(TransformMode::Rotate));
        let undo = c.undo_label().map(str::to_owned);
        c.transform_ok(TransformMode::Rotate, Units::Mm);
        assert_eq!(c.undo_label().map(str::to_owned), undo);
        assert!(c.doc.notice.is_some());
    }

    fn gizmo(mode: TransformMode) -> Gizmo {
        Gizmo {
            mode,
            origin: Pos2::new(100.0, 100.0),
            angle_deg: 0.0,
            factor: 1.0,
        }
    }

    #[test]
    fn grips_are_where_they_are_drawn() {
        let m = gizmo(TransformMode::Move);
        assert_eq!(m.grip_at(Pos2::new(100.0, 100.0)), Some(Grip::MoveFree));
        assert_eq!(m.grip_at(Pos2::new(150.0, 102.0)), Some(Grip::MoveX));
        // Screen y grows down, so the Y arrow points up.
        assert_eq!(m.grip_at(Pos2::new(101.0, 50.0)), Some(Grip::MoveY));
        assert_eq!(m.grip_at(Pos2::new(50.0, 50.0)), None);
        let r = gizmo(TransformMode::Rotate);
        assert_eq!(r.grip_at(Pos2::new(160.0, 100.0)), Some(Grip::Ring));
        assert_eq!(r.grip_at(Pos2::new(100.0, 40.0)), Some(Grip::Ring));
        assert_eq!(r.grip_at(Pos2::new(100.0, 100.0)), Some(Grip::Pivot));
        let s = gizmo(TransformMode::Scale);
        let c = s.corner();
        assert_eq!(s.grip_at(c), Some(Grip::Factor));
        assert_eq!(s.grip_at(Pos2::new(100.0, 100.0)), Some(Grip::Pivot));
    }

    #[test]
    fn the_ghost_is_the_transformed_shape() {
        let l = Shape::Line(Line::new(Vec2::new(1.0, 0.0), Vec2::new(2.0, 0.0)));
        let t = Transform::Rotate {
            center: Vec2::ZERO,
            angle: core::f64::consts::FRAC_PI_2,
        };
        let Shape::Line(m) = moved_shape(&l, &t) else {
            panic!("a line stays a line");
        };
        assert!(m.a.approx_eq(Vec2::new(0.0, 1.0), 1e-12));
        let c = Shape::Circle(Circle {
            center: Vec2::new(1.0, 1.0),
            radius: 2.0,
        });
        let s = Transform::Scale {
            center: Vec2::ZERO,
            factor: 3.0,
        };
        let Shape::Circle(m) = moved_shape(&c, &s) else {
            panic!("a circle stays a circle");
        };
        assert!(m.center.approx_eq(Vec2::new(3.0, 3.0), 1e-12) && (m.radius - 6.0).abs() < 1e-12);
    }
}
