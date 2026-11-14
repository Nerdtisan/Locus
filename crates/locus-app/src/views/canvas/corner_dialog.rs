// SPDX-License-Identifier: AGPL-3.0-or-later
//! Fillet and chamfer sizing (#217, SPEC §6.2, design §3).
//!
//! The tool picks a corner: two curves, or the point they share. The
//! command dialog then sizes it, as Fusion's does, with a live preview on
//! the canvas. A size is a number or an expression over parameters (SPEC
//! §5). Enter or OK commits it as one command (§11), and its sizes become
//! driving dimensions that can be edited later. A size typed as an
//! expression keeps that expression, so chamfers that share a parameter
//! follow it together.

use crate::modify_tool::{ChamferMode, CornerSettings, Modify};
use crate::render::Frame;
use crate::scene::Shape;
use crate::settings::Units;
use crate::state::{AppState, parse_number, to_stored};
use locus_doc::modify::Plan;
use locus_doc::{Document, Editor, EntityId, Geom};

/// A size field's value in mm (or degrees, for an angle), and the
/// expression to keep when it wasn't a plain number.
pub(crate) fn field(
    text: &str,
    angle: bool,
    units: Units,
    doc: &Document,
) -> Result<(f64, Option<String>), String> {
    if let Some(v) = parse_number(text) {
        let v = if !angle && units == Units::Inch {
            v * 25.4
        } else {
            v
        };
        return Ok((v, None));
    }
    let stored = if angle {
        text.trim().to_owned()
    } else {
        to_stored(text, units)
    };
    locus_doc::expr::evaluate(&stored, doc.parameters())
        .map(|v| (v, Some(stored)))
        .map_err(|e| format!("\"{}\": {e}", text.trim()))
}

/// The sizes for `m` from the typed fields, and the dimensions' expressions
/// in the order the plan creates them (`locus_doc::corner::with_exprs`).
pub(crate) fn sizes(
    m: Modify,
    c: &CornerSettings,
    units: Units,
    doc: &Document,
) -> Result<(CornerSettings, Vec<Option<String>>), String> {
    let mut out = c.clone();
    let t = &c.text;
    let exprs = match m {
        Modify::Fillet => {
            let (r, e) = field(&t.radius, false, units, doc)?;
            out.radius = r;
            vec![e]
        }
        Modify::Chamfer => {
            let (d1, e1) = field(&t.d1, false, units, doc)?;
            out.d1 = d1;
            match c.mode {
                ChamferMode::Equal => vec![e1.clone(), e1],
                ChamferMode::TwoDistances => {
                    let (d2, e2) = field(&t.d2, false, units, doc)?;
                    out.d2 = d2;
                    vec![e1, e2]
                }
                ChamferMode::DistanceAngle => {
                    let (a, e2) = field(&t.angle, true, units, doc)?;
                    out.angle_deg = a;
                    vec![e1, e2]
                }
            }
        }
        _ => Vec::new(),
    };
    Ok((out, exprs))
}

impl AppState {
    /// The plan for the picked corner at the dialog's sizes.
    pub(crate) fn corner_plan(&self, m: Modify, units: Units) -> Result<Plan, String> {
        let (a, b) = self
            .tools
            .corner_pick
            .ok_or_else(|| "Pick a corner first".to_owned())?;
        let doc = self.doc.sketcher.document();
        let (size, exprs) = sizes(m, &self.tools.corner, units, doc)?;
        let plan = m.plan_corner(doc, a, b, &size).map_err(|e| e.to_string())?;
        Ok(locus_doc::corner::with_exprs(plan, &exprs))
    }

    /// OK / Enter: run the corner as one undo step. The tool stays active
    /// for the next corner, as Fusion's does.
    pub(crate) fn corner_ok(&mut self, m: Modify, units: Units) -> bool {
        match self.corner_plan(m, units) {
            Ok(plan) => {
                let ok = self.apply_reporting(&plan.command);
                if ok {
                    self.tools.corner_pick = None;
                    self.selection.items.clear();
                }
                ok
            }
            Err(why) => {
                self.doc.notice = Some(why);
                false
            }
        }
    }

