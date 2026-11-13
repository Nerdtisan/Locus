// SPDX-License-Identifier: AGPL-3.0-or-later
//! App state (#294, design §7.3): the one mutable state every view and
//! action works on. It owns the document behind the command API, the view,
//! the selection and the active tool's state, and holds no egui widgets.
//!
//! Views and actions read it and change the document only through
//! [`AppState::apply`] / [`AppState::apply_reporting`], which run a
//! `locus_doc::Command` through the `Sketcher` (SPEC §11).

use crate::constrain::{self, Kind};
use crate::demo;
use crate::modify_tool::{Modify, TransformMode};
use crate::scene::Scene;
use crate::settings::Units;
use crate::tool::{ConstrainTool, Flow, ModifyTool, ToolCx, ToolEvent, ToolKind};
use crate::tools::Tool;
use crate::views::text_ui::FontCatalog;
use locus_doc::{Command, ConstraintId, DocError, Document, EntityId, Geom, Sketcher};
use locus_geom::{Aabb, Vec2};
use std::collections::BTreeSet;

mod doc;
mod selection;
mod tools;
mod view;
pub(crate) use view::Marking;

pub use doc::{DocState, Refusal};
pub use selection::SelectionState;
pub use tools::{DimEdit, DimTarget, ToolState};
pub use view::ViewState;

/// Pick radius in screen points (converted to mm by the zoom).
pub(crate) const HIT_TOL_PT: f64 = 6.0;

/// Everything the editor knows (design §7.3), split by concern so views
/// and tools borrow only what they need.
pub struct AppState {
    /// The document behind the command API, its scene and solve report.
    pub(crate) doc: DocState,
    /// Zoom and pan, the canvas rectangle, preview toggles, touch input.
    pub(crate) view: ViewState,
    /// Selected and hovered entities and regions; never in the document.
    pub(crate) selection: SelectionState,
    /// The active drawing or modify tool and its in-progress input.
    pub(crate) tools: ToolState,
}

impl Default for AppState {
    /// Opens on the demo sketch.
    fn default() -> Self {
        let mut st = Self {
            doc: DocState::default(),
            view: ViewState::default(),
            selection: SelectionState::default(),
            tools: ToolState::default(),
        };
        st.load(demo::sketch());
        st
    }
}

impl AppState {
    /// Replace the document (new / sample). Clears the undo history.
    pub fn load(&mut self, doc: Document) {
        self.doc.sketcher = Sketcher::new(doc);
        self.doc
            .sketcher
            .set_plugin_runner(self.doc.plugin_runner.clone());
        self.doc.generation += 1;
        self.selection.regions = crate::views::canvas::RegionUi::default();
        self.doc.active_layer = self.active_layer();
        self.tools.dragging = false;
        self.selection.items.clear();
        self.tools.host.tool_mut().reset();
        self.tools.typed.clear();
        self.doc.notice = None;
        self.tools.dim_edit = None;
        self.doc.refusal = None;
        self.rebuild();
        self.request_fit();
    }

    /// The drawing tool in use (Select while a modify tool is active).
    pub fn tool(&self) -> Tool {
        self.tools.host.draw_kind()
    }

    pub fn set_tool(&mut self, tool: Tool) {
        let layer = self.active_layer();
        self.tools.host.set_draw(tool, layer);
        self.tools.typed.clear();
        self.tools.corner_pick = None;
        self.doc.notice = None;
    }

    /// The active modify tool (SPEC §6.2), if any.
    pub fn modify(&self) -> Option<Modify> {
        self.tools.host.modify()
    }

    pub fn set_modify(&mut self, m: Modify) {
        // Switching between Move, Rotate and Scale keeps the values and the
        // pivot, as the dialog's mode switch does (design §6.7).
        let was_transform = matches!(self.modify(), Some(Modify::Transform(_)));
        self.set_tool(Tool::Select);
        // Move/rotate/scale works on the selection; the others pick curves.
        if let Modify::Transform(_) = m {
            if !was_transform {
                self.tools.transform.reset();
            }
        } else {
            self.selection.items.clear();
        }
        self.tools.host.set(Box::new(ModifyTool::new(m)));
    }

    /// Hand an event to the active tool and carry out what it asks
    /// (design §7.2): notices, selection, and a commit as one command
    /// (SPEC §11). `hover` is the curve under the pointer.
    pub(crate) fn tool_event(
        &mut self,
        ev: ToolEvent,
        hover: Option<EntityId>,
        units: Units,
    ) -> Flow {
        // The Style eyedropper takes the next click, whatever the tool
        // (design §6.2); Esc puts it away.
        if self.tools.eyedropper {
            match ev {
                ToolEvent::Click(_) => {
                    if let Some(id) = hover {
                        self.eyedrop(id);
                    }
                    return Flow::Continue;
                }
                ToolEvent::Esc => {
                    self.tools.eyedropper = false;
                    return Flow::Continue;
                }
                _ => {}
            }
        }
        let text = if self.tool() == Tool::Text && matches!(ev, ToolEvent::Click(_)) {
            match self.tools.text_settings.to_data(&self.tools.fonts.library) {
                Ok(t) => Some(t),
                Err(e) => {
                    self.doc.notice = Some(e.to_string());
                    None
                }
            }
        } else {
            None
        };
        let layer = self.active_layer();
        let selection = self.selected();
        let mut cx = ToolCx {
            doc: self.doc.sketcher.document(),
            scene: &self.doc.scene,
            tol: self.view.tol_pt / self.view.camera.pt_per_mm,
            units,
            layer,
            style: self.doc.current_style,
            construction: self.doc.construction_linetype,
            selection,
            hover,
            text,
            notice: None,
            select: None,
            offset: None,
            corner_pick: None,
        };
        let flow = self.tools.host.tool_mut().event(ev, &mut cx);
        let (notice, select, offset) = (cx.notice, cx.select, cx.offset);
        if let Some(pair) = cx.corner_pick {
            self.tools.corner_pick = Some(pair);
            self.tools.corner_focus = true;
        }
        if let Some(n) = notice {
            self.doc.notice = n;
        }
        if let Some(ids) = select {
            self.selection.items.set_all(ids);
        }
        if let Some(id) = offset {
            self.offset_click(id, units);
        }
        match flow {
            Flow::Commit => self.commit_tool(),
            Flow::Done if !matches!(self.tools.host.kind(), ToolKind::Constrain(_)) => {
                self.set_tool(Tool::Select);
            }
            _ => {}
        }
        flow
    }

