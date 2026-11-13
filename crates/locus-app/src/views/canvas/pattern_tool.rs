// SPDX-License-Identifier: AGPL-3.0-or-later
//! Rectangular and Circular Pattern tools (#221, design §6.8, SPEC
//! §6.2.1, ADR 0011).
//!
//! One dialog with two tabs works on the selection, which can be made
//! before or while the tool is active. Everything can also be set on the
//! canvas, with a live preview of the copies:
//! - an arrowhead on each direction drags its spacing;
//! - − / + beside it change that direction's count;
//! - for a circular pattern, a knob at the end of the arc drags the total
//!   angle, snapping to 360° near a full turn, with − / + for the count.
//!
//! The dialog and the handles are one set of values. Enter or OK adds the
//! recipe as one command (§11), and its count, spacing and angle stay
//! editable as labels on the canvas.

use crate::hit;
use crate::modify_tool::Modify;
use crate::render::Frame;
use crate::scene::Shape;
use crate::settings::Units;
use crate::state::{AppState, format_value, parse_number, to_stored};
use egui::{Align2, FontId, PointerButton, Pos2, Rect, Stroke, Vec2 as EVec2};
use locus_doc::{Axis, Command, Document, Editor, EntityId, Geom, LinearDir, RecipeKind};
use locus_geom::Vec2;

const GRAB_PT: f32 = 9.0;
/// The − / + buttons: their size and their distance from the handle.
const BUTTON_PT: f32 = 9.0;
const BUTTON_OFF_PT: f32 = 22.0;

/// What the next click on the canvas picks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PatternPick {
    /// Objects: clicks and boxes select, as in Select.
    Objects,
    /// The circular pattern's centre point.
    Centre,
    /// A line for direction 1 or 2.
    Direction(u8),
}

/// A canvas handle being dragged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Drag {
    Spacing(u8),
    Angle,
}

/// The pattern being set up (persists between uses, like tool options).
#[derive(Clone, Debug)]
pub struct PatternDraft {
    pub axis1: Axis,
    pub count1: String,
    pub spacing1: String,
    pub symmetric: bool,
    pub second: bool,
    pub axis2: Axis,
    pub count2: String,
    pub spacing2: String,
    pub centre: Option<EntityId>,
    pub c_count: String,
    pub c_angle: String,
    pub rotate: bool,
    pub pick: PatternPick,
    drag: Option<Drag>,
    /// The preview for a key (the command's debug text).
    preview: Option<(String, Vec<Shape>)>,
}

impl Default for PatternDraft {
    fn default() -> Self {
        Self {
            axis1: Axis::X,
            count1: "3".to_owned(),
            spacing1: "20".to_owned(),
            symmetric: false,
            second: false,
            axis2: Axis::Y,
            count2: "2".to_owned(),
            spacing2: "20".to_owned(),
            centre: None,
            c_count: "6".to_owned(),
            c_angle: "360".to_owned(),
            rotate: true,
            pick: PatternPick::Objects,
            drag: None,
            preview: None,
        }
    }
}

impl PatternDraft {
    /// The recipe kind, with spacings stored in mm (`to_stored`).
    pub fn kind(&self, circular: bool, units: Units) -> Option<RecipeKind> {
        let dir = |axis: &Axis, count: &str, spacing: &str, symmetric: bool| LinearDir {
            axis: axis.clone(),
            count: count.trim().to_owned(),
            spacing: to_stored(spacing, units),
            symmetric,
        };
        if circular {
            Some(RecipeKind::Circular {
                centre: self.centre?,
                count: self.c_count.trim().to_owned(),
                angle: self.c_angle.trim().to_owned(),
                symmetric: false,
                rotate: self.rotate,
            })
        } else {
            Some(RecipeKind::Linear {
                dir1: dir(&self.axis1, &self.count1, &self.spacing1, self.symmetric),
                dir2: self
                    .second
                    .then(|| dir(&self.axis2, &self.count2, &self.spacing2, false)),
            })
        }
    }

