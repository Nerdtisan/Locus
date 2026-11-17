// SPDX-License-Identifier: AGPL-3.0-or-later
//! Rectangular and Circular Pattern tools (#221, design §6.8, SPEC
//! §6.2.1, ADR 0011).
//!
//! One dialog with two tabs works on the selection, which can be made
//! before or while the tool is active. Everything can also be set on the
//! canvas, with a live preview of the copies:
//! - an arrowhead on each direction drags its spacing;
//! - a count handle one spacing beyond the last copy drags the count, and
//!   − / + beside the arrow step it (#337);
//! - for a circular pattern, a knob at the end of the arc drags the total
//!   angle, snapping to 360° near a full turn; a handle on the second copy
//!   drags the count, with − / + to step it.
//!
//! Clicking a handle focuses its value in the dialog (#337, design §6.8).
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

const GRAB_PT: f32 = 11.0;
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

/// A canvas handle: dragged, or the value a click on it focuses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Drag {
    /// Direction 1 or 2's spacing (its arrow).
    Spacing(u8),
    /// Direction 1 or 2's count (the ring beyond the last copy).
    Count(u8),
    /// The circular pattern's total angle (the knob at the arc's end).
    Angle,
    /// The circular pattern's count (the ring on the second copy).
    CircCount,
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
    /// A handle was clicked: the dialog focuses its field (#337).
    pub focus: Option<Drag>,
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
            focus: None,
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

/// Most copies a handle drag sets in one direction (SPEC §6.2.1 caps a
/// whole pattern at `MAX_INSTANCES`; the recipe refuses more).
const MAX_COUNT: usize = 1000;

