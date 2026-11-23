// SPDX-License-Identifier: AGPL-3.0-or-later
//! Text on a path (#392, owner's flow, SPEC §6.2, §9).
//!
//! The tool binds **existing** text to a path: click a text, then click a
//! line, arc, circle, shape outline or Bézier (its connected chain is the
//! path). That is one command (§11): the text's own data gains a
//! [`TextPath`], so the text stays one re-editable entity (#222) whose
//! glyphs follow the path, also when the path changes.
//!
//! Afterwards, dragging the text slides it along the path, snapped to it,
//! with a live preview; releasing sets its offset as one undo step. Side,
//! flip, offset and Detach are in the Inspector.

use crate::render::Frame;
use crate::state::AppState;
use egui::{PointerButton, Rect};
use locus_doc::path_text::{self, PathSide, TextPath};
use locus_doc::{Command, EntityId, Geom};
use locus_geom::Vec2;

/// The tool's picks and the options a new binding takes.
#[derive(Clone, Debug, Default)]
pub struct PathTextDraft {
    /// The text to bind, once clicked.
    pub text: Option<EntityId>,
    pub side: PathSide,
    pub flip: bool,
}

impl PathTextDraft {
    /// A fresh run: the text already selected, if one is.
    pub fn begin(&mut self, text: Option<EntityId>) {
        self.text = text;
    }
}

/// A text being slid along its path by a drag.
#[derive(Clone, Debug)]
pub struct Slide {
    pub text: EntityId,
    /// The path polyline and whether it is closed.
    pts: Vec<Vec2>,
    /// Where along the path the drag started, mm.
    grab: f64,
    /// The text's offset when the drag started, and now.
    from: f64,
    pub offset: f64,
}

impl AppState {
    /// `text` with `edit` applied to its data, as the command that does it.
    /// Re-lays out the glyphs with the font library.
    pub(crate) fn text_path_command(
        &self,
        text: EntityId,
        label: &str,
        edit: impl FnOnce(&mut locus_doc::TextData),
    ) -> Result<Command, String> {
        let doc = self.doc.sketcher.document();
        let (data, anchor, dir) = doc.text(text).ok_or("Click a text")?;
        let next = data
            .edited(&self.tools.fonts.library, edit)
            .map_err(|e| e.to_string())?;
        Ok(Command::Compound {
            label: label.to_owned(),
            commands: vec![Command::ModifyEntity {
                id: text,
                geom: Geom::Text {
                    anchor,
                    dir,
                    text: Box::new(next),
                },
            }],
        })
    }

    /// Bind the picked text to the chain through `curve` (one undo step).
    pub(crate) fn bind_text_to_path(&mut self, curve: EntityId) -> bool {
        let Some(text) = self.tools.path_text.text else {
            self.doc.notice = Some("Click the text first".to_owned());
            return false;
        };
        let doc = self.doc.sketcher.document();
        let chain = locus_doc::offset::chain_through(doc, curve);
        let curves = if chain.is_empty() { vec![curve] } else { chain };
        if let Err(e) = path_text::path(doc, &curves, self.tools.path_text.flip) {
            self.doc.notice = Some(e.to_string());
            return false;
        }
        let (side, flip) = (self.tools.path_text.side, self.tools.path_text.flip);
        let cmd = self.text_path_command(text, "Text on path", |t| {
            let mut p = TextPath::new(curves);
            p.side = side;
            p.flip = flip;
            t.path = Some(Box::new(p));
        });
        match cmd {
            Ok(cmd) if self.apply_reporting(&cmd) => {
                self.tools.path_text.text = None;
                self.selection.items.set_all([text]);
                self.set_tool(crate::tools::Tool::Select);
                true
            }
            Ok(_) => false,
            Err(why) => {
                self.doc.notice = Some(why);
                false
            }
        }
    }

    /// Put bound text back to plain text at its anchor (one undo step).
    pub(crate) fn detach_text(&mut self, text: EntityId) -> bool {
        match self.text_path_command(text, "Detach text from path", |t| t.path = None) {
            Ok(cmd) => self.apply_reporting(&cmd),
            Err(why) => {
                self.doc.notice = Some(why);
                false
            }
        }
    }

