// SPDX-License-Identifier: AGPL-3.0-or-later

//! The document and the constraint solver together (SPEC §3, §5, §10, §11;
//! ADR 0007; #105).
//!
//! `locus-solver` knows nothing about documents. This module lowers a
//! [`Document`] into a [`locus_solver::Sketch`] (keys are the document's own
//! IDs), runs the solver, and maps the results back. [`Sketcher`] is the
//! front end every client uses: an [`Editor`] plus one cached solver.
//!
//! - Every command is applied and solved as **one** undo step: the solver's
//!   moves are recorded with the command, so undo restores the geometry
//!   exactly (§11).
//! - A driving constraint that would conflict with, or be implied by, the
//!   existing ones is refused before anything changes, naming the
//!   constraints involved (§5 Solver UX).
//! - A command the solver can't satisfy is rolled back. Driven (reference)
//!   dimensions get their measured values written back.

use crate::boolean::BooleanError;
use crate::command::{Applied, Command, Editor, Solution};
use crate::constraint::{Constraint, G1Other, g1_joint};
use crate::document::Document;
use crate::entity::{EntityKind, Geom};
use crate::error::DocError;
use crate::ids::BooleanId;
use crate::ids::{ConstraintId, EntityId, LayerId, MAX_ID};
use locus_geom::Vec2;
use locus_solver::{self as ls, Check, Solver as _};
use std::collections::{BTreeMap, BTreeSet};

pub use locus_solver::Status;

/// Constraints that depend on each other, in document terms.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConstraintGroup {
    pub constraints: Vec<ConstraintId>,
    /// The points and curves those constraints touch.
    pub entities: Vec<EntityId>,
}

/// The effect locking a layer would have on constraints from other layers
/// (SPEC §4 rule 3). Structured so a UI can warn and list them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LockReport {
    /// Constraints that could no longer hold together once the layer is
    /// fixed. [`Sketcher::apply`] refuses the lock while any exist
    /// ([`DocError::Conflict`]).
    pub conflicts: Vec<ConstraintGroup>,
    /// Constraints the locked geometry makes redundant (over-constrained but
    /// consistent). The lock is allowed; the user is warned.
    pub redundant: Vec<ConstraintGroup>,
}

/// The sketch's solve status after the last change (§5 Solver UX).
#[derive(Debug, Clone, PartialEq)]
pub struct SolveState {
    /// Whether every driving constraint is satisfied.
    pub converged: bool,
    /// Remaining degrees of freedom of the whole sketch.
    pub dof: usize,
    pub status: Status,
    /// Points and curves that can no longer move (drawn in the
    /// fully-constrained colour).
    pub fully_constrained: BTreeSet<EntityId>,
    pub redundant: Vec<ConstraintGroup>,
    pub conflicts: Vec<ConstraintGroup>,
}

impl SolveState {
    fn unknown() -> Self {
        Self {
            converged: false,
            dof: 0,
            status: Status::UnderConstrained,
            fully_constrained: BTreeSet::new(),
            redundant: Vec::new(),
            conflicts: Vec::new(),
        }
    }
}

/// What one drag frame achieved (#71).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DragFrame {
    /// The dragged point reached the cursor. False when the constraints stop
    /// it (a fully constrained point doesn't move, §5): it then sits as near
    /// as they allow.
    pub at_target: bool,
    /// Every driving constraint held. When false the frame was **not**
    /// applied and the geometry is as it was after the last good frame.
    pub converged: bool,
}

/// A drag in progress: the cached solver input stays warm between frames, and
/// the state at the start is kept for undo and cancel.
#[derive(Debug)]
struct Drag {
    low: Lowered,
    point: ls::PointId,
    start: Solution,
}

/// An [`Editor`] that keeps the sketch solved. The way clients change a
/// document once solving matters.
#[derive(Debug)]
pub struct Sketcher {
    editor: Editor,
    // One solver per document: it caches factorisations between solves.
    solver: ls::LmSolver,
    state: SolveState,
    drag: Option<Drag>,
    // Derived, recomputed with the solve state (§6.3, #121).
    regions: Vec<locus_region::Region>,
    results: BTreeMap<BooleanId, Result<locus_region::Area, BooleanError>>,
}

