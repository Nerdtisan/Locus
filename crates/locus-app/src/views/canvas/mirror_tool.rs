// SPDX-License-Identifier: AGPL-3.0-or-later
//! The Mirror tool (#215, design §6.9, SPEC §6.2.1, ADR 0011).
//!
//! Select the objects (before or while the tool is active), then pick a
//! line or construction line as the mirror line. A live preview shows the
//! copies, and Enter or OK adds the mirror recipe as one command (§11). The
//! copies are tied to their sources by Symmetric constraints, so they
//! follow the sources and the line.
//!
//! "Merge on mirror line" (on by default) leaves out curves that lie on
//! the mirror line. Their mirror image is themselves, so copying them would
//! cut twice. Points on the line mirror onto themselves through their
//! Symmetric constraint.

use crate::hit;
use crate::modify_tool::Modify;
use crate::render::Frame;
use crate::scene::Shape;
use crate::settings::Units;
use crate::state::AppState;
use egui::{PointerButton, Rect};
use locus_doc::recipe::RecipeKind;
use locus_doc::{Command, Document, EntityId};
use locus_geom::{Line, SOLVER_TOL};

/// The Mirror tool's state, kept between uses.
#[derive(Clone, Debug)]
pub struct MirrorDraft {
    /// The mirror line.
    pub axis: Option<EntityId>,
    /// The next click picks the mirror line (not objects).
    pub picking: bool,
    /// Leave out curves that lie on the mirror line (design §6.9).
    pub merge: bool,
    preview: Option<(String, Vec<Shape>)>,
}

impl Default for MirrorDraft {
    fn default() -> Self {
        Self {
            axis: None,
            picking: true,
            merge: true,
            preview: None,
        }
    }
}

/// Whether every point of curve `id` lies on the (infinite) line `axis`.
fn on_line(doc: &Document, id: EntityId, axis: &Line) -> bool {
    let Some(e) = doc.entity(id) else {
        return false;
    };
    let Some(u) = axis.tangent() else {
        return false;
    };
    let pts = e.geom.point_refs();
    // Circles and arcs bulge off any line, so only straight pieces qualify.
    doc.line(id).is_some()
        && !pts.is_empty()
        && pts.iter().all(|p| {
            doc.point_pos(*p)
                .is_some_and(|q| (q - axis.a).cross(u).abs() <= SOLVER_TOL)
        })
}

impl AppState {
    /// The sources for the mirror: the selection minus the mirror line,
    /// and minus curves on the line when merging.
    fn mirror_sources(&self) -> Vec<EntityId> {
        let doc = self.doc.sketcher.document();
        let d = &self.tools.mirror;
        let axis = d.axis.and_then(|a| doc.line(a));
        self.selection
            .items
            .iter()
            .filter(|id| Some(*id) != d.axis)
            .filter(|id| !(d.merge && axis.is_some_and(|l| on_line(doc, *id, &l))))
            .collect()
    }

    fn mirror_command(&self) -> Result<Command, String> {
        let axis = self
            .tools
            .mirror
            .axis
            .filter(|a| self.doc.sketcher.document().line(*a).is_some())
            .ok_or_else(|| "Pick the mirror line".to_owned())?;
        let sources = self.mirror_sources();
        if sources.is_empty() {
            return Err("Select the objects to mirror".to_owned());
        }
        Ok(Command::AddRecipe {
            kind: RecipeKind::Mirror { axis },
            sources,
        })
    }