    /// Run the active tool's command. Drawing tools report a failure in the
    /// status bar; the rest also highlight a refusal (#72).
    fn commit_tool(&mut self) {
        let Some(cmd) = self.tools.host.tool_mut().commit() else {
            return;
        };
        let ok = match self.tools.host.kind() {
            ToolKind::Draw(_) => match self.apply(&cmd) {
                Ok(()) => {
                    self.doc.notice = None;
                    true
                }
                Err(e) => {
                    self.doc.notice = Some(e.to_string());
                    false
                }
            },
            _ => self.apply_reporting(&cmd),
        };
        let after = self.tools.host.tool_mut().committed(ok);
        if let Some(id) = after.select {
            self.selection.items.set_all([id]);
        }
        if self.doc.notice.is_none() {
            self.doc.notice = after.notice;
        }
        if ok && self.modify().is_some() {
            self.selection.hover = None;
        }
    }

    /// True when the selection has something to move, rotate or scale.
    pub fn can_transform(&self) -> bool {
        !self.selection.items.is_empty()
    }

    /// The centre of the selection's bounding box: where the manipulator
    /// sits and, unless a pivot was placed, what Rotate and Scale turn
    /// about (design §6.7).
    pub fn selection_center(&self) -> Option<Vec2> {
        self.doc
            .scene
            .items
            .iter()
            .filter(|i| self.selection.items.contains(i.id))
            .map(|i| i.bbox)
            .reduce(Aabb::union)
            .map(|b| b.min.lerp(b.max, 0.5))
    }

    /// Move, rotate or scale the selection by the tool's values, as one undo
    /// step (SPEC §6.2, §11). Constraints the transform itself breaks
    /// (a rotated rectangle's horizontals) go in the same step and are named
    /// in the status bar; when constraints forbid it, they are named and
    /// highlighted instead. Returns whether it was applied.
    pub fn apply_transform(&mut self, mode: TransformMode, units: Units) -> bool {
        let sel: Vec<EntityId> = self.selection.items.iter().collect();
        let center = self.selection_center().unwrap_or(Vec2::ZERO);
        let t = self.tools.transform.transform(mode, center);
        let doc = self.doc.sketcher.document();
        match locus_doc::transform::transform(doc, &sel, t, self.tools.transform.copy) {
            Ok(plan) => {
                let names: Vec<String> = plan
                    .dropped
                    .iter()
                    .filter_map(|c| doc.constraint(*c))
                    .map(|c| constrain::describe(doc, c, units))
                    .collect();
                let ok = self.apply_reporting(&plan.command);
                if ok && !names.is_empty() {
                    let s = if names.len() == 1 { "" } else { "s" };
                    self.doc.notice = Some(format!(
                        "{}: removed {} constraint{s} that no longer hold: {}",
                        mode.label(),
                        names.len(),
                        names.join("; ")
                    ));
                }
                ok
            }
            Err(locus_doc::modify::ModifyError::Blocked(cs)) => {
                let cs_len = cs.len();
                let names: Vec<String> = cs
                    .iter()
                    .filter_map(|c| doc.constraint(*c))
                    .map(|c| constrain::describe(doc, c, units))
                    .collect();
                self.doc.notice = Some(if names.is_empty() {
                    "Constraints hold the selection in place".to_owned()
                } else {
                    format!("Held in place by: {}", names.join("; "))
                });
                self.doc.refusal = Some(Refusal {
                    constraints: cs,
                    entities: sel,
                    redundant: false,
                    as_driven: None,
                    message: Some(format!(
                        "The {} constraint(s) highlighted hold the selection, so it was not moved.",
                        cs_len
                    )),
                });
                false
            }
            Err(e) => {
                self.doc.notice = Some(e.to_string());
                false
            }
        }
    }

    /// What to tell the user: the last problem, else the tool's next step.
    pub fn hint(&self) -> String {
        match &self.doc.notice {
            Some(n) => n.clone(),
            None => self.tools.host.tool().prompt().into_owned(),
        }
    }