impl Sketcher {
    /// Wrap a document and solve it once. The result is written back without
    /// an undo step. If it can't be solved (e.g. a file that was saved
    /// over-constrained), the stored positions are kept and [`Self::state`]
    /// says so (§10: files degrade gracefully).
    #[must_use]
    pub fn new(doc: Document) -> Self {
        let mut s = Self {
            editor: Editor::new(doc),
            solver: ls::LmSolver::new(),
            state: SolveState::unknown(),
            drag: None,
            regions: Vec::new(),
            results: BTreeMap::new(),
        };
        if let Ok(sol) = solve(&mut s.solver, s.editor.document()) {
            // Can only fail on a solver bug (non-finite output); the stored
            // positions are then kept, which is the safe fallback.
            let _ = s.editor.apply_solution_unrecorded(&sol);
        }
        s.refresh();
        s
    }

    #[must_use]
    pub fn document(&self) -> &Document {
        self.editor.document()
    }

    /// The editor, for history queries (undo/redo labels).
    #[must_use]
    pub fn editor(&self) -> &Editor {
        &self.editor
    }

    #[must_use]
    pub fn into_document(self) -> Document {
        self.editor.into_document()
    }

    #[must_use]
    pub fn state(&self) -> &SolveState {
        &self.state
    }

    /// Apply a command and re-solve, as one undo step.
    ///
    /// # Errors
    /// [`DocError::Conflict`] / [`DocError::Redundant`] when a driving
    /// constraint can't be added (or a dimension made driving);
    /// [`DocError::Unsolvable`] when the solver can't satisfy the result;
    /// otherwise whatever validation rejected. In every case the document and
    /// the history are unchanged.
    pub fn apply(&mut self, cmd: &Command) -> Result<Applied, DocError> {
        self.not_dragging()?;
        self.precheck(cmd)?;
        // A sketch that is already unsolvable (e.g. loaded that way) must
        // still accept edits, or the user could never repair it: keep stored
        // positions instead of refusing.
        let strict = self.state.converged;
        let solver = &mut self.solver;
        let out = self
            .editor
            .apply_solved(cmd, |doc| match solve(solver, doc) {
                Ok(sol) => Ok(sol),
                Err(e) if strict => Err(e),
                Err(_) => Ok(Solution::default()),
            })?;
        self.refresh();
        Ok(out)
    }

    /// Set (or clear) what re-runs modifier plugins when their sources
    /// change (SPEC §15.2, #184).
    pub fn set_plugin_runner(
        &mut self,
        runner: Option<std::sync::Arc<dyn crate::generated::PluginRunner>>,
    ) {
        self.editor.set_plugin_runner(runner);
    }

    /// Undo the last step. Positions come back exactly from the history, so
    /// no solve is needed.
    ///
    /// # Errors
    /// [`DocError::NothingToDo`] when there is nothing to undo.
    pub fn undo(&mut self) -> Result<String, DocError> {
        self.not_dragging()?;
        let label = self.editor.undo()?;
        self.refresh();
        Ok(label)
    }

    /// Redo the last undone step.
    ///
    /// # Errors
    /// [`DocError::NothingToDo`] when there is nothing to redo.
    pub fn redo(&mut self) -> Result<String, DocError> {
        self.not_dragging()?;
        let label = self.editor.redo()?;
        self.refresh();
        Ok(label)
    }

    fn not_dragging(&self) -> Result<(), DocError> {
        if self.drag.is_some() {
            Err(DocError::Invalid("a drag is in progress".to_owned()))
        } else {
            Ok(())
        }
    }

    // ── Dragging (§3, §5, #71) ───────────────────────────────────────────

