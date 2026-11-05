// SPDX-License-Identifier: AGPL-3.0-or-later
//! The sketch canvas: owns the document (behind the command API), its
//! render scene, the view and the selection, and handles pointer input.

use crate::app::Status;
use crate::constrain::{self, DimSpec, Kind};
use crate::hit::{self, BoxMode, Mods, Selection};
use crate::infer;
use crate::render::{Emphasis, Frame, Palette};
use crate::scene::Scene;
use crate::settings::{Settings, Units};
use crate::tools::{Draft, Outcome, Pick, Tool};
use crate::view::View;
use crate::{demo, grid};
use egui::{Color32, PointerButton, Pos2, Sense, Stroke};
use locus_doc::{Command, ConstraintId, DocError, Document, EntityId, Geom, LayerId, Sketcher};
use locus_geom::{Aabb, Vec2};
use std::collections::BTreeSet;

mod layers_panel;
mod regions;

/// Pick radius in screen points (converted to mm by the zoom).
const HIT_TOL_PT: f64 = 6.0;
/// A drag shorter than this is a click, not a box.
const MIN_BOX_PT: f32 = 4.0;

/// What an inline dimension editor will do on Enter (#70).
#[derive(Clone, Copy, Debug)]
enum DimTarget {
    New(DimSpec),
    Existing(ConstraintId),
}

/// A constraint the solver refused (SPEC §5 Solver UX): shown with the set it
/// clashes with highlighted, and — for a dimension — the offer to add it as a
/// driven (reference) dimension instead.
#[derive(Debug, Clone)]
struct Refusal {
    constraints: Vec<ConstraintId>,
    entities: Vec<EntityId>,
    redundant: bool,
    /// The same dimension, driven, if the refused command was one.
    as_driven: Option<Command>,
}

/// The inline value box shown over the canvas while a dimension is typed.
#[derive(Debug)]
struct DimEdit {
    target: DimTarget,
    what: &'static str,
    angle: bool,
    text: String,
    /// World position the box hangs from.
    at: Vec2,
    /// First frame: take focus and select the text.
    fresh: bool,
}

pub struct Canvas {
    /// Every document change goes through `Sketcher::apply` (SPEC §11): the
    /// command and the re-solve are one undo step.
    sketcher: Sketcher,
    scene: Scene,
    view: View,
    /// Keep the view fitted to the content, re-fitting when the canvas
    /// resizes, until the user pans or zooms (the web canvas is not at its
    /// final size on the first frame).
    auto_fit: bool,
    fit_size: egui::Vec2,
    drawn: usize,
    /// Selection and hover are UI state, never part of the document (§11).
    selection: Selection,
    hover: Option<EntityId>,
    /// Screen position where a rubber-band box started.
    rubber: Option<Pos2>,
    /// The active drawing tool and its clicks so far (#68).
    draft: Draft,
    /// Digits typed while drawing: a length or radius (SPEC §6.1).
    typed: String,
    /// Last cursor position over the canvas, snapped to a point if near one.
    cursor: Option<Pick>,
    /// Last refusal or error, shown in the status bar until the next action.
    notice: Option<String>,
    /// The inline dimension editor, while open (#70).
    dim_edit: Option<DimEdit>,
    /// The last constraint the solver refused, until dismissed (#72).
    refusal: Option<Refusal>,
    /// A point drag is in progress (#71): frames re-solve, release records.
    dragging: bool,
    /// Screen rectangles of the dimension labels drawn last frame.
    dim_rects: Vec<(ConstraintId, egui::Rect)>,
    /// The layer new geometry is drawn on (#118).
    active_layer: LayerId,
    layer_ui: layers_panel::LayerUi,
    /// Region selection, hover and the boolean combine (#122, #124).
    region_ui: regions::RegionUi,
}

impl Default for Canvas {
    fn default() -> Self {
        let mut c = Self {
            sketcher: Sketcher::new(Document::new()),
            scene: Scene::default(),
            view: View::default(),
            auto_fit: true,
            fit_size: egui::Vec2::ZERO,
            drawn: 0,
            selection: Selection::default(),
            hover: None,
            rubber: None,
            draft: Draft::new(Tool::Select),
            typed: String::new(),
            cursor: None,
            notice: None,
            dim_edit: None,
            refusal: None,
            dragging: false,
            dim_rects: Vec::new(),
            active_layer: Document::default_layer(),
            layer_ui: layers_panel::LayerUi::default(),
            region_ui: regions::RegionUi::default(),
        };
        c.load(demo::sketch());
        c
    }
}

impl Canvas {
    /// Replace the document (new / sample). Clears the undo history.
    pub fn load(&mut self, doc: Document) {
        self.sketcher = Sketcher::new(doc);
        self.layer_ui = layers_panel::LayerUi::default();
        self.region_ui = regions::RegionUi::default();
        self.active_layer = self.active_layer();
        self.draft.set_layer(self.active_layer);
        self.dragging = false;
        self.selection.clear();
        self.draft.cancel();
        self.typed.clear();
        self.notice = None;
        self.dim_edit = None;
        self.refusal = None;
        self.rebuild();
        self.request_fit();
    }

    pub fn tool(&self) -> Tool {
        self.draft.tool()
    }

    pub fn set_tool(&mut self, tool: Tool) {
        self.draft.set_tool(tool);
        self.typed.clear();
        self.notice = None;
    }

    /// What to tell the user: the last problem, else the tool's next step.
    pub fn hint(&self) -> String {
        match &self.notice {
            Some(n) => n.clone(),
            None => self.draft.tool().prompt(self.draft.clicks()).to_owned(),
        }
    }

    /// DOF and solve status for the status bar — the solver's own report
    /// (#72, SPEC §5 Solver UX).
    pub fn solve_summary(&self) -> String {
        let st = self.sketcher.state();
        match st.status {
            locus_doc::Status::FullyConstrained => "Fully constrained".to_owned(),
            locus_doc::Status::UnderConstrained => format!("Under-constrained · DOF {}", st.dof),
            _ => format!(
                "Over-constrained · {} conflicting, {} redundant",
                st.conflicts.len(),
                st.redundant.len()
            ),
        }
    }