    /// Esc with a corner picked drops the pick (the tool stays). Returns
    /// whether there was one.
    pub(crate) fn corner_esc(&mut self) -> bool {
        let had = self.tools.corner_pick.take().is_some();
        if had {
            self.selection.items.clear();
            self.doc.notice = None;
        }
        had
    }

    /// The live preview: the trimmed curves and the new corner piece, as
    /// the command would leave them (written positions, before re-solving).
    pub(crate) fn draw_corner_preview(&self, frame: &Frame<'_>, m: Modify) {
        let Some((a, b)) = self.tools.corner_pick else {
            return;
        };
        let Ok(plan) = self.corner_plan(m, frame.units) else {
            return;
        };
        let doc = self.doc.sketcher.document();
        let first_new = doc.next_id();
        let mut ed = Editor::new(doc.clone());
        if ed.apply(&plan.command).is_err() {
            return;
        }
        let after = ed.document();
        let shapes: Vec<Shape> = after
            .entities()
            .iter()
            .filter(|(id, _)| *id == a || *id == b || id.get() >= first_new)
            .filter_map(|(id, e)| shape(after, id, &e.geom))
            .collect();
        frame.draw_preview(&shapes);
    }

    /// The Fillet / Chamfer command dialog.
    pub(crate) fn corner_dialog(
        &mut self,
        ui: &egui::Ui,
        rect: egui::Rect,
        m: Modify,
        units: Units,
    ) {
        let picked = self.tools.corner_pick.is_some();
        let focus = std::mem::take(&mut self.tools.corner_focus);
        let preview = self.corner_plan(m, units).err().filter(|_| picked);
        let (mut ok, mut cancel) = (false, false);
        let unit = units.label();
        let icon = if m == Modify::Chamfer {
            "modify.chamfer"
        } else {
            "modify.fillet"
        };
        let mut repick = false;
        let header = super::dialog::Header::new(icon, m.label());
        super::dialog::command_dialog(
            ui,
            rect,
            header,
            |_| {},
            |ui| {
                use super::dialog::row;
                row(ui, "Corner", |ui| {
                    let text = if picked {
                        "1 corner"
                    } else {
                        "Click two curves or their point"
                    };
                    let (_, cleared) = super::dialog::selection(ui, text, picked, !picked);
                    repick = cleared;
                });
                let c = &mut self.tools.corner;
                if m == Modify::Chamfer {
                    row(ui, "Type", |ui| {
                        egui::ComboBox::from_id_salt("chamfer-mode")
                            .selected_text(c.mode.label())
                            .show_ui(ui, |ui| {
                                for mode in ChamferMode::ALL {
                                    ui.selectable_value(&mut c.mode, mode, mode.label());
                                }
                            });
                    });
                }
                let mut first = true;
                let mut field = |ui: &mut egui::Ui, name: &str, text: &mut String, suffix: &str| {
                    row(ui, name, |ui| {
                        let out = egui::TextEdit::singleline(text)
                            .desired_width(90.0)
                            .show(ui);
                        let r = out.response.response.clone();
                        if first && focus {
                            focus_all(ui, &r, out.state, text);
                        }
                        first = false;
                        if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            ok = true;
                        }
                        r.on_hover_text(
                            "A number, or an expression over parameters (e.g. bevel * 2)",
                        );
                        ui.weak(suffix);
                    });
                };
                let t = &mut c.text;
                match (m, c.mode) {
                    (Modify::Fillet, _) => field(ui, "Radius", &mut t.radius, unit),
                    (_, ChamferMode::Equal) => field(ui, "Distance", &mut t.d1, unit),
                    (_, ChamferMode::TwoDistances) => {
                        field(ui, "Distance 1", &mut t.d1, unit);
                        field(ui, "Distance 2", &mut t.d2, unit);
                    }
                    (_, ChamferMode::DistanceAngle) => {
                        field(ui, "Distance", &mut t.d1, unit);
                        field(ui, "Angle", &mut t.angle, "°");
                    }
                }
                if let Some(why) = &preview {
                    ui.colored_label(ui.visuals().error_fg_color, why);
                }
                let (o, c) = super::dialog::footer(
                    ui,
                    if picked {
                        Ok(())
                    } else {
                        Err("Pick a corner first")
                    },
                );
                ok |= o;
                cancel = c;
            },
        );
        if repick {
            self.corner_esc();
        }
        if ok && picked {
            self.corner_ok(m, units);
        } else if cancel && !self.corner_esc() {
            self.set_tool(crate::tools::Tool::Select);
        }
    }
}