    /// Whether a drag is in progress.
    #[must_use]
    pub fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// Start dragging a point. Frames ([`Self::drag_to`]) move the geometry
    /// without recording history; [`Self::end_drag`] records the whole drag
    /// as one undo step and [`Self::cancel_drag`] puts everything back.
    /// While dragging, [`Self::apply`], [`Self::undo`] and [`Self::redo`]
    /// are refused, and [`Self::state`] describes the sketch as it was
    /// before the drag.
    ///
    /// # Errors
    /// [`DocError::UnknownEntity`] / [`DocError::WrongKind`] when `point`
    /// isn't a point; [`DocError::Invalid`] when a drag is already running.
    pub fn begin_drag(&mut self, point: EntityId) -> Result<(), DocError> {
        self.not_dragging()?;
        let doc = self.editor.document();
        doc.check_not_driven(point)?;
        match doc.entity(point).map(|e| e.geom.kind()) {
            None => return Err(DocError::UnknownEntity(point)),
            Some(EntityKind::Point) => {}
            Some(found) => {
                return Err(DocError::WrongKind {
                    what: "drag",
                    entity: point,
                    expected: "point",
                    found,
                });
            }
        }
        let low = lower(doc)?;
        let pid = low.point(point)?;
        self.drag = Some(Drag {
            start: Solution::capture(doc),
            low,
            point: pid,
        });
        Ok(())
    }

    /// One drag frame: pull the point towards `to`, re-solving from the
    /// previous frame with everything else kept as still as the constraints
    /// allow (§3). Nothing is recorded.
    ///
    /// # Errors
    /// [`DocError::Invalid`] when no drag is running or `to` isn't finite.
    pub fn drag_to(&mut self, to: Vec2) -> Result<DragFrame, DocError> {
        let Some(drag) = self.drag.as_mut() else {
            return Err(DocError::Invalid("no drag in progress".to_owned()));
        };
        let report = self
            .solver
            .drag(&mut drag.low.sketch, drag.point, to)
            .map_err(solver_err)?;
        let frame = DragFrame {
            at_target: report.at_target,
            converged: report.converged,
        };
        if !report.converged {
            return Ok(frame);
        }
        let sol = Solution {
            points: drag
                .low
                .points
                .iter()
                .zip(&drag.low.sketch.points)
                .map(|(id, p)| (*id, p.at))
                .collect(),
            radii: radii(&drag.low),
            driven: edge_driven(self.editor.document(), &report.driven),
        };
        self.editor.apply_solution_unrecorded(&sol)?;
        Ok(frame)
    }

    /// Finish the drag: everything it changed becomes **one** undo step, so
    /// undo restores the pre-drag geometry exactly. Does nothing if the
    /// geometry didn't move.
    ///
    /// # Errors
    /// [`DocError::Invalid`] when no drag is running.
    pub fn end_drag(&mut self) -> Result<(), DocError> {
        let drag = self
            .drag
            .take()
            .ok_or_else(|| DocError::Invalid("no drag in progress".to_owned()))?;
        self.editor.commit_from("Drag", &drag.start)?;
        // Offsets follow the dragged geometry in the same undo step (#164).
        self.editor.regenerate_into_last()?;
        self.refresh();
        Ok(())
    }

    /// Abandon the drag (Esc): the geometry goes back to the start, exactly.
    pub fn cancel_drag(&mut self) {
        if let Some(drag) = self.drag.take() {
            // Restoring values that were valid a moment ago can't fail; if it
            // somehow did, the next solve re-derives the geometry anyway.
            let _ = self.editor.apply_solution_unrecorded(&drag.start);
        }
    }