/// The angle between circular copies: a full turn shares it among all `n`,
/// a partial sweep among the `n - 1` gaps (as `locus_doc` lays them out).
fn circ_step(sweep: f64, n: i64) -> f64 {
    let full = sweep.abs() >= std::f64::consts::TAU - 1e-9;
    if full || n < 2 {
        sweep / n.max(1) as f64
    } else {
        sweep / (n - 1) as f64
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
        .flat_map(|(id, e)| match e.geom {
            Geom::Line { .. } => after.line(id).map(Shape::Line).into_iter().collect(),
            Geom::Arc { .. } => after.arc(id).map(Shape::Arc).into_iter().collect(),
            Geom::Circle { .. } => after.circle(id).map(Shape::Circle).into_iter().collect(),
            Geom::Bezier { .. } => after.cubic(id).map(Shape::Cubic).into_iter().collect(),
            // Copies of a text preview as their outline (#364).
            Geom::Text { .. } => after
                .text_geometry(id)
                .map(|g| crate::scene::text_shapes(&g))
                .unwrap_or_default(),
            _ => Vec::new(),
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

    /// The count handles (#337), in screen points: one spacing beyond
    /// each direction's last copy, and on a circular pattern's second copy.
    fn count_spots(&self, h: &Handles, rect: Rect) -> Vec<(Drag, Pos2)> {
        let cam = &self.view.camera;
        let s = |p: Vec2| cam.to_screen(p, rect);
        let mut out = Vec::new();
        for (i, d) in h.dirs.iter().enumerate() {
            if let Some((v, spacing, n)) = *d {
                let at = h.start + v * spacing * n.max(1) as f64;
                out.push((Drag::Count(i as u8 + 1), s(at)));
            }
        }
        if let Some((c, r, a0, sweep, n)) = h.arc {
            let a = a0 + circ_step(sweep, n);
            out.push((Drag::CircCount, s(c + Vec2::new(a.cos(), a.sin()) * r)));
        }
        out
    }

    /// The handle under screen point `p`: a drag target or a count ring.
    fn pattern_handle_at(&self, h: &Handles, rect: Rect, p: Pos2) -> Option<Drag> {
        self.pattern_spots(h, rect)
            .into_iter()
            .find_map(|(d, _, tip, ..)| d.filter(|_| tip.distance(p) <= GRAB_PT))
            .or_else(|| {
                self.count_spots(h, rect)
                    .into_iter()
                    .find(|(_, at)| at.distance(p) <= GRAB_PT)
                    .map(|(d, _)| d)
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
        let under_press = press.and_then(|o| self.pattern_handle_at(&h, rect, o));
        if resp.drag_started_by(PointerButton::Primary)
            && let Some(d) = under_press
        {
            self.tools.pattern.drag = Some(d);
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
        let on_handle = under_press.is_some();
        if let Some(hp) = resp.hover_pos()
            && (self.pattern_handle_at(&h, rect, hp).is_some()
                || spots
                    .iter()
                    .any(|s| near(hp, s.3, BUTTON_PT + 2.0) || near(hp, s.4, BUTTON_PT + 2.0)))
        {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        // A click on a handle focuses its value in the dialog (#337).
        if resp.clicked_by(PointerButton::Primary)
            && let Some(d) = under_press
        {
            self.tools.pattern.focus = Some(d);
            return;
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
            Drag::Count(which) => {
                let Some((v, spacing, _)) = h.dirs[usize::from(which) - 1] else {
                    return;
                };
                if spacing.abs() < f64::EPSILON {
                    return;
                }
                // The ring sits at index `count`: where it lands is the count.
                let along = (w - h.start).dot(v) / spacing;
                let n = along.round().clamp(1.0, MAX_COUNT as f64) as i64;
                *d.count_mut(which) = n.to_string();
            }
            Drag::CircCount => {
                let Some((c, _, a0, sweep, _)) = h.arc else {
                    return;
                };
                let full = sweep.abs() >= std::f64::consts::TAU - 1e-9;
                let a = (w - c).y.atan2((w - c).x);
                // The angle from the first copy to the ring, the same way
                // round as the sweep.
                let mut step = (a - a0).rem_euclid(std::f64::consts::TAU);
                if sweep < 0.0 {
                    step -= std::f64::consts::TAU;
                }
                if step.abs() < 1e-3 {
                    return;
                }
                let gaps = (sweep / step).round().max(1.0);
                let n = if full { gaps } else { gaps + 1.0 };
                let n = n.clamp(2.0, MAX_COUNT as f64) as i64;
                d.c_count = n.to_string();
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
            // Above the arrow: the count ring sits ahead of it and the
            // − / + buttons below.
            p.text(
                tip - EVec2::new(-dir.y, dir.x) * 14.0,
                Align2::CENTER_BOTTOM,
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
        // The count rings (#337), filled while dragged.
        for (d, at) in self.count_spots(&h, frame.rect) {
            let live = self.tools.pattern.drag == Some(d);
            p.circle_filled(at, 6.0, frame.pal.background);
            p.circle_stroke(at, 6.0, Stroke::new(1.8, c));
            if live {
                p.circle_filled(at, 3.5, c);
            }
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
        let mut clear = false;
        let icon = if circular {
            "sketch.pattern_circ"
        } else {
            "sketch.pattern_rect"
        };
        let header = super::dialog::Header::new(icon, "Pattern");
        super::dialog::command_dialog(
            ui,
            rect,
            header,
            |ui| {
                // Right-to-left: the last option is drawn first.
                switch = super::dialog::segmented(
                    ui,
                    circular,
                    &[(true, "Circular"), (false, "Rectangular")],
                );
            },
            |ui| {
                use super::dialog::{row, segmented, selection};
                let d = &mut self.tools.pattern;
                row(ui, "Objects", |ui| {
                    let text = if selected == 0 {
                        "Select geometry".to_owned()
                    } else {
                        super::dialog::count(selected, "curve", "curves")
                    };
                    let objects = d.pick == PatternPick::Objects;
                    let (pick, x) = selection(ui, &text, selected > 0, objects && selected == 0);
                    if pick {
                        d.pick = PatternPick::Objects;
                    }
                    clear = x;
                });
                let focus = d.focus.take();
                let field = |ui: &mut egui::Ui, text: &mut String, w: f32, me: Drag| {
                    let r = ui
                        .add(egui::TextEdit::singleline(text).desired_width(w))
                        .on_hover_text("A number, or an expression over parameters");
                    if focus == Some(me) {
                        r.request_focus();
                    }
                    r
                };
                if circular {
                    row(ui, "Centre", |ui| {
                        let picking = d.pick == PatternPick::Centre;
                        let (text, filled) = match d.centre {
                            Some(_) => ("1 point", true),
                            None => ("Pick a point", false),
                        };
                        let (pick, x) = selection(ui, text, filled, picking);
                        if pick {
                            d.pick = if picking {
                                PatternPick::Objects
                            } else {
                                PatternPick::Centre
                            };
                        }
                        if x {
                            d.centre = None;
                            d.pick = PatternPick::Centre;
                        }
                    });
                    row(ui, "Quantity", |ui| {
                        field(ui, &mut d.c_count, 56.0, Drag::CircCount)
                    });
                    row(ui, "Total angle", |ui| {
                        field(ui, &mut d.c_angle, 72.0, Drag::Angle);
                        ui.weak("°");
                    });
                    row(ui, "", |ui| ui.checkbox(&mut d.rotate, "Rotate instances"));
                } else {
                    row(ui, "Directions", |ui| {
                        for n in [1u8, 2] {
                            let on = n == 1 || d.second;
                            let picking = d.pick == PatternPick::Direction(n);
                            let axis = if n == 1 { &mut d.axis1 } else { &mut d.axis2 };
                            let text = if !on {
                                "+ Direction 2".to_owned()
                            } else {
                                match axis {
                                    Axis::X => "X axis".to_owned(),
                                    Axis::Y => "Y axis".to_owned(),
                                    Axis::Line(_) => "1 line".to_owned(),
                                }
                            };
                            let (click, x) = selection(ui, &text, on, picking);
                            if click {
                                if !on {
                                    d.second = true;
                                } else {
                                    // Cycle X axis → Y axis → pick a line.
                                    match axis {
                                        Axis::X => *axis = Axis::Y,
                                        Axis::Y => d.pick = PatternPick::Direction(n),
                                        Axis::Line(_) => *axis = Axis::X,
                                    }
                                }
                            }
                            if x {
                                if n == 2 {
                                    d.second = false;
                                } else {
                                    d.axis1 = Axis::X;
                                }
                                if picking {
                                    d.pick = PatternPick::Objects;
                                }
                            }
                        }
                    });
                    row(ui, "Quantity", |ui| {
                        field(ui, &mut d.count1, 56.0, Drag::Count(1));
                        ui.add_enabled_ui(d.second, |ui| {
                            field(ui, &mut d.count2, 56.0, Drag::Count(2))
                        });
                    });
                    row(ui, "Distance", |ui| {
                        field(ui, &mut d.spacing1, 64.0, Drag::Spacing(1));
                        ui.add_enabled_ui(d.second, |ui| {
                            field(ui, &mut d.spacing2, 64.0, Drag::Spacing(2))
                        });
                        ui.weak(unit);
                    });
                    row(ui, "Direction type", |ui| {
                        if let Some(v) =
                            segmented(ui, d.symmetric, &[(false, "One"), (true, "Symmetric")])
                        {
                            d.symmetric = v;
                        }
                    });
                }
                match d.pick {
                    PatternPick::Centre => {
                        ui.weak("Click a point on the canvas");
                    }
                    PatternPick::Direction(_) => {
                        ui.weak("Click a line on the canvas");
                    }
                    PatternPick::Objects => {}
                }
                ui.weak("Fields take numbers or expressions (pitch/2). Dragging on the canvas updates them.");
                if let Some(why) = &error {
                    ui.colored_label(ui.visuals().error_fg_color, why);
                }
                (ok, cancel) = super::dialog::footer(ui, error.as_deref().map_or(Ok(()), Err));
            },
        );
        if clear {
            self.clear_selection();
        }
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

    /// #337: the count rings drag the count (linear: where the ring lands
    /// along the direction; circular: the angle to the second copy), and
    /// each handle is found under the pointer for click-to-focus.
    #[test]
    fn count_handles_drag_the_count_and_are_found_under_the_pointer() {
        let mut c = ready(false);
        c.view.rect = Rect::from_min_size(Pos2::ZERO, EVec2::new(800.0, 600.0));
        c.tools.pattern.count1 = "3".to_owned();
        c.tools.pattern.spacing1 = "30".to_owned();
        let h = c.pattern_handles(false, Units::Mm).expect("handles");
        // Start (10, 5); the ring sits at index 3 (x = 100). Dragged to
        // x = 160 it lands on index 5.
        c.pattern_drag(Drag::Count(1), &h, Vec2::new(160.0, 7.0), false, Units::Mm);
        assert_eq!(c.tools.pattern.count1, "5");
        let h = c.pattern_handles(false, Units::Mm).expect("handles");
        let rect = c.view.rect;
        for (d, at) in c.count_spots(&h, rect) {
            assert_eq!(c.pattern_handle_at(&h, rect, at), Some(d));
        }
        let (tip_drag, tip) = c
            .pattern_spots(&h, rect)
            .into_iter()
            .find_map(|(d, _, tip, ..)| d.map(|d| (d, tip)))
            .expect("an arrow");
        assert_eq!(c.pattern_handle_at(&h, rect, tip), Some(tip_drag));

        // Circular: centre straight below the selection centre.
        let mut c = ready(true);
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
        c.tools.pattern.c_angle = "360".to_owned();
        c.tools.pattern.c_count = "6".to_owned();
        let h = c.pattern_handles(true, Units::Mm).expect("handles");
        // A quarter turn from the first copy: four copies round a full turn.
        c.pattern_drag(
            Drag::CircCount,
            &h,
            Vec2::new(-35.0, -40.0),
            false,
            Units::Mm,
        );
        assert_eq!(c.tools.pattern.c_count, "4");
        // A partial sweep of 90° with copies 30° apart: four copies.
        c.tools.pattern.c_angle = "90".to_owned();
        let h = c.pattern_handles(true, Units::Mm).expect("handles");
        let a = 120f64.to_radians();
        let w = Vec2::new(10.0, -40.0) + Vec2::new(a.cos(), a.sin()) * 45.0;
        c.pattern_drag(Drag::CircCount, &h, w, false, Units::Mm);
        assert_eq!(c.tools.pattern.c_count, "4");
        assert!((circ_step(90f64.to_radians(), 4).to_degrees() - 30.0).abs() < 1e-9);
    }

    /// #364: placed text patterns, rectangular and circular: the preview
    /// shows the copies' outlines and OK adds the recipe.
    #[test]
    fn text_patterns_preview_and_commit() {
        use crate::tool::ToolEvent;
        use crate::tools::{Pick, Tool};
        for circular in [false, true] {
            let mut c = AppState::default();
            c.load(Document::new());
            c.set_tool(Tool::Text);
            c.tools.text_settings.content = "Hi".to_owned();
            c.tool_event(ToolEvent::Click(Pick::free(Vec2::ZERO)), None, Units::Mm);
            c.close_text_dialog();
            let doc = c.doc.sketcher.document();
            let t = doc
                .entities()
                .iter()
                .find(|(_, e)| matches!(e.geom, Geom::Text { .. }))
                .map(|(id, _)| id)
                .expect("a text");
            if circular {
                let centre = c
                    .apply_reporting(&Command::AddEntity {
                        layer: Document::default_layer(),
                        construction: false,
                        geom: Geom::Point {
                            pos: Vec2::new(0.0, -40.0),
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
                    .expect("a centre");
                c.tools.pattern.centre = Some(centre);
            }
            c.set_modify(Modify::Pattern(circular));
            c.selection.items.set_all([t]);
            c.refresh_pattern_preview(circular, Units::Mm);
            let preview = c.tools.pattern.preview.as_ref().map_or(0, |(_, s)| s.len());
            assert!(preview > 0, "circular {circular}: a preview of the copies");
            assert!(
                c.pattern_ok(circular, Units::Mm),
                "circular {circular}: committed"
            );
            assert_eq!(c.doc.sketcher.document().recipes().len(), 1);
        }
    }
}
