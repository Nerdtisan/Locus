// SPDX-License-Identifier: AGPL-3.0-or-later
//! The sketch canvas: owns the document (behind the command API), its
//! render scene, the view and the selection, and handles pointer input.

use crate::app::Status;
use crate::constrain::{self, DimSpec, Kind};
use crate::hit::{self, BoxMode, Mods, Selection};
use crate::infer;
use crate::modify_tool::{CornerSettings, Modify, TransformSettings};
use crate::render::{Emphasis, Frame, Palette};
use crate::scene::Scene;
use crate::settings::{Settings, Units};
use crate::tools::{Draft, Outcome, Pick, Tool};
use crate::view::View;
use crate::{demo, grid};
use egui::{Color32, PointerButton, Pos2, Sense, Stroke};
use locus_doc::{
    Command, ConstraintId, DocError, Document, Edge, EntityId, Geom, LayerId, Sketcher,
};
use locus_geom::{Aabb, Vec2};
use std::collections::BTreeSet;
use text_ui::{FontCatalog, TextSettings};

mod bed;
mod layers_panel;
mod machine;
mod offsets;
mod params_panel;
mod pattern_labels;
mod patterns_panel;
pub mod plugin_objects;
mod regions;
mod text_ui;

/// Pick radius in screen points (converted to mm by the zoom).
const HIT_TOL_PT: f64 = 6.0;
/// A drag shorter than this is a click, not a box.
const MIN_BOX_PT: f32 = 4.0;

/// What an inline dimension editor will do on Enter (#70).
#[derive(Clone, Copy, Debug)]
enum DimTarget {
    New(DimSpec),
    Existing(ConstraintId),
    /// A live offset's distance (#164).
    Offset(locus_doc::RecipeId),
    /// A pattern's count and spacing / angle (#168, #169).
    Pattern(locus_doc::RecipeId),
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
    /// Banner text for a refusal that isn't a new constraint (a transform).
    message: Option<String>,
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
    /// Re-runs modifier plugins (#184); kept across document loads.
    plugin_runner: Option<std::sync::Arc<dyn locus_doc::generated::PluginRunner>>,
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
    /// Béziers whose handles are shown: selected, or with a selected point (#160).
    handles: BTreeSet<EntityId>,
    /// Pen tool: where the button went down and where the pointer is now
    /// while dragging out a handle (#160).
    pen: Option<(Pick, Vec2)>,
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
    param_ui: params_panel::ParamUi,
    pattern_ui: patterns_panel::PatternUi,
    /// Region selection, hover and the boolean combine (#122, #124).
    region_ui: regions::RegionUi,
    /// Machine preview on, and its polylines (#176).
    machine_on: bool,
    /// Draw pen width / kerf at true scale under the sketch (#240).
    stroke_on: bool,
    machine: Vec<machine::MachinePath>,
    /// The active modify tool, if any (SPEC §6.2): clicks go to it.
    modify: Option<Modify>,
    /// The offset picked with the Offset tool, and the screen rectangles of
    /// the offset distance labels drawn last frame (#164).
    offset_pick: Option<locus_doc::RecipeId>,
    offset_rects: Vec<(locus_doc::RecipeId, egui::Rect)>,
    /// The first curve picked for a fillet or chamfer.
    corner_first: Option<EntityId>,
    /// Fillet radius and chamfer sizes, edited in the tool strip.
    pub corner: CornerSettings,
    /// Move/rotate/scale values, edited in the tool strip.
    pub transform: TransformSettings,
    /// Settings for the next text the Text tool places, and the fonts it can use (#180).
    text_settings: TextSettings,
    fonts: FontCatalog,
    /// The selected text being edited: its id and the not-yet-committed settings.
    text_edit: Option<(EntityId, TextSettings)>,
}

impl Default for Canvas {
    fn default() -> Self {
        let mut c = Self {
            sketcher: Sketcher::new(Document::new()),
            plugin_runner: None,
            scene: Scene::default(),
            view: View::default(),
            auto_fit: true,
            fit_size: egui::Vec2::ZERO,
            drawn: 0,
            selection: Selection::default(),
            hover: None,
            rubber: None,
            handles: BTreeSet::new(),
            pen: None,
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
            param_ui: params_panel::ParamUi::default(),
            pattern_ui: patterns_panel::PatternUi::default(),
            region_ui: regions::RegionUi::default(),
            machine_on: false,
            stroke_on: false,
            machine: Vec::new(),
            modify: None,
            offset_pick: None,
            offset_rects: Vec::new(),
            corner_first: None,
            corner: CornerSettings::default(),
            transform: TransformSettings::default(),
            text_settings: TextSettings::default(),
            fonts: FontCatalog::headless(),
            text_edit: None,
        };
        c.load(demo::sketch());
        c
    }
}