    /// What locking `layer` would do to the constraints that reach into it
    /// (SPEC §4 rule 3): its geometry becomes fixed, so a constraint on
    /// geometry that stays free may stop holding (`conflicts`) or stop
    /// restricting anything (`redundant`). Only constraints touching at
    /// least one entity on a layer that stays unlocked are listed, and only
    /// groups the lock itself causes. Nothing is changed.
    ///
    /// # Errors
    /// [`DocError::UnknownLayer`], or a solver-input error.
    pub fn check_lock(&mut self, layer: LayerId) -> Result<LockReport, DocError> {
        let doc = self.editor.document();
        if doc.layer(layer).is_none() {
            return Err(DocError::UnknownLayer(layer));
        }
        let low = lower_locking(doc, Some(layer))?;
        let a = self.solver.analyse(&low.sketch).map_err(solver_err)?;
        let stays_free = |id: &ConstraintId| {
            doc.constraint(*id).is_some_and(|c| {
                c.references().iter().any(|e| {
                    doc.entity(*e).is_some_and(|e| {
                        e.layer != layer && !doc.layer(e.layer).is_some_and(|l| l.locked)
                    })
                })
            })
        };
        let groups = |sets: &[ls::ConstraintSet]| -> Vec<ConstraintGroup> {
            sets.iter()
                .filter(|s| has_lock(s))
                .map(|s| ConstraintGroup {
                    constraints: s
                        .constraints
                        .iter()
                        .filter_map(|k| doc_constraint(*k))
                        .filter(|id| stays_free(id))
                        .collect(),
                    entities: entities(s),
                })
                .filter(|g| !g.constraints.is_empty())
                .collect()
        };
        Ok(LockReport {
            conflicts: groups(&a.conflicts),
            redundant: groups(&a.redundant),
        })
    }

    /// Refuse a driving constraint that can't add a real restriction, before
    /// anything changes. Only single commands are pre-checked; a compound is
    /// guarded by the solve that follows it.
    fn precheck(&mut self, cmd: &Command) -> Result<(), DocError> {
        // A dimension entered as an expression is checked as the plain
        // dimension it will become.
        if let Command::AddDimensionExpr { constraint, expr } = cmd {
            let doc = self.editor.document();
            let v = crate::expr::evaluate(expr, doc.parameters())?;
            let mut c = constraint.clone();
            let value = crate::params::dimension_value(&c, v);
            if let Some(d) = c.dimension_mut() {
                d.value = value;
                d.driven = false;
            }
            return self.precheck(&Command::AddConstraint { constraint: c });
        }
        if let Command::SetLayerLocked { id, locked: true } = cmd {
            let report = self.check_lock(*id)?;
            if report.conflicts.is_empty() {
                return Ok(());
            }
            let mut constraints = Vec::new();
            let mut entities = Vec::new();
            for g in &report.conflicts {
                constraints.extend(&g.constraints);
                entities.extend(&g.entities);
            }
            constraints.sort_unstable();
            constraints.dedup();
            entities.sort_unstable();
            entities.dedup();
            return Err(DocError::Conflict {
                constraints,
                entities,
            });
        }
        let doc = self.editor.document();
        let check = match cmd {
            Command::AddConstraint { constraint } => {
                if constraint.dimension().is_some_and(|d| d.driven) {
                    return Ok(());
                }
                constraint.validate(doc)?;
                let low = lower(doc)?;
                let kind = low.constraint_kind(doc, constraint)?;
                // `next_id` is not yet allocated, so it can't clash with a key.
                let probe = ls::Constraint {
                    key: ls::Key(doc.next_id()),
                    kind,
                    driven: false,
                };
                self.solver
                    .check_add(&low.sketch, probe)
                    .map_err(solver_err)?
            }
            Command::SetDriven { id, driven: false } => {
                let c = doc
                    .constraint(*id)
                    .ok_or(DocError::UnknownConstraint(*id))?;
                if !c.dimension().is_some_and(|d| d.driven) {
                    return Ok(());
                }
                let low = lower(doc)?;
                let idx = low
                    .constraints
                    .iter()
                    .position(|c| c == id)
                    .ok_or(DocError::UnknownConstraint(*id))?;
                let sid = ls::ConstraintId(u32::try_from(idx).map_err(|_| too_big())?);
                self.solver
                    .check_driving(&low.sketch, sid)
                    .map_err(solver_err)?
            }
            _ => return Ok(()),
        };
        match check {
            Check::Ok => Ok(()),
            Check::Conflict(set) => Err(DocError::Conflict {
                constraints: existing(&set),
                entities: entities(&set),
            }),
            Check::Redundant(set) => Err(DocError::Redundant {
                constraints: existing(&set),
                entities: entities(&set),
            }),
        }
    }