    /// Pointer input while Mirror is active: pick the line, or select.
    pub(crate) fn mirror_input(
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
                if self.tools.mirror.picking && self.tools.mirror.axis.is_some() {
                    self.tools.mirror.picking = false;
                } else {
                    self.set_tool(crate::tools::Tool::Select);
                }
                return;
            }
            if enter {
                self.mirror_ok();
                return;
            }
        }
        if self.tools.mirror.picking {
            let tol = self.view.tol_pt / self.view.camera.pt_per_mm;
            let w = resp.hover_pos().map(|p| self.view.camera.to_world(p, rect));
            let line = w
                .and_then(|w| hit::pick_curve(&self.doc.scene.items, w, tol))
                .filter(|id| self.doc.sketcher.document().line(*id).is_some());
            self.selection.hover = line;
            if resp.hovered() {
                ui.ctx().set_cursor_icon(if line.is_some() {
                    egui::CursorIcon::PointingHand
                } else {
                    egui::CursorIcon::Crosshair
                });
            }
            if resp.clicked_by(PointerButton::Primary) {
                match line {
                    Some(id) => {
                        self.tools.mirror.axis = Some(id);
                        self.tools.mirror.picking = false;
                        self.doc.notice = None;
                    }
                    None => self.doc.notice = Some("Click a line for the mirror line".to_owned()),
                }
            }
            return;
        }
        self.pick_and_select(ui, resp, rect, units, false);
    }

    /// Enter / OK: add the mirror as one undo step, then back to Select.
    pub(crate) fn mirror_ok(&mut self) -> bool {
        let cmd = match self.mirror_command() {
            Ok(c) => c,
            Err(why) => {
                self.doc.notice = Some(why);
                return false;
            }
        };
        if self.apply_reporting(&cmd) {
            self.selection.items.clear();
            self.tools.mirror.preview = None;
            self.set_tool(crate::tools::Tool::Select);
            true
        } else {
            false
        }
    }

    pub(crate) fn refresh_mirror_preview(&mut self) {
        let Ok(cmd) = self.mirror_command() else {
            self.tools.mirror.preview = None;
            return;
        };
        let key = format!("{cmd:?}");
        if self
            .tools
            .mirror
            .preview
            .as_ref()
            .is_some_and(|(k, _)| *k == key)
        {
            return;
        }
        let shapes = super::pattern_tool::recipe_preview(self.doc.sketcher.document(), &cmd);
        self.tools.mirror.preview = Some((key, shapes));
    }

    /// The preview, and the mirror line highlighted.
    pub(crate) fn draw_mirror_draft(&self, frame: &Frame<'_>) {
        if let Some((_, shapes)) = &self.tools.mirror.preview {
            frame.draw_preview(shapes);
        }
        if let Some(l) = self
            .tools
            .mirror
            .axis
            .and_then(|a| self.doc.sketcher.document().line(a))
        {
            let (a, b) = (
                frame.view.to_screen(l.a, frame.rect),
                frame.view.to_screen(l.b, frame.rect),
            );
            frame
                .painter
                .line_segment([a, b], egui::Stroke::new(3.0, frame.pal.selected));
        }
    }

    /// The Mirror command dialog.
    pub(crate) fn mirror_dialog(&mut self, ui: &egui::Ui, rect: Rect) {
        let selected = self.mirror_sources().len();
        let error = self.mirror_command().err();
        let (mut ok, mut cancel) = (false, false);
        let mut clear = false;
        let header = super::dialog::Header::new("sketch.mirror", Modify::Mirror.label());
        super::dialog::command_dialog(
            ui,
            rect,
            header,
            |_| {},
            |ui| {
                use super::dialog::{row, selection};
                let d = &mut self.tools.mirror;
                row(ui, "Objects", |ui| {
                    let text = if selected == 0 {
                        "Select geometry".to_owned()
                    } else {
                        super::dialog::count(selected, "curve", "curves")
                    };
                    let (pick, x) = selection(ui, &text, selected > 0, !d.picking);
                    if pick {
                        d.picking = false;
                    }
                    clear = x;
                });
                row(ui, "Mirror line", |ui| {
                    let (text, filled) = match d.axis {
                        Some(_) => ("1 line", true),
                        None => ("Pick a line", false),
                    };
                    let (pick, x) = selection(ui, text, filled, d.picking);
                    if pick {
                        d.picking = !d.picking;
                    }
                    if x {
                        d.axis = None;
                        d.picking = true;
                    }
                });
                row(ui, "", |ui| {
                    ui.checkbox(&mut d.merge, "Merge on mirror line")
                        .on_hover_text(
                            "Leave out curves that lie on the mirror line, so nothing is cut twice",
                        )
                });
                if d.picking {
                    ui.weak("Click the mirror line on the canvas (Esc to select objects)");
                }
                if let Some(why) = &error {
                    ui.colored_label(ui.visuals().error_fg_color, why);
                }
                (ok, cancel) = super::dialog::footer(ui, error.as_deref().map_or(Ok(()), Err));
            },
        );
        if clear {
            self.clear_selection();
        }
        if ok {
            self.mirror_ok();
        } else if cancel {
            self.set_tool(crate::tools::Tool::Select);
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)] // a missing fixture should fail loudly
mod tests {
    use super::*;
    use crate::tool::ToolEvent;
    use crate::tools::{Pick, Tool};
    use locus_geom::Vec2;