/// Focus a dialog field with its text selected, so typing replaces it
/// (design §3: typing goes into the focused field).
pub(crate) fn focus_all(
    ui: &egui::Ui,
    r: &egui::Response,
    mut state: egui::text_edit::TextEditState,
    text: &str,
) {
    r.request_focus();
    let end = text.chars().count();
    state
        .cursor
        .set_char_range(Some(egui::text::CCursorRange::two(
            egui::text::CCursor::new(0),
            egui::text::CCursor::new(end),
        )));
    state.store(ui.ctx(), r.id);
}

/// A curve's shape from its geometry in `doc`, for previews.
fn shape(doc: &Document, id: EntityId, geom: &Geom) -> Option<Shape> {
    match geom {
        Geom::Line { .. } => doc.line(id).map(Shape::Line),
        Geom::Arc { .. } => doc.arc(id).map(Shape::Arc),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::ToolEvent;
    use crate::tools::{Pick, Tool};
    use locus_doc::{Command, Constraint};
    use locus_geom::Vec2;

    /// A blank editor with one rectangle from the Rectangle tool, and the
    /// chamfer tool picking its bottom-left corner.
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
        assert_eq!(at_origin.len(), 2);
        for id in at_origin {
            c.tool_event(
                ToolEvent::Click(Pick::free(Vec2::ZERO)),
                Some(id),
                Units::Mm,
            );
        }
        assert!(
            c.tools.corner_pick.is_some(),
            "the pick waits for the dialog"
        );
        c
    }

    #[test]
    fn a_chamfer_waits_for_its_size_then_commits_one_step() {
        let mut c = picked(Modify::Chamfer);
        let before = c.undo_label().map(str::to_owned);
        assert_eq!(c.undo_label().map(str::to_owned), before, "nothing yet");
        c.tools.corner.text.d1 = "3".to_owned();
        assert!(c.corner_ok(Modify::Chamfer, Units::Mm));
        assert_eq!(c.undo_label(), Some("Chamfer"));
        let set_backs: Vec<f64> = c
            .doc
            .sketcher
            .document()
            .constraints()
            .values()
            .filter_map(|k| match k {
                Constraint::Distance { dim, .. } => Some(dim.value),
                _ => None,
            })
            .collect();
        assert_eq!(set_backs, [3.0, 3.0], "editable driving dimensions");
        assert!(c.tools.corner_pick.is_none());
        assert_eq!(
            c.modify(),
            Some(Modify::Chamfer),
            "ready for the next corner"
        );
    }

    #[test]
    fn a_size_can_be_an_expression_and_follows_its_parameter() {
        let mut c = picked(Modify::Chamfer);
        assert!(c.apply_reporting(&Command::SetParameter {
            name: "bevel".to_owned(),
            expr: "1.5".to_owned(),
        }));
        c.tools.corner.text.d1 = "bevel * 2".to_owned();
        assert!(c.corner_ok(Modify::Chamfer, Units::Mm));
        let doc = c.doc.sketcher.document();
        let exprs = doc
            .constraints()
            .keys()
            .filter(|k| doc.dimension_expr(**k) == Some("bevel * 2"))
            .count();
        assert_eq!(exprs, 2);
    }

    #[test]
    fn bad_sizes_are_explained_and_esc_drops_the_pick() {
        let mut c = picked(Modify::Fillet);
        c.tools.corner.text.radius = "nope +".to_owned();
        assert!(!c.corner_ok(Modify::Fillet, Units::Mm));
        assert!(c.doc.notice.is_some());
        assert!(c.corner_esc());
        assert!(!c.corner_esc(), "a second Esc has nothing to drop");
        // Inches: a typed 0.1 in fillet is 2.54 mm.
        let mut c = picked(Modify::Fillet);
        c.tools.corner.text.radius = "0.1".to_owned();
        assert!(c.corner_ok(Modify::Fillet, Units::Inch));
        let doc = c.doc.sketcher.document();
        assert!(doc.constraints().values().any(|k| matches!(
            k,
            Constraint::Radius { dim, .. } if (dim.value - 2.54).abs() < 1e-9
        )));
    }
}