    fn count_mut(&mut self, which: u8) -> &mut String {
        match which {
            1 => &mut self.count1,
            2 => &mut self.count2,
            _ => &mut self.c_count,
        }
    }
}

/// `text` as a whole number, if it is one.
fn int(text: &str) -> Option<i64> {
    text.trim().parse::<i64>().ok()
}

/// Step a count field by `by`, keeping it at least 2 (a pattern of one is
/// no pattern). An expression count is replaced by its evaluated value.
fn step_count(text: &mut String, by: i64, doc: &Document) {
    let now = int(text).or_else(|| {
        locus_doc::expr::evaluate(text, doc.parameters())
            .ok()
            .map(|v| v.round() as i64)
    });
    if let Some(n) = now {
        *text = (n + by).max(2).to_string();
    }
}

/// The copies a recipe would make, from a scratch copy of the document.
pub(crate) fn recipe_preview(doc: &Document, cmd: &Command) -> Vec<Shape> {
    let first_new = doc.next_id();
    let mut ed = Editor::new(doc.clone());
    if ed.apply(cmd).is_err() {
        return Vec::new();
    }
    let after = ed.document();
    after
        .entities()
        .iter()
        .filter(|(id, _)| id.get() >= first_new)
        .filter_map(|(id, e)| match e.geom {
            Geom::Line { .. } => after.line(id).map(Shape::Line),
            Geom::Arc { .. } => after.arc(id).map(Shape::Arc),
            Geom::Circle { .. } => after.circle(id).map(Shape::Circle),
            Geom::Bezier { .. } => after.cubic(id).map(Shape::Cubic),
            _ => None,
        })
        .collect()
}

/// Where the handles are, in world mm.
#[derive(Clone, Copy, Debug)]
struct Handles {
    /// The selection's centre: where direction arrows start.
    start: Vec2,
    /// Each direction's unit vector, spacing and count (direction 2 only
    /// when used).
    dirs: [Option<(Vec2, f64, i64)>; 2],
    /// Circular: centre, radius to the selection, start angle, total sweep
    /// (radians) and count.
    arc: Option<(Vec2, f64, f64, f64, i64)>,
}

impl AppState {
    /// The pattern tool active: false for rectangular, true for circular.
    pub(crate) fn pattern_kind(&self) -> Option<bool> {
        match self.modify() {
            Some(Modify::Pattern(c)) => Some(c),
            _ => None,
        }
    }

    fn new_pattern_command(&self, circular: bool, units: Units) -> Result<Command, String> {
        let sources: Vec<EntityId> = self.selection.items.iter().collect();
        if sources.is_empty() {
            return Err("Select the objects to pattern".to_owned());
        }
        let kind = self
            .tools
            .pattern
            .kind(circular, units)
            .ok_or_else(|| "Pick the centre point".to_owned())?;
        Ok(Command::AddRecipe { kind, sources })
    }

    fn pattern_handles(&self, circular: bool, units: Units) -> Option<Handles> {
        let doc = self.doc.sketcher.document();
        let start = self.selection_center()?;
        let d = &self.tools.pattern;
        let num = |t: &str| {
            parse_number(t)
                .map(|v| if units == Units::Inch { v * 25.4 } else { v })
                .or_else(|| locus_doc::expr::evaluate(t, doc.parameters()).ok())
        };
        let count = |t: &str| {
            locus_doc::expr::evaluate(t, doc.parameters())
                .ok()
                .map(|v| v.round() as i64)
        };
        let axis = |a: &Axis| match a {
            Axis::X => Some(Vec2::new(1.0, 0.0)),
            Axis::Y => Some(Vec2::new(0.0, 1.0)),
            Axis::Line(id) => doc
                .line(*id)
                .and_then(|l| (l.b - l.a).normalized(f64::EPSILON)),
        };
        if circular {
            let c = doc.point_pos(d.centre?)?;
            let r = start.distance(c);
            let a0 = (start - c).y.atan2((start - c).x);
            let sweep = num(&d.c_angle)?.to_radians();
            return Some(Handles {
                start,
                dirs: [None, None],
                arc: Some((c, r, a0, sweep, count(&d.c_count)?)),
            });
        }
        let one = |a: &Axis, s: &str, n: &str| Some((axis(a)?, num(s)?, count(n)?));
        Some(Handles {
            start,
            dirs: [
                one(&d.axis1, &d.spacing1, &d.count1),
                d.second
                    .then(|| one(&d.axis2, &d.spacing2, &d.count2))
                    .flatten(),
            ],
            arc: None,
        })
    }