    fn refresh(&mut self) {
        self.state = analyse(&mut self.solver, self.editor.document());
        let doc = self.editor.document();
        self.regions = crate::regions::regions(doc);
        self.results = doc
            .booleans()
            .iter()
            .map(|(id, b)| (*id, b.evaluate(&self.regions)))
            .collect();
    }

    /// Every boolean recipe's result (§6.3), as of the last solve: the exact
    /// area, or why there is none (a source region is gone).
    #[must_use]
    pub fn boolean_results(
        &self,
    ) -> &BTreeMap<BooleanId, Result<locus_region::Area, BooleanError>> {
        &self.results
    }

    /// The sketch's closed regions (§6.3), as of the last solve. Not updated
    /// during a drag: they catch up when it ends.
    #[must_use]
    pub fn regions(&self) -> &[locus_region::Region] {
        &self.regions
    }
}

/// A document lowered into solver input, with the maps back to document IDs.
#[derive(Debug)]
struct Lowered {
    sketch: ls::Sketch,
    point_index: BTreeMap<EntityId, ls::PointId>,
    curve_index: BTreeMap<EntityId, ls::CurveId>,
    points: Vec<EntityId>,
    curves: Vec<EntityId>,
    constraints: Vec<ConstraintId>,
}

fn key(raw: u64) -> ls::Key {
    ls::Key(raw)
}

fn too_big() -> DocError {
    DocError::Solver("sketch too large for the solver's index type".to_owned())
}

fn solver_err(e: ls::SketchError) -> DocError {
    DocError::Solver(e.to_string())
}

fn lower(doc: &Document) -> Result<Lowered, DocError> {
    lower_locking(doc, None)
}

/// Keys at or above this belong to the lock constraints [`lower_locking`]
/// adds (document IDs stay below `MAX_ID`, ADR-0003); they never reach a
/// caller as a [`ConstraintId`].
const LOCK_KEY_BASE: u64 = MAX_ID;

/// The document constraint a solver key stands for; `None` for a lock
/// constraint.
fn doc_constraint(k: ls::Key) -> Option<ConstraintId> {
    (k.0 < LOCK_KEY_BASE).then(|| ConstraintId::new(k.0))
}

fn has_lock(set: &ls::ConstraintSet) -> bool {
    set.constraints.iter().any(|k| doc_constraint(*k).is_none())
}

