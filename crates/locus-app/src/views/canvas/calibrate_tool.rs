// SPDX-License-Identifier: AGPL-3.0-or-later
//! Calibrate a reference (#391), as Fusion's Calibrate: pick two points on
//! a reference (an image or imported geometry), type the real distance
//! between them, and the reference scales so that distance is exact. It
//! scales about its centre by default, so a reference centred on 0,0 stays
//! centred (#468). Its anchor corner (an image's placement origin, or the
//! bottom-left of the reference's geometry, #464) and the first picked
//! point are the other choices. One command (`Command::ScaleReference`, §11), stored
//! in the reference itself, so it survives saving and undoes in one step.
//!
//! Points snap to the reference's own points; on an image any spot counts.

use crate::render::Frame;
use crate::settings::Units;
use crate::state::{AppState, format_value, to_stored};
use egui::{Align2, FontId, PointerButton, Rect, Stroke};
use locus_doc::{Command, LayerId, Operation};
use locus_geom::Vec2;

/// What a reference scales about (#464, #468).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Pivot {
    /// The middle of the reference: it stays centred where it is.
    #[default]
    Centre,
    /// Its anchor corner: an image's placement origin, or the bottom-left
    /// of its geometry.
    AnchorCorner,
    /// The first point picked in Calibrate.
    FirstPoint,
}

impl Pivot {
    /// The hint under the choice.
    pub const fn hint(self) -> &'static str {
        match self {
            Pivot::Centre => "The reference keeps its centre where it is.",
            Pivot::AnchorCorner => {
                "The reference keeps its anchor corner (its bottom-left) where it is."
            }
            Pivot::FirstPoint => "The reference scales about the first point you picked.",
        }
    }

    /// The point to scale `layer` about: `first` is the first pick, if any.
    pub fn point(
        self,
        doc: &locus_doc::Document,
        layer: LayerId,
        first: Option<Vec2>,
    ) -> Option<Vec2> {
        match self {
            Pivot::Centre => locus_doc::reference::centre(doc, layer),
            Pivot::AnchorCorner => locus_doc::reference::anchor(doc, layer),
            Pivot::FirstPoint => first,
        }
        .or(first)
    }
}

/// The Calibrate tool's picks and the typed distance.
#[derive(Clone, Debug, Default)]
pub struct CalibrateDraft {
    /// The reference being calibrated: the layer of the first pick.
    pub layer: Option<LayerId>,
    pub a: Option<Vec2>,
    pub b: Option<Vec2>,
    /// The real distance between the points, in display units.
    pub distance: String,
    /// What the reference scales about (#468: the centre by default).
    pub pivot: Pivot,
}

impl CalibrateDraft {
    /// A fresh run: on `layer` if one is given (the reference picked in the
    /// Browser), no points yet.
    pub fn begin(&mut self, layer: Option<LayerId>) {
        // The pivot choice is the user's: it stays for the next run.
        *self = Self {
            layer,
            pivot: self.pivot,
            ..Self::default()
        };
    }

    /// The distance between the picks, mm.
    pub fn measured(&self) -> Option<f64> {
        Some(self.a?.distance(self.b?))
    }
}

impl AppState {
    /// The Reference layer under `w`: one of its points within `tol`, or
    /// else an image covering `w`. Returns the layer and the point to use
    /// (snapped to the reference's point when one is near).
    fn reference_at(&self, w: Vec2, tol: f64, only: Option<LayerId>) -> Option<(LayerId, Vec2)> {
        let doc = self.doc.sketcher.document();
        let is_ref = |l: LayerId| {
            only.is_none_or(|o| o == l)
                && doc
                    .layer(l)
                    .is_some_and(|l| l.visible && matches!(l.operation, Operation::Reference))
        };
        let near = self.doc.scene.items.iter().filter_map(|item| {
            let crate::scene::Shape::Point(q) = item.shape else {
                return None;
            };
            let layer = doc.entity(item.id)?.layer;
            (is_ref(layer) && q.distance(w) <= tol).then_some((q.distance(w), layer, q))
        });
        if let Some((_, layer, q)) = near.min_by(|a, b| a.0.total_cmp(&b.0)) {
            return Some((layer, q));
        }
        doc.images().iter().find_map(|(l, img)| {
            let (lo, hi) = img.rect();
            (is_ref(*l) && w.x >= lo.x && w.x <= hi.x && w.y >= lo.y && w.y <= hi.y)
                .then_some((*l, w))
        })
    }