    /// Screen spots of the handles: (what, tip, minus, plus).
    fn pattern_spots(&self, h: &Handles, rect: Rect) -> Vec<(Option<Drag>, u8, Pos2, Pos2, Pos2)> {
        let cam = &self.view.camera;
        let s = |p: Vec2| cam.to_screen(p, rect);
        let mut out = Vec::new();
        for (i, d) in h.dirs.iter().enumerate() {
            let Some((v, spacing, n)) = *d else {
                continue;
            };
            let reach = spacing * (n.max(2) - 1) as f64;
            let tip = s(h.start + v * reach);
            let dir = (s(h.start + v) - s(h.start)).normalized();
            let side = EVec2::new(-dir.y, dir.x) * BUTTON_OFF_PT;
            let w = i as u8 + 1;
            out.push((
                Some(Drag::Spacing(w)),
                w,
                tip,
                tip + side - dir * 12.0,
                tip + side + dir * 12.0,
            ));
        }
        if let Some((c, r, a0, sweep, _)) = h.arc {
            let end = c + Vec2::new((a0 + sweep).cos(), (a0 + sweep).sin()) * r;
            let tip = s(end);
            let out_dir = (tip - s(c)).normalized();
            let a = tip + out_dir * BUTTON_OFF_PT;
            let side = EVec2::new(-out_dir.y, out_dir.x) * 12.0;
            out.push((Some(Drag::Angle), 0, tip, a - side, a + side));
        }
        out
    }