impl Canvas {
    /// Replace the document (new / sample). Clears the undo history.
    pub fn load(&mut self, doc: Document) {
        self.sketcher = Sketcher::new(doc);
        self.sketcher.set_plugin_runner(self.plugin_runner.clone());
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

    /// Sides and kind of the polygon tool (#157).
    pub fn polygon(&self) -> (usize, bool) {
        self.draft.polygon()
    }

    pub fn set_polygon(&mut self, sides: usize, circumscribed: bool) {
        self.draft.set_polygon(sides, circumscribed);
    }

    pub fn set_tool(&mut self, tool: Tool) {
        self.draft.set_tool(tool);
        self.typed.clear();
        self.notice = None;
        self.modify = None;
        self.corner_first = None;
    }

    /// The active modify tool (SPEC §6.2), if any.
    pub fn modify(&self) -> Option<Modify> {
        self.modify
    }

    pub fn set_modify(&mut self, m: Modify) {
        self.set_tool(Tool::Select);
        // Move/rotate/scale works on the selection; the others pick curves.
        if m != Modify::Transform {
            self.selection.clear();
        }
        self.modify = Some(m);
    }

    /// True when the selection has something to move, rotate or scale.
    pub fn can_transform(&self) -> bool {
        !self.selection.is_empty()
    }

    /// Move, rotate or scale the selection by the typed values, about the
    /// centre of its bounding box, as one undo step (SPEC §6.2). When
    /// constraints forbid it, they are named and highlighted instead.
    pub fn apply_transform(&mut self, units: Units) {
        let sel: Vec<EntityId> = self.selection.iter().collect();
        let center = self
            .scene
            .items
            .iter()
            .filter(|i| self.selection.contains(i.id))
            .map(|i| i.bbox)
            .reduce(Aabb::union)
            .map_or(Vec2::ZERO, |b| b.min.lerp(b.max, 0.5));
        let t = self.transform.transform(center);
        let doc = self.sketcher.document();
        match locus_doc::transform::transform(doc, &sel, t, self.transform.copy) {
            Ok(plan) => {
                self.apply_reporting(&plan.command);
            }
            Err(locus_doc::modify::ModifyError::Blocked(cs)) => {
                let cs_len = cs.len();
                let names: Vec<String> = cs
                    .iter()
                    .filter_map(|c| doc.constraint(*c))
                    .map(|c| constrain::describe(doc, c, units))
                    .collect();
                self.notice = Some(if names.is_empty() {
                    "Constraints hold the selection in place".to_owned()
                } else {
                    format!("Held in place by: {}", names.join("; "))
                });
                self.refusal = Some(Refusal {
                    constraints: cs,
                    entities: sel,
                    redundant: false,
                    as_driven: None,
                    message: Some(format!(
                        "The {} constraint(s) highlighted hold the selection, so it was not moved.",
                        cs_len
                    )),
                });
            }
            Err(e) => self.notice = Some(e.to_string()),
        }
    }

    /// What to tell the user: the last problem, else the tool's next step.
    pub fn hint(&self) -> String {
        match (&self.notice, self.modify) {
            (Some(n), _) => n.clone(),
            (None, Some(m)) => m.prompt(self.corner_first.is_some()).to_owned(),
            (None, None) => self.draft.tool().prompt(self.draft.clicks()).to_owned(),
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
            Ok(cmd) => {
                self.apply_reporting(&cmd);
            }
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
                Geom::Bezier { .. } => doc.cubic(r).map(|c| c.eval(0.5)),
                Geom::Text { anchor, .. } => doc.point_pos(anchor),
            })
            .unwrap_or(Vec2::ZERO);
        self.dim_edit = Some(DimEdit {
            target: DimTarget::Existing(id),
            what: c.name(),
            angle,
            text: doc.dimension_expr(id).map_or_else(
                || format_value(dim.value, angle, units),
                |s| to_display(s, units),
            ),
            at,
            fresh: true,
        });
    }