    /// A blank editor with a rectangle from (0,0) to (20,10) and a vertical
    /// mirror line on its right edge's carrier at x = 30.
    fn ready() -> (AppState, Vec<EntityId>, EntityId) {
        let mut c = AppState::default();
        c.load(Document::new());
        c.set_tool(Tool::RectCorner);
        for p in [Vec2::ZERO, Vec2::new(20.0, 10.0)] {
            c.tool_event(ToolEvent::Click(Pick::free(p)), None, Units::Mm);
        }
        c.set_tool(Tool::Line);
        for p in [Vec2::new(30.0, -5.0), Vec2::new(30.0, 15.0)] {
            c.tool_event(ToolEvent::Click(Pick::free(p)), None, Units::Mm);
        }
        c.set_tool(Tool::Select);
        let doc = c.doc.sketcher.document();
        let lines: Vec<EntityId> = doc
            .entities()
            .iter()
            .filter(|(id, _)| doc.line(*id).is_some())
            .map(|(id, _)| id)
            .collect();
        let axis = *lines.last().expect("the mirror line");
        (c, lines[..4].to_vec(), axis)
    }

    #[test]
    fn mirror_a_shape_across_a_line_in_one_step_and_it_follows() {
        let (mut c, rect, axis) = ready();
        c.selection.items.set_all(rect.iter().copied());
        c.set_modify(Modify::Mirror);
        assert!(!c.mirror_ok(), "no mirror line yet");
        c.tools.mirror.axis = Some(axis);
        c.tools.mirror.picking = false;
        c.refresh_mirror_preview();
        let shapes = &c.tools.mirror.preview.as_ref().expect("a preview").1;
        assert_eq!(shapes.len(), 4);
        assert!(c.mirror_ok());
        assert_eq!(c.undo_label(), Some("Add mirror"));
        let doc = c.doc.sketcher.document();
        let xs: Vec<f64> = doc
            .entities()
            .iter()
            .filter_map(|(id, _)| doc.point_pos(id))
            .map(|p| p.x)
            .collect();
        assert!(
            xs.iter().any(|x| (x - 60.0).abs() < 1e-6),
            "the far corner mirrors to x = 60: {xs:?}"
        );
        // Undo removes the mirror in one step.
        c.undo();
        assert!(c.doc.sketcher.document().recipes().is_empty());
    }

    #[test]
    fn merge_leaves_out_curves_on_the_mirror_line() {
        let (mut c, rect, axis) = ready();
        // A short line lying on the mirror line (x = 30), selected with the
        // rectangle: its mirror image is itself.
        c.set_tool(Tool::Line);
        for p in [Vec2::new(30.0, 20.0), Vec2::new(30.0, 25.0)] {
            c.tool_event(ToolEvent::Click(Pick::free(p)), None, Units::Mm);
        }
        c.set_tool(Tool::Select);
        let doc = c.doc.sketcher.document();
        let on_axis = doc
            .entities()
            .iter()
            .map(|(id, _)| id)
            .filter(|id| doc.line(*id).is_some())
            .last()
            .expect("the line on the axis");
        let mut sel = rect.clone();
        sel.push(on_axis);
        sel.push(axis);
        c.selection.items.set_all(sel);
        c.set_modify(Modify::Mirror);
        c.tools.mirror.axis = Some(axis);
        c.tools.mirror.picking = false;
        // The mirror line itself is never a source; the line on it is left
        // out while merging.
        assert_eq!(c.mirror_sources().len(), 4);
        c.tools.mirror.merge = false;
        assert_eq!(c.mirror_sources().len(), 5);
        c.tools.mirror.merge = true;
        assert!(c.mirror_ok());
    }
}
