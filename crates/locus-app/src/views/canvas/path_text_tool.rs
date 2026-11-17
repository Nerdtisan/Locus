// SPDX-License-Identifier: AGPL-3.0-or-later
//! Text on a path (#392, SPEC §6.2, design §6.5 placement "on a path").
//!
//! Click a line, arc, circle, shape outline or Bézier (its connected chain
//! is the path), type the text and set the font, height, letter spacing,
//! start offset, side and alignment, with a live preview. Enter adds a
//! `TextOnPath` recipe as one command (§11); the text then follows the path
//! when it changes. Double-clicking the placed text reopens the dialog, and
//! OK applies the edit as one `EditRecipe`.

use crate::render::Frame;
use crate::scene::Shape;
use crate::settings::Units;
use crate::state::{AppState, format_value, to_stored};
use egui::{PointerButton, Rect};
use locus_doc::path_text::{PathSide, PathText};
use locus_doc::{Command, Document, Editor, EntityId, Geom, RecipeId, RecipeKind};
use locus_text::{Align, BundledFont, FontRef};

/// The tool's path, text and placement (kept between uses, like options).
#[derive(Clone, Debug)]
pub struct PathTextDraft {
    /// The path: one chain of curves.
    pub sources: Vec<EntityId>,
    /// The recipe being edited, or `None` for new text.
    pub editing: Option<RecipeId>,
    pub content: String,
    pub font: FontRef,
    /// Height, letter spacing and start offset, in display units (numbers
    /// or expressions).
    pub size: String,
    pub tracking: String,
    pub offset: String,
    pub side: PathSide,
    pub align: Align,
    pub flip: bool,
    preview: Option<(String, Vec<Shape>)>,
}

impl Default for PathTextDraft {
    fn default() -> Self {
        Self {
            sources: Vec::new(),
            editing: None,
            content: "Text".to_owned(),
            font: FontRef::bundled(BundledFont::HersheySans),
            size: "5".to_owned(),
            tracking: "0".to_owned(),
            offset: "0".to_owned(),
            side: PathSide::Above,
            align: Align::Center,
            flip: false,
            preview: None,
        }
    }
}

impl PathTextDraft {
    /// A fresh run for new text: no path picked, the options kept.
    pub fn begin(&mut self) {
        self.sources.clear();
        self.editing = None;
        self.preview = None;
    }
}

/// A length typed in display units: a number or an expression.
fn length(doc: &Document, text: &str, units: Units) -> Option<f64> {
    locus_doc::expr::evaluate(&to_stored(text, units), doc.parameters())
        .ok()
        .filter(|v| v.is_finite())
}

impl AppState {
    /// The recipe kind for the draft, or why there isn't one.
    fn path_text_kind(&self, units: Units) -> Result<RecipeKind, String> {
        let d = &self.tools.path_text;
        let doc = self.doc.sketcher.document();
        let size = length(doc, &d.size, units)
            .filter(|v| *v > 0.0)
            .ok_or_else(|| "Type the text height".to_owned())?;
        let tracking =
            length(doc, &d.tracking, units).ok_or_else(|| "Type the letter spacing".to_owned())?;
        let offset =
            length(doc, &d.offset, units).ok_or_else(|| "Type the start offset".to_owned())?;
        let mut t = PathText::new(
            &d.content,
            d.font.clone(),
            size,
            tracking,
            offset,
            d.side,
            d.align,
            &self.tools.fonts.library,
        )
        .map_err(|e| e.to_string())?;
        t.flip = d.flip;
        Ok(RecipeKind::TextOnPath(Box::new(t)))
    }

    fn path_text_command(&self, units: Units) -> Result<Command, String> {
        let d = &self.tools.path_text;
        if d.sources.is_empty() {
            return Err("Click a line, arc, circle, shape or Bézier for the path".to_owned());
        }
        let kind = self.path_text_kind(units)?;
        Ok(match d.editing {
            Some(id) => Command::EditRecipe { id, kind },
            None => Command::AddRecipe {
                kind,
                sources: d.sources.clone(),
            },
        })
    }

    /// Reopen placed text on a path in the dialog (double-click, #392).
    pub(crate) fn edit_path_text(&mut self, rid: RecipeId, units: Units) {
        let Some(r) = self.doc.sketcher.document().recipe(rid).cloned() else {
            return;
        };
        let RecipeKind::TextOnPath(t) = &r.kind else {
            return;
        };
        self.set_modify(crate::modify_tool::Modify::TextOnPath);
        let d = &mut self.tools.path_text;
        d.sources = r.sources.clone();
        d.editing = Some(rid);
        d.content = t.content.clone();
        d.font = t.font.clone();
        d.size = format_value(t.size, false, units);
        d.tracking = format_value(t.tracking, false, units);
        d.offset = format_value(t.offset, false, units);
        d.side = t.side;
        d.align = t.align;
        d.flip = t.flip;
    }