/// [`lower`], additionally treating `also_locked` as locked (the pre-lock
/// check, SPEC §4 rule 3).
fn lower_locking(doc: &Document, also_locked: Option<LayerId>) -> Result<Lowered, DocError> {
    let mut low = Lowered {
        sketch: ls::Sketch::default(),
        point_index: BTreeMap::new(),
        curve_index: BTreeMap::new(),
        points: Vec::new(),
        curves: Vec::new(),
        constraints: Vec::new(),
    };
    // Offset instances are written after the solve, not solved (#164).
    let unsolved = doc.unsolved_instances();
    // Points first: curves refer to them by index.
    for (id, e) in doc.entities().iter() {
        if unsolved.contains(&id) {
            continue;
        }
        if let Geom::Point { pos } = e.geom {
            let pid = ls::PointId(u32::try_from(low.points.len()).map_err(|_| too_big())?);
            low.sketch.points.push(ls::Point {
                key: key(id.get()),
                at: pos,
            });
            low.point_index.insert(id, pid);
            low.points.push(id);
        }
    }
    for (id, e) in doc.entities().iter() {
        if unsolved.contains(&id) {
            continue;
        }
        let kind = match &e.geom {
            // A Bézier's points are ordinary solver points; its only
            // constraints (G1 tangency) lower to point constraints below.
            Geom::Point { .. } | Geom::Bezier { .. } | Geom::Text { .. } => continue,
            Geom::Line { start, end } => ls::CurveKind::Line {
                a: low.point(*start)?,
                b: low.point(*end)?,
            },
            Geom::Circle { center, radius } => ls::CurveKind::Circle {
                centre: low.point(*center)?,
                r: *radius,
            },
            Geom::Arc { center, start, end } => ls::CurveKind::Arc {
                centre: low.point(*center)?,
                a: low.point(*start)?,
                b: low.point(*end)?,
            },
        };
        let cid = ls::CurveId(u32::try_from(low.curves.len()).map_err(|_| too_big())?);
        low.sketch.curves.push(ls::Curve {
            key: key(id.get()),
            kind,
        });
        low.curve_index.insert(id, cid);
        low.curves.push(id);
    }
    for (id, c) in doc.constraints() {
        let kind = low.constraint_kind(doc, c)?;
        low.sketch.constraints.push(ls::Constraint {
            key: key(id.get()),
            kind,
            driven: c.dimension().is_some_and(|d| d.driven),
        });
        low.constraints.push(*id);
    }
    // SPEC §4 rule 3: geometry on a locked layer is fixed. These constraints
    // are appended last, so document-constraint indices are unchanged.
    let mut pinned = BTreeSet::new();
    for (id, e) in doc.entities().iter() {
        if unsolved.contains(&id) {
            continue;
        }
        let locked = also_locked == Some(e.layer) || doc.layer(e.layer).is_some_and(|l| l.locked);
        if !locked {
            continue;
        }
        pinned.extend(e.geom.point_refs());
        match e.geom {
            Geom::Point { .. } => {
                pinned.insert(id);
            }
            Geom::Circle { radius, .. } => {
                let c = low.curve(id)?;
                low.sketch.constraints.push(ls::Constraint {
                    key: key(LOCK_KEY_BASE + id.get()),
                    kind: ls::ConstraintKind::Radius { c, r: radius },
                    driven: false,
                });
            }
            Geom::Line { .. } | Geom::Arc { .. } | Geom::Bezier { .. } | Geom::Text { .. } => {}
        }
    }
    for id in pinned {
        let p = low.point(id)?;
        let at = doc.point_pos(id).ok_or(DocError::UnknownEntity(id))?;
        low.sketch.constraints.push(ls::Constraint {
            key: key(LOCK_KEY_BASE + id.get()),
            kind: ls::ConstraintKind::Fix { p, at },
            driven: false,
        });
    }
    Ok(low)
}

impl Lowered {
    fn point(&self, id: EntityId) -> Result<ls::PointId, DocError> {
        self.point_index
            .get(&id)
            .copied()
            .ok_or(DocError::UnknownEntity(id))
    }

    fn curve(&self, id: EntityId) -> Result<ls::CurveId, DocError> {
        self.curve_index
            .get(&id)
            .copied()
            .ok_or(DocError::UnknownEntity(id))
    }