    /// Constraints and entities to draw in the conflict colour: the solver's
    /// conflicting and redundant sets, plus whatever a refused constraint
    /// clashed with.
    fn flagged(&self) -> (BTreeSet<ConstraintId>, BTreeSet<EntityId>) {
        let st = self.sketcher.state();
        let mut cs = BTreeSet::new();
        let mut es = BTreeSet::new();
        for g in st.conflicts.iter().chain(&st.redundant) {
            cs.extend(g.constraints.iter().copied());
            es.extend(g.entities.iter().copied());
        }
        if let Some(r) = &self.refusal {
            cs.extend(r.constraints.iter().copied());
            es.extend(r.entities.iter().copied());
        }
        (cs, es)
    }

    fn selected(&self) -> Vec<EntityId> {
        self.selection.iter().collect()
    }

    /// Does the current selection support this constraint (#70)?
    pub fn can_constrain(&self, kind: Kind) -> bool {
        constrain::plan(kind, self.sketcher.document(), &self.selected()).is_ok()
    }

    /// Apply a geometric constraint to the selection (#70). A refusal (wrong
    /// selection, or the solver's conflict pre-check) is explained in the
    /// status bar.
    pub fn constrain(&mut self, kind: Kind) {
        match constrain::plan(kind, self.sketcher.document(), &self.selected()) {
            Ok(cmd) => self.apply_reporting(&cmd),
            Err(why) => self.notice = Some(why),
        }
    }

    pub fn can_dimension(&self) -> bool {
        constrain::infer_dimension(self.sketcher.document(), &self.selected()).is_ok()
    }

    /// The dimension tool: infer the dimension from the selection and open
    /// the inline value editor (#70).
    pub fn start_dimension(&mut self, units: Units) {
        match constrain::infer_dimension(self.sketcher.document(), &self.selected()) {
            Ok((spec, value)) => {
                self.dim_edit = Some(DimEdit {
                    target: DimTarget::New(spec),
                    what: spec.label(),
                    angle: spec.is_angle(),
                    text: format_value(value, spec.is_angle(), units),
                    at: constrain::dim_anchor(self.sketcher.document(), spec),
                    fresh: true,
                });
                self.notice = None;
            }
            Err(why) => self.notice = Some(why),
        }
    }

    /// Double-click on a dimension: edit its value in place.
    fn edit_existing_dimension(&mut self, id: ConstraintId, units: Units) {
        let doc = self.sketcher.document();
        let Some(c) = doc.constraint(id) else {
            return;
        };
        let Some(dim) = c.dimension() else {
            return;
        };
        if dim.driven {
            self.notice = Some("A driven dimension is measured; it can't be edited".to_owned());
            return;
        }
        let angle = matches!(c, locus_doc::Constraint::Angle { .. });
        let at = c
            .references()
            .first()
            .and_then(|&r| match doc.entity(r)?.geom {
                Geom::Point { pos } => Some(pos),
                Geom::Line { .. } => doc.line(r).map(|l| l.eval(0.5)),
                Geom::Circle { .. } => doc.circle(r).map(|c| c.center),
                Geom::Arc { .. } => doc.arc(r).map(|a| a.center),
            })
            .unwrap_or(Vec2::ZERO);
        self.dim_edit = Some(DimEdit {
            target: DimTarget::Existing(id),
            what: c.name(),
            angle,
            text: format_value(dim.value, angle, units),
            at,
            fresh: true,
        });
    }

    /// Run a command; on failure, explain in the status bar. A constraint the
    /// solver's pre-check refuses is kept as a [`Refusal`]: the conflicting
    /// set is highlighted and a dimension can be added as driven (#72).
    fn apply_reporting(&mut self, cmd: &Command) {
        match self.apply(cmd) {
            Ok(()) => {
                self.notice = None;
                self.refusal = None;
            }
            Err(e) => {
                self.notice = Some(e.to_string());
                self.refusal = match &e {
                    DocError::Conflict {
                        constraints,
                        entities,
                    }
                    | DocError::Redundant {
                        constraints,
                        entities,
                    } => Some(Refusal {
                        constraints: constraints.clone(),
                        entities: entities.clone(),
                        redundant: matches!(e, DocError::Redundant { .. }),
                        as_driven: match cmd {
                            Command::AddConstraint { constraint } => {
                                constrain::as_driven(constraint)
                                    .map(|constraint| Command::AddConstraint { constraint })
                            }
                            _ => None,
                        },
                    }),
                    _ => None,
                };
            }
        }
    }