    /// Pointer input while a pattern tool is active.
    pub(crate) fn pattern_input(
        &mut self,
        ui: &egui::Ui,
        resp: &egui::Response,
        rect: Rect,
        units: Units,
        circular: bool,
    ) {
        use egui::{Key, Modifiers};
        if resp.hovered() {
            let (esc, enter) = ui.ctx().input_mut(|i| {
                (
                    i.consume_key(Modifiers::NONE, Key::Escape),
                    i.consume_key(Modifiers::NONE, Key::Enter),
                )
            });
            if esc {
                if self.tools.pattern.pick != PatternPick::Objects {
                    self.tools.pattern.pick = PatternPick::Objects;
                } else {
                    self.set_tool(crate::tools::Tool::Select);
                }
                return;
            }
            if enter {
                self.pattern_ok(circular, units);
                return;
            }
        }
        let tol = self.view.tol_pt / self.view.camera.pt_per_mm;
        let world = |p: Pos2| self.view.camera.to_world(p, rect);
        // Picking a centre point or a direction line.
        match self.tools.pattern.pick {
            PatternPick::Objects => {}
            pick => {
                if resp.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
                }
                if resp.clicked_by(PointerButton::Primary)
                    && let Some(p) = resp.interact_pointer_pos()
                {
                    let w = world(p);
                    let items = &self.doc.scene.items;
                    match pick {
                        PatternPick::Centre => {
                            if let Some((id, _)) = hit::pick_point(items, w, tol) {
                                self.tools.pattern.centre = Some(id);
                                self.tools.pattern.pick = PatternPick::Objects;
                            } else {
                                self.doc.notice = Some("Click a point for the centre".to_owned());
                            }
                        }
                        PatternPick::Direction(n) => {
                            let line = hit::pick_curve(items, w, tol)
                                .filter(|id| self.doc.sketcher.document().line(*id).is_some());
                            if let Some(id) = line {
                                if n == 1 {
                                    self.tools.pattern.axis1 = Axis::Line(id);
                                } else {
                                    self.tools.pattern.axis2 = Axis::Line(id);
                                }
                                self.tools.pattern.pick = PatternPick::Objects;
                            } else {
                                self.doc.notice = Some("Click a line for the direction".to_owned());
                            }
                        }
                        PatternPick::Objects => {}
                    }
                }
                return;
            }
        }
        let Some(h) = self.pattern_handles(circular, units) else {
            self.pick_and_select(ui, resp, rect, units, false);
            return;
        };
        let spots = self.pattern_spots(&h, rect);
        let press = super::press_pos(ui, resp);
        let near = |a: Pos2, b: Pos2, r: f32| a.distance(b) <= r;
        if resp.drag_started_by(PointerButton::Primary)
            && let Some(o) = press
            && let Some((Some(d), ..)) = spots.iter().find(|s| near(o, s.2, GRAB_PT))
        {
            self.tools.pattern.drag = Some(*d);
        }
        if let Some(drag) = self.tools.pattern.drag {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            if let Some(p) = resp.interact_pointer_pos() {
                let w = world(p);
                let coarse = ui.input(|i| i.modifiers.shift);
                self.pattern_drag(drag, &h, w, coarse, units);
            }
            if !resp.dragged_by(PointerButton::Primary) {
                self.tools.pattern.drag = None;
            }
            return;
        }
        let on_button = press.and_then(|o| {
            spots.iter().find_map(|(_, which, _, minus, plus)| {
                if near(o, *minus, BUTTON_PT + 2.0) {
                    Some((*which, -1))
                } else if near(o, *plus, BUTTON_PT + 2.0) {
                    Some((*which, 1))
                } else {
                    None
                }
            })
        });
        let on_handle = press.is_some_and(|o| spots.iter().any(|s| near(o, s.2, GRAB_PT)));
        if let Some(hp) = resp.hover_pos()
            && spots.iter().any(|s| {
                near(hp, s.2, GRAB_PT)
                    || near(hp, s.3, BUTTON_PT + 2.0)
                    || near(hp, s.4, BUTTON_PT + 2.0)
            })
        {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if resp.clicked_by(PointerButton::Primary)
            && let Some((which, by)) = on_button
        {
            let doc = self.doc.sketcher.document().clone();
            step_count(self.tools.pattern.count_mut(which), by, &doc);
            return;
        }
        if on_button.is_none() && !on_handle {
            self.pick_and_select(ui, resp, rect, units, false);
        }
    }

    /// Drag a handle to `w`: spacing along its direction (0.1 mm, 1 mm with
    /// Shift), or the circular sweep (1°, 15° with Shift; 360° near a full
    /// turn).
    fn pattern_drag(&mut self, drag: Drag, h: &Handles, w: Vec2, coarse: bool, units: Units) {
        let d = &mut self.tools.pattern;
        match drag {
            Drag::Spacing(which) => {
                let Some((v, _, n)) = h.dirs[usize::from(which) - 1] else {
                    return;
                };
                let step = if coarse { 1.0 } else { 0.1 };
                let along = (w - h.start).dot(v) / (n.max(2) - 1) as f64;
                let mm = (along / step).round() * step;
                if mm != 0.0 {
                    let text = format_value(mm, false, units);
                    if which == 1 {
                        d.spacing1 = text;
                    } else {
                        d.spacing2 = text;
                    }
                }
            }
            Drag::Angle => {
                let Some((c, _, a0, _, _)) = h.arc else {
                    return;
                };
                let a = (w - c).y.atan2((w - c).x);
                let mut deg = (a - a0).to_degrees().rem_euclid(360.0);
                let step = if coarse { 15.0 } else { 1.0 };
                deg = (deg / step).round() * step;
                if deg < 5.0 || deg > 355.0 {
                    deg = 360.0;
                }
                d.c_angle = format!("{deg}");
            }
        }
    }