    /// Pointer input: the first click picks a text, the next a path.
    pub(crate) fn path_text_input(&mut self, ui: &egui::Ui, resp: &egui::Response, rect: Rect) {
        use egui::{Key, Modifiers};
        if resp.hovered()
            && ui
                .ctx()
                .input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape))
        {
            if self.tools.path_text.text.take().is_none() {
                self.set_tool(crate::tools::Tool::Select);
            }
            return;
        }
        let tol = self.view.tol_pt / self.view.camera.pt_per_mm;
        let doc = self.doc.sketcher.document();
        let hit = resp
            .hover_pos()
            .map(|p| self.view.camera.to_world(p, rect))
            .and_then(|w| crate::hit::pick(&self.doc.scene.items, w, tol));
        let want_text = self.tools.path_text.text.is_none();
        let target = hit.filter(|id| {
            let geom = doc.entity(*id).map(|e| &e.geom);
            if want_text {
                matches!(geom, Some(Geom::Text { .. }))
            } else {
                matches!(
                    geom,
                    Some(
                        Geom::Line { .. }
                            | Geom::Arc { .. }
                            | Geom::Circle { .. }
                            | Geom::Bezier { .. }
                    )
                ) && doc.driven_by(*id).is_none()
            }
        });
        self.selection.hover = target;
        if resp.hovered() {
            ui.ctx().set_cursor_icon(if target.is_some() {
                egui::CursorIcon::PointingHand
            } else {
                egui::CursorIcon::Crosshair
            });
        }
        if !resp.clicked_by(PointerButton::Primary) {
            return;
        }
        match (target, want_text) {
            (Some(t), true) => {
                self.tools.path_text.text = Some(t);
                self.selection.items.set_all([t]);
                self.doc.notice = None;
            }
            (Some(c), false) => {
                self.bind_text_to_path(c);
            }
            (None, true) => self.doc.notice = Some("Click a text to put on a path".to_owned()),
            (None, false) => {
                self.doc.notice =
                    Some("Click a line, arc, circle, shape or Bézier for the path".to_owned());
            }
        }
    }

    /// Start sliding bound text along its path from `at` (a drag on it).
    /// `false` when the text isn't on a path.
    pub(crate) fn begin_slide(&mut self, text: EntityId, at: Vec2) -> bool {
        let doc = self.doc.sketcher.document();
        let Some(p) = doc.text(text).and_then(|(t, _, _)| t.path.as_deref()) else {
            return false;
        };
        let Ok((pts, _)) = path_text::path(doc, &p.curves, p.flip) else {
            return false;
        };
        let grab = path_text::along(&pts, at);
        self.tools.slide = Some(Slide {
            text,
            pts,
            grab,
            from: p.offset,
            offset: p.offset,
        });
        self.tools.dragging = true;
        true
    }

    /// One frame of a slide: the offset follows the pointer along the path;
    /// release applies it as one step, Esc cancels.
    pub(crate) fn slide_frame(&mut self, ui: &egui::Ui, resp: &egui::Response, rect: Rect) {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.tools.slide = None;
            self.tools.dragging = false;
            return;
        }
        if resp.dragged_by(PointerButton::Primary)
            && let Some(p) = resp.interact_pointer_pos()
            && let Some(s) = self.tools.slide.as_mut()
        {
            let w = self.view.camera.to_world(p, rect);
            s.offset = s.from + path_text::along(&s.pts, w) - s.grab;
        }
        if resp.drag_stopped_by(PointerButton::Primary) || !resp.dragged() {
            self.tools.dragging = false;
            if let Some(s) = self.tools.slide.take()
                && (s.offset - s.from).abs() > 1e-9
            {
                let offset = s.offset;
                match self.text_path_command(s.text, "Slide text along path", |t| {
                    if let Some(p) = t.path.as_mut() {
                        p.offset = offset;
                    }
                }) {
                    Ok(cmd) => {
                        self.apply_reporting(&cmd);
                    }
                    Err(why) => self.doc.notice = Some(why),
                }
            }
        }
    }

    /// While sliding, the text where it will land.
    pub(crate) fn draw_slide(&self, frame: &Frame<'_>) {
        let Some(s) = &self.tools.slide else {
            return;
        };
        let doc = self.doc.sketcher.document();
        let Some((t, _, _)) = doc.text(s.text) else {
            return;
        };
        let Some(p) = t.path.as_deref() else {
            return;
        };
        let mut moved = p.clone();
        moved.offset = s.offset;
        let Ok((pts, closed)) = path_text::path(doc, &p.curves, p.flip) else {
            return;
        };
        let g = locus_text::TextGeometry {
            contours: path_text::bend(&moved, t.align, t.tracking, &pts, closed),
            ..Default::default()
        };
        let mut shapes = crate::scene::text_shapes(&g);
        // The alignment guide slides with the text (#416).
        if let Some((a, b)) = path_text::guide(&moved, t.align, t.size, &pts, closed) {
            shapes.push(crate::scene::Shape::Line(locus_geom::Line::new(a, b)));
        }
        frame.draw_preview(&shapes);
    }

    /// The Text on path dialog: what is picked, and the options.
    pub(crate) fn path_text_dialog(&mut self, ui: &egui::Ui, rect: Rect) {
        let mut cancel = false;
        let header = super::dialog::Header::new("sketch.text_path", "Text on path");
        super::dialog::command_dialog(
            ui,
            rect,
            header,
            |_| {},
            |ui| {
                use super::dialog::{row, segmented, selection};
                let d = &mut self.tools.path_text;
                row(ui, "Text", |ui| {
                    let (text, filled) = match d.text {
                        Some(_) => ("1 text", true),
                        None => ("Pick a text", false),
                    };
                    let (pick, x) = selection(ui, text, filled, d.text.is_none());
                    if pick || x {
                        d.text = None;
                    }
                });
                row(ui, "Path", |ui| {
                    let _ = selection(ui, "Pick a path", false, d.text.is_some());
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
                row(ui, "", |ui| {
                    crate::theme::checkbox(ui, &mut d.flip, "Flip direction")
                        .on_hover_text("Run the text the other way along the path")
                });
                ui.weak(if d.text.is_none() {
                    "Click a text on the canvas."
                } else {
                    "Click a line, arc, circle, shape or Bézier for it to follow."
                });
                ui.weak("Then drag the text to slide it along the path.");
                cancel = super::dialog::close_footer(ui);
            },
        );
        if cancel {
            self.set_tool(crate::tools::Tool::Select);
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // test fixtures
mod tests {
    use super::*;
    use crate::modify_tool::Modify;
    use crate::settings::Units;
    use crate::tool::ToolEvent;
    use crate::tools::{Pick, Tool};
    use locus_doc::Document;
    use locus_text::{Align, BundledFont, BundledProvider, FontRef};

    /// A 100 mm line along y = 0 and a text placed at (−50, 50).
    fn ready() -> (AppState, EntityId, EntityId) {
        let mut c = AppState::default();
        c.load(Document::new());
        c.set_tool(Tool::Line);
        for p in [Vec2::ZERO, Vec2::new(100.0, 0.0)] {
            c.tool_event(ToolEvent::Click(Pick::free(p)), None, Units::Mm);
        }
        c.set_tool(Tool::Select);
        let layer = c.active_layer();
        let point = |c: &mut AppState, x: f64| {
            let id = EntityId::new(c.document().next_id());
            assert!(c.apply_reporting(&Command::AddEntity {
                layer,
                construction: false,
                geom: Geom::Point {
                    pos: Vec2::new(x, 50.0),
                },
                style: Default::default(),
            }));
            id
        };
        let (a, d) = (point(&mut c, -50.0), point(&mut c, -40.0));
        let data = locus_doc::TextData::new(
            "HELLO",
            FontRef::bundled(BundledFont::HersheySans),
            5.0,
            Align::Left,
            1.0,
            &BundledProvider,
        )
        .unwrap();
        let text = EntityId::new(c.document().next_id());
        assert!(c.apply_reporting(&Command::AddEntity {
            layer,
            construction: false,
            geom: Geom::Text {
                anchor: a,
                dir: d,
                text: Box::new(data),
            },
            style: Default::default(),
        }));
        let doc = c.document();
        let line = doc
            .entities()
            .iter()
            .find(|(id, _)| doc.line(*id).is_some())
            .map(|(id, _)| id)
            .unwrap();
        (c, text, line)
    }

    fn xs(c: &AppState, t: EntityId) -> (f64, f64) {
        let g = c.document().text_geometry(t).unwrap();
        let b = g.bbox().unwrap();
        (b.min.x, b.max.y)
    }

    /// #392, the owner's flow: pick a text, pick a path: one undo step; the
    /// text sits on the line and stays one text entity; detach puts it back.
    #[test]
    fn bind_existing_text_then_detach() {
        let (mut c, text, line) = ready();
        let straight = xs(&c, text);
        c.set_modify(Modify::TextOnPath);
        assert!(!c.bind_text_to_path(line), "no text picked yet");
        c.tools.path_text.text = Some(text);
        assert!(c.bind_text_to_path(line));
        assert_eq!(c.undo_label(), Some("Text on path"));
        assert_eq!(c.tool(), Tool::Select);
        let (min_x, top) = xs(&c, text);
        assert!(min_x >= -1e-9 && top <= 6.0, "on the line: {min_x}, {top}");
        assert!(c.document().text(text).unwrap().0.path.is_some());
        assert!(c.detach_text(text));
        assert_eq!(c.undo_label(), Some("Detach text from path"));
        assert_eq!(xs(&c, text), straight, "back at its anchor");
    }

    /// Dragging bound text slides it along the path; release is one step.
    #[test]
    fn dragging_bound_text_slides_it() {
        let (mut c, text, line) = ready();
        c.set_modify(Modify::TextOnPath);
        c.tools.path_text.text = Some(text);
        assert!(c.bind_text_to_path(line));
        let (before, _) = xs(&c, text);
        // Grab at x = 5 and move 30 mm along (and off) the line.
        assert!(c.begin_drag(text, Vec2::new(5.0, 1.0)));
        assert!(c.tools.slide.is_some(), "a slide, not a solver drag");
        let s = c.tools.slide.as_mut().unwrap();
        s.offset = s.from + path_text::along(&s.pts, Vec2::new(35.0, 7.0)) - s.grab;
        let offset = s.offset;
        assert!((offset - 30.0).abs() < 1e-9, "{offset}");
        // Release, as `slide_frame` does on drag stop.
        c.tools.dragging = false;
        let s = c.tools.slide.take().unwrap();
        let cmd = c
            .text_path_command(s.text, "Slide text along path", |t| {
                t.path.as_mut().unwrap().offset = s.offset;
            })
            .unwrap();
        assert!(c.apply_reporting(&cmd));
        let (after, _) = xs(&c, text);
        assert!((after - before - 30.0).abs() < 1e-6, "{before} → {after}");
        c.undo();
        assert!((xs(&c, text).0 - before).abs() < 1e-9);
    }

    /// #415: editing the words in the Text dialog / Inspector (the commit
    /// they share) keeps the text on its path: same path, side, offset and
    /// flip, the new words laid out along it, one undo step.
    #[test]
    fn re_editing_keeps_the_path() {
        let (mut c, text, line) = ready();
        c.set_modify(Modify::TextOnPath);
        c.tools.path_text.text = Some(text);
        assert!(c.bind_text_to_path(line));
        let slide = c
            .text_path_command(text, "Slide text along path", |t| {
                t.path.as_mut().unwrap().offset = 20.0;
            })
            .unwrap();
        assert!(c.apply_reporting(&slide));
        let (stored, anchor, dir) = c
            .document()
            .text(text)
            .map(|(t, a, d)| (t.clone(), a, d))
            .unwrap();
        let before = stored.path.as_deref().unwrap().clone();
        let (min_before, _) = xs(&c, text);
        let mut s = crate::views::text_ui::TextSettings::from_data(&stored);
        s.content = "HELLO WORLD".into();
        c.commit_text_edit(text, anchor, dir, &stored, &s).unwrap();
        let t = c.document().text(text).unwrap().0;
        assert_eq!(t.content, "HELLO WORLD");
        let p = t.path.as_deref().expect("still bound");
        assert_eq!(
            (&p.curves, p.side, p.offset, p.flip),
            (&before.curves, before.side, before.offset, before.flip)
        );
        assert_eq!(p.run.glyphs.len(), "HELLO WORLD".chars().count());
        let (min_x, top) = xs(&c, text);
        assert!(
            (min_x - min_before).abs() < 1e-6 && top <= 6.0,
            "still on the line at offset 20: {min_before} → {min_x}, {top}"
        );
        c.undo();
        assert_eq!(c.document().text(text).unwrap().0.content, "HELLO");
    }

    /// #415: changing the alignment of bound text keeps it bound and places
    /// it along the path; the anchor isn't shifted.
    #[test]
    fn re_aligning_keeps_the_path() {
        let (mut c, text, line) = ready();
        c.set_modify(Modify::TextOnPath);
        c.tools.path_text.text = Some(text);
        assert!(c.bind_text_to_path(line));
        let (stored, anchor, dir) = c
            .document()
            .text(text)
            .map(|(t, a, d)| (t.clone(), a, d))
            .unwrap();
        let at = c.document().point_pos(anchor);
        let mut s = crate::views::text_ui::TextSettings::from_data(&stored);
        s.align = Align::Right;
        c.commit_text_edit(text, anchor, dir, &stored, &s).unwrap();
        assert!(c.document().text(text).unwrap().0.path.is_some());
        assert_eq!(c.document().point_pos(anchor), at);
        let b = c.document().text_geometry(text).unwrap().bbox().unwrap();
        assert!(
            b.max.x <= 100.0 + 1e-6 && b.max.x > 90.0,
            "at the end: {b:?}"
        );
    }

    /// #416: once text made with the Text tool is bound, its dashed guide
    /// marks the alignment point on the path (not the old baseline), and
    /// follows the text as it slides along.
    #[test]
    fn the_guide_follows_bound_text() {
        let mut c = AppState::default();
        c.load(Document::new());
        c.set_tool(Tool::Line);
        for p in [Vec2::ZERO, Vec2::new(100.0, 0.0)] {
            c.tool_event(ToolEvent::Click(Pick::free(p)), None, Units::Mm);
        }
        c.set_tool(Tool::Text);
        c.tools.text_settings.content = "HELLO".to_owned();
        c.tools.text_settings.align = Align::Center;
        c.tool_event(
            ToolEvent::Click(Pick::free(Vec2::new(-50.0, 50.0))),
            None,
            Units::Mm,
        );
        c.close_text_dialog();
        c.set_tool(Tool::Select);
        let doc = c.document();
        let text = doc
            .entities()
            .iter()
            .find(|(_, e)| matches!(e.geom, Geom::Text { .. }))
            .map(|(id, _)| id)
            .unwrap();
        let (_, anchor, _) = doc.text(text).unwrap();
        let baseline = doc
            .entities()
            .iter()
            .find(|(_, e)| e.construction && e.geom.point_refs().contains(&anchor))
            .map(|(id, _)| id)
            .unwrap();
        let line = doc
            .entities()
            .iter()
            .find(|(id, e)| !e.construction && doc.line(*id).is_some())
            .map(|(id, _)| id)
            .unwrap();
        // Plain text: the baseline from the anchor is the guide.
        let shown = |c: &AppState, id: EntityId| c.doc.scene.items.iter().any(|i| i.id == id);
        assert!(shown(&c, baseline) && shown(&c, anchor));
        // The guide: the text's one construction line.
        let guide = |c: &AppState| {
            let g: Vec<_> = c
                .doc
                .scene
                .items
                .iter()
                .filter(|i| i.id == text && i.construction)
                .filter_map(|i| match i.shape {
                    crate::scene::Shape::Line(l) => Some(l),
                    _ => None,
                })
                .collect();
            assert_eq!(g.len(), 1, "one guide");
            g[0]
        };
        c.set_modify(Modify::TextOnPath);
        c.tools.path_text.text = Some(text);
        assert!(c.bind_text_to_path(line));
        assert!(!shown(&c, baseline) && !shown(&c, anchor), "parked");
        let g = guide(&c);
        assert!(
            (g.a.x - 50.0).abs() < 1e-9 && (g.b.x - 50.0).abs() < 1e-9,
            "centred on the line's middle: {g:?}"
        );
        assert!(g.a.y < 0.0 && g.b.y > 0.0, "across the line: {g:?}");
        // Slide 25 mm along: the guide moves with the text.
        assert!(c.begin_drag(text, Vec2::new(50.0, 1.0)));
        c.tools.dragging = false;
        let s = c.tools.slide.take().unwrap();
        let cmd = c
            .text_path_command(s.text, "Slide text along path", |t| {
                t.path.as_mut().unwrap().offset = 25.0;
            })
            .unwrap();
        assert!(c.apply_reporting(&cmd));
        let g = guide(&c);
        assert!((g.a.x - 75.0).abs() < 1e-9, "slid with the text: {g:?}");
        // Detach: the baseline is back where the text was placed.
        assert!(c.detach_text(text));
        assert!(shown(&c, baseline) && shown(&c, anchor));
    }

    /// Regression (#473): after a text went on a path, the Text tool
    /// placed nothing. In real egui frames: place a text with the Text
    /// tool, put it on the line with Text on path, then open the Text tool
    /// again and click: a second, free text; the first stays on its path.
    #[test]
    fn the_text_tool_places_again_after_text_on_path() {
        use egui::{Event, PointerButton as B, RawInput, pos2, vec2};
        let (mut c, _, _) = ready();
        let ctx = egui::Context::default();
        let settings = crate::settings::Settings::default();
        let mut t = 0.0;
        let mut frame = |c: &mut AppState, events: Vec<Event>| {
            let input = RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    pos2(0.0, 0.0),
                    vec2(800.0, 600.0),
                )),
                time: Some(t),
                events,
                ..RawInput::default()
            };
            t += 0.03;
            let mut status = crate::app::Status::default();
            let _ = ctx.run_ui(input, |ui| c.canvas_ui(ui, &settings, &mut status));
        };
        let btn = |pos, pressed| Event::PointerButton {
            pos,
            button: B::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let mut click = |c: &mut AppState, at: Vec2| {
            frame(c, Vec::new());
            let p = c.view.camera.to_screen(at, c.view.rect);
            frame(c, vec![Event::PointerMoved(p)]);
            frame(c, vec![btn(p, true)]);
            frame(c, vec![btn(p, false)]);
            frame(c, Vec::new());
        };
        let texts = |c: &AppState| {
            let doc = c.document();
            doc.entities()
                .iter()
                .filter(|(_, e)| matches!(e.geom, Geom::Text { .. }))
                .map(|(id, _)| id)
                .collect::<Vec<_>>()
        };
        // The scene on screen; clicks stay low and left, clear of the dialogs.
        c.view.camera.center = Vec2::new(0.0, 20.0);
        c.view.camera.pt_per_mm = 3.0;
        // Place a text with the Text tool, as the owner did.
        c.set_tool(Tool::Text);
        c.tools.text_settings.content = "FIRST".to_owned();
        click(&mut c, Vec2::new(-60.0, -10.0));
        let placed = texts(&c);
        assert_eq!(placed.len(), 2, "placed with the tool");
        let first = *placed.last().expect("the new text");
        // Text on path: click the text, then the line.
        c.set_modify(Modify::TextOnPath);
        let on = c
            .document()
            .text_geometry(first)
            .and_then(|g| g.bbox())
            .expect("glyphs");
        click(&mut c, (on.min + on.max) * 0.5);
        assert_eq!(c.tools.path_text.text, Some(first), "{:?}", c.doc.notice);
        click(&mut c, Vec2::new(20.0, 0.0));
        assert!(
            c.document().text(first).expect("text").0.path.is_some(),
            "{:?}",
            c.doc.notice
        );
        // The Text tool again: a click places a second, free text.
        c.set_tool(Tool::Text);
        c.tools.text_settings.content = "SECOND".to_owned();
        click(&mut c, Vec2::new(-60.0, -40.0));
        let now = texts(&c);
        assert_eq!(now.len(), 3, "a new text: {:?}", c.doc.notice);
        let second = *now.last().expect("the second text");
        assert!(c.document().text(second).expect("text").0.path.is_none());
        assert!(c.document().text(first).expect("text").0.path.is_some());
    }
}