    /// The solver's explicit constraint for a document constraint. The
    /// document's generic `Equal` and `Distance` dispatch on entity kinds.
    fn constraint_kind(
        &self,
        doc: &Document,
        c: &Constraint,
    ) -> Result<ls::ConstraintKind, DocError> {
        use ls::ConstraintKind as K;
        let kind_of = |id: EntityId| {
            doc.entity(id)
                .map(|e| e.geom.kind())
                .ok_or(DocError::UnknownEntity(id))
        };
        // The centreline value: the dimension plus its stroke-edge offset.
        let centre = doc.edge_offset(c);
        let value = |c: &Constraint| c.dimension().map_or(0.0, |d| d.value + centre);
        if let Some(d) = c.dimension()
            && !d.driven
            && centre != 0.0
        {
            let v = d.value + centre;
            let bad = match c {
                Constraint::Distance { .. } => v < 0.0,
                _ => v <= 0.0,
            };
            if bad {
                return Err(DocError::Invalid(format!(
                    "a {} dimension of {} mm needs a negative centreline value; \
                     adjust the dimension or the layer's pen width / kerf",
                    d.edge.name(),
                    d.value
                )));
            }
        }
        Ok(match c {
            Constraint::Coincident { a, b } => K::Coincident(self.point(*a)?, self.point(*b)?),
            Constraint::PointOnCurve { point, curve } => {
                K::PointOnCurve(self.point(*point)?, self.curve(*curve)?)
            }
            Constraint::Horizontal { line } => K::Horizontal(self.curve(*line)?),
            Constraint::Vertical { line } => K::Vertical(self.curve(*line)?),
            Constraint::Parallel { a, b } => K::Parallel(self.curve(*a)?, self.curve(*b)?),
            Constraint::Perpendicular { a, b } => {
                K::Perpendicular(self.curve(*a)?, self.curve(*b)?)
            }
            Constraint::Tangent { a, b }
                if kind_of(*a)? == EntityKind::Bezier || kind_of(*b)? == EntityKind::Bezier =>
            {
                // G1 at the shared end: the Bézier's handle leaves along the
                // other curve's tangent (SPEC §4).
                let g = g1_joint(doc, *a, *b).ok_or_else(|| {
                    DocError::Invalid("tangent Bézier shares no end point".to_owned())
                })?;
                let (h, j) = (self.point(g.handle)?, self.point(g.joint)?);
                match g.other {
                    G1Other::LineFar(o) | G1Other::Handle(o) => K::Aligned {
                        a: h,
                        p: j,
                        b: self.point(o)?,
                    },
                    G1Other::ArcCentre(c) => K::RightAngle {
                        a: h,
                        p: j,
                        b: self.point(c)?,
                    },
                }
            }
            Constraint::Tangent { a, b } => K::Tangent(self.curve(*a)?, self.curve(*b)?),
            Constraint::Collinear { a, b } => K::Collinear(self.curve(*a)?, self.curve(*b)?),
            Constraint::Concentric { a, b } => K::Concentric(self.curve(*a)?, self.curve(*b)?),
            Constraint::Equal { a, b } => {
                if kind_of(*a)? == EntityKind::Line {
                    K::EqualLength(self.curve(*a)?, self.curve(*b)?)
                } else {
                    K::EqualRadius(self.curve(*a)?, self.curve(*b)?)
                }
            }
            Constraint::Symmetric { a, b, axis } => K::Symmetric {
                p: self.point(*a)?,
                q: self.point(*b)?,
                about: self.curve(*axis)?,
            },
            Constraint::Midpoint { point, line } => K::Midpoint {
                p: self.point(*point)?,
                line: self.curve(*line)?,
            },
            // Document Fix means "stay where it is now".
            Constraint::Fix { point } => K::Fix {
                p: self.point(*point)?,
                at: doc
                    .point_pos(*point)
                    .ok_or(DocError::UnknownEntity(*point))?,
            },
            Constraint::Distance { a, b, .. } => {
                let d = value(c);
                match (kind_of(*a)?, kind_of(*b)?) {
                    (EntityKind::Point, EntityKind::Point) => K::Distance {
                        p: self.point(*a)?,
                        q: self.point(*b)?,
                        d,
                    },
                    (EntityKind::Point, _) => K::PointLineDistance {
                        p: self.point(*a)?,
                        line: self.curve(*b)?,
                        d,
                    },
                    (_, EntityKind::Point) => K::PointLineDistance {
                        p: self.point(*b)?,
                        line: self.curve(*a)?,
                        d,
                    },
                    _ => K::LineLineDistance {
                        l: self.curve(*a)?,
                        m: self.curve(*b)?,
                        d,
                    },
                }
            }
            Constraint::HorizontalDistance { a, b, .. } => K::HorizontalDistance {
                p: self.point(*a)?,
                q: self.point(*b)?,
                d: value(c),
            },
            Constraint::VerticalDistance { a, b, .. } => K::VerticalDistance {
                p: self.point(*a)?,
                q: self.point(*b)?,
                d: value(c),
            },
            Constraint::Angle { a, b, .. } => K::Angle {
                l: self.curve(*a)?,
                m: self.curve(*b)?,
                rad: value(c),
            },
            Constraint::Radius { curve, .. } => K::Radius {
                c: self.curve(*curve)?,
                r: value(c),
            },
            Constraint::Diameter { curve, .. } => K::Diameter {
                c: self.curve(*curve)?,
                d: value(c),
            },
        })
    }
}