    /// Enter / OK: add the pattern as one undo step, then back to Select.
    pub(crate) fn pattern_ok(&mut self, circular: bool, units: Units) -> bool {
        let cmd = match self.new_pattern_command(circular, units) {
            Ok(c) => c,
            Err(why) => {
                self.doc.notice = Some(why);
                return false;
            }
        };
        if self.apply_reporting(&cmd) {
            self.selection.items.clear();
            self.tools.pattern.preview = None;
            self.set_tool(crate::tools::Tool::Select);
            true
        } else {
            false
        }
    }

    /// Recompute the preview when the values or the selection changed.
    pub(crate) fn refresh_pattern_preview(&mut self, circular: bool, units: Units) {
        let Ok(cmd) = self.new_pattern_command(circular, units) else {
            self.tools.pattern.preview = None;
            return;
        };
        let key = format!("{cmd:?}");
        if self
            .tools
            .pattern
            .preview
            .as_ref()
            .is_some_and(|(k, _)| *k == key)
        {
            return;
        }
        let shapes = recipe_preview(self.doc.sketcher.document(), &cmd);
        self.tools.pattern.preview = Some((key, shapes));
    }

    /// The preview, the handles and their read-outs.
    pub(crate) fn draw_pattern_draft(&self, frame: &Frame<'_>, circular: bool) {
        if let Some((_, shapes)) = &self.tools.pattern.preview {
            frame.draw_preview(shapes);
        }
        let Some(h) = self.pattern_handles(circular, frame.units) else {
            return;
        };
        let p = frame.painter;
        let c = frame.pal.selected;
        let s = |w: Vec2| frame.view.to_screen(w, frame.rect);
        let font = FontId::monospace(13.0);
        for (i, d) in h.dirs.iter().enumerate() {
            let Some((v, spacing, n)) = *d else {
                continue;
            };
            let base = s(h.start);
            let tip = s(h.start + v * spacing * (n.max(2) - 1) as f64);
            let dir = (tip - base).normalized();
            p.line_segment([base, tip], Stroke::new(2.0, c));
            let side = EVec2::new(-dir.y, dir.x) * 6.0;
            p.add(egui::Shape::convex_polygon(
                vec![tip + dir * 12.0, tip + side, tip - side],
                c,
                Stroke::NONE,
            ));
            p.text(
                tip + dir * 18.0,
                if dir.x < -0.3 {
                    Align2::RIGHT_CENTER
                } else {
                    Align2::LEFT_CENTER
                },
                format!("{n} × {}", frame.units.format_len(spacing)),
                font.clone(),
                frame.pal.hover,
            );
            let _ = i;
        }
        if let Some((cc, r, a0, sweep, n)) = h.arc {
            let steps = 64;
            let pts: Vec<Pos2> = (0..=steps)
                .map(|k| {
                    let a = a0 + sweep * f64::from(k) / f64::from(steps);
                    s(cc + Vec2::new(a.cos(), a.sin()) * r)
                })
                .collect();
            p.add(egui::Shape::line(pts, Stroke::new(1.5, c)));
            p.circle_filled(s(cc), 3.0, c);
            let end = s(cc + Vec2::new((a0 + sweep).cos(), (a0 + sweep).sin()) * r);
            p.circle_filled(end, 7.0, c);
            p.text(
                end + EVec2::new(10.0, -14.0),
                Align2::LEFT_BOTTOM,
                format!("{n} × {:.1}°", sweep.to_degrees()),
                font,
                frame.pal.hover,
            );
        }
        // − / + beside each handle.
        for (_, _, _, minus, plus) in self.pattern_spots(&h, frame.rect) {
            for (at, label) in [(minus, "−"), (plus, "+")] {
                let r = Rect::from_center_size(at, EVec2::splat(BUTTON_PT * 2.0));
                p.rect_filled(r, 3.0, frame.pal.background);
                p.rect_stroke(r, 3.0, Stroke::new(1.2, c), egui::StrokeKind::Inside);
                p.text(
                    at,
                    Align2::CENTER_CENTER,
                    label,
                    FontId::proportional(14.0),
                    c,
                );
            }
        }
    }