    /// Pointer input: a click on a curve picks its chain as the path.
    pub(crate) fn path_text_input(
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
                self.set_tool(crate::tools::Tool::Select);
                return;
            }
            if enter {
                self.path_text_ok(units);
                return;
            }
        }
        if self.tools.path_text.editing.is_some() {
            return;
        }
        let tol = self.view.tol_pt / self.view.camera.pt_per_mm;
        let doc = self.doc.sketcher.document();
        let curve = resp
            .hover_pos()
            .map(|p| self.view.camera.to_world(p, rect))
            .and_then(|w| crate::hit::pick_curve(&self.doc.scene.items, w, tol))
            .filter(|id| {
                doc.entity(*id).is_some_and(|e| {
                    matches!(
                        e.geom,
                        Geom::Line { .. }
                            | Geom::Arc { .. }
                            | Geom::Circle { .. }
                            | Geom::Bezier { .. }
                    )
                }) && doc.driven_by(*id).is_none()
            });
        self.selection.hover = curve;
        if resp.hovered() {
            ui.ctx().set_cursor_icon(if curve.is_some() {
                egui::CursorIcon::PointingHand
            } else {
                egui::CursorIcon::Crosshair
            });
        }
        if resp.clicked_by(PointerButton::Primary) {
            match curve {
                Some(id) => {
                    let chain = locus_doc::offset::chain_through(doc, id);
                    self.tools.path_text.sources = if chain.is_empty() { vec![id] } else { chain };
                    self.doc.notice = None;
                }
                None => {
                    self.doc.notice =
                        Some("Click a line, arc, circle, shape or Bézier for the path".to_owned());
                }
            }
        }
    }

    /// Enter / OK: add or change the text as one undo step, then Select.
    pub(crate) fn path_text_ok(&mut self, units: Units) -> bool {
        match self.path_text_command(units) {
            Ok(cmd) => {
                if self.apply_reporting(&cmd) {
                    self.tools.path_text.begin();
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

    pub(crate) fn refresh_path_text_preview(&mut self, units: Units) {
        let Ok(cmd) = self.path_text_command(units) else {
            self.tools.path_text.preview = None;
            return;
        };
        let key = format!("{cmd:?}");
        if self
            .tools
            .path_text
            .preview
            .as_ref()
            .is_some_and(|(k, _)| *k == key)
        {
            return;
        }
        let shapes = path_text_preview(self.doc.sketcher.document(), &cmd);
        self.tools.path_text.preview = Some((key, shapes));
    }

    /// The preview and the picked path, highlighted.
    pub(crate) fn draw_path_text_draft(&self, frame: &Frame<'_>) {
        if let Some((_, shapes)) = &self.tools.path_text.preview {
            frame.draw_preview(shapes);
        }
    }

    /// The Text on path command dialog.
    pub(crate) fn path_text_dialog(&mut self, ui: &egui::Ui, rect: Rect, units: Units) {
        let error = self.path_text_command(units).err();
        let (mut ok, mut cancel) = (false, false);
        let mut fonts: Vec<FontRef> = BundledFont::ALL.into_iter().map(FontRef::bundled).collect();
        let lib = &self.tools.fonts.library;
        fonts.extend(lib.user_fonts().map(|n| FontRef::File(n.to_owned())));
        fonts.extend(lib.system_families().into_iter().map(FontRef::System));
        let unit = units.label();
        let header = super::dialog::Header::new("sketch.text_path", "Text on path");
        super::dialog::command_dialog(
            ui,
            rect,
            header,
            |_| {},
            |ui| {
                use super::dialog::{row, segmented, selection};
                let d = &mut self.tools.path_text;
                row(ui, "Path", |ui| {
                    let n = d.sources.len();
                    let text = match n {
                        0 => "Pick a path".to_owned(),
                        _ => super::dialog::count(n, "curve", "curves"),
                    };
                    let (pick, x) = selection(ui, &text, n > 0, n == 0);
                    if (pick || x) && d.editing.is_none() {
                        d.sources.clear();
                    }
                });
                row(ui, "Text", |ui| {
                    ui.add(egui::TextEdit::singleline(&mut d.content).desired_width(180.0))
                });
                row(ui, "Font", |ui| {
                    egui::ComboBox::from_id_salt("path-text-font")
                        .selected_text(d.font.display_name())
                        .width(180.0)
                        .show_ui(ui, |ui| {
                            for f in fonts {
                                let name = f.display_name();
                                ui.selectable_value(&mut d.font, f, name);
                            }
                        })
                });
                let field = |ui: &mut egui::Ui, text: &mut String, hint: &str| {
                    ui.add(egui::TextEdit::singleline(text).desired_width(64.0))
                        .on_hover_text(hint);
                    ui.weak(unit);
                };
                row(ui, "Height", |ui| field(ui, &mut d.size, "Letter height"));
                row(ui, "Spacing", |ui| {
                    field(ui, &mut d.tracking, "Extra space between letters")
                });
                row(ui, "Start offset", |ui| {
                    field(
                        ui,
                        &mut d.offset,
                        "How far the text moves along the path from where the alignment puts it",
                    )
                });
                row(ui, "Side", |ui| {
                    if let Some(s) = segmented(
                        ui,
                        d.side,
                        &[(PathSide::Above, "Above"), (PathSide::Below, "Below")],
                    ) {
                        d.side = s;
                    }
                });
                row(ui, "Alignment", |ui| {
                    if let Some(a) = segmented(
                        ui,
                        d.align,
                        &[
                            (Align::Left, "Start"),
                            (Align::Center, "Centre"),
                            (Align::Right, "End"),
                        ],
                    ) {
                        d.align = a;
                    }
                });
                row(ui, "", |ui| {
                    crate::theme::checkbox(ui, &mut d.flip, "Flip direction")
                        .on_hover_text("Run the text the other way along the path")
                });
                if let Some(why) = &error {
                    ui.colored_label(ui.visuals().error_fg_color, why);
                }
                (ok, cancel) = super::dialog::footer(ui, error.as_deref().map_or(Ok(()), Err));
            },
        );
        if ok {
            self.path_text_ok(units);
        } else if cancel {
            self.set_tool(crate::tools::Tool::Select);
        }
    }
}

/// The glyphs `cmd` would place: the recipe's instances after applying it
/// to a copy of the document.
fn path_text_preview(doc: &Document, cmd: &Command) -> Vec<Shape> {
    let mut ed = Editor::new(doc.clone());
    let Ok(applied) = ed.apply(cmd) else {
        return Vec::new();
    };
    let rid = match cmd {
        Command::EditRecipe { id, .. } => *id,
        _ => match applied.recipes.first() {
            Some(r) => *r,
            None => return Vec::new(),
        },
    };
    let after = ed.document();
    let Some(r) = after.recipe(rid) else {
        return Vec::new();
    };
    r.instances()
        .filter_map(|id| match after.entity(id)?.geom {
            Geom::Line { .. } => after.line(id).map(Shape::Line),
            Geom::Bezier { .. } => after.cubic(id).map(Shape::Cubic),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // test fixtures
mod tests {
    use super::*;
    use crate::modify_tool::Modify;
    use crate::tool::ToolEvent;
    use crate::tools::{Pick, Tool};
    use locus_geom::Vec2;

    /// A blank editor with one 100 mm line.
    fn with_line() -> (AppState, EntityId) {
        let mut c = AppState::default();
        c.load(Document::new());
        c.set_tool(Tool::Line);
        for p in [Vec2::ZERO, Vec2::new(100.0, 0.0)] {
            c.tool_event(ToolEvent::Click(Pick::free(p)), None, Units::Mm);
        }
        c.set_tool(Tool::Select);
        let doc = c.document();
        let line = doc
            .entities()
            .iter()
            .find(|(id, _)| doc.line(*id).is_some())
            .map(|(id, _)| id)
            .unwrap();
        (c, line)
    }

    fn recipe(c: &AppState) -> Option<(RecipeId, PathText)> {
        c.document()
            .recipes()
            .iter()
            .find_map(|(id, r)| match &r.kind {
                RecipeKind::TextOnPath(t) => Some((*id, (**t).clone())),
                _ => None,
            })
    }

    /// #392: pick a path, type, OK: one undo step; reopening the text edits
    /// it in place (re-editable), as one more step.
    #[test]
    fn add_then_reedit_text_on_a_line() {
        let (mut c, line) = with_line();
        c.set_modify(Modify::TextOnPath);
        assert!(!c.path_text_ok(Units::Mm), "no path yet");
        c.tools.path_text.sources = vec![line];
        c.tools.path_text.content = "HELLO".to_owned();
        c.refresh_path_text_preview(Units::Mm);
        assert!(
            !c.tools.path_text.preview.as_ref().unwrap().1.is_empty(),
            "a preview"
        );
        assert!(c.path_text_ok(Units::Mm));
        assert_eq!(c.undo_label(), Some("Add text on path"));
        let (rid, t) = recipe(&c).unwrap();
        assert_eq!(t.content, "HELLO");
        c.edit_path_text(rid, Units::Mm);
        assert_eq!(c.modify(), Some(Modify::TextOnPath));
        assert_eq!(c.tools.path_text.content, "HELLO");
        c.tools.path_text.content = "WORLD".to_owned();
        c.tools.path_text.side = PathSide::Below;
        assert!(c.path_text_ok(Units::Mm));
        assert_eq!(c.undo_label(), Some("Edit text on path"));
        let (rid2, t) = recipe(&c).unwrap();
        assert_eq!(
            (rid2, t.content.as_str(), t.side),
            (rid, "WORLD", PathSide::Below)
        );
        c.undo();
        assert_eq!(recipe(&c).unwrap().1.content, "HELLO");
    }

    #[test]
    fn bad_fields_say_why() {
        let (mut c, line) = with_line();
        c.set_modify(Modify::TextOnPath);
        c.tools.path_text.sources = vec![line];
        c.tools.path_text.size = "-1".to_owned();
        assert!(c.path_text_command(Units::Mm).is_err());
        c.tools.path_text.size = "5".to_owned();
        c.tools.path_text.content = "  ".to_owned();
        assert!(c.path_text_command(Units::Mm).is_err());
    }
}