/// Measured values of driven dimensions, from the solver's centreline
/// measurement to the dimension's own edge (SPEC §5, ADR 0014).
fn edge_driven(doc: &Document, driven: &[(ls::Key, f64)]) -> Vec<(ConstraintId, f64)> {
    driven
        .iter()
        .map(|(k, v)| {
            let id = ConstraintId::new(k.0);
            let off = doc.constraint(id).map_or(0.0, |c| doc.edge_offset(c));
            (id, v - off)
        })
        .collect()
}

/// Circle radii from the solver's sketch.
fn radii(low: &Lowered) -> Vec<(EntityId, f64)> {
    low.curves
        .iter()
        .zip(&low.sketch.curves)
        .filter_map(|(id, c)| match c.kind {
            ls::CurveKind::Circle { r, .. } => Some((*id, r)),
            _ => None,
        })
        .collect()
}

/// Solve the document and return the result in document terms.
fn solve(solver: &mut ls::LmSolver, doc: &Document) -> Result<Solution, DocError> {
    let mut low = lower(doc)?;
    let report = solver.solve(&mut low.sketch).map_err(solver_err)?;
    if !report.converged {
        return Err(DocError::Unsolvable {
            unsatisfied: report
                .unsatisfied
                .iter()
                .filter_map(|k| doc_constraint(*k))
                .collect(),
        });
    }
    let points = low
        .points
        .iter()
        .zip(&low.sketch.points)
        .map(|(id, p)| (*id, p.at))
        .collect();
    let radii = radii(&low);
    let driven = edge_driven(doc, &report.driven);
    Ok(Solution {
        points,
        radii,
        driven,
    })
}

fn analyse(solver: &mut ls::LmSolver, doc: &Document) -> SolveState {
    let Ok(low) = lower(doc) else {
        return SolveState::unknown();
    };
    let Ok(a) = solver.analyse(&low.sketch) else {
        return SolveState::unknown();
    };
    let fully_constrained = low
        .points
        .iter()
        .zip(&a.points)
        .chain(low.curves.iter().zip(&a.curves))
        .filter(|(_, f)| f.is_fixed())
        .map(|(id, _)| *id)
        .collect();
    let group = |s: &ls::ConstraintSet| ConstraintGroup {
        constraints: s
            .constraints
            .iter()
            .filter_map(|k| doc_constraint(*k))
            .collect(),
        entities: entities(s),
    };
    SolveState {
        converged: a.converged,
        dof: a.dof,
        status: a.status,
        fully_constrained,
        redundant: a
            .redundant
            .iter()
            .filter(|s| !has_lock(s))
            .map(group)
            .collect(),
        conflicts: a
            .conflicts
            .iter()
            .map(group)
            .filter(|g| !g.constraints.is_empty())
            .collect(),
    }
}

/// The existing constraints of a pre-check set (the probe itself excluded).
fn existing(set: &ls::ConstraintSet) -> Vec<ConstraintId> {
    set.constraints
        .iter()
        .filter(|k| **k != set.constraint)
        .filter_map(|k| doc_constraint(*k))
        .collect()
}

fn entities(set: &ls::ConstraintSet) -> Vec<EntityId> {
    set.points
        .iter()
        .chain(&set.curves)
        .map(|k| EntityId::new(k.0))
        .collect()
}