    /// The Pattern command dialog: Rectangular | Circular, the objects, the
    /// fields, OK and Cancel.
    pub(crate) fn pattern_dialog(
        &mut self,
        ui: &egui::Ui,
        rect: Rect,
        circular: bool,
        units: Units,
    ) {
        let selected = self.selection.items.iter().count();
        let error = self.new_pattern_command(circular, units).err();
        let (mut ok, mut cancel, mut switch) = (false, false, None);
        let unit = units.label();
        let title = if circular {
            "Circular Pattern"
        } else {
            "Rectangular Pattern"
        };
        let doc_lines = |id: &EntityId| format!("line #{id}");
        super::dialog::command_dialog(ui, rect, title, |ui| {
            ui.horizontal(|ui| {
                for (c, label) in [(false, "Rectangular"), (true, "Circular")] {
                    if ui.selectable_label(c == circular, label).clicked() && c != circular {
                        switch = Some(c);
                    }
                }
            });
            ui.label(if selected == 0 {
                "Objects: select geometry".to_owned()
            } else {
                format!("Objects: {selected} selected")
            });
            let d = &mut self.tools.pattern;
            let field = |ui: &mut egui::Ui, text: &mut String, w: f32| {
                ui.add(egui::TextEdit::singleline(text).desired_width(w))
                    .on_hover_text("A number, or an expression over parameters")
            };
            egui::Grid::new("pattern-fields")
                .num_columns(2)
                .show(ui, |ui| {
                    if circular {
                        ui.label("Centre");
                        ui.horizontal(|ui| {
                            ui.label(match d.centre {
                                Some(id) => format!("point #{id}"),
                                None => "—".to_owned(),
                            });
                            let picking = d.pick == PatternPick::Centre;
                            if ui.selectable_label(picking, "Pick").clicked() {
                                d.pick = if picking {
                                    PatternPick::Objects
                                } else {
                                    PatternPick::Centre
                                };
                            }
                        });
                        ui.end_row();
                        ui.label("Quantity");
                        field(ui, &mut d.c_count, 60.0);
                        ui.end_row();
                        ui.label("Total angle");
                        ui.horizontal(|ui| {
                            field(ui, &mut d.c_angle, 60.0);
                            ui.weak("°");
                        });
                        ui.end_row();
                        ui.label("");
                        ui.checkbox(&mut d.rotate, "Rotate copies");
                        ui.end_row();
                    } else {
                        for n in [1u8, 2] {
                            if n == 2 {
                                ui.label("");
                                ui.checkbox(&mut d.second, "Second direction");
                                ui.end_row();
                                if !d.second {
                                    break;
                                }
                            }
                            let picking = d.pick == PatternPick::Direction(n);
                            let (axis, count, spacing) = if n == 1 {
                                (&mut d.axis1, &mut d.count1, &mut d.spacing1)
                            } else {
                                (&mut d.axis2, &mut d.count2, &mut d.spacing2)
                            };
                            ui.label(format!("Direction {n}"));
                            let mut pick = false;
                            ui.horizontal(|ui| {
                                for (a, label) in [(Axis::X, "X"), (Axis::Y, "Y")] {
                                    if ui.selectable_label(*axis == a, label).clicked() {
                                        *axis = a;
                                    }
                                }
                                let on_line = matches!(axis, Axis::Line(_));
                                let text = match axis {
                                    Axis::Line(id) => doc_lines(id),
                                    _ => "Line…".to_owned(),
                                };
                                if ui
                                    .selectable_label(on_line || picking, text)
                                    .on_hover_text("Pick a line to pattern along")
                                    .clicked()
                                {
                                    pick = true;
                                }
                            });
                            if pick {
                                d.pick = PatternPick::Direction(n);
                            }
                            ui.end_row();
                            ui.label("Quantity");
                            field(ui, count, 60.0);
                            ui.end_row();
                            ui.label("Distance");
                            ui.horizontal(|ui| {
                                field(ui, spacing, 60.0);
                                ui.weak(unit);
                            });
                            ui.end_row();
                        }
                        ui.label("");
                        ui.checkbox(&mut d.symmetric, "Symmetric (both sides)");
                        ui.end_row();
                    }
                });
            match d.pick {
                PatternPick::Centre => {
                    ui.weak("Click a point on the canvas");
                }
                PatternPick::Direction(_) => {
                    ui.weak("Click a line on the canvas");
                }
                PatternPick::Objects => {}
            }
            if let Some(why) = &error {
                ui.colored_label(ui.visuals().error_fg_color, why);
            }
            ui.horizontal(|ui| {
                ok = ui
                    .add_enabled(error.is_none(), egui::Button::new("OK"))
                    .clicked();
                cancel = ui.button("Cancel").clicked();
            });
        });
        if let Some(c) = switch {
            self.set_modify(Modify::Pattern(c));
        } else if ok {
            self.pattern_ok(circular, units);
        } else if cancel {
            self.set_tool(crate::tools::Tool::Select);
        }
    }
}