    /// DOF and solve status for the status bar — the solver's own report
    /// (#72, SPEC §5 Solver UX).
    pub fn solve_summary(&self) -> String {
        let st = self.doc.sketcher.state();
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
    pub(crate) fn flagged(&self) -> (BTreeSet<ConstraintId>, BTreeSet<EntityId>) {
        let st = self.doc.sketcher.state();
        let mut cs = BTreeSet::new();
        let mut es = BTreeSet::new();
        for g in st.conflicts.iter().chain(&st.redundant) {
            cs.extend(g.constraints.iter().copied());
            es.extend(g.entities.iter().copied());
        }
        if let Some(r) = &self.doc.refusal {
            cs.extend(r.constraints.iter().copied());
            es.extend(r.entities.iter().copied());
        }
        (cs, es)
    }

    pub(crate) fn selected(&self) -> Vec<EntityId> {
        self.selection.items.iter().collect()
    }

    /// Does the current selection support this constraint (#70)?
    pub fn can_constrain(&self, kind: Kind) -> bool {
        constrain::plan(kind, self.doc.sketcher.document(), &self.selected()).is_ok()
    }

    /// Apply a geometric constraint to the selection (#70). A refusal (wrong
    /// selection, or the solver's conflict pre-check) is explained in the
    /// status bar.
    pub fn constrain(&mut self, kind: Kind) {
        // A constraint is a tool whose needs the selection meets, so it
        // commits as soon as it is picked; the active tool stays (design §3).
        let prev = self.tools.host.swap(Box::new(ConstrainTool::new(kind)));
        self.tool_event(ToolEvent::Activate, None, Units::Mm);
        self.tools.host.swap(prev);
    }

    pub fn can_dimension(&self) -> bool {
        constrain::infer_dimension(self.doc.sketcher.document(), &self.selected()).is_ok()
    }

    /// The dimension tool: infer the dimension from the selection and open
    /// the inline value editor (#70).
    pub fn start_dimension(&mut self, units: Units) {
        match constrain::infer_dimension(self.doc.sketcher.document(), &self.selected()) {
            Ok((spec, value)) => {
                self.tools.dim_edit = Some(DimEdit {
                    target: DimTarget::New(spec),
                    what: spec.label(),
                    angle: spec.is_angle(),
                    text: format_value(value, spec.is_angle(), units),
                    at: constrain::dim_anchor(self.doc.sketcher.document(), spec),
                    fresh: true,
                });
                self.doc.notice = None;
            }
            Err(why) => self.doc.notice = Some(why),
        }
    }

    /// Double-click on a dimension: edit its value in place.
    pub(crate) fn edit_existing_dimension(&mut self, id: ConstraintId, units: Units) {
        let doc = self.doc.sketcher.document();
        let Some(c) = doc.constraint(id) else {
            return;
        };
        let Some(dim) = c.dimension() else {
            return;
        };
        if dim.driven {
            self.doc.notice = Some("A driven dimension is measured; it can't be edited".to_owned());
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
        self.tools.dim_edit = Some(DimEdit {
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
    pub(crate) fn apply_reporting(&mut self, cmd: &Command) -> bool {
        match self.apply(cmd) {
            Ok(()) => {
                self.doc.notice = None;
                self.doc.refusal = None;
                true
            }
            Err(e) => {
                self.doc.notice = Some(e.to_string());
                self.doc.refusal = match &e {
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

    /// Enter in the inline editor: a plain number (in the display units), or
    /// an expression over the document's parameters (in millimetres, angles
    /// in degrees). Returns whether it was applied; on failure the editor
    /// stays open with the reason shown.
    pub(crate) fn commit_dimension(&mut self, edit: &DimEdit, units: Units) -> bool {
        if let DimTarget::Pattern(rid) = edit.target {
            return match self.pattern_command(rid, &edit.text, units) {
                Ok(cmd) => self.apply_reporting(&cmd),
                Err(why) => {
                    self.doc.notice = Some(why);
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

    /// Whether the canvas lets a plain-key shortcut through: not while a
    /// value is typed or the dimension editor is open (#68, #70). A focused
    /// text field is checked by the registry's dispatch.
    pub fn accepts_keys(&self) -> bool {
        self.tools.dim_edit.is_none() && self.tools.typed.is_empty()
    }

    /// True while a drawing tool has clicks pending.
    pub fn is_drawing(&self) -> bool {
        matches!(self.tools.host.kind(), ToolKind::Draw(_)) && self.tools.host.tool().busy()
    }

    /// Whether the pointer was over the canvas last frame: plain-letter
    /// shortcuts only act there.
    pub fn pointer_over(&self) -> bool {
        self.view.pointer_over
    }

    /// Delete the selection with its dependents as one undo step (#223,
    /// SPEC §11). Run by the `edit.delete` action, whose key waits while a
    /// typed value or a text field has the keyboard.
    /// Normal/Construction on the selection (design §6.10, #289), as one
    /// command: if every selected entity is construction they all become
    /// normal, otherwise the normal ones become construction. Pattern and
    /// mirror copies follow their source and are left out; `None` when
    /// nothing selected can change.
    pub(crate) fn construction_toggle(&self) -> Option<Command> {
        let doc = self.doc.sketcher.document();
        let ids: Vec<(EntityId, bool)> = self
            .selection
            .items
            .iter()
            .filter(|id| doc.driven_by(*id).is_none())
            .filter_map(|id| doc.entity(id).map(|e| (id, e.construction)))
            .collect();
        if ids.is_empty() {
            return None;
        }
        let to_normal = ids.iter().all(|(_, c)| *c);
        let commands: Vec<Command> = ids
            .into_iter()
            .filter(|(_, c)| *c == to_normal)
            .map(|(id, _)| Command::ToggleConstruction { id })
            .collect();
        Some(Command::Compound {
            label: if to_normal {
                "Make normal"
            } else {
                "Make construction"
            }
            .to_owned(),
            commands,
        })
    }

    pub fn delete_selection(&mut self) {
        let ids: Vec<EntityId> = self.selection.items.iter().collect();
        if let Some(cmd) = locus_doc::delete::delete_entities(self.doc.sketcher.document(), &ids)
            && self.apply_reporting(&cmd)
        {
            self.selection.items.clear();
        }
    }

    /// The document as it stands (read-only; changes go through commands).
    pub fn document(&self) -> &Document {
        self.doc.sketcher.document()
    }

    /// Run a command and refresh the derived scene.
    #[allow(dead_code)] // first caller: the sketch tools (#68)
    pub fn apply(&mut self, cmd: &Command) -> Result<(), DocError> {
        self.doc.sketcher.apply(cmd)?;
        self.rebuild();
        Ok(())
    }

    /// Undo the last step (SPEC §11); returns its label.
    pub fn undo(&mut self) -> Option<String> {
        // A half-drawn shape may refer to what was just undone.
        self.tools.host.tool_mut().reset();
        let label = self.doc.sketcher.undo().ok()?;
        self.rebuild();
        Some(label)
    }

    pub fn redo(&mut self) -> Option<String> {
        self.tools.host.tool_mut().reset();
        let label = self.doc.sketcher.redo().ok()?;
        self.rebuild();
        Some(label)
    }

    pub fn undo_label(&self) -> Option<&str> {
        self.doc.sketcher.editor().undo_label()
    }

    pub fn redo_label(&self) -> Option<&str> {
        self.doc.sketcher.editor().redo_label()
    }

    /// The Béziers whose handles should show for the current selection.
    pub(crate) fn wanted_handles(&self) -> BTreeSet<EntityId> {
        let doc = self.doc.sketcher.document();
        doc.entities()
            .iter()
            .filter(|(id, e)| {
                matches!(e.geom, Geom::Bezier { .. })
                    && (self.selection.items.contains(*id)
                        || e.geom
                            .point_refs()
                            .iter()
                            .any(|p| self.selection.items.contains(*p)))
            })
            .map(|(id, _)| id)
            .collect()
    }

    pub(crate) fn rebuild(&mut self) {
        self.doc.scene = Scene::build(
            self.doc.sketcher.document(),
            &self.doc.sketcher.state().fully_constrained,
            &self.selection.handles,
        );
        let scene = &self.doc.scene;
        self.selection
            .items
            .retain(|id| scene.items.iter().any(|i| i.id == id));
        self.selection.hover = None;
        self.retain_regions();
        self.refresh_machine();
    }

    /// World bounds of the content; a sheet-sized default while empty.
    pub(crate) fn bounds(&self) -> (Vec2, Vec2) {
        let (lo, hi) = self
            .doc
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
        self.view.auto_fit = true;
        self.view.fit_size = egui::Vec2::ZERO;
    }

    /// Select chain (#220): add the chain of every selected curve.
    pub fn select_chains(&mut self) {
        let doc = self.doc.sketcher.document();
        let chains: Vec<EntityId> = self
            .selection
            .items
            .iter()
            .flat_map(|c| crate::chain::chain(doc, c))
            .collect();
        self.selection.items.set_all(
            self.selection
                .items
                .iter()
                .chain(chains)
                .collect::<Vec<_>>(),
        );
    }

    pub fn select_all(&mut self) {
        self.selection
            .items
            .set_all(self.doc.scene.items.iter().map(|i| i.id));
    }

    /// Select exactly these entities (the pre-export panel's highlight, #128).
    pub fn select_entities(&mut self, ids: &[locus_doc::EntityId]) {
        let present: Vec<_> = ids
            .iter()
            .copied()
            .filter(|id| self.doc.scene.items.iter().any(|i| i.id == *id))
            .collect();
        self.selection.items.set_all(present);
    }

    pub fn clear_selection(&mut self) {
        self.selection.items.clear();
        self.selection.regions.selected.clear();
    }

    pub fn summary(&self) -> String {
        let n = self.doc.sketcher.document().entities().len();
        format!(
            "{n} entities · {} drawn · {} selected",
            self.view.drawn,
            self.selection.items.len()
        )
    }

    /// Swap in the app's real font catalog (system fonts, browser picker).
    pub fn install_fonts(&mut self, ctx: &egui::Context) {
        self.tools.fonts = FontCatalog::new(ctx);
    }

    /// The open marking menu, and the canvas it stays inside.
    pub fn marking(&self) -> Option<(crate::state::Marking, egui::Rect)> {
        self.view.marking.map(|m| (m, self.view.rect))
    }

    pub fn set_marking_held(&mut self, held: bool) {
        if let Some(m) = &mut self.view.marking {
            m.held = held;
        }
    }

    pub fn close_marking(&mut self) {
        self.view.marking = None;
    }

    /// Start dragging what was grabbed (#71). A point moves itself; a line
    /// moves by its end nearer the cursor; a circle or arc by its centre.
    pub(crate) fn begin_drag(&mut self, grabbed: EntityId, at: Vec2) -> bool {
        let doc = self.doc.sketcher.document();
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
        match self.doc.sketcher.begin_drag(point) {
            Ok(()) => {
                self.tools.dragging = true;
                self.doc.notice = None;
                true
            }
            Err(e) => {
                self.doc.notice = Some(e.to_string());
                false
            }
        }
    }
}

/// The number shown in the dimension editor: mm or inches, degrees for angles.
pub(crate) fn format_value(value: f64, angle: bool, units: Units) -> String {
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
pub(crate) fn parse_number(text: &str) -> Option<f64> {
    let t = text
        .trim()
        .trim_end_matches(|c: char| c.is_alphabetic() || c == '°' || c == '"');
    t.trim().parse::<f64>().ok().filter(|v| v.is_finite())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constrain::DimSpec;
    use crate::settings::Settings;
    use crate::views::layers::LayersView;
    use crate::views::parameters::ParametersView;
    use crate::views::patterns::PatternsView;
    use crate::views::properties::describe;
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
    fn frames(c: &mut AppState, lv: &mut LayersView, n: usize) {
        let ctx = egui::Context::default();
        let settings = Settings::default();
        for _ in 0..n {
            let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
                lv.ui(ui, c, &settings);
            });
        }
    }

    #[test]
    #[allow(clippy::unwrap_used)]
    fn pattern_label_edit_sets_count_and_spacing_with_units() {
        let mut c = AppState::default();
        c.apply_reporting(&Command::AddEntity {
            layer: c.active_layer(),
            construction: false,
            style: Default::default(),
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
        assert_eq!(c.tools.dim_edit.as_ref().unwrap().text, "3, 10");
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
        assert_eq!(c.tools.dim_edit.as_ref().unwrap().text, "2, 1");
        // A malformed entry is refused with a reason.
        assert!(!c.commit_dimension(&edit("7"), Units::Mm));
        assert!(c.doc.notice.is_some());
    }

    #[test]
    #[allow(clippy::unwrap_used)]
    fn patterns_panel_creates_edits_and_suppresses() {
        let mut c = AppState::default();
        let ctx = egui::Context::default();
        let mut view = PatternsView::default();
        let mut frame = |c: &mut AppState| {
            let _ = ctx.run_ui(egui::RawInput::default(), |ui| view.ui(ui, c, Units::Mm));
        };
        frame(&mut c);
        c.apply_reporting(&Command::AddEntity {
            layer: c.active_layer(),
            construction: false,
            style: Default::default(),
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
        let mut c = AppState::default();
        c.apply_reporting(&Command::AddEntity {
            layer: c.active_layer(),
            construction: false,
            style: Default::default(),
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
            style: Default::default(),
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
        let value = |c: &AppState| {
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
        let mut c = AppState::default();
        let ctx = egui::Context::default();
        let mut view = ParametersView::default();
        let mut frame = |c: &mut AppState| {
            let _ = ctx.run_ui(egui::RawInput::default(), |ui| view.ui(ui, c));
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
            style: Default::default(),
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
            style: Default::default(),
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
        assert!(c.doc.notice.is_some());
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

    /// #292: confirming the delete dialog removes a layer that holds a
    /// pattern (it used to be refused with `RecipeInUse`).
    #[test]
    #[allow(clippy::unwrap_used)]
    fn confirming_the_delete_dialog_removes_a_layer_with_a_pattern() {
        let mut c = AppState::default();
        let mut lv = LayersView::default();
        c.apply_reporting(&Command::AddLayer {
            name: "Pattern".to_owned(),
            colour: [1, 2, 3],
            operation: locus_doc::OperationKind::Draw.default_operation(),
        });
        let l2 = *c.document().layers().keys().next_back().unwrap();
        c.apply_reporting(&Command::AddEntity {
            layer: l2,
            construction: false,
            style: Default::default(),
            geom: Geom::Point {
                pos: Vec2::new(1.0, 1.0),
            },
        });
        let src = c
            .document()
            .entities()
            .iter()
            .map(|(i, _)| i)
            .last()
            .unwrap();
        assert!(c.apply_reporting(&Command::AddRecipe {
            kind: locus_doc::RecipeKind::Linear {
                dir1: locus_doc::LinearDir {
                    axis: locus_doc::Axis::X,
                    count: "3".to_owned(),
                    spacing: "10".to_owned(),
                    symmetric: false,
                },
                dir2: None,
            },
            sources: vec![src],
        }));
        let plan = c.document().layer_deletion(l2).unwrap();
        lv.dialog_for_test(l2, plan);
        lv.confirm_delete_for_test(&mut c);
        assert!(c.document().layer(l2).is_none());
        assert!(!lv.has_dialog_for_test(), "dialog closes on success");
        assert_eq!(c.undo().as_deref(), Some("Delete layer"));
        assert!(c.document().layer(l2).is_some());
    }

    /// #292: a refusal stays in the dialog with its reason instead of the
    /// dialog closing as if the layer had been deleted.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn a_refused_layer_delete_keeps_the_dialog_and_shows_why() {
        let mut c = AppState::default();
        let mut lv = LayersView::default();
        c.apply_reporting(&Command::AddLayer {
            name: "Locked".to_owned(),
            colour: [1, 2, 3],
            operation: locus_doc::OperationKind::Draw.default_operation(),
        });
        let l2 = *c.document().layers().keys().next_back().unwrap();
        c.apply_reporting(&Command::SetLayerLocked {
            id: l2,
            locked: true,
        });
        let plan = c.document().layer_deletion(l2).unwrap();
        lv.dialog_for_test(l2, plan);
        lv.confirm_delete_for_test(&mut c);
        assert!(c.document().layer(l2).is_some());
        let why = lv.dialog_error_for_test().unwrap();
        assert!(why.to_lowercase().contains("lock"), "{why}");
    }

    #[test]
    #[allow(clippy::unwrap_used)]
    fn layers_panel_renders_and_every_layer_action_is_one_undo_step() {
        let mut c = AppState::default();
        let mut lv = LayersView::default();
        frames(&mut c, &mut lv, 2);
        let l1 = c.active_layer();
        let cmd = Command::AddLayer {
            name: "Cut 2".to_owned(),
            colour: [1, 2, 3],
            operation: locus_doc::OperationKind::Draw.default_operation(),
        };
        c.apply_reporting(&cmd);
        let l2 = *c.document().layers().keys().next_back().unwrap();
        lv.expanded_for_test(l2);
        frames(&mut c, &mut lv, 2);
        // Lock → delete dialog paths run without panicking.
        c.apply_reporting(&Command::SetLayerLocked {
            id: l1,
            locked: true,
        });
        assert!(c.document().layer(l1).unwrap().locked);
        assert_eq!(c.undo().as_deref(), Some("Lock layer"));
        let plan = c.document().layer_deletion(l2).unwrap();
        lv.dialog_for_test(l2, plan);
        frames(&mut c, &mut lv, 2);
        c.apply_reporting(&Command::DeleteLayer { id: l2 });
        frames(&mut c, &mut lv, 1);
        assert!(c.document().layer(l2).is_none());
        // The active layer survives its own deletion by falling back.
        c.doc.active_layer = l2;
        assert_eq!(c.active_layer(), l1);
    }

    /// Runs one frame of the registry's key dispatch with `key` pressed and
    /// the pointer over the canvas.
    fn press(c: &mut AppState, key: egui::Key) {
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            events: vec![egui::Event::Key {
                key,
                physical_key: Some(key),
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        };
        let reg = crate::actions::Registry::default();
        let mut settings = Settings::default();
        let _ = ctx.run_ui(input, |ui| {
            let mut cx = crate::actions::Cx {
                state: c,
                settings: &mut settings,
                plugins: None,
            };
            reg.dispatch_keys(ui.ctx(), &mut cx, true);
        });
    }

    #[test]
    #[allow(clippy::unwrap_used)]
    fn delete_and_backspace_delete_the_selection_as_one_undo_step() {
        for key in [egui::Key::Delete, egui::Key::Backspace] {
            let mut c = AppState::default();
            let layer = c.active_layer();
            let add = |c: &mut AppState, geom| {
                c.apply_reporting(&Command::AddEntity {
                    layer,
                    construction: false,
                    style: Default::default(),
                    geom,
                });
                c.document()
                    .entities()
                    .iter()
                    .map(|(i, _)| i)
                    .last()
                    .unwrap()
            };
            let a = add(&mut c, Geom::Point { pos: Vec2::ZERO });
            let b = add(
                &mut c,
                Geom::Point {
                    pos: Vec2::new(5.0, 0.0),
                },
            );
            let l = add(&mut c, Geom::Line { start: a, end: b });
            c.apply_reporting(&Command::AddConstraint {
                constraint: locus_doc::Constraint::Horizontal { line: l },
            });
            let before = c.document().clone();
            c.selection.items.set_all([l]);
            press(&mut c, key);
            assert!(c.document().entity(l).is_none(), "{key:?}");
            assert_eq!(
                c.document().constraints().len(),
                before.constraints().len() - 1
            );
            assert!(c.selection.items.is_empty());
            assert_eq!(c.undo().as_deref(), Some("Delete"));
            assert_eq!(c.document().entities(), before.entities());
            assert_eq!(c.document().constraints(), before.constraints());
        }
    }

    /// X toggles Normal/Construction on the selection as one undo step
    /// (design §6.10, #289): a mixed selection becomes all construction, then
    /// all normal. Construction keeps its constraints and is not exported.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn x_toggles_construction_on_the_selection_as_one_step() {
        let mut c = AppState::default();
        let layer = c.active_layer();
        let add = |c: &mut AppState, geom| {
            c.apply_reporting(&Command::AddEntity {
                layer,
                construction: false,
                style: Default::default(),
                geom,
            });
            c.document()
                .entities()
                .iter()
                .map(|(i, _)| i)
                .last()
                .unwrap()
        };
        let line = |c: &mut AppState, y: f64| {
            let a = add(
                c,
                Geom::Point {
                    pos: Vec2::new(500.0, y),
                },
            );
            let b = add(
                c,
                Geom::Point {
                    pos: Vec2::new(510.0, y),
                },
            );
            add(c, Geom::Line { start: a, end: b })
        };
        let (l1, l2) = (line(&mut c, 500.0), line(&mut c, 520.0));
        c.apply_reporting(&Command::ToggleConstruction { id: l2 });
        let cons = |c: &AppState, id| c.document().entity(id).unwrap().construction;
        c.selection.items.set_all([l1, l2]);
        press(&mut c, egui::Key::X);
        assert!(cons(&c, l1) && cons(&c, l2), "mixed → all construction");
        assert_eq!(c.undo_label(), Some("Make construction"));
        // Constraints still work on construction geometry (SPEC §4).
        assert!(c.apply_reporting(&Command::AddConstraint {
            constraint: locus_doc::Constraint::Horizontal { line: l1 },
        }));
        // …and it is never exported.
        let exported: Vec<EntityId> = locus_export::collect(c.document())
            .layers
            .iter()
            .flat_map(|l| l.paths.iter().flat_map(|p| p.entities()))
            .collect();
        assert!(!exported.contains(&l1) && !exported.contains(&l2));
        c.undo();
        press(&mut c, egui::Key::X);
        assert!(
            !cons(&c, l1) && !cons(&c, l2),
            "all construction → all normal"
        );
        c.undo();
        assert!(cons(&c, l1) && cons(&c, l2), "one undo per toggle");
        c.selection.items.clear();
        assert!(c.construction_toggle().is_none());
    }

    /// The Linetype switch makes drawing tools create construction (#289).
    #[test]
    #[allow(clippy::unwrap_used)]
    fn the_construction_linetype_draws_construction() {
        use crate::tool::ToolEvent;
        use crate::tools::Pick;
        let mut c = AppState::default();
        c.doc.construction_linetype = true;
        c.set_tool(Tool::Line);
        for x in [0.0, 40.0] {
            c.tool_event(
                ToolEvent::Click(Pick::free(Vec2::new(x, 300.0))),
                None,
                Units::Mm,
            );
        }
        let last = c
            .document()
            .entities()
            .iter()
            .filter(|(_, e)| matches!(e.geom, Geom::Line { .. }))
            .last()
            .map(|(_, e)| e.construction);
        assert_eq!(last, Some(true));
    }

    /// Select chain (#220) grows the selection to whole chains; points
    /// alone add nothing.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn select_chain_grows_the_selection_to_whole_chains() {
        let mut c = AppState::default();
        let layer = c.active_layer();
        let add = |c: &mut AppState, geom| {
            c.apply_reporting(&Command::AddEntity {
                layer,
                construction: false,
                style: Default::default(),
                geom,
            });
            c.document()
                .entities()
                .iter()
                .map(|(i, _)| i)
                .last()
                .unwrap()
        };
        let p: Vec<EntityId> = [(500.0, 500.0), (510.0, 500.0), (520.0, 510.0)]
            .iter()
            .map(|&(x, y)| {
                add(
                    &mut c,
                    Geom::Point {
                        pos: Vec2::new(x, y),
                    },
                )
            })
            .collect();
        let l1 = add(
            &mut c,
            Geom::Line {
                start: p[0],
                end: p[1],
            },
        );
        let l2 = add(
            &mut c,
            Geom::Line {
                start: p[1],
                end: p[2],
            },
        );
        c.selection.items.set_all([l1]);
        c.select_chains();
        assert!(c.selection.items.contains(l2));
        c.selection.items.set_all([p[0]]);
        c.select_chains();
        assert_eq!(c.selection.items.len(), 1);
    }

    /// The design's shortcut map (§5): tool letters pick tools; constraints
    /// have no letters, so H no longer adds Horizontal (owner decision).
    #[test]
    fn letters_pick_tools_and_constraints_have_none() {
        let mut c = AppState::default();
        press(&mut c, egui::Key::L);
        assert_eq!(c.tool(), Tool::Line);
        press(&mut c, egui::Key::R);
        assert_eq!(c.tool(), Tool::RectCorner);
        press(&mut c, egui::Key::T);
        assert_eq!(c.modify(), Some(Modify::Trim));
        // B (old rectangle key) is unbound now.
        press(&mut c, egui::Key::B);
        assert_eq!(c.modify(), Some(Modify::Trim));
        c.set_tool(Tool::Select);
        let line = c
            .document()
            .entities()
            .iter()
            .find(|(_, e)| matches!(e.geom, Geom::Line { .. }) && e.construction)
            .map(|(id, _)| id);
        c.selection.items.set_all(line);
        let before = c.document().constraints().len();
        for k in [egui::Key::H, egui::Key::V, egui::Key::K, egui::Key::F] {
            press(&mut c, k);
        }
        assert_eq!(c.document().constraints().len(), before);
    }

    #[test]
    #[allow(clippy::unwrap_used)]
    fn delete_does_nothing_while_a_value_is_being_typed() {
        let mut c = AppState::default();
        c.apply_reporting(&Command::AddEntity {
            layer: c.active_layer(),
            construction: false,
            style: Default::default(),
            geom: Geom::Point { pos: Vec2::ZERO },
        });
        let p = c
            .document()
            .entities()
            .iter()
            .map(|(i, _)| i)
            .last()
            .unwrap();
        c.selection.items.set_all([p]);
        c.tools.typed = "12".to_owned();
        press(&mut c, egui::Key::Backspace);
        press(&mut c, egui::Key::Delete);
        assert!(c.document().entity(p).is_some());
    }

    #[test]
    fn undo_restores_the_document_exactly_and_redo_reapplies() {
        let mut c = AppState::default();
        let before = c.doc.sketcher.document().clone();
        let cmd = Command::AddEntity {
            layer: Document::default_layer(),
            construction: false,
            style: Default::default(),
            geom: Geom::Point {
                pos: Vec2::new(1.0, 2.0),
            },
        };
        assert!(c.apply(&cmd).is_ok());
        let after = c.doc.sketcher.document().clone();
        assert_ne!(after, before);
        assert_eq!(c.undo_label(), Some("Add entity"));
        assert_eq!(c.undo().as_deref(), Some("Add entity"));
        // IDs are never reused, so only the contents are compared.
        let same = |a: &Document, b: &Document| {
            a.entities() == b.entities() && a.constraints() == b.constraints()
        };
        assert!(same(c.doc.sketcher.document(), &before));
        assert_eq!(c.redo_label(), Some("Add entity"));
        c.redo();
        assert!(same(c.doc.sketcher.document(), &after));
        assert_eq!(c.doc.scene.items.len(), after.entities().len());
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
        let mut c = AppState::default();
        // Nothing selected: refused with what is needed.
        c.constrain(Kind::Parallel);
        assert!(
            c.doc
                .notice
                .as_deref()
                .is_some_and(|n| n.contains("two lines"))
        );
        let lines: Vec<_> = c
            .doc
            .sketcher
            .document()
            .entities()
            .iter()
            .filter(|(_, e)| matches!(e.geom, Geom::Line { .. }) && !e.construction)
            .map(|(id, _)| id)
            .collect();
        c.selection.items.set_all([lines[0]]);
        assert!(c.can_dimension());
        let before = c.doc.sketcher.document().constraints().len();
        // The demo's frame is fully constrained, so this is already implied.
        c.selection.items.set_all([lines[1], lines[3]]);
        c.constrain(Kind::Equal);
        assert!(
            c.doc
                .notice
                .as_deref()
                .is_some_and(|n| n.contains("implied"))
        );
        assert_eq!(c.doc.sketcher.document().constraints().len(), before);
        // The loose point can be fixed: one undo step.
        let free = c
            .doc
            .scene
            .items
            .iter()
            .find(|i| matches!(i.shape, crate::scene::Shape::Point(_)) && !i.owned_point)
            .map(|i| i.id);
        c.selection.items.set_all(free);
        c.constrain(Kind::Fix);
        assert_eq!(c.doc.sketcher.document().constraints().len(), before + 1);
        assert_eq!(c.undo_label(), Some("Add fix constraint"));
        c.undo();
        assert_eq!(c.doc.sketcher.document().constraints().len(), before);
    }

    #[test]
    fn grabbing_a_line_drags_its_nearer_end_and_undo_restores_the_geometry() {
        let mut c = AppState::default();
        let line = c
            .doc
            .sketcher
            .document()
            .entities()
            .iter()
            .find(|(_, e)| matches!(e.geom, Geom::Line { .. }) && e.construction)
            .map(|(id, _)| id)
            .unwrap_or(EntityId::new(0));
        let before = c.doc.sketcher.document().entities().clone();
        // The construction diagonal runs (20, 20) → (220, 140): grab near its end.
        assert!(c.begin_drag(line, Vec2::new(215.0, 138.0)));
        assert!(c.tools.dragging);
        let moved = c.doc.sketcher.drag_to(Vec2::new(200.0, 100.0));
        assert!(moved.is_ok());
        assert!(c.doc.sketcher.end_drag().is_ok());
        c.tools.dragging = false;
        c.rebuild();
        assert_ne!(c.doc.sketcher.document().entities(), &before);
        assert_eq!(c.undo_label(), Some("Drag"));
        c.undo();
        assert_eq!(c.doc.sketcher.document().entities(), &before);
    }

    #[test]
    fn a_refused_dimension_is_highlighted_and_can_be_added_as_driven() {
        let mut c = AppState::default();
        let circle = c
            .doc
            .sketcher
            .document()
            .entities()
            .iter()
            .find(|(_, e)| matches!(e.geom, Geom::Circle { .. }))
            .map(|(id, _)| id)
            .unwrap_or(EntityId::new(0));
        let before = c.doc.sketcher.document().constraints().len();
        // The demo's hole already has a diameter dimension.
        let spec = DimSpec::Diameter(circle);
        c.apply_reporting(&Command::AddConstraint {
            constraint: spec.constraint(30.0, false),
        });
        assert_eq!(c.doc.sketcher.document().constraints().len(), before);
        let Some(r) = c.doc.refusal.clone() else {
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
        assert!(c.doc.refusal.is_none());
        assert_eq!(c.doc.sketcher.document().constraints().len(), before + 1);
        let driven = c
            .doc
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
        let c = AppState::default();
        let st = c.doc.sketcher.state();
        assert_eq!(
            c.solve_summary(),
            format!("Under-constrained · DOF {}", st.dof)
        );
    }

    /// The tool host (design §7.2): a drawing tool commits one command per
    /// segment, and Esc cancels the step, then a
    /// second Esc returns to Select.
    #[test]
    fn the_tool_host_draws_a_line_and_esc_steps_back_to_select() {
        use crate::tool::{Flow, ToolEvent};
        use crate::tools::Pick;
        let mut c = AppState::default();
        let n = c.document().entities().len();
        c.set_tool(Tool::Line);
        let click = |c: &mut AppState, x: f64| {
            c.tool_event(
                ToolEvent::Click(Pick::free(Vec2::new(x, 300.0))),
                None,
                Units::Mm,
            )
        };
        assert_eq!(click(&mut c, 0.0), Flow::Continue);
        assert!(c.is_drawing());
        assert_eq!(click(&mut c, 40.0), Flow::Commit);
        assert!(c.document().entities().len() > n);
        assert!(c.undo_label().is_some());
        assert_eq!(c.tool_event(ToolEvent::Esc, None, Units::Mm), Flow::Cancel);
        assert_eq!(c.tool(), Tool::Line);
        assert_eq!(c.tool_event(ToolEvent::Esc, None, Units::Mm), Flow::Done);
        assert_eq!(c.tool(), Tool::Select);
    }

    /// Modify tools go through the same host; Esc leaves them for Select
    /// with the selection cleared, as before (SPEC §6.2).
    #[test]
    fn a_modify_tool_is_left_with_esc_and_keeps_polygon_options() {
        use crate::tool::ToolEvent;
        let mut c = AppState::default();
        c.set_tool(Tool::Polygon);
        c.tools.host.tool_mut().dialog_set_polygon_for_test(7, true);
        c.set_modify(Modify::Fillet);
        assert_eq!(c.modify(), Some(Modify::Fillet));
        assert_eq!(c.tool(), Tool::Select);
        c.select_all();
        c.tool_event(ToolEvent::Esc, None, Units::Mm);
        assert_eq!(c.modify(), None);
        assert!(c.selection.items.is_empty());
        c.set_tool(Tool::Polygon);
        assert_eq!(c.tools.host.tool().polygon(), Some((7, true)));
    }

    #[test]
    fn selection_does_not_touch_the_document() {
        let mut c = AppState::default();
        let before = c.doc.sketcher.document().clone();
        c.select_all();
        assert!(c.selection.items.len() > 1);
        c.clear_selection();
        assert_eq!(c.doc.sketcher.document(), &before);
    }
}