    /// The refusal banner over the canvas: why, and the driven offer.
    fn refusal_banner(&mut self, ui: &egui::Ui, rect: egui::Rect) {
        let Some(r) = self.refusal.clone() else {
            return;
        };
        let what = if r.as_driven.is_some() {
            "dimension"
        } else {
            "constraint"
        };
        let why = if r.redundant {
            format!(
                "is already implied by the {} highlighted constraint(s)",
                r.constraints.len()
            )
        } else {
            format!(
                "conflicts with the {} highlighted constraint(s)",
                r.constraints.len()
            )
        };
        let (mut add, mut dismiss) = (false, false);
        egui::Area::new(egui::Id::new("refusal"))
            .order(egui::Order::Foreground)
            .fixed_pos(rect.left_top() + egui::vec2(12.0, 12.0))
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.colored_label(
                        Palette::new(ui.visuals().dark_mode).conflict,
                        format!("This {what} {why}, so it was not added."),
                    );
                    ui.horizontal(|ui| {
                        if r.as_driven.is_some()
                            && ui
                                .button("Add as driven")
                                .on_hover_text(
                                    "A driven (reference) dimension shows the measured value without constraining the sketch",
                                )
                                .clicked()
                        {
                            add = true;
                        }
                        dismiss = ui.button("Dismiss").clicked();
                    });
                });
            });
        if add && let Some(cmd) = r.as_driven {
            self.refusal = None;
            self.apply_reporting(&cmd);
        } else if dismiss {
            self.refusal = None;
        }
    }

    /// The constraints panel: every constraint, conflicting ones in red,
    /// with driven toggles and removal (#72).
    pub fn constraints_ui(&mut self, ui: &mut egui::Ui, settings: &Settings) {
        let (flagged, _) = self.flagged();
        let doc = self.sketcher.document();
        if doc.constraints().is_empty() {
            ui.weak("No constraints yet.");
            return;
        }
        let red = Palette::new(ui.visuals().dark_mode).conflict;
        let mut select: Option<Vec<EntityId>> = None;
        let mut action: Option<Command> = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            for (&id, c) in doc.constraints() {
                ui.horizontal(|ui| {
                    let text = constrain::describe(doc, c, settings.units);
                    let text = if flagged.contains(&id) {
                        egui::RichText::new(text).color(red)
                    } else {
                        egui::RichText::new(text)
                    };
                    if ui.selectable_label(false, text).clicked() {
                        select = Some(c.references());
                    }
                    if let Some(d) = c.dimension() {
                        let mut driven = d.driven;
                        if ui
                            .checkbox(&mut driven, "driven")
                            .on_hover_text("Measured instead of enforced")
                            .changed()
                        {
                            action = Some(Command::SetDriven { id, driven });
                        }
                    }
                    if ui.small_button("✕").on_hover_text("Remove").clicked() {
                        action = Some(Command::RemoveConstraint { id });
                    }
                });
            }
        });
        if let Some(ids) = select {
            self.selection.set_all(ids);
        }
        if let Some(cmd) = action {
            self.apply_reporting(&cmd);
        }
    }

    /// Enter in the inline editor: parse, convert units, apply.
    fn commit_dimension(&mut self, edit: &DimEdit, units: Units) {
        let Some(v) = parse_number(&edit.text) else {
            self.notice = Some(format!("\"{}\" is not a number", edit.text.trim()));
            return;
        };
        let value = if edit.angle {
            v.to_radians()
        } else if units == Units::Inch {
            v * 25.4
        } else {
            v
        };
        let cmd = match edit.target {
            DimTarget::New(spec) => Command::AddConstraint {
                constraint: spec.constraint(value, false),
            },
            DimTarget::Existing(id) => Command::SetDimension { id, value },
        };
        self.apply_reporting(&cmd);
    }

    /// The inline value box (#70).
    fn dimension_editor(&mut self, ui: &egui::Ui, rect: egui::Rect, units: Units) {
        let Some(edit) = self.dim_edit.as_mut() else {
            return;
        };
        let id = egui::Id::new("dim-edit-text");
        let at = self.view.to_screen(edit.at, rect) + egui::vec2(14.0, -34.0);
        let (mut commit, mut cancel) = (false, false);
        egui::Area::new(egui::Id::new("dim-edit"))
            .order(egui::Order::Foreground)
            .fixed_pos(at)
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(edit.what);
                        let out = egui::TextEdit::singleline(&mut edit.text)
                            .id(id)
                            .desired_width(84.0)
                            .show(ui);
                        if edit.fresh {
                            out.response.request_focus();
                            let mut state = out.state.clone();
                            let end = edit.text.chars().count();
                            state
                                .cursor
                                .set_char_range(Some(egui::text::CCursorRange::two(
                                    egui::text::CCursor::new(0),
                                    egui::text::CCursor::new(end),
                                )));
                            state.store(ui.ctx(), id);
                            edit.fresh = false;
                        } else if out.response.lost_focus() {
                            if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                                commit = true;
                            } else {
                                cancel = true;
                            }
                        }
                        ui.weak(if edit.angle { "°" } else { units.label() });
                    });
                });
            });
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            cancel = true;
        }
        if commit && let Some(edit) = self.dim_edit.take() {
            self.commit_dimension(&edit, units);
        } else if cancel {
            self.dim_edit = None;
        }
    }

    /// Single-key shortcuts: tools anywhere, constraints in Select mode (#68,
    /// #70). Not while typing a value or in a text field.
    fn hotkeys(&mut self, ui: &egui::Ui, units: Units) {
        use egui::Key;
        if self.dim_edit.is_some() || !self.typed.is_empty() || ui.ctx().egui_wants_keyboard_input()
        {
            return;
        }
        let pressed = |c: char| {
            ui.input(|i| {
                !(i.modifiers.command || i.modifiers.alt)
                    && Key::from_name(&c.to_string()).is_some_and(|k| i.key_pressed(k))
            })
        };
        if let Some(t) = [Tool::Select]
            .into_iter()
            .chain(Tool::DRAWING)
            .find(|t| t.key().is_some_and(pressed))
        {
            self.set_tool(t);
            return;
        }
        if self.draft.tool() != Tool::Select {
            return;
        }
        if pressed('D') {
            self.start_dimension(units);
        } else if let Some(k) = Kind::ALL.into_iter().find(|k| pressed(k.key())) {
            self.constrain(k);
        }
    }

    /// Run a tool outcome through the command API (SPEC §11).
    fn run(&mut self, outcome: Outcome) {
        match outcome {
            Outcome::Pending => {}
            Outcome::Rejected(why) => self.notice = Some(why.to_owned()),
            Outcome::Commit(cmd) => match self.apply(&cmd) {
                Ok(()) => self.notice = None,
                Err(e) => {
                    // The draft's predicted IDs are void after a failure.
                    self.draft.cancel();
                    self.notice = Some(e.to_string());
                }
            },
        }
    }

    /// The document as it stands (read-only; changes go through commands).
    pub fn document(&self) -> &Document {
        self.sketcher.document()
    }

    /// Run a command and refresh the derived scene.
    #[allow(dead_code)] // first caller: the sketch tools (#68)
    pub fn apply(&mut self, cmd: &Command) -> Result<(), DocError> {
        self.sketcher.apply(cmd)?;
        self.rebuild();
        Ok(())
    }

    /// Undo the last step (SPEC §11); returns its label.
    pub fn undo(&mut self) -> Option<String> {
        // A half-drawn shape may refer to what was just undone.
        self.draft.cancel();
        let label = self.sketcher.undo().ok()?;
        self.rebuild();
        Some(label)
    }

    pub fn redo(&mut self) -> Option<String> {
        self.draft.cancel();
        let label = self.sketcher.redo().ok()?;
        self.rebuild();
        Some(label)
    }

    pub fn undo_label(&self) -> Option<&str> {
        self.sketcher.editor().undo_label()
    }

    pub fn redo_label(&self) -> Option<&str> {
        self.sketcher.editor().redo_label()
    }

    fn rebuild(&mut self) {
        self.scene = Scene::build(
            self.sketcher.document(),
            &self.sketcher.state().fully_constrained,
        );
        let scene = &self.scene;
        self.selection
            .retain(|id| scene.items.iter().any(|i| i.id == id));
        self.hover = None;
        self.retain_regions();
    }

    /// World bounds of the content; a sheet-sized default while empty.
    fn bounds(&self) -> (Vec2, Vec2) {
        let (lo, hi) = self
            .scene
            .bounds
            .map_or((Vec2::new(0.0, 0.0), Vec2::new(300.0, 200.0)), |b| {
                (b.min, b.max)
            });
        // Room for dimension lines and glyphs, which sit outside the geometry.
        let pad = (hi - lo) * 0.08;
        (lo - pad, hi + pad)
    }

    pub fn request_fit(&mut self) {
        self.auto_fit = true;
        self.fit_size = egui::Vec2::ZERO;
    }

    pub fn select_all(&mut self) {
        self.selection
            .set_all(self.scene.items.iter().map(|i| i.id));
    }

    /// Select exactly these entities (the pre-export panel's highlight, #128).
    pub fn select_entities(&mut self, ids: &[locus_doc::EntityId]) {
        let present: Vec<_> = ids
            .iter()
            .copied()
            .filter(|id| self.scene.items.iter().any(|i| i.id == *id))
            .collect();
        self.selection.set_all(present);
    }

    pub fn clear_selection(&mut self) {
        self.selection.clear();
        self.region_ui.selected.clear();
    }

    pub fn summary(&self) -> String {
        let n = self.sketcher.document().entities().len();
        format!(
            "{n} entities · {} drawn · {} selected",
            self.drawn,
            self.selection.len()
        )
    }

    pub fn properties_ui(&mut self, ui: &mut egui::Ui, settings: &Settings) {
        if let Some(s) = self.region_summary() {
            ui.label(s);
        }
        let doc = self.sketcher.document();
        match self.selection.len() {
            0 if !self.region_ui.selected.is_empty() => {}
            0 => {
                ui.weak("Nothing selected.");
            }
            1 => {
                if let Some(id) = self.selection.iter().next() {
                    let rows = describe(doc, id, settings.units);
                    egui::Grid::new("props").num_columns(2).show(ui, |ui| {
                        for (k, v) in rows {
                            ui.label(k);
                            ui.monospace(v);
                            ui.end_row();
                        }
                    });
                }
            }
            n => {
                ui.label(format!("{n} entities selected"));
                let mut counts = [0usize; 4];
                for id in self.selection.iter() {
                    if let Some(e) = doc.entity(id) {
                        counts[e.geom.kind() as usize] += 1;
                    }
                }
                for (name, c) in ["Points", "Lines", "Circles", "Arcs"].iter().zip(counts) {
                    if c > 0 {
                        ui.label(format!("{name}: {c}"));
                    }
                }
            }
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, settings: &Settings, status: &mut Status) {
        let (resp, painter) = ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
        let rect = resp.rect;
        let dark = ui.visuals().dark_mode;
        let pal = Palette::new(dark);
        painter.rect_filled(rect, 0.0, pal.background);

        if self.auto_fit && rect.size() != self.fit_size && rect.width() > 10.0 {
            let (lo, hi) = self.bounds();
            self.view.fit(lo, hi, rect);
        }
        self.fit_size = rect.size();

        // Zoom about the cursor; middle/right-drag pans.
        let hover = resp.hover_pos();
        if let Some(hp) = hover {
            let (scroll, zoom) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
            let factor = zoom as f64 * (scroll as f64 * 0.0025).exp();
            if factor != 1.0 {
                self.view.zoom_about(hp, factor, rect);
                self.auto_fit = false;
            }
        }
        if resp.dragged_by(PointerButton::Middle) || resp.dragged_by(PointerButton::Secondary) {
            self.view.pan_points(resp.drag_delta());
            self.auto_fit = false;
        }

        if resp.hovered() {
            self.hotkeys(ui, settings.units);
        }
        if self.draft.tool() == Tool::Select {
            self.pick_and_select(ui, &resp, rect, settings.units);
        } else {
            self.tool_input(ui, &resp, rect, settings);
        }

        let painter = painter.with_clip_rect(rect);
        grid::draw(&painter, &self.view, rect, dark);
        let frame = Frame {
            painter: &painter,
            view: &self.view,
            rect,
            pal,
            units: settings.units,
        };
        let (selection, hovered) = (&self.selection, self.hover);
        let (flag_constraints, flag_entities) = self.flagged();
        self.draw_regions(&painter, rect, dark);
        self.drawn = frame.draw_items(&self.scene, |item| {
            if flag_entities.contains(&item.id) {
                Emphasis::Conflict
            } else if selection.contains(item.id) {
                Emphasis::Selected
            } else if hovered == Some(item.id) {
                Emphasis::Hover
            } else {
                Emphasis::None
            }
        });
        self.dim_rects = frame.draw_annotations(&self.scene, &flag_constraints);
        self.draw_tool_overlay(&frame, &painter);
        if let (Some(start), Some(now)) = (self.rubber, resp.interact_pointer_pos()) {
            draw_rubber_band(&painter, start, now);
        }
        grid::rulers(&painter, &self.view, rect);
        self.dimension_editor(ui, rect, settings.units);
        self.refusal_banner(ui, rect);

        status.cursor = hover.map(|p| self.view.to_world(p, rect));
        status.extent_mm = self.view.extent_mm(rect);
        status.grid = grid::label(grid::minor_step(self.view.pt_per_mm).0);
    }

    /// What the cursor designates for the active tool: snapped onto a point,
    /// or with inferred constraints (#68, #69). Holding Alt suppresses
    /// inference for the moment, like the global toggle.
    fn designate(&self, world: Vec2, auto: bool) -> Pick {
        let opts = infer::Options {
            enabled: auto,
            tol: HIT_TOL_PT / self.view.pt_per_mm,
        };
        let anchor = self
            .draft
            .line_anchor()
            .map(|(pos, point)| infer::Anchor { pos, point });
        infer::infer(
            &self.scene.items,
            opts,
            self.draft.tool(),
            anchor,
            self.draft.curve_snap(),
            world,
        )
    }

    /// Clicks, typed values and keys while a drawing tool is active (#68).
    fn tool_input(
        &mut self,
        ui: &egui::Ui,
        resp: &egui::Response,
        rect: egui::Rect,
        settings: &Settings,
    ) {
        self.hover = None;
        let auto = settings.auto_constrain && !ui.input(|i| i.modifiers.alt);
        self.cursor = resp
            .hover_pos()
            .map(|p| self.designate(self.view.to_world(p, rect), auto));
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        }
        if resp.hovered() || !self.typed.is_empty() {
            self.tool_keys(ui, settings.units);
        }
        if resp.clicked_by(PointerButton::Primary)
            && let Some(pick) = self.cursor
        {
            self.draft.set_layer(self.active_layer());
            let out = self.draft.click(pick, self.sketcher.document().next_id());
            self.typed.clear();
            self.run(out);
        }
    }

    fn tool_keys(&mut self, ui: &egui::Ui, units: Units) {
        use egui::Key;
        let (text, enter, esc, back) = ui.input(|i| {
            let text: String = i
                .events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::Text(t) => Some(t.as_str()),
                    _ => None,
                })
                .collect();
            (
                text,
                i.key_pressed(Key::Enter),
                i.key_pressed(Key::Escape),
                i.key_pressed(Key::Backspace),
            )
        });
        if self.draft.is_drawing() {
            self.typed
                .extend(text.chars().filter(|c| c.is_ascii_digit() || *c == '.'));
        }
        if back {
            self.typed.pop();
        }
        if enter {
            let value = std::mem::take(&mut self.typed);
            if value.is_empty() {
                self.draft.finish();
            } else if let Ok(v) = value.parse::<f64>() {
                let mm = match units {
                    Units::Mm => v,
                    Units::Inch => v * 25.4,
                };
                let at = self.cursor.unwrap_or_else(|| Pick::free(Vec2::ZERO));
                self.draft.set_layer(self.active_layer());
                let out = self.draft.typed(mm, at, self.sketcher.document().next_id());
                self.run(out);
            } else {
                self.notice = Some(format!("\"{value}\" is not a number"));
            }
        }
        if esc {
            if !self.typed.is_empty() {
                self.typed.clear();
            } else if self.draft.is_drawing() {
                self.draft.cancel();
            } else {
                self.set_tool(Tool::Select);
            }
        }
    }

    fn draw_tool_overlay(&self, frame: &Frame<'_>, painter: &egui::Painter) {
        if self.draft.tool() == Tool::Select {
            return;
        }
        let Some(pick) = self.cursor else {
            return;
        };
        frame.draw_preview(&self.draft.preview(pick.pos));
        if pick.point.is_some() {
            frame.draw_snap(pick.pos);
        }
        // Inference hint glyph (#69): what the shape will be constrained to.
        let badge = match (pick.line, pick.on_curve) {
            (Some(h), _) => Some(h.badge()),
            (None, Some(_)) => Some("o"),
            (None, None) => pick.point.map(|_| "•"),
        };
        if let Some(b) = badge {
            frame.draw_hint(pick.pos, b);
        }
        let label = if self.typed.is_empty() {
            self.draft
                .readout(pick.pos)
                .map(|(what, mm)| format!("{what} {}", frame.units.format_len(mm)))
        } else {
            let what = self.draft.readout(pick.pos).map_or("Value", |(w, _)| w);
            Some(format!("{what} {}▏", self.typed))
        };
        if let Some(text) = label {
            let at = frame.view.to_screen(pick.pos, frame.rect) + egui::vec2(14.0, -14.0);
            // Keep the label on the canvas near the right edge.
            let anchor = if at.x > frame.rect.right() - 170.0 {
                egui::Align2::RIGHT_BOTTOM
            } else {
                egui::Align2::LEFT_BOTTOM
            };
            painter.text(
                at - egui::vec2(
                    if anchor == egui::Align2::RIGHT_BOTTOM {
                        28.0
                    } else {
                        0.0
                    },
                    0.0,
                ),
                anchor,
                text,
                egui::FontId::monospace(13.0),
                frame.pal.hover,
            );
        }
    }

    /// Start dragging what was grabbed (#71). A point moves itself; a line
    /// moves by its end nearer the cursor; a circle or arc by its centre.
    fn begin_drag(&mut self, grabbed: EntityId, at: Vec2) -> bool {
        let doc = self.sketcher.document();
        let near = |a: EntityId, b: EntityId| {
            let d = |p: EntityId| doc.point_pos(p).map_or(f64::MAX, |q| q.distance(at));
            if d(a) <= d(b) { a } else { b }
        };
        let point = match doc.entity(grabbed).map(|e| &e.geom) {
            Some(Geom::Point { .. }) => grabbed,
            Some(Geom::Line { start, end }) => near(*start, *end),
            Some(Geom::Circle { center, .. } | Geom::Arc { center, .. }) => *center,
            None => return false,
        };
        match self.sketcher.begin_drag(point) {
            Ok(()) => {
                self.dragging = true;
                self.notice = None;
                true
            }
            Err(e) => {
                self.notice = Some(e.to_string());
                false
            }
        }
    }

    /// One frame of a drag: re-solve towards the pointer; release records
    /// one undo step; Esc puts everything back (#71).
    fn drag_frame(&mut self, ui: &egui::Ui, resp: &egui::Response, rect: egui::Rect) {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.sketcher.cancel_drag();
            self.dragging = false;
            self.rebuild();
            return;
        }
        if resp.dragged_by(PointerButton::Primary)
            && let Some(p) = resp.interact_pointer_pos()
        {
            let to = self.view.to_world(p, rect);
            match self.sketcher.drag_to(to) {
                Ok(f) if f.converged => {
                    // Keep the selection: rebuild() prunes it by id, and a
                    // drag never removes entities.
                    self.rebuild();
                }
                Ok(_) => {}
                Err(e) => {
                    self.sketcher.cancel_drag();
                    self.dragging = false;
                    self.notice = Some(e.to_string());
                    self.rebuild();
                    return;
                }
            }
        }
        if resp.drag_stopped_by(PointerButton::Primary) || !resp.dragged() {
            self.dragging = false;
            if let Err(e) = self.sketcher.end_drag() {
                self.notice = Some(e.to_string());
            }
            self.rebuild();
        }
    }

    /// Hover, click and box selection (#67). Left-drag is a rubber-band box;
    /// pan stays on middle/right-drag.
    fn pick_and_select(
        &mut self,
        ui: &egui::Ui,
        resp: &egui::Response,
        rect: egui::Rect,
        units: Units,
    ) {
        let mods = ui.input(|i| Mods {
            add: i.modifiers.shift,
            toggle: i.modifiers.command,
        });
        let tol = HIT_TOL_PT / self.view.pt_per_mm;
        let pick_at = |this: &Self, p: Pos2| {
            let w = this.view.to_world(p, rect);
            hit::pick(&this.scene.items, w, tol)
        };

        if self.dragging {
            self.drag_frame(ui, resp, rect);
            return;
        }
        if resp.drag_started_by(PointerButton::Primary) {
            // The press, not where the drag threshold was crossed.
            let origin = ui.input(|i| i.pointer.press_origin());
            match origin.and_then(|o| pick_at(self, o).map(|id| (o, id))) {
                Some((o, id)) if self.begin_drag(id, self.view.to_world(o, rect)) => {
                    self.selection.set_all([id]);
                }
                _ => self.rubber = resp.interact_pointer_pos(),
            }
        }
        let boxing = self.rubber.is_some();
        self.hover = match resp.hover_pos() {
            Some(p) if !boxing => pick_at(self, p),
            _ => None,
        };
        // Inside a region but off its curves: shade the region (#122).
        self.region_ui.hover = match resp.hover_pos() {
            Some(p) if !boxing && self.hover.is_none() => {
                self.region_at(self.view.to_world(p, rect))
            }
            _ => None,
        };
        if self.hover.is_some() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }

        if resp.double_clicked_by(PointerButton::Primary)
            && let Some(p) = resp.interact_pointer_pos()
            && let Some(&(id, _)) = self.dim_rects.iter().find(|(_, r)| r.contains(p))
        {
            self.edit_existing_dimension(id, units);
        } else if resp.clicked_by(PointerButton::Primary)
            && let Some(p) = resp.interact_pointer_pos()
        {
            let hit = pick_at(self, p);
            let region = self.region_at(self.view.to_world(p, rect));
            let plain = !(mods.add || mods.toggle);
            if self.region_ui.combine.is_some() {
                // Picking tool regions for a boolean (#124).
                if hit.is_none() {
                    self.combine_click(region);
                }
            } else if hit.is_some() || region.is_none() {
                self.selection.click(hit, mods);
                if plain {
                    self.region_ui.selected.clear();
                }
            } else {
                // Regions select alongside entities, by the same rules.
                self.region_ui.click(region.as_ref(), mods);
                if plain {
                    self.selection.clear();
                }
            }
        }
        if resp.drag_stopped_by(PointerButton::Primary)
            && let Some(start) = self.rubber.take()
            && let Some(end) = resp.interact_pointer_pos()
            && (end - start).length() >= MIN_BOX_PT
        {
            let a = self.view.to_world(start, rect);
            let b = self.view.to_world(end, rect);
            let mode = BoxMode::from_drag(start.x, end.x);
            let hits = hit::box_select(&self.scene.items, Aabb::from_points(a, b), mode);
            self.selection.apply_box(&hits, mods);
        }
        if !resp.dragged_by(PointerButton::Primary) {
            self.rubber = None;
        }

        if resp.hovered() {
            let (esc, all) = ui.input(|i| {
                (
                    i.key_pressed(egui::Key::Escape),
                    i.modifiers.command && i.key_pressed(egui::Key::A),
                )
            });
            let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
            if esc && self.region_ui.combine.is_some() {
                self.cancel_combine();
            } else if esc {
                self.refusal = None;
                self.clear_selection();
            }
            if enter && self.region_ui.combine.is_some() {
                self.finish_combine();
            }
            if all {
                self.select_all();
            }
        }
    }
}