/// The selection's bounding box, for tests.
#[cfg(test)]
fn bbox_of(shapes: &[Shape]) -> Option<locus_geom::Aabb> {
    shapes
        .iter()
        .map(Shape::bbox)
        .reduce(locus_geom::Aabb::union)
}

#[cfg(test)]
#[allow(clippy::expect_used)] // a missing fixture should fail loudly
mod tests {
    use super::*;
    use crate::tool::ToolEvent;
    use crate::tools::{Pick, Tool};

    /// A blank editor with a 20 × 10 rectangle, all of it selected, and the
    /// pattern tool `circular` active.
    fn ready(circular: bool) -> AppState {
        let mut c = AppState::default();
        c.load(Document::new());
        c.set_tool(Tool::RectCorner);
        for p in [Vec2::ZERO, Vec2::new(20.0, 10.0)] {
            c.tool_event(ToolEvent::Click(Pick::free(p)), None, Units::Mm);
        }
        let lines: Vec<EntityId> = {
            let doc = c.doc.sketcher.document();
            doc.entities()
                .iter()
                .filter(|(id, _)| doc.line(*id).is_some())
                .map(|(id, _)| id)
                .collect()
        };
        c.selection.items.set_all(lines);
        c.set_modify(Modify::Pattern(circular));
        assert_eq!(c.selection.items.iter().count(), 4, "the selection is kept");
        c
    }

    #[test]
    fn a_rectangular_pattern_previews_then_commits_one_step() {
        let mut c = ready(false);
        c.tools.pattern.count1 = "3".to_owned();
        c.tools.pattern.spacing1 = "30".to_owned();
        c.refresh_pattern_preview(false, Units::Mm);
        let shapes = &c.tools.pattern.preview.as_ref().expect("a preview").1;
        assert_eq!(shapes.len(), 8, "two copies of four lines");
        let b = bbox_of(shapes).expect("shapes");
        assert!((b.max.x - 80.0).abs() < 1e-6, "{b:?}");
        assert!(
            c.doc.sketcher.document().recipes().is_empty(),
            "nothing yet"
        );
        assert!(c.pattern_ok(false, Units::Mm));
        assert_eq!(c.undo_label(), Some("Add linear pattern"));
        assert_eq!(c.tool(), Tool::Select);
    }