    /// The calibration command, or why there isn't one yet.
    fn calibrate_command(&self, units: Units) -> Result<Command, String> {
        let d = &self.tools.calibrate;
        let (Some(layer), Some(a)) = (d.layer, d.a) else {
            return Err("Click the first point on a reference".to_owned());
        };
        let Some(measured) = d.measured() else {
            return Err("Click the second point".to_owned());
        };
        if measured <= f64::EPSILON {
            return Err("Pick two different points".to_owned());
        }
        // A number or an expression over parameters, in display units.
        let doc = self.doc.sketcher.document();
        let real = locus_doc::expr::evaluate(&to_stored(&d.distance, units), doc.parameters())
            .ok()
            .filter(|v| v.is_finite() && *v > 0.0)
            .ok_or_else(|| "Type the real distance between the points".to_owned())?;
        // The chosen pivot stays put (#468: the centre by default).
        let about = d.pivot.point(doc, layer, Some(a)).unwrap_or(a);
        Ok(Command::ScaleReference {
            layer,
            about,
            factor: real / measured,
        })
    }

    /// Pointer input while Calibrate is active: two picks on one reference.
    pub(crate) fn calibrate_input(
        &mut self,
        ui: &egui::Ui,
        resp: &egui::Response,
        rect: Rect,
        units: Units,
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
                let d = &mut self.tools.calibrate;
                if d.a.is_some() {
                    // Esc first clears the picks, then leaves.
                    let layer = d.layer;
                    d.begin(layer);
                } else {
                    self.set_tool(crate::tools::Tool::Select);
                }
                return;
            }
            if enter {
                self.calibrate_ok(units);
                return;
            }
        }
        let tol = self.view.tol_pt / self.view.camera.pt_per_mm;
        let only = self.tools.calibrate.a.and(self.tools.calibrate.layer);
        let hover = resp
            .hover_pos()
            .map(|p| self.view.camera.to_world(p, rect))
            .and_then(|w| self.reference_at(w, tol, only));
        if resp.hovered() {
            ui.ctx().set_cursor_icon(if hover.is_some() {
                egui::CursorIcon::Crosshair
            } else {
                egui::CursorIcon::NotAllowed
            });
        }
        if !resp.clicked_by(PointerButton::Primary) {
            return;
        }
        let d = &mut self.tools.calibrate;
        match (hover, d.a.is_some() && d.b.is_none()) {
            (Some((layer, p)), false) => {
                d.layer = Some(layer);
                d.a = Some(p);
                d.b = None;
                self.doc.notice = None;
            }
            (Some((_, p)), true) => {
                d.b = Some(p);
                // Prefill with what it measures now, to type over.
                if let Some(m) = d.measured() {
                    d.distance = format_value(m, false, units);
                }
                self.doc.notice = None;
            }
            (None, _) => {
                self.doc.notice = Some(if d.a.is_some() {
                    "Click a second point on the same reference".to_owned()
                } else {
                    "Click a point on a reference image or imported reference".to_owned()
                });
            }
        }
    }

    /// Enter / OK: scale the reference as one undo step, then Select.
    pub(crate) fn calibrate_ok(&mut self, units: Units) -> bool {
        match self.calibrate_command(units) {
            Ok(cmd) => {
                if self.apply_reporting(&cmd) {
                    self.tools.calibrate.begin(None);
                    self.set_tool(crate::tools::Tool::Select);
                    true
                } else {
                    false
                }
            }
            Err(why) => {
                self.doc.notice = Some(why);
                false
            }
        }
    }

    /// The picked points, the line between them and what it measures.
    pub(crate) fn draw_calibrate_draft(&self, frame: &Frame<'_>, units: Units) {
        let d = &self.tools.calibrate;
        let c = frame.pal.selected;
        let s = |p: Vec2| frame.view.to_screen(p, frame.rect);
        if let (Some(a), Some(b)) = (d.a, d.b) {
            frame
                .painter
                .line_segment([s(a), s(b)], Stroke::new(2.0, c));
            let mid = s((a + b) * 0.5);
            frame.painter.text(
                mid + egui::vec2(0.0, -10.0),
                Align2::CENTER_BOTTOM,
                format!(
                    "{} {}",
                    format_value(a.distance(b), false, units),
                    units.label()
                ),
                FontId::proportional(12.0),
                c,
            );
        }
        for p in [d.a, d.b].into_iter().flatten() {
            frame.painter.circle_filled(s(p), 4.5, c);
            frame.painter.circle_stroke(s(p), 7.0, Stroke::new(1.2, c));
        }
    }

    /// The Calibrate command dialog.
    pub(crate) fn calibrate_dialog(&mut self, ui: &egui::Ui, rect: Rect, units: Units) {
        let error = self.calibrate_command(units).err();
        let (mut ok, mut cancel) = (false, false);
        let name = self
            .tools
            .calibrate
            .layer
            .and_then(|l| self.doc.sketcher.document().layer(l))
            .map(|l| l.name.clone());
        let header = super::dialog::Header::new("sketch.calibrate", "Calibrate");
        super::dialog::command_dialog(
            ui,
            rect,
            header,
            |_| {},
            |ui| {
                use super::dialog::{row, selection};
                let d = &mut self.tools.calibrate;
                row(ui, "Reference", |ui| match &name {
                    Some(n) => ui.label(n),
                    None => ui.weak("Pick on a reference"),
                });
                row(ui, "Points", |ui| {
                    let n = usize::from(d.a.is_some()) + usize::from(d.b.is_some());
                    let text = match n {
                        0 => "Pick 2 points".to_owned(),
                        _ => super::dialog::count(n, "point", "points"),
                    };
                    let (pick, x) = selection(ui, &text, n == 2, n < 2);
                    if pick || x {
                        let layer = d.layer;
                        d.begin(layer);
                    }
                });
                if let Some(m) = d.measured() {
                    row(ui, "Measured", |ui| {
                        ui.label(format!(
                            "{} {}",
                            format_value(m, false, units),
                            units.label()
                        ))
                    });
                }
                row(ui, "Real distance", |ui| {
                    ui.add_enabled_ui(d.b.is_some(), |ui| {
                        ui.add(egui::TextEdit::singleline(&mut d.distance).desired_width(80.0))
                            .on_hover_text("The true distance between the two points");
                        ui.weak(units.label());
                    });
                });
                row(ui, "Scale about", |ui| {
                    if let Some(p) = super::dialog::segmented(
                        ui,
                        d.pivot,
                        &[
                            (Pivot::Centre, "Centre"),
                            (Pivot::AnchorCorner, "Anchor corner"),
                            (Pivot::FirstPoint, "First point"),
                        ],
                    ) {
                        d.pivot = p;
                    }
                });
                ui.weak(d.pivot.hint());
                if let Some(why) = &error {
                    ui.colored_label(ui.visuals().error_fg_color, why);
                }
                (ok, cancel) = super::dialog::footer(ui, error.as_deref().map_or(Ok(()), Err));
            },
        );
        if ok {
            self.calibrate_ok(units);
        } else if cancel {
            self.set_tool(crate::tools::Tool::Select);
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // test fixtures
mod tests {
    use super::*;
    use crate::modify_tool::Modify;
    use locus_doc::Document;

    /// A document with an imported reference: a 10 mm line on a locked
    /// Reference layer.
    fn with_reference() -> (AppState, LayerId) {
        let mut c = AppState::default();
        c.load(Document::new());
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100mm" height="100mm" viewBox="0 0 100 100"><line x1="0" y1="50" x2="10" y2="50" stroke="black"/></svg>"#;
        let (cmd, _) = locus_import::reference_command(c.document(), "plan.svg", svg).unwrap();
        c.apply(&cmd).unwrap();
        let doc = c.document();
        let layer = doc
            .layers()
            .iter()
            .find(|(_, l)| matches!(l.operation, Operation::Reference))
            .map(|(id, _)| *id)
            .unwrap();
        (c, layer)
    }

    fn points(c: &AppState, layer: LayerId) -> Vec<Vec2> {
        let doc = c.document();
        doc.entities()
            .iter()
            .filter(|(_, e)| e.layer == layer)
            .filter_map(|(id, _)| doc.point_pos(id))
            .collect()
    }

    /// #464: with "Anchor corner", Calibrate keeps the reference's anchor
    /// (the bottom-left of its geometry, here the line's start) fixed,
    /// scaled ×2 then ×0.5. (`locus-doc` checks one placed at 0,0 stays.)
    #[test]
    fn calibrate_keeps_the_anchor_where_it_was_placed() {
        let (mut c, layer) = with_reference();
        let anchor = |c: &AppState| locus_doc::reference::anchor(c.document(), layer).unwrap();
        let start = anchor(&c);
        assert_eq!(start, Vec2::new(0.0, 50.0), "the line's left end");
        for (real, factor) in [("20", 2.0), ("10", 0.5)] {
            let p = points(&c, layer);
            let (a, b) = if p[0].x <= p[1].x {
                (p[1], p[0])
            } else {
                (p[0], p[1])
            };
            c.set_modify(Modify::Calibrate);
            c.tools.calibrate.pivot = Pivot::AnchorCorner;
            c.tools.calibrate.layer = Some(layer);
            c.tools.calibrate.a = Some(a); // not the anchor
            c.tools.calibrate.b = Some(b);
            c.tools.calibrate.distance = real.to_owned();
            let before = a.distance(b);
            assert!(c.calibrate_ok(Units::Mm));
            assert_eq!(anchor(&c), start, "×{factor}: the anchor stays put");
            let p = points(&c, layer);
            assert!((p[0].distance(p[1]) - before * factor).abs() < 1e-9);
        }
    }

    /// #468: Calibrate keeps the reference's centre by default, scaled
    /// ×0.0875 then ×2.
    #[test]
    fn calibrate_keeps_the_centre_by_default() {
        let (mut c, layer) = with_reference();
        assert_eq!(c.tools.calibrate.pivot, Pivot::Centre);
        let mid = |c: &AppState| locus_doc::reference::centre(c.document(), layer).unwrap();
        let start = mid(&c);
        for factor in [0.0875, 2.0] {
            let p = points(&c, layer);
            c.set_modify(Modify::Calibrate);
            c.tools.calibrate.layer = Some(layer);
            c.tools.calibrate.a = Some(p[0]);
            c.tools.calibrate.b = Some(p[1]);
            c.tools.calibrate.distance = format!("{}", p[0].distance(p[1]) * factor);
            assert!(c.calibrate_ok(Units::Mm));
            assert!(
                mid(&c).distance(start) < 1e-9,
                "×{factor}: the centre stays"
            );
        }
    }

    /// #464, #468: the anchor corner and the first point are explicit
    /// choices; the centre is the default.
    #[test]
    fn the_first_point_is_the_pivot_only_when_chosen() {
        let (mut c, layer) = with_reference();
        let p = points(&c, layer);
        c.tools.calibrate.layer = Some(layer);
        c.tools.calibrate.a = Some(p[1]);
        c.tools.calibrate.b = Some(p[0]);
        c.tools.calibrate.distance = "20".to_owned();
        let about = |c: &AppState| match c.calibrate_command(Units::Mm) {
            Ok(Command::ScaleReference { about, .. }) => about,
            other => panic!("{other:?}"),
        };
        let mid = locus_doc::reference::centre(c.document(), layer).unwrap();
        assert_eq!(about(&c), mid, "the centre by default");
        c.tools.calibrate.pivot = Pivot::AnchorCorner;
        let at = locus_doc::reference::anchor(c.document(), layer).unwrap();
        assert_eq!(about(&c), at, "the anchor corner when chosen");
        assert_ne!(at, p[1]);
        c.tools.calibrate.pivot = Pivot::FirstPoint;
        assert_eq!(about(&c), p[1], "the first point when chosen");
        c.tools.calibrate.begin(Some(layer));
        assert_eq!(
            c.tools.calibrate.pivot,
            Pivot::FirstPoint,
            "the choice is kept"
        );
    }

    /// #391: pick two points on the reference, type the real distance, and
    /// the reference scales about the first point (when chosen) in one undo
    /// step.
    #[test]
    fn two_picks_and_a_distance_scale_the_reference() {
        let (mut c, layer) = with_reference();
        let p = points(&c, layer);
        assert_eq!(p.len(), 2);
        let (a, b) = (p[0], p[1]);
        assert!((a.distance(b) - 10.0).abs() < 1e-9);
        c.set_modify(Modify::Calibrate);
        // Picking snaps to the reference's points.
        let tol = 0.5;
        let pick = c.reference_at(a + Vec2::new(0.2, 0.1), tol, None);
        assert_eq!(pick, Some((layer, a)));
        assert!(c.reference_at(Vec2::new(500.0, 500.0), tol, None).is_none());
        c.tools.calibrate.layer = Some(layer);
        c.tools.calibrate.a = Some(a);
        c.tools.calibrate.pivot = Pivot::FirstPoint;
        assert!(!c.calibrate_ok(Units::Mm), "one point");
        c.tools.calibrate.b = Some(b);
        c.tools.calibrate.distance = "25".to_owned();
        assert!(c.calibrate_ok(Units::Mm));
        assert_eq!(c.undo_label(), Some("Calibrate reference"));
        let p = points(&c, layer);
        assert!(p.contains(&a), "the first point stays");
        let far = p.iter().find(|q| **q != a).unwrap();
        assert!((far.distance(a) - 25.0).abs() < 1e-9, "{far:?}");
        assert!(c.document().layer(layer).unwrap().locked, "still locked");
        c.undo();
        assert!((points(&c, layer)[0].distance(points(&c, layer)[1]) - 10.0).abs() < 1e-9);
    }

    /// Inches are typed in inches; nonsense is refused with a reason.
    #[test]
    fn the_distance_is_in_display_units_and_checked() {
        let (mut c, layer) = with_reference();
        let p = points(&c, layer);
        c.set_modify(Modify::Calibrate);
        c.tools.calibrate.layer = Some(layer);
        c.tools.calibrate.a = Some(p[0]);
        c.tools.calibrate.b = Some(p[1]);
        for bad in ["", "-3", "0", "abc"] {
            c.tools.calibrate.distance = bad.to_owned();
            assert!(c.calibrate_command(Units::Mm).is_err(), "{bad}");
        }
        c.tools.calibrate.distance = "1".to_owned();
        let Ok(Command::ScaleReference { factor, .. }) = c.calibrate_command(Units::Inch) else {
            panic!("a command");
        };
        assert!((factor - 2.54).abs() < 1e-9, "1 in = 25.4 mm over 10 mm");
    }
}