/// The number shown in the dimension editor: mm or inches, degrees for angles.
fn format_value(value: f64, angle: bool, units: Units) -> String {
    let v = if angle {
        value.to_degrees()
    } else if units == Units::Inch {
        value / 25.4
    } else {
        value
    };
    let s = format!("{v:.4}");
    // Trim trailing zeros: "20.0000" → "20".
    let s = s.trim_end_matches('0').trim_end_matches('.');
    s.to_owned()
}

/// A plain decimal, optionally followed by a unit suffix the box shows.
fn parse_number(text: &str) -> Option<f64> {
    let t = text
        .trim()
        .trim_end_matches(|c: char| c.is_alphabetic() || c == '°' || c == '"');
    t.trim().parse::<f64>().ok().filter(|v| v.is_finite())
}

/// Window boxes are solid blue, crossing boxes dashed green (Fusion style).
fn draw_rubber_band(painter: &egui::Painter, start: Pos2, now: Pos2) {
    let r = egui::Rect::from_two_pos(start, now);
    let window = BoxMode::from_drag(start.x, now.x) == BoxMode::Window;
    let c = if window {
        Color32::from_rgb(70, 140, 240)
    } else {
        Color32::from_rgb(70, 190, 110)
    };
    painter.rect_filled(r, 0.0, c.gamma_multiply(0.15));
    let stroke = Stroke::new(1.0, c);
    if window {
        painter.rect_stroke(r, 0.0, stroke, egui::StrokeKind::Inside);
    } else {
        let path = [
            r.left_top(),
            r.right_top(),
            r.right_bottom(),
            r.left_bottom(),
            r.left_top(),
        ];
        painter.extend(egui::Shape::dashed_line(&path, stroke, 6.0, 4.0));
    }
}