    /// Run a command; on failure, explain in the status bar. A constraint the
    /// solver's pre-check refuses is kept as a [`Refusal`]: the conflicting
    /// set is highlighted and a dimension can be added as driven (#72).
    fn apply_reporting(&mut self, cmd: &Command) -> bool {
        match self.apply(cmd) {
            Ok(()) => {
                self.notice = None;
                self.refusal = None;
                true
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
                        message: None,
                    }),
                    _ => None,
                };
                false
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
                        r.message
                            .clone()
                            .unwrap_or_else(|| format!("This {what} {why}, so it was not added.")),
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
                        if c.has_stroke_edge(doc) {
                            let mut edge = d.edge;
                            egui::ComboBox::from_id_salt(("dim-edge", id))
                                .selected_text(edge.name())
                                .width(96.0)
                                .show_ui(ui, |ui| {
                                    for e in Edge::ALL {
                                        ui.selectable_value(&mut edge, e, e.name());
                                    }
                                })
                                .response
                                .on_hover_text(
                                    "Measure to the centreline or to the edge of the pen line / cut",
                                );
                            if edge != d.edge {
                                action = Some(Command::SetDimensionEdge { id, edge });
                            }
                        }
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

    /// Enter in the inline editor: a plain number (in the display units), or
    /// an expression over the document's parameters (in millimetres, angles
    /// in degrees). Returns whether it was applied; on failure the editor
    /// stays open with the reason shown.
    fn commit_dimension(&mut self, edit: &DimEdit, units: Units) -> bool {
        if let DimTarget::Pattern(rid) = edit.target {
            return match self.pattern_command(rid, &edit.text, units) {
                Ok(cmd) => self.apply_reporting(&cmd),
                Err(why) => {
                    self.notice = Some(why);
                    false
                }
            };
        }
        if let DimTarget::Offset(rid) = edit.target {
            let mm =
                parse_number(&edit.text).map(|v| if units == Units::Inch { v * 25.4 } else { v });
            return match self.offset_command(rid, mm, &to_stored(&edit.text, units)) {
                Some(cmd) => self.apply_reporting(&cmd),
                None => false,
            };
        }
        if let Some(v) = parse_number(&edit.text) {
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
                DimTarget::Offset(_) | DimTarget::Pattern(_) => return false,
            };
            return self.apply_reporting(&cmd);
        }
        let expr = if edit.angle {
            edit.text.trim().to_owned()
        } else {
            to_stored(&edit.text, units)
        };
        let cmd = match edit.target {
            DimTarget::New(spec) => Command::AddDimensionExpr {
                constraint: spec.constraint(1.0, false),
                expr,
            },
            DimTarget::Existing(id) => Command::SetDimensionExpr {
                id,
                expr: Some(expr),
            },
            DimTarget::Offset(_) | DimTarget::Pattern(_) => return false,
        };
        self.apply_reporting(&cmd)
    }

    /// The inline value box (#70).
    fn dimension_editor(&mut self, ui: &egui::Ui, rect: egui::Rect, units: Units) {
        let notice = self.notice.clone();
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
                    if let Some(msg) = &notice {
                        ui.colored_label(ui.visuals().error_fg_color, msg);
                    }
                });
            });
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            cancel = true;
        }
        if commit && let Some(mut edit) = self.dim_edit.take() {
            if !self.commit_dimension(&edit, units) {
                // Keep the box open, with the error beneath it.
                edit.fresh = false;
                self.dim_edit = Some(edit);
            }
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
        if self.draft.tool() != Tool::Select || self.modify.is_some() {
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
                Ok(()) => {
                    self.notice = None;
                    if let Some(id) = self.draft.take_placed() {
                        self.selection.set_all([id]);
                    }
                }
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

    /// The Béziers whose handles should show for the current selection.
    fn wanted_handles(&self) -> BTreeSet<EntityId> {
        let doc = self.sketcher.document();
        doc.entities()
            .iter()
            .filter(|(id, e)| {
                matches!(e.geom, Geom::Bezier { .. })
                    && (self.selection.contains(*id)
                        || e.geom
                            .point_refs()
                            .iter()
                            .any(|p| self.selection.contains(*p)))
            })
            .map(|(id, _)| id)
            .collect()
    }

    fn rebuild(&mut self) {
        self.scene = Scene::build(
            self.sketcher.document(),
            &self.sketcher.state().fully_constrained,
            &self.handles,
        );
        let scene = &self.scene;
        self.selection
            .retain(|id| scene.items.iter().any(|i| i.id == id));
        self.hover = None;
        self.retain_regions();
        self.refresh_machine();
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
        let selected_text = self
            .selection
            .iter()
            .next()
            .filter(|_| self.selection.len() == 1)
            .and_then(|id| {
                let (t, anchor, dir) = self.sketcher.document().text(id)?;
                Some((id, anchor, dir, t.clone()))
            });
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
                let mut counts = [0usize; 6];
                for id in self.selection.iter() {
                    if let Some(e) = doc.entity(id) {
                        counts[e.geom.kind() as usize] += 1;
                    }
                }
                for (name, c) in ["Points", "Lines", "Circles", "Arcs", "Béziers", "Texts"]
                    .iter()
                    .zip(counts)
                {
                    if c > 0 {
                        ui.label(format!("{name}: {c}"));
                    }
                }
            }
        }
        if let Some((id, anchor, dir, text)) = selected_text {
            ui.separator();
            self.edit_text(ui, id, anchor, dir, &text);
        } else {
            self.text_edit = None;
        }
    }

    /// Edit the selected text's content, font and style. Style changes apply
    /// at once; the content applies when its field loses focus, so one edit
    /// is one undo step (SPEC §11). With the font missing only the font can
    /// change (the content can't be re-laid out), and picking one re-lays
    /// the saved text out in it.
    fn edit_text(
        &mut self,
        ui: &mut egui::Ui,
        id: EntityId,
        anchor: EntityId,
        dir: EntityId,
        stored: &locus_doc::TextData,
    ) {
        let mut s = match self.text_edit.take() {
            Some((i, s)) if i == id => s,
            _ => TextSettings::from_data(stored),
        };
        // An undo/redo or solver change under the buffer: show the stored text
        // unless the user is mid-edit of the content.
        let live = TextSettings::from_data(stored);
        if s.content != live.content && !ui.memory(|m| m.focused().is_some()) {
            s = live;
        }
        let missing = self.fonts.missing(&stored.font);
        let mut edited = text_ui::controls(ui, &mut s, &mut self.fonts, !missing, 3);
        if s.content != stored.content
            && !missing
            && ui
                .button("Apply text")
                .on_hover_text("Ctrl+Enter, or click away from the field")
                .clicked()
        {
            edited.content_done = true;
        }
        if edited.commit() && s != TextSettings::from_data(stored) {
            match s.to_data(&self.fonts.library) {
                Ok(text) => {
                    let cmd = Command::ModifyEntity {
                        id,
                        geom: Geom::Text {
                            anchor,
                            dir,
                            text: Box::new(text),
                        },
                    };
                    match self.apply(&cmd) {
                        Ok(()) => self.notice = None,
                        Err(e) => self.notice = Some(e.to_string()),
                    }
                }
                Err(e) => self.notice = Some(e.to_string()),
            }
        }
        self.text_edit = Some((id, s));
    }

    /// Tool-bar controls while the Text tool is active.
    pub fn text_tool_ui(&mut self, ui: &mut egui::Ui) {
        // Keep the strip compact: the controls go in a wrapped row.
        text_ui::controls(ui, &mut self.text_settings, &mut self.fonts, true, 1);
    }

    /// Swap in the app's real font catalog (system fonts, browser picker).
    pub fn install_fonts(&mut self, ctx: &egui::Context) {
        self.fonts = FontCatalog::new(ctx);
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
        let wanted = self.wanted_handles();
        if wanted != self.handles {
            self.handles = wanted;
            self.rebuild();
        }
        if let Some(m) = self.modify.filter(|m| *m != Modify::Transform) {
            self.modify_input(ui, &resp, rect, m, settings.units);
        } else if self.draft.tool() == Tool::Select {
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
        self.draw_bed(&painter, rect, dark, settings);
        self.draw_machine(&painter, rect, dark);
        self.draw_stroke_preview(&frame, dark);
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
        let doc = self.sketcher.document();
        let guides: Vec<(Vec2, Vec2)> = self
            .handles
            .iter()
            .filter_map(|id| doc.cubic(*id))
            .flat_map(|c| [(c.p0, c.c0), (c.p1, c.c1)])
            .collect();
        frame.draw_handle_guides(&guides);
        self.dim_rects = frame.draw_annotations(&self.scene, &flag_constraints);
        self.draw_tool_overlay(&frame, &painter);
        if let (Some(start), Some(now)) = (self.rubber, resp.interact_pointer_pos()) {
            draw_rubber_band(&painter, start, now);
        }
        grid::rulers(&painter, &self.view, rect);
        self.draw_offset_labels(&painter, rect, dark, settings.units);
        self.draw_pattern_labels(&painter, rect, dark, settings.units);
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
        if self.draft.tool() == Tool::Bezier {
            self.pen_drag(ui, resp, rect, auto);
        }
        if resp.clicked_by(PointerButton::Primary)
            && let Some(pick) = self.cursor
        {
            self.draft.set_layer(self.active_layer());
            if self.draft.tool() == Tool::Text {
                match self.text_settings.to_data(&self.fonts.library) {
                    Ok(t) => self.draft.set_text(Some(t)),
                    Err(e) => {
                        self.draft.set_text(None);
                        self.notice = Some(e.to_string());
                    }
                }
            }
            let out = self.draft.click(pick, self.sketcher.document().next_id());
            self.typed.clear();
            self.run(out);
        }
    }

    /// Clicks for the modify tools (SPEC §6.2): the curve under the pointer is
    /// trimmed, extended or broken as one undo step, and any constraint the
    /// step removes is named in the status bar (SPEC §4 rule 5).
    fn modify_input(
        &mut self,
        ui: &egui::Ui,
        resp: &egui::Response,
        rect: egui::Rect,
        m: Modify,
        units: Units,
    ) {
        let tol = HIT_TOL_PT / self.view.pt_per_mm;
        let at = resp.hover_pos().map(|p| self.view.to_world(p, rect));
        self.hover = at.and_then(|w| hit::pick_curve(&self.scene.items, w, tol));
        if resp.hovered() {
            ui.ctx().set_cursor_icon(if self.hover.is_some() {
                egui::CursorIcon::PointingHand
            } else {
                egui::CursorIcon::Default
            });
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.modify = None;
                self.corner_first = None;
                self.selection.clear();
                self.notice = None;
                return;
            }
        }
        if !resp.clicked_by(PointerButton::Primary) {
            return;
        }
        if m == Modify::Offset {
            if let Some(id) = self.hover {
                self.offset_click(id, units);
            }
            return;
        }
        let doc = self.sketcher.document();
        let plan = if m.takes_two() {
            // A corner point shared by exactly two curves picks both.
            let corner = at
                .and_then(|w| hit::pick_point(&self.scene.items, w, tol))
                .map(|(p, _)| doc.dependents(p).0)
                .filter(|users| users.len() == 2);
            match (corner, self.corner_first, self.hover) {
                (Some(users), _, _) => m.plan_corner(doc, users[0], users[1], &self.corner),
                (None, None, Some(id)) => {
                    self.corner_first = Some(id);
                    self.selection.set_all([id]);
                    self.notice = None;
                    return;
                }
                (None, Some(first), Some(id)) => m.plan_corner(doc, first, id, &self.corner),
                (None, _, None) => return,
            }
        } else {
            let (Some(id), Some(w)) = (self.hover, at) else {
                return;
            };
            m.plan(doc, id, w)
        };
        self.corner_first = None;
        self.selection.clear();
        match plan {
            Err(e) => self.notice = Some(e.to_string()),
            Ok(plan) => {
                let names: Vec<String> = plan
                    .dropped
                    .iter()
                    .filter_map(|c| doc.constraint(*c))
                    .map(|c| constrain::describe(doc, c, units))
                    .collect();
                self.apply_reporting(&plan.command);
                if self.notice.is_none() && !names.is_empty() {
                    let s = if names.len() == 1 { "" } else { "s" };
                    self.notice = Some(format!(
                        "{}: removed {} constraint{s}: {}",
                        m.label(),
                        names.len(),
                        names.join("; ")
                    ));
                }
                self.hover = None;
            }
        }
    }

    /// Click-drag with the pen: the press places the anchor, the drag pulls
    /// out its handle and the release commits the curve (#160).
    fn pen_drag(&mut self, ui: &egui::Ui, resp: &egui::Response, rect: egui::Rect, auto: bool) {
        if resp.drag_started_by(PointerButton::Primary)
            && let Some(o) = ui.input(|i| i.pointer.press_origin())
        {
            let pick = self.designate(self.view.to_world(o, rect), auto);
            self.pen = Some((pick, pick.pos));
        }
        if resp.dragged_by(PointerButton::Primary)
            && let (Some((pick, _)), Some(p)) = (self.pen, resp.interact_pointer_pos())
        {
            self.pen = Some((pick, self.view.to_world(p, rect)));
        }
        if resp.drag_stopped_by(PointerButton::Primary)
            && let Some((pick, now)) = self.pen.take()
        {
            self.draft.set_layer(self.active_layer());
            let out = self.draft.pen(
                pick,
                Some(now - pick.pos),
                self.sketcher.document().next_id(),
            );
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
        match self.pen {
            Some((anchor, now)) => frame.draw_preview(&self.draft.preview_pen(anchor.pos, now)),
            None => frame.draw_preview(&self.draft.preview(pick.pos)),
        }
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
            // A Bézier moves by its end nearer the cursor, like a line.
            Some(Geom::Bezier { start, end, .. }) => near(*start, *end),
            // Text is dragged by its anchor.
            Some(Geom::Text { anchor, .. }) => *anchor,
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
            && let Some(&(rid, _)) = self.offset_rects.iter().find(|(_, r)| r.contains(p))
        {
            if self
                .sketcher
                .document()
                .recipe(rid)
                .is_some_and(|r| r.kind.is_pattern())
            {
                self.edit_pattern(rid, units);
            } else {
                self.edit_offset(rid, units);
            }
        } else if resp.double_clicked_by(PointerButton::Primary)
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

/// Inch-mode entry of a length expression (SPEC §5): bare numbers follow the
/// display unit like plain numbers do, so `1/2 + 0.125` typed in inches is
/// stored as `(1/2 + 0.125) * in`. An expression that names parameters or a
/// unit is already unambiguous (parameters are in millimetres) and is kept
/// as typed.
pub(crate) fn to_stored(text: &str, units: Units) -> String {
    let t = text.trim();
    if units != Units::Inch {
        return t.to_owned();
    }
    match locus_doc::expr::parse(t) {
        Ok(e) if e.references().is_empty() => format!("({t}) * in"),
        _ => t.to_owned(),
    }
}

/// The inverse of [`to_stored`], for showing a stored expression back.
pub(crate) fn to_display(stored: &str, units: Units) -> String {
    if units == Units::Inch
        && let Some(inner) = stored
            .strip_prefix('(')
            .and_then(|s| s.strip_suffix(") * in"))
        && locus_doc::expr::parse(inner).is_ok_and(|e| e.references().is_empty())
    {
        return inner.to_owned();
    }
    stored.to_owned()
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
        Geom::Bezier { .. } => "Bézier",
        Geom::Text { .. } => "Text",
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
        Geom::Bezier { .. } => {
            if let Some(c) = doc.cubic(id) {
                rows.push(("Start".to_owned(), pt(c.p0)));
                rows.push(("End".to_owned(), pt(c.p1)));
                rows.push(("Length".to_owned(), units.format_len(c.length())));
            }
        }
        Geom::Text { .. } => {
            if let Some((t, anchor, _)) = doc.text(id) {
                rows.push(("Text".to_owned(), t.content.clone()));
                rows.push(("Font".to_owned(), t.font.display_name()));
                rows.push(("Size".to_owned(), units.format_len(t.size)));
                if let Some(p) = doc.point_pos(anchor) {
                    rows.push(("Anchor".to_owned(), pt(p)));
                }
            }
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use locus_doc::RecipeKind;

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
    fn pattern_label_edit_sets_count_and_spacing_with_units() {
        let mut c = Canvas::default();
        c.apply_reporting(&Command::AddEntity {
            layer: c.active_layer(),
            construction: false,
            geom: Geom::Point { pos: Vec2::ZERO },
        });
        let p = c
            .document()
            .entities()
            .iter()
            .map(|(i, _)| i)
            .last()
            .unwrap();
        c.apply_reporting(&Command::AddRecipe {
            kind: RecipeKind::Linear {
                dir1: locus_doc::LinearDir {
                    axis: locus_doc::Axis::X,
                    count: "3".to_owned(),
                    spacing: "10".to_owned(),
                    symmetric: false,
                },
                dir2: None,
            },
            sources: vec![p],
        });
        let rid = *c.document().recipes().keys().next().unwrap();
        c.edit_pattern(rid, Units::Mm);
        assert_eq!(c.dim_edit.as_ref().unwrap().text, "3, 10");
        let edit = |text: &str| DimEdit {
            target: DimTarget::Pattern(rid),
            what: "count, spacing",
            angle: false,
            text: text.to_owned(),
            at: Vec2::ZERO,
            fresh: false,
        };
        assert!(c.commit_dimension(&edit("5, 20"), Units::Mm));
        assert_eq!(c.document().recipe(rid).unwrap().active, 4);
        // Inch mode: the spacing is inches, like a plain number.
        assert!(c.commit_dimension(&edit("2, 1"), Units::Inch));
        let far = c
            .document()
            .recipe(rid)
            .unwrap()
            .instances()
            .map(|e| c.document().point_pos(e).unwrap().x)
            .fold(0.0_f64, f64::max);
        assert!((far - 25.4).abs() < 1e-9);
        c.edit_pattern(rid, Units::Inch);
        assert_eq!(c.dim_edit.as_ref().unwrap().text, "2, 1");
        // A malformed entry is refused with a reason.
        assert!(!c.commit_dimension(&edit("7"), Units::Mm));
        assert!(c.notice.is_some());
    }

    #[test]
    #[allow(clippy::unwrap_used)]
    fn patterns_panel_creates_edits_and_suppresses() {
        let mut c = Canvas::default();
        let ctx = egui::Context::default();
        let frame = |c: &mut Canvas| {
            let _ = ctx.run_ui(egui::RawInput::default(), |ui| c.patterns_ui(ui, Units::Mm));
        };
        frame(&mut c);
        c.apply_reporting(&Command::AddEntity {
            layer: c.active_layer(),
            construction: false,
            geom: Geom::Point {
                pos: Vec2::new(1.0, 1.0),
            },
        });
        let p = c
            .document()
            .entities()
            .iter()
            .map(|(i, _)| i)
            .last()
            .unwrap();
        let kind = RecipeKind::Linear {
            dir1: locus_doc::LinearDir {
                axis: locus_doc::Axis::X,
                count: "4".to_owned(),
                spacing: "5".to_owned(),
                symmetric: false,
            },
            dir2: None,
        };
        assert!(c.apply_reporting(&Command::AddRecipe {
            kind,
            sources: vec![p]
        }));
        frame(&mut c);
        let rid = *c.document().recipes().keys().next().unwrap();
        assert_eq!(c.document().recipe(rid).unwrap().active, 3);
        assert!(c.apply_reporting(&Command::SetInstanceSuppressed {
            id: rid,
            index: 0,
            suppressed: true
        }));
        frame(&mut c);
        assert_eq!(c.document().recipe(rid).unwrap().instances().count(), 2);
    }

    #[test]
    fn expressions_follow_the_display_unit_like_plain_numbers() {
        // Bare numbers are inches in inch mode, stored unambiguously.
        assert_eq!(to_stored("1/2 + 0.125", Units::Inch), "(1/2 + 0.125) * in");
        assert_eq!(to_stored("1/2 + 0.125", Units::Mm), "1/2 + 0.125");
        // Parameters (mm) and explicit units are kept as typed.
        assert_eq!(to_stored("w * 2", Units::Inch), "w * 2");
        assert_eq!(to_stored("0.5 * in", Units::Inch), "0.5 * in");
        // Showing it back strips the wrapper again.
        assert_eq!(to_display("(1/2 + 0.125) * in", Units::Inch), "1/2 + 0.125");
        assert_eq!(to_display("(1/2) * in", Units::Mm), "(1/2) * in");
        assert_eq!(to_display("(w) * in", Units::Inch), "(w) * in");
    }

    #[test]
    #[allow(clippy::unwrap_used)]
    fn inch_mode_expression_entry_gives_the_same_length_as_a_plain_number() {
        let mut c = Canvas::default();
        c.apply_reporting(&Command::AddEntity {
            layer: c.active_layer(),
            construction: false,
            geom: Geom::Point { pos: Vec2::ZERO },
        });
        let p = c
            .document()
            .entities()
            .iter()
            .map(|(i, _)| i)
            .last()
            .unwrap();
        c.apply_reporting(&Command::AddEntity {
            layer: c.active_layer(),
            construction: false,
            geom: Geom::Circle {
                center: p,
                radius: 1.0,
            },
        });
        let circle = c
            .document()
            .entities()
            .iter()
            .map(|(i, _)| i)
            .last()
            .unwrap();
        c.apply_reporting(&Command::AddConstraint {
            constraint: locus_doc::Constraint::Diameter {
                curve: circle,
                dim: locus_doc::Dimension::driving(2.0),
            },
        });
        let cid = *c.document().constraints().keys().next_back().unwrap();
        let edit = |text: &str| DimEdit {
            target: DimTarget::Existing(cid),
            what: "diameter",
            angle: false,
            text: text.to_owned(),
            at: Vec2::ZERO,
            fresh: false,
        };
        let value = |c: &Canvas| {
            c.document()
                .constraint(cid)
                .unwrap()
                .dimension()
                .unwrap()
                .value
        };
        assert!(c.commit_dimension(&edit("0.5"), Units::Inch));
        assert!((value(&c) - 12.7).abs() < 1e-9, "plain number: inches");
        assert!(c.commit_dimension(&edit("1/4 + 1/4"), Units::Inch));
        assert!((value(&c) - 12.7).abs() < 1e-9, "expression: same inches");
        assert_eq!(c.document().dimension_expr(cid), Some("(1/4 + 1/4) * in"));
        // Switching the display unit later does not move the geometry.
        assert!(c.commit_dimension(&edit("12.7"), Units::Mm));
        assert!((value(&c) - 12.7).abs() < 1e-9);
    }

    #[test]
    #[allow(clippy::unwrap_used)]
    fn parameters_panel_renders_and_expressions_drive_dimensions() {
        let mut c = Canvas::default();
        let ctx = egui::Context::default();
        let frame = |c: &mut Canvas| {
            let _ = ctx.run_ui(egui::RawInput::default(), |ui| c.parameters_ui(ui));
        };
        frame(&mut c);
        assert!(c.apply_reporting(&Command::SetParameter {
            name: "material".to_owned(),
            expr: "3".to_owned(),
        }));
        frame(&mut c);
        // A circle with a diameter dimension typed as an expression.
        let centre = Command::AddEntity {
            layer: c.active_layer(),
            construction: false,
            geom: Geom::Point { pos: Vec2::ZERO },
        };
        c.apply_reporting(&centre);
        let p = *c
            .document()
            .entities()
            .iter()
            .map(|(i, _)| i)
            .collect::<Vec<_>>()
            .last()
            .unwrap();
        c.apply_reporting(&Command::AddEntity {
            layer: c.active_layer(),
            construction: false,
            geom: Geom::Circle {
                center: p,
                radius: 1.0,
            },
        });
        let circle = c
            .document()
            .entities()
            .iter()
            .map(|(i, _)| i)
            .last()
            .unwrap();
        let edit = |text: &str, id| DimEdit {
            target: DimTarget::Existing(id),
            what: "diameter",
            angle: false,
            text: text.to_owned(),
            at: Vec2::ZERO,
            fresh: false,
        };
        let cid = {
            c.apply_reporting(&Command::AddConstraint {
                constraint: locus_doc::Constraint::Diameter {
                    curve: circle,
                    dim: locus_doc::Dimension::driving(2.0),
                },
            });
            *c.document().constraints().keys().next_back().unwrap()
        };
        assert!(c.commit_dimension(&edit("material * 2 + 0.2", cid), Units::Mm));
        let v = c
            .document()
            .constraint(cid)
            .unwrap()
            .dimension()
            .unwrap()
            .value;
        assert!((v - 6.2).abs() < 1e-6);
        // A bad expression is refused and reported, and changes nothing.
        assert!(!c.commit_dimension(&edit("nope * 2", cid), Units::Mm));
        assert!(c.notice.is_some());
        // The parameter drives it.
        assert!(c.apply_reporting(&Command::SetParameter {
            name: "material".to_owned(),
            expr: "6".to_owned(),
        }));
        let v = c
            .document()
            .constraint(cid)
            .unwrap()
            .dimension()
            .unwrap()
            .value;
        assert!((v - 12.2).abs() < 1e-6);
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