    /// Regression (#221): a profile and its offset, selected together,
    /// pattern as one step (the offset's curves were refused as "driven").
    #[test]
    fn a_profile_with_its_offset_patterns() {
        let mut c = ready(false);
        let lines: Vec<EntityId> = c.selection.items.iter().collect();
        assert!(c.apply_reporting(&Command::AddRecipe {
            kind: RecipeKind::Offset {
                distance: 2.0,
                sharp: true,
                expr: None,
            },
            sources: lines,
        }));
        let all: Vec<EntityId> = {
            let doc = c.doc.sketcher.document();
            doc.entities()
                .iter()
                .filter(|(id, _)| doc.line(*id).is_some())
                .map(|(id, _)| id)
                .collect()
        };
        assert_eq!(all.len(), 8, "the rectangle and its offset");
        c.selection.items.set_all(all);
        c.tools.pattern.count1 = "2".to_owned();
        c.tools.pattern.spacing1 = "40".to_owned();
        assert!(c.pattern_ok(false, Units::Mm), "{:?}", c.doc.notice);
        let doc = c.doc.sketcher.document();
        let pattern = doc
            .recipes()
            .values()
            .find(|r| r.kind.is_pattern())
            .expect("the pattern");
        let copied_lines = pattern
            .instances()
            .filter(|e| doc.line(*e).is_some())
            .count();
        assert_eq!(copied_lines, 8, "the copy has the profile and its offset");
    }

    #[test]
    fn handles_drag_the_spacing_and_step_the_count() {
        let mut c = ready(false);
        c.tools.pattern.count1 = "3".to_owned();
        c.tools.pattern.spacing1 = "30".to_owned();
        let h = c.pattern_handles(false, Units::Mm).expect("handles");
        // The selection's centre is (10, 5); dragging the arrowhead to x = 60
        // over two gaps makes the spacing 25.
        c.pattern_drag(Drag::Spacing(1), &h, Vec2::new(60.0, 9.0), false, Units::Mm);
        assert_eq!(parse_number(&c.tools.pattern.spacing1), Some(25.0));
        let doc = c.doc.sketcher.document().clone();
        step_count(&mut c.tools.pattern.count1, 1, &doc);
        assert_eq!(c.tools.pattern.count1, "4");
        step_count(&mut c.tools.pattern.count1, -5, &doc);
        assert_eq!(c.tools.pattern.count1, "2", "never below two");
    }

    #[test]
    fn a_circular_pattern_needs_a_centre_and_its_knob_snaps_to_a_full_turn() {
        let mut c = ready(true);
        assert!(!c.pattern_ok(true, Units::Mm), "no centre yet");
        assert!(
            c.doc
                .notice
                .as_deref()
                .is_some_and(|n| n.contains("centre"))
        );
        let centre = c
            .apply_reporting(&Command::AddEntity {
                layer: Document::default_layer(),
                construction: false,
                geom: Geom::Point {
                    pos: Vec2::new(10.0, -40.0),
                },
                style: Default::default(),
            })
            .then(|| {
                c.doc
                    .sketcher
                    .document()
                    .entities()
                    .iter()
                    .last()
                    .map(|(id, _)| id)
            })
            .flatten()
            .expect("a centre point");
        c.tools.pattern.centre = Some(centre);
        c.tools.pattern.c_angle = "90".to_owned();
        let h = c.pattern_handles(true, Units::Mm).expect("handles");
        // The selection centre is straight above the centre (start angle
        // 90°); a pointer just short of a full turn snaps to 360°.
        c.pattern_drag(Drag::Angle, &h, Vec2::new(9.9, 10.0), false, Units::Mm);
        assert_eq!(c.tools.pattern.c_angle, "360");
        c.pattern_drag(Drag::Angle, &h, Vec2::new(-40.0, -40.0), false, Units::Mm);
        assert_eq!(c.tools.pattern.c_angle, "90");
        assert!(c.pattern_ok(true, Units::Mm));
        assert_eq!(c.doc.sketcher.document().recipes().len(), 1);
    }
}