/// Label/value rows for the properties panel.
fn describe(doc: &Document, id: EntityId, units: Units) -> Vec<(String, String)> {
    let Some(e) = doc.entity(id) else {
        return Vec::new();
    };
    let pt = |p: Vec2| format!("{}, {}", units.format_len(p.x), units.format_len(p.y));
    let kind = match e.geom {
        Geom::Point { .. } => "Point",
        Geom::Line { .. } => "Line",
        Geom::Circle { .. } => "Circle",
        Geom::Arc { .. } => "Arc",
    };
    let mut rows = vec![
        ("ID".to_owned(), id.to_string()),
        ("Type".to_owned(), kind.to_owned()),
    ];
    if let Some(l) = doc.layer(e.layer) {
        rows.push(("Layer".to_owned(), l.name.clone()));
    }
    rows.push((
        "Construction".to_owned(),
        if e.construction { "yes" } else { "no" }.to_owned(),
    ));
    match e.geom {
        Geom::Point { pos } => rows.push(("Position".to_owned(), pt(pos))),
        Geom::Line { .. } => {
            if let Some(l) = doc.line(id) {
                rows.push(("Start".to_owned(), pt(l.a)));
                rows.push(("End".to_owned(), pt(l.b)));
                rows.push(("Length".to_owned(), units.format_len(l.length())));
                let ang = (l.b - l.a).angle().to_degrees();
                rows.push(("Angle".to_owned(), format!("{ang:.2}°")));
            }
        }
        Geom::Circle { .. } => {
            if let Some(c) = doc.circle(id) {
                rows.push(("Centre".to_owned(), pt(c.center)));
                rows.push(("Radius".to_owned(), units.format_len(c.radius)));
                rows.push(("Diameter".to_owned(), units.format_len(c.radius * 2.0)));
            }
        }
        Geom::Arc { .. } => {
            if let Some(a) = doc.arc(id) {
                rows.push(("Centre".to_owned(), pt(a.center)));
                rows.push(("Radius".to_owned(), units.format_len(a.radius())));
                rows.push((
                    "Sweep".to_owned(),
                    format!("{:.2}°", a.sweep().to_degrees()),
                ));
            }
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describe_reports_a_line_and_a_circle() {
        let doc = demo::sketch();
        let line = doc
            .entities()
            .iter()
            .find(|(_, e)| matches!(e.geom, Geom::Line { .. }))
            .map(|(id, _)| id);
        let rows = line
            .map(|id| describe(&doc, id, Units::Mm))
            .unwrap_or_default();
        assert!(rows.iter().any(|(k, _)| k == "Length"));
        let circle = doc
            .entities()
            .iter()
            .find(|(_, e)| matches!(e.geom, Geom::Circle { .. }))
            .map(|(id, _)| id);
        let rows = circle
            .map(|id| describe(&doc, id, Units::Mm))
            .unwrap_or_default();
        assert!(
            rows.iter()
                .any(|(k, v)| k == "Diameter" && v == "40.000 mm")
        );
    }

    /// Run the Layers panel headlessly for a few frames.
    fn frames(c: &mut Canvas, n: usize) {
        let ctx = egui::Context::default();
        let settings = Settings::default();
        for _ in 0..n {
            let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
                c.layers_ui(ui, &settings);
            });
        }
    }

    #[test]
    #[allow(clippy::unwrap_used)]
    fn layers_panel_renders_and_every_layer_action_is_one_undo_step() {
        let mut c = Canvas::default();
        frames(&mut c, 2);
        let l1 = c.active_layer();
        let cmd = Command::AddLayer {
            name: "Cut 2".to_owned(),
            colour: [1, 2, 3],
            operation: locus_doc::OperationKind::Draw.default_operation(),
        };
        c.apply_reporting(&cmd);
        let l2 = *c.document().layers().keys().next_back().unwrap();
        c.layer_ui.expanded_for_test(l2);
        frames(&mut c, 2);
        // Lock → delete dialog paths run without panicking.
        c.apply_reporting(&Command::SetLayerLocked {
            id: l1,
            locked: true,
        });
        assert!(c.document().layer(l1).unwrap().locked);
        assert_eq!(c.undo().as_deref(), Some("Lock layer"));
        let plan = c.document().layer_deletion(l2).unwrap();
        c.layer_ui.dialog_for_test(l2, plan);
        frames(&mut c, 2);
        c.apply_reporting(&Command::DeleteLayer { id: l2 });
        frames(&mut c, 1);
        assert!(c.document().layer(l2).is_none());
        // The active layer survives its own deletion by falling back.
        c.active_layer = l2;
        assert_eq!(c.active_layer(), l1);
    }

    #[test]
    fn undo_restores_the_document_exactly_and_redo_reapplies() {
        let mut c = Canvas::default();
        let before = c.sketcher.document().clone();
        let cmd = Command::AddEntity {
            layer: Document::default_layer(),
            construction: false,
            geom: Geom::Point {
                pos: Vec2::new(1.0, 2.0),
            },
        };
        assert!(c.apply(&cmd).is_ok());
        let after = c.sketcher.document().clone();
        assert_ne!(after, before);
        assert_eq!(c.undo_label(), Some("Add entity"));
        assert_eq!(c.undo().as_deref(), Some("Add entity"));
        // IDs are never reused, so only the contents are compared.
        let same = |a: &Document, b: &Document| {
            a.entities() == b.entities() && a.constraints() == b.constraints()
        };
        assert!(same(c.sketcher.document(), &before));
        assert_eq!(c.redo_label(), Some("Add entity"));
        c.redo();
        assert!(same(c.sketcher.document(), &after));
        assert_eq!(c.scene.items.len(), after.entities().len());
    }

    #[test]
    fn dimension_text_round_trips_in_each_unit() {
        assert_eq!(format_value(20.0, false, Units::Mm), "20");
        assert_eq!(format_value(25.4, false, Units::Inch), "1");
        assert_eq!(
            format_value(std::f64::consts::FRAC_PI_2, true, Units::Mm),
            "90"
        );
        assert_eq!(parse_number(" 12.5 "), Some(12.5));
        assert_eq!(parse_number("90°"), Some(90.0));
        assert_eq!(parse_number("3 mm"), Some(3.0));
        assert_eq!(parse_number("abc"), None);
        assert_eq!(parse_number("NaN"), None);
    }

    #[test]
    fn constraining_through_the_canvas_is_one_undo_step_and_refusals_explain() {
        let mut c = Canvas::default();
        // Nothing selected: refused with what is needed.
        c.constrain(Kind::Parallel);
        assert!(c.notice.as_deref().is_some_and(|n| n.contains("two lines")));
        let lines: Vec<_> = c
            .sketcher
            .document()
            .entities()
            .iter()
            .filter(|(_, e)| matches!(e.geom, Geom::Line { .. }) && !e.construction)
            .map(|(id, _)| id)
            .collect();
        c.selection.set_all([lines[0]]);
        assert!(c.can_dimension());
        let before = c.sketcher.document().constraints().len();
        // The demo's frame is fully constrained, so this is already implied.
        c.selection.set_all([lines[1], lines[3]]);
        c.constrain(Kind::Equal);
        assert!(c.notice.as_deref().is_some_and(|n| n.contains("implied")));
        assert_eq!(c.sketcher.document().constraints().len(), before);
        // The loose point can be fixed: one undo step.
        let free = c
            .scene
            .items
            .iter()
            .find(|i| matches!(i.shape, crate::scene::Shape::Point(_)) && !i.owned_point)
            .map(|i| i.id);
        c.selection.set_all(free);
        c.constrain(Kind::Fix);
        assert_eq!(c.sketcher.document().constraints().len(), before + 1);
        assert_eq!(c.undo_label(), Some("Add fix constraint"));
        c.undo();
        assert_eq!(c.sketcher.document().constraints().len(), before);
    }

    #[test]
    fn grabbing_a_line_drags_its_nearer_end_and_undo_restores_the_geometry() {
        let mut c = Canvas::default();
        let line = c
            .sketcher
            .document()
            .entities()
            .iter()
            .find(|(_, e)| matches!(e.geom, Geom::Line { .. }) && e.construction)
            .map(|(id, _)| id)
            .unwrap_or(EntityId::new(0));
        let before = c.sketcher.document().entities().clone();
        // The construction diagonal runs (20, 20) → (220, 140): grab near its end.
        assert!(c.begin_drag(line, Vec2::new(215.0, 138.0)));
        assert!(c.dragging);
        let moved = c.sketcher.drag_to(Vec2::new(200.0, 100.0));
        assert!(moved.is_ok());
        assert!(c.sketcher.end_drag().is_ok());
        c.dragging = false;
        c.rebuild();
        assert_ne!(c.sketcher.document().entities(), &before);
        assert_eq!(c.undo_label(), Some("Drag"));
        c.undo();
        assert_eq!(c.sketcher.document().entities(), &before);
    }

    #[test]
    fn a_refused_dimension_is_highlighted_and_can_be_added_as_driven() {
        let mut c = Canvas::default();
        let circle = c
            .sketcher
            .document()
            .entities()
            .iter()
            .find(|(_, e)| matches!(e.geom, Geom::Circle { .. }))
            .map(|(id, _)| id)
            .unwrap_or(EntityId::new(0));
        let before = c.sketcher.document().constraints().len();
        // The demo's hole already has a diameter dimension.
        let spec = DimSpec::Diameter(circle);
        c.apply_reporting(&Command::AddConstraint {
            constraint: spec.constraint(30.0, false),
        });
        assert_eq!(c.sketcher.document().constraints().len(), before);
        let Some(r) = c.refusal.clone() else {
            panic!("the conflicting dimension should have been refused");
        };
        assert!(!r.constraints.is_empty() && r.as_driven.is_some());
        let (cs, es) = c.flagged();
        assert!(r.constraints.iter().all(|id| cs.contains(id)));
        assert!(r.entities.iter().all(|id| es.contains(id)));
        // Accept the offer: it lands as a driven dimension showing the
        // measured value, and the sketch is untouched.
        let cmd = r.as_driven.clone().unwrap_or(Command::Compound {
            label: String::new(),
            commands: Vec::new(),
        });
        c.apply_reporting(&cmd);
        assert!(c.refusal.is_none());
        assert_eq!(c.sketcher.document().constraints().len(), before + 1);
        let driven = c
            .sketcher
            .document()
            .constraints()
            .values()
            .filter_map(|k| k.dimension())
            .filter(|d| d.driven)
            .collect::<Vec<_>>();
        assert_eq!(driven.len(), 1);
        assert!((driven[0].value - 40.0).abs() < 1e-6, "measured, not 30");
    }

    #[test]
    fn status_text_follows_the_solver_report() {
        let c = Canvas::default();
        let st = c.sketcher.state();
        assert_eq!(
            c.solve_summary(),
            format!("Under-constrained · DOF {}", st.dof)
        );
    }

    #[test]
    fn selection_does_not_touch_the_document() {
        let mut c = Canvas::default();
        let before = c.sketcher.document().clone();
        c.select_all();
        assert!(c.selection.len() > 1);
        c.clear_selection();
        assert_eq!(c.sketcher.document(), &before);
    }
}
