// SPDX-License-Identifier: AGPL-3.0-or-later

//! Trim, extend and break (SPEC §6.2), planned as one compound command each.
//!
//! Each function reads the document and returns a [`Plan`]: the command to
//! run (one undo step) and the constraints it removes. Nothing is dropped
//! silently (SPEC §4 rule 5): constraints are kept on the piece that keeps the
//! entity's ID, moved to its replacement when a circle becomes arcs, and
//! otherwise listed in [`Plan::dropped`] for the UI to report.
//!
//! - **Trim** removes the piece of a line, arc or circle between the
//!   intersections either side of the pick.
//! - **Break** splits it at those intersections and keeps every piece.
//! - **Extend** moves a free end of a line or arc to the next curve it meets.
//!
//! Cut points get a point-on-curve constraint to the curve that cut them, or
//! reuse a point entity already there, so the result stays connected when it
//! re-solves. Pieces of one line stay collinear; pieces of one arc share its
//! centre point and radius.

use crate::command::Command;
use crate::constraint::Constraint;
use crate::document::Document;
use crate::entity::{EntityKind, Geom};
use crate::ids::{ConstraintId, EntityId, LayerId};
use core::f64::consts::TAU;
use locus_geom::{Arc, Circle, EXPORT_TOL, Line, Piece, SOLVER_TOL, Vec2};
use std::collections::BTreeSet;
use thiserror::Error;

/// A planned modify operation.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    /// One compound: a single undo step.
    pub command: Command,
    /// Constraints the command removes and does not re-create, for the UI to
    /// report (SPEC §4 rule 5).
    pub dropped: Vec<ConstraintId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ModifyError {
    #[error("unknown entity {0:?}")]
    UnknownEntity(EntityId),
    #[error("{0:?} entities can't be trimmed, extended or broken yet")]
    Unsupported(EntityKind),
    #[error("nothing crosses this curve, so there is nowhere to break it")]
    NoIntersections,
    #[error("no curve to extend to in that direction")]
    NoBoundary,
    #[error("this end is joined to other geometry; only a free end can be extended")]
    EndConnected,
}

/// The curve being modified, parametrised by `s`: `t` ∈ [0, 1] along a line,
/// the CCW angle from the start ∈ [0, sweep] on an arc, the angle ∈ [0, 2π)
/// on a circle.
#[derive(Debug, Clone, Copy)]
enum Target {
    Line(Line),
    Arc(Arc),
    Circle(Circle),
}

impl Target {
    fn of(doc: &Document, id: EntityId) -> Result<Self, ModifyError> {
        let e = doc.entity(id).ok_or(ModifyError::UnknownEntity(id))?;
        let kind = e.geom.kind();
        match kind {
            EntityKind::Line => doc.line(id).map(Target::Line),
            EntityKind::Arc => doc.arc(id).map(Target::Arc),
            EntityKind::Circle => doc.circle(id).map(Target::Circle),
            _ => None,
        }
        .ok_or(ModifyError::Unsupported(kind))
    }

    fn center(&self) -> Vec2 {
        match self {
            Target::Line(l) => l.a,
            Target::Arc(a) => a.center,
            Target::Circle(c) => c.center,
        }
    }

    fn radius(&self) -> f64 {
        match self {
            Target::Line(_) => 0.0,
            Target::Arc(a) => a.radius(),
            Target::Circle(c) => c.radius,
        }
    }

    fn start_angle(&self) -> f64 {
        match self {
            Target::Arc(a) => a.start_angle(),
            _ => 0.0,
        }
    }

    /// Length per unit of `s`.
    fn scale(&self) -> f64 {
        match self {
            Target::Line(l) => l.length(),
            _ => self.radius(),
        }
    }

    /// The parameter range of the entity itself.
    fn span(&self) -> f64 {
        match self {
            Target::Line(_) => 1.0,
            Target::Arc(a) => a.sweep(),
            Target::Circle(_) => TAU,
        }
    }

    fn param(&self, p: Vec2) -> f64 {
        match self {
            Target::Line(l) => {
                let d = l.b - l.a;
                (p - l.a).dot(d) / d.length_sq().max(f64::MIN_POSITIVE)
            }
            _ => ((p - self.center()).angle() - self.start_angle()).rem_euclid(TAU),
        }
    }

    fn at(&self, s: f64) -> Vec2 {
        match self {
            Target::Line(l) => l.eval(s),
            _ => self.center() + Vec2::from_angle(self.start_angle() + s) * self.radius(),
        }
    }

    /// Where the target's carrier (whole line or whole circle) meets `piece`.
    fn meets(&self, piece: &Piece) -> Vec<Vec2> {
        match self {
            Target::Line(l) => {
                // A segment long enough to stand in for the whole line.
                let Some(u) = l.tangent() else {
                    return Vec::new();
                };
                let far = 1e7;
                let carrier = Line::new(l.a - u * far, l.a + u * far);
                match piece {
                    Piece::Line(o) => carrier.intersect_line(o).into_iter().collect(),
                    Piece::Arc(o) => carrier.intersect_arc(o),
                }
            }
            _ => {
                let c = Circle::new(self.center(), self.radius());
                match piece {
                    Piece::Line(o) => c.intersect_line(o),
                    Piece::Arc(o) => c.intersect_arc(o),
                }
            }
        }
    }
}

/// Every other curve as lines and arcs, with its entity. Circles are two
/// half arcs; Béziers their biarc pieces within `EXPORT_TOL`.
fn cutters(doc: &Document, skip: EntityId) -> Vec<(EntityId, Piece)> {
    let mut out = Vec::new();
    for (id, e) in doc.entities().iter() {
        if id == skip {
            continue;
        }
        match e.geom.kind() {
            EntityKind::Line => out.extend(doc.line(id).map(|l| (id, Piece::Line(l)))),
            EntityKind::Arc => out.extend(doc.arc(id).map(|a| (id, Piece::Arc(a)))),
            EntityKind::Circle => {
                if let Some(c) = doc.circle(id) {
                    let (e, w) = (Vec2::new(c.radius, 0.0), Vec2::new(-c.radius, 0.0));
                    out.push((
                        id,
                        Piece::Arc(Arc::new(c.center, c.center + e, c.center + w)),
                    ));
                    out.push((
                        id,
                        Piece::Arc(Arc::new(c.center, c.center + w, c.center + e)),
                    ));
                }
            }
            EntityKind::Bezier => {
                if let Some(c) = doc.cubic(id) {
                    out.extend(c.to_pieces(EXPORT_TOL).into_iter().map(|p| (id, p)));
                }
            }
            EntityKind::Point => {}
        }
    }
    out
}

/// A cut on the target: its parameter, position and the entity that cut it.
#[derive(Debug, Clone, Copy)]
struct Cut {
    s: f64,
    pos: Vec2,
    by: EntityId,
}

/// Intersections of the target's carrier with every other curve, sorted by
/// parameter, duplicates (within `SOLVER_TOL`) merged.
fn cuts(doc: &Document, id: EntityId, t: &Target) -> Vec<Cut> {
    let mut out: Vec<Cut> = cutters(doc, id)
        .iter()
        .flat_map(|(by, p)| {
            t.meets(p).into_iter().map(|pos| Cut {
                s: t.param(pos),
                pos,
                by: *by,
            })
        })
        .filter(|c| c.s.is_finite())
        .collect();
    out.sort_by(|a, b| a.s.total_cmp(&b.s));
    let tol = SOLVER_TOL / t.scale().max(SOLVER_TOL);
    out.dedup_by(|b, a| (b.s - a.s).abs() <= tol);
    out
}

/// One side of a piece: an existing point entity or a cut.
#[derive(Debug, Clone, Copy)]
enum End {
    Point(EntityId),
    Cut(Cut),
}

/// Builds the compound, predicting the IDs the document will assign (it
/// hands them out in order, ADR 0003).
struct Builder<'a> {
    doc: &'a Document,
    next: u64,
    commands: Vec<Command>,
    layer: LayerId,
    construction: bool,
}

impl<'a> Builder<'a> {
    fn new(doc: &'a Document, of: EntityId) -> Self {
        let (layer, construction) = doc
            .entity(of)
            .map_or((Document::default_layer(), false), |e| {
                (e.layer, e.construction)
            });
        Self {
            doc,
            next: doc.next_id(),
            commands: Vec::new(),
            layer,
            construction,
        }
    }

    fn alloc(&mut self) -> u64 {
        let id = self.next;
        self.next += 1;
        id
    }

    fn add(&mut self, geom: Geom) -> EntityId {
        self.commands.push(Command::AddEntity {
            layer: self.layer,
            construction: self.construction,
            geom,
        });
        EntityId::new(self.alloc())
    }

    fn constrain(&mut self, constraint: Constraint) {
        self.commands.push(Command::AddConstraint { constraint });
        self.alloc();
    }

    /// The point for an end: an existing point entity at a cut is reused
    /// (shared identity); otherwise a new point held on the cutting curve.
    fn end(&mut self, end: End, made: &mut Vec<(Vec2, EntityId)>) -> EntityId {
        match end {
            End::Point(p) => p,
            End::Cut(c) => {
                if let Some(&(_, p)) = made.iter().find(|(q, _)| q.distance(c.pos) <= SOLVER_TOL) {
                    return p;
                }
                let existing = self.doc.entities().iter().find_map(|(id, e)| match e.geom {
                    Geom::Point { pos } if pos.distance(c.pos) <= SOLVER_TOL => Some(id),
                    _ => None,
                });
                let p = existing.unwrap_or_else(|| {
                    let p = self.add(Geom::Point { pos: c.pos });
                    let kind = self.doc.entity(c.by).map(|e| e.geom.kind());
                    // Point-on-curve takes lines, circles and arcs (not
                    // Béziers): the cut is held where it is by the solve.
                    if matches!(
                        kind,
                        Some(EntityKind::Line | EntityKind::Circle | EntityKind::Arc)
                    ) {
                        self.constrain(Constraint::PointOnCurve {
                            point: p,
                            curve: c.by,
                        });
                    }
                    p
                });
                made.push((c.pos, p));
                p
            }
        }
    }

    fn finish(self, label: &str, dropped: Vec<ConstraintId>) -> Plan {
        Plan {
            command: Command::Compound {
                label: label.to_owned(),
                commands: self.commands,
            },
            dropped,
        }
    }
}

/// Replaces `entity` by `with` in a constraint.
fn retarget(c: &Constraint, entity: EntityId, with: EntityId) -> Constraint {
    let mut c = c.clone();
    let swap = |x: &mut EntityId| {
        if *x == entity {
            *x = with;
        }
    };
    match &mut c {
        Constraint::Coincident { a, b }
        | Constraint::Parallel { a, b }
        | Constraint::Perpendicular { a, b }
        | Constraint::Tangent { a, b }
        | Constraint::Collinear { a, b }
        | Constraint::Concentric { a, b }
        | Constraint::Equal { a, b }
        | Constraint::Distance { a, b, .. }
        | Constraint::HorizontalDistance { a, b, .. }
        | Constraint::VerticalDistance { a, b, .. }
        | Constraint::Angle { a, b, .. } => {
            swap(a);
            swap(b);
        }
        Constraint::PointOnCurve { point, curve } => {
            swap(point);
            swap(curve);
        }
        Constraint::Horizontal { line } | Constraint::Vertical { line } => swap(line),
        Constraint::Symmetric { a, b, axis } => {
            swap(a);
            swap(b);
            swap(axis);
        }
        Constraint::Midpoint { point, line } => {
            swap(point);
            swap(line);
        }
        Constraint::Fix { point } => swap(point),
        Constraint::Radius { curve, .. } | Constraint::Diameter { curve, .. } => swap(curve),
    }
    c
}

/// Constraints on a line that depend on its extent, which a trim or break
/// changes: its midpoint and equal-length.
fn extent_bound(doc: &Document, c: &Constraint, line: EntityId) -> bool {
    match c {
        Constraint::Midpoint { line: l, .. } => *l == line,
        Constraint::Equal { a, b } => {
            (*a == line || *b == line)
                && doc.entity(line).map(|e| e.geom.kind()) == Some(EntityKind::Line)
        }
        _ => false,
    }
}

/// Removes constraint `id` (reported) in the builder.
fn drop_constraint(b: &mut Builder<'_>, id: ConstraintId, dropped: &mut Vec<ConstraintId>) {
    b.commands.push(Command::RemoveConstraint { id });
    dropped.push(id);
}

/// Deletes point `p` if nothing but `owner` used it, with its constraints.
fn delete_if_orphan(
    b: &mut Builder<'_>,
    p: EntityId,
    owner: EntityId,
    removed: &mut BTreeSet<ConstraintId>,
    dropped: &mut Vec<ConstraintId>,
) {
    let (ents, cons) = b.doc.dependents(p);
    if ents.iter().any(|e| *e != owner) {
        return;
    }
    for c in cons {
        if removed.insert(c) {
            drop_constraint(b, c, dropped);
        }
    }
    b.commands.push(Command::DeleteEntity { id: p });
}

/// The pieces of the target kept by a trim or break, as `[from, to]` ends.
/// For circles both ends are cuts.
struct Pieces {
    kept: Vec<(End, End, f64, f64)>,
}

/// Rebuilds `id` as `pieces`. The first piece keeps the ID (lines and arcs)
/// with its constraints; a circle is replaced by arcs, its constraints moved
/// to the first. Returns the plan.
fn rebuild(doc: &Document, id: EntityId, t: &Target, pieces: Pieces, label: &str) -> Plan {
    let mut b = Builder::new(doc, id);
    let mut dropped = Vec::new();
    let mut removed = BTreeSet::new();
    let mut made = Vec::new();
    let geom = doc.entity(id).map(|e| e.geom.clone());
    let (_, cons) = doc.dependents(id);
    if pieces.kept.is_empty() {
        // Nothing left: delete the entity, its constraints, and its points
        // that nothing else uses.
        for c in &cons {
            removed.insert(*c);
            drop_constraint(&mut b, *c, &mut dropped);
        }
        b.commands.push(Command::DeleteEntity { id });
        for p in geom.map(|g| g.point_refs()).unwrap_or_default() {
            delete_if_orphan(&mut b, p, id, &mut removed, &mut dropped);
        }
        return b.finish(label, dropped);
    }
    let is_line = matches!(t, Target::Line(_));
    if is_line {
        for c in &cons {
            if let Some(con) = doc.constraint(*c)
                && extent_bound(doc, con, id)
                && removed.insert(*c)
            {
                drop_constraint(&mut b, *c, &mut dropped);
            }
        }
    }
    let center = match geom {
        Some(Geom::Arc { center, .. } | Geom::Circle { center, .. }) => Some(center),
        _ => None,
    };
    let mut ids = Vec::new();
    for (k, &(from, to, s0, s1)) in pieces.kept.iter().enumerate() {
        let (a, z) = (b.end(from, &mut made), b.end(to, &mut made));
        let g = match (t, center) {
            (Target::Line(_), _) => Geom::Line { start: a, end: z },
            (_, Some(center)) => Geom::Arc {
                center,
                start: a,
                end: z,
            },
            _ => continue,
        };
        if k == 0 && !matches!(t, Target::Circle(_)) {
            b.commands.push(Command::ModifyEntity { id, geom: g });
            ids.push((id, s0, s1));
        } else {
            let n = b.add(g);
            ids.push((n, s0, s1));
        }
    }
    let keeper = ids[0].0;
    // Keep the pieces on one carrier.
    for w in 1..ids.len() {
        let (prev, cur) = (ids[w - 1], ids[w]);
        let adjacent = (prev.2 - cur.1).abs() <= 1e-12;
        if is_line {
            // Adjacent pieces share a point, so parallel is enough (and
            // collinear would be redundant with it).
            b.constrain(if adjacent {
                Constraint::Parallel {
                    a: prev.0,
                    b: cur.0,
                }
            } else {
                Constraint::Collinear {
                    a: keeper,
                    b: cur.0,
                }
            });
        } else if !adjacent && !matches!(t, Target::Circle(_)) {
            // Arcs on one centre point: equal radius unless they share an end.
            b.constrain(Constraint::Equal {
                a: keeper,
                b: cur.0,
            });
        }
    }
    if let Target::Circle(_) = t {
        // The circle becomes arcs: move its constraints to the first.
        for c in &cons {
            if let Some(con) = doc.constraint(*c) {
                b.commands.push(Command::RemoveConstraint { id: *c });
                b.constrain(retarget(con, id, keeper));
            }
        }
        b.commands.push(Command::DeleteEntity { id });
    } else if let Some(g) = geom {
        // Original end points no piece uses any more.
        let used: BTreeSet<EntityId> = pieces
            .kept
            .iter()
            .flat_map(|(f, t, _, _)| [*f, *t])
            .filter_map(|e| match e {
                End::Point(p) => Some(p),
                End::Cut(_) => None,
            })
            .collect();
        for p in g.point_refs() {
            if Some(p) != center && !used.contains(&p) {
                delete_if_orphan(&mut b, p, id, &mut removed, &mut dropped);
            }
        }
    }
    b.finish(label, dropped)
}

/// Ends of a line or arc entity.
fn ends(doc: &Document, id: EntityId) -> Option<(EntityId, EntityId)> {
    match doc.entity(id)?.geom {
        Geom::Line { start, end } | Geom::Arc { start, end, .. } => Some((start, end)),
        _ => None,
    }
}

/// The cuts strictly inside the entity, and the pick's parameter.
fn inner(doc: &Document, id: EntityId, pick: Vec2) -> Result<(Target, Vec<Cut>, f64), ModifyError> {
    let t = Target::of(doc, id)?;
    let tol = SOLVER_TOL / t.scale().max(SOLVER_TOL);
    let span = t.span();
    let mut cs = cuts(doc, id, &t);
    match t {
        Target::Circle(_) => {}
        _ => cs.retain(|c| c.s > tol && c.s < span - tol),
    }
    Ok((t, cs, t.param(pick)))
}

/// The cut before and after parameter `p` (cyclic on a circle).
fn around(t: &Target, cs: &[Cut], p: f64) -> (Option<Cut>, Option<Cut>) {
    let i = cs.partition_point(|c| c.s < p);
    match t {
        Target::Circle(_) if cs.len() >= 2 => {
            let lo = cs[(i + cs.len() - 1) % cs.len()];
            let hi = cs[i % cs.len()];
            (Some(lo), Some(hi))
        }
        Target::Circle(_) => (None, None),
        _ => (i.checked_sub(1).map(|k| cs[k]), cs.get(i).copied()),
    }
}

/// Trims the piece of `id` (line, arc or circle) between the intersections
/// either side of `pick`. With none, the whole entity goes.
///
/// # Errors
/// [`ModifyError::UnknownEntity`], [`ModifyError::Unsupported`].
pub fn trim(doc: &Document, id: EntityId, pick: Vec2) -> Result<Plan, ModifyError> {
    let (t, cs, p) = inner(doc, id, pick)?;
    let (lo, hi) = around(&t, &cs, p);
    let kept = match t {
        Target::Circle(_) => match (lo, hi) {
            // The rest of the circle, CCW from the far cut back round.
            (Some(lo), Some(hi)) => vec![(End::Cut(hi), End::Cut(lo), hi.s, lo.s + TAU)],
            _ => Vec::new(),
        },
        _ => {
            let (start, end) = ends(doc, id).ok_or(ModifyError::UnknownEntity(id))?;
            let span = t.span();
            let mut kept = Vec::new();
            if let Some(lo) = lo {
                kept.push((End::Point(start), End::Cut(lo), 0.0, lo.s));
            }
            if let Some(hi) = hi {
                kept.push((End::Cut(hi), End::Point(end), hi.s, span));
            }
            kept
        }
    };
    Ok(rebuild(doc, id, &t, Pieces { kept }, "Trim"))
}

/// Breaks `id` (line, arc or circle) at the intersections either side of
/// `pick`, keeping every piece.
///
/// # Errors
/// [`ModifyError::NoIntersections`] when nothing crosses it (a circle needs
/// two crossings), and those of [`trim`].
pub fn split(doc: &Document, id: EntityId, pick: Vec2) -> Result<Plan, ModifyError> {
    let (t, cs, p) = inner(doc, id, pick)?;
    let (lo, hi) = around(&t, &cs, p);
    let kept = match t {
        Target::Circle(_) => match (lo, hi) {
            (Some(lo), Some(hi)) => {
                let hs = if hi.s <= lo.s { hi.s + TAU } else { hi.s };
                vec![
                    (End::Cut(lo), End::Cut(hi), lo.s, hs),
                    (End::Cut(hi), End::Cut(lo), hs, lo.s + TAU),
                ]
            }
            _ => return Err(ModifyError::NoIntersections),
        },
        _ => {
            if lo.is_none() && hi.is_none() {
                return Err(ModifyError::NoIntersections);
            }
            let (start, end) = ends(doc, id).ok_or(ModifyError::UnknownEntity(id))?;
            let mut stops: Vec<(End, f64)> = vec![(End::Point(start), 0.0)];
            stops.extend(lo.map(|c| (End::Cut(c), c.s)));
            stops.extend(hi.map(|c| (End::Cut(c), c.s)));
            stops.push((End::Point(end), t.span()));
            stops
                .windows(2)
                .map(|w| (w[0].0, w[1].0, w[0].1, w[1].1))
                .collect()
        }
    };
    Ok(rebuild(doc, id, &t, Pieces { kept }, "Break"))
}

/// Extends the free end of line or arc `id` nearest `pick` to the next curve
/// its carrier meets. The end point moves (keeping its constraints) and is
/// held on that curve; if a point entity is already there, the end joins it.
///
/// # Errors
/// [`ModifyError::EndConnected`] when that end is shared with other
/// geometry, [`ModifyError::NoBoundary`] when nothing lies ahead, and
/// [`ModifyError::Unsupported`] for circles and Béziers.
pub fn extend(doc: &Document, id: EntityId, pick: Vec2) -> Result<Plan, ModifyError> {
    let t = Target::of(doc, id)?;
    let (start, end) = ends(doc, id).ok_or(ModifyError::Unsupported(EntityKind::Circle))?;
    let span = t.span();
    let at_end = t.at(span).distance(pick) <= t.at(0.0).distance(pick);
    let moving = if at_end { end } else { start };
    let (users, _) = doc.dependents(moving);
    if users.iter().any(|u| *u != id) {
        return Err(ModifyError::EndConnected);
    }
    let tol = SOLVER_TOL / t.scale().max(SOLVER_TOL);
    // Distance ahead of the moving end, in parameter units.
    let ahead = |s: f64| -> Option<f64> {
        let d = match t {
            Target::Line(_) => {
                if at_end {
                    s - 1.0
                } else {
                    -s
                }
            }
            _ => {
                // Beyond the end going CCW, or before the start going CW,
                // never wrapping onto the arc itself.
                let d = if at_end {
                    (s - span).rem_euclid(TAU)
                } else {
                    (-s).rem_euclid(TAU)
                };
                if d >= TAU - span - tol {
                    return None;
                }
                d
            }
        };
        (d > tol).then_some(d)
    };
    let mut best: Option<(f64, Cut)> = None;
    for c in cuts(doc, id, &t) {
        if let Some(d) = ahead(c.s)
            && best.is_none_or(|(bd, _)| d < bd)
        {
            best = Some((d, c));
        }
    }
    let (_, cut) = best.ok_or(ModifyError::NoBoundary)?;
    let mut b = Builder::new(doc, id);
    let existing = doc.entities().iter().find_map(|(pid, e)| match e.geom {
        Geom::Point { pos } if pid != moving && pos.distance(cut.pos) <= SOLVER_TOL => Some(pid),
        _ => None,
    });
    let mut dropped = Vec::new();
    match existing {
        Some(p) => {
            // Join the point that is already there; the old end goes.
            let geom = match doc.entity(id).map(|e| e.geom.clone()) {
                Some(Geom::Line { .. }) if at_end => Geom::Line { start, end: p },
                Some(Geom::Line { .. }) => Geom::Line { start: p, end },
                Some(Geom::Arc { center, .. }) if at_end => Geom::Arc {
                    center,
                    start,
                    end: p,
                },
                Some(Geom::Arc { center, .. }) => Geom::Arc {
                    center,
                    start: p,
                    end,
                },
                _ => return Err(ModifyError::Unsupported(EntityKind::Circle)),
            };
            b.commands.push(Command::ModifyEntity { id, geom });
            let mut removed = BTreeSet::new();
            delete_if_orphan(&mut b, moving, id, &mut removed, &mut dropped);
        }
        None => {
            b.commands.push(Command::ModifyEntity {
                id: moving,
                geom: Geom::Point { pos: cut.pos },
            });
            let kind = doc.entity(cut.by).map(|e| e.geom.kind());
            if matches!(
                kind,
                Some(EntityKind::Line | EntityKind::Circle | EntityKind::Arc)
            ) {
                b.constrain(Constraint::PointOnCurve {
                    point: moving,
                    curve: cut.by,
                });
            }
        }
    }
    Ok(b.finish("Extend", dropped))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::Editor;
    use crate::constraint::Dimension;
    use crate::testutil::{add, line, pt};

    fn v(x: f64, y: f64) -> Vec2 {
        Vec2::new(x, y)
    }

    /// A horizontal line (0,0)–(30,0) crossed by verticals at x = 10, 20.
    fn crossed() -> (Document, EntityId, [EntityId; 2]) {
        let mut d = Document::new();
        let (a, b) = (pt(&mut d, 0.0, 0.0), pt(&mut d, 30.0, 0.0));
        let h = line(&mut d, a, b);
        let mut vs = [h; 2];
        for (i, x) in [10.0, 20.0].into_iter().enumerate() {
            let (p, q) = (pt(&mut d, x, -5.0), pt(&mut d, x, 5.0));
            vs[i] = line(&mut d, p, q);
        }
        (d, h, vs)
    }

    fn run(doc: Document, plan: &Plan) -> Document {
        let mut ed = Editor::new(doc);
        ed.apply(&plan.command).unwrap();
        ed.document().clone()
    }

    fn lines(d: &Document) -> Vec<Line> {
        let mut ls: Vec<Line> = d
            .entities()
            .iter()
            .filter_map(|(id, _)| d.line(id))
            .filter(|l| l.a.y.abs() < 1e-9 && l.b.y.abs() < 1e-9)
            .collect();
        ls.sort_by(|a, b| a.a.x.min(a.b.x).total_cmp(&b.a.x.min(b.b.x)));
        ls
    }

    #[test]
    fn trim_middle_splits_and_keeps_collinear() {
        let (d, h, vs) = crossed();
        let plan = trim(&d, h, v(15.0, 0.0)).unwrap();
        assert!(plan.dropped.is_empty());
        let out = run(d, &plan);
        let ls = lines(&out);
        assert_eq!(ls.len(), 2);
        assert!(ls[0].b.approx_eq(v(10.0, 0.0), 1e-12) && ls[1].a.approx_eq(v(20.0, 0.0), 1e-12));
        // The original keeps its ID; cut points sit on the verticals.
        assert!(out.line(h).is_some());
        let cons: Vec<_> = out.constraints().values().cloned().collect();
        assert!(
            cons.iter()
                .any(|c| matches!(c, Constraint::Collinear { .. }))
        );
        for vline in vs {
            assert!(
                cons.iter().any(
                    |c| matches!(c, Constraint::PointOnCurve { curve, .. } if *curve == vline)
                )
            );
        }
    }

    #[test]
    fn trim_end_moves_the_end_and_drops_an_orphan_point() {
        let (mut d, h, _) = crossed();
        let Geom::Line { start, .. } = d.entity(h).unwrap().geom else {
            panic!()
        };
        // A fix on the start point is dropped with it, and reported.
        let fix = ConstraintId::new(d.alloc_id().unwrap());
        d.constraints.insert(fix, Constraint::Fix { point: start });
        let plan = trim(&d, h, v(3.0, 0.0)).unwrap();
        assert_eq!(plan.dropped, vec![fix]);
        let out = run(d, &plan);
        let l = out.line(h).unwrap();
        assert!(l.a.approx_eq(v(10.0, 0.0), 1e-12) && l.b.approx_eq(v(30.0, 0.0), 1e-12));
        assert!(out.entity(start).is_none());
    }

    #[test]
    fn trim_with_nothing_crossing_deletes_and_reports() {
        let mut d = Document::new();
        let (a, b) = (pt(&mut d, 0.0, 0.0), pt(&mut d, 5.0, 0.0));
        let l = line(&mut d, a, b);
        let hz = ConstraintId::new(d.alloc_id().unwrap());
        d.constraints.insert(hz, Constraint::Horizontal { line: l });
        let plan = trim(&d, l, v(2.0, 0.0)).unwrap();
        assert_eq!(plan.dropped, vec![hz]);
        let out = run(d, &plan);
        assert!(out.entities().is_empty());
    }

    #[test]
    fn length_bound_constraints_are_reported() {
        let (mut d, h, vs) = crossed();
        let mid = pt(&mut d, 15.0, 0.0);
        let m = ConstraintId::new(d.alloc_id().unwrap());
        d.constraints.insert(
            m,
            Constraint::Midpoint {
                point: mid,
                line: h,
            },
        );
        let hz = ConstraintId::new(d.alloc_id().unwrap());
        d.constraints.insert(hz, Constraint::Horizontal { line: h });
        let plan = split(&d, h, v(15.0, 0.0)).unwrap();
        assert_eq!(plan.dropped, vec![m]);
        let out = run(d, &plan);
        assert_eq!(lines(&out).len(), 3);
        // Horizontal stays on the piece that kept the ID.
        assert!(out.constraint(hz).is_some());
        let _ = vs;
    }

    #[test]
    fn break_reuses_points_already_at_the_cut() {
        // A T-junction: the vertical's end lies on the line.
        let mut d = Document::new();
        let (a, b) = (pt(&mut d, 0.0, 0.0), pt(&mut d, 30.0, 0.0));
        let h = line(&mut d, a, b);
        let (p, q) = (pt(&mut d, 10.0, 0.0), pt(&mut d, 10.0, 5.0));
        line(&mut d, p, q);
        let plan = split(&d, h, v(5.0, 0.0)).unwrap();
        let out = run(d, &plan);
        let Geom::Line { end, .. } = out.entity(h).unwrap().geom else {
            panic!()
        };
        assert_eq!(end, p);
        assert!(
            !out.constraints()
                .values()
                .any(|c| matches!(c, Constraint::PointOnCurve { .. }))
        );
    }

    fn circle_with_bar() -> (Document, EntityId, ConstraintId) {
        let mut d = Document::new();
        let c0 = pt(&mut d, 0.0, 0.0);
        let c = add(
            &mut d,
            Geom::Circle {
                center: c0,
                radius: 10.0,
            },
        );
        let (p, q) = (pt(&mut d, -20.0, 0.0), pt(&mut d, 20.0, 0.0));
        line(&mut d, p, q);
        let r = ConstraintId::new(d.alloc_id().unwrap());
        d.constraints.insert(
            r,
            Constraint::Radius {
                curve: c,
                dim: Dimension::driving(10.0),
            },
        );
        (d, c, r)
    }

    #[test]
    fn trimming_a_circle_leaves_an_arc_with_its_constraints() {
        let (d, c, r) = circle_with_bar();
        let plan = trim(&d, c, v(0.0, 10.0)).unwrap();
        assert!(plan.dropped.is_empty());
        let out = run(d, &plan);
        assert!(out.entity(c).is_none());
        let (aid, arc) = out
            .entities()
            .iter()
            .find_map(|(id, _)| out.arc(id).map(|a| (id, a)))
            .unwrap();
        // The lower half is left: CCW from (-10,0) to (10,0).
        assert!(arc.start.approx_eq(v(-10.0, 0.0), 1e-9) && arc.end.approx_eq(v(10.0, 0.0), 1e-9));
        assert!((arc.sweep() - core::f64::consts::PI).abs() < 1e-9);
        assert!(out.constraint(r).is_none());
        assert!(
            out.constraints()
                .values()
                .any(|k| matches!(k, Constraint::Radius { curve, .. } if *curve == aid))
        );
    }

    #[test]
    fn breaking_a_circle_gives_two_arcs() {
        let (d, c, _) = circle_with_bar();
        assert_eq!(
            split(&d, c, v(0.0, 10.0)).map(|p| p.dropped),
            Ok(Vec::new())
        );
        let out = run(d.clone(), &split(&d, c, v(0.0, 10.0)).unwrap());
        let arcs: Vec<Arc> = out
            .entities()
            .iter()
            .filter_map(|(id, _)| out.arc(id))
            .collect();
        assert_eq!(arcs.len(), 2);
        let total: f64 = arcs.iter().map(Arc::sweep).sum();
        assert!((total - TAU).abs() < 1e-9);
        // No crossings: nothing to break at.
        let mut lone = Document::new();
        let c0 = pt(&mut lone, 0.0, 0.0);
        let k = add(
            &mut lone,
            Geom::Circle {
                center: c0,
                radius: 1.0,
            },
        );
        assert_eq!(
            split(&lone, k, v(1.0, 0.0)),
            Err(ModifyError::NoIntersections)
        );
    }

    #[test]
    fn trimming_an_arc_keeps_its_centre() {
        let mut d = Document::new();
        let c0 = pt(&mut d, 0.0, 0.0);
        let (s, e) = (pt(&mut d, 10.0, 0.0), pt(&mut d, -10.0, 0.0));
        let a = add(
            &mut d,
            Geom::Arc {
                center: c0,
                start: s,
                end: e,
            },
        );
        let (p, q) = (pt(&mut d, 0.0, -1.0), pt(&mut d, 0.0, 20.0));
        line(&mut d, p, q);
        let out = run(d.clone(), &trim(&d, a, v(-7.0, 7.0)).unwrap());
        let arc = out.arc(a).unwrap();
        assert!(arc.end.approx_eq(v(0.0, 10.0), 1e-9));
        assert!(out.point_pos(c0).is_some());
    }

    #[test]
    fn extend_reaches_the_next_curve() {
        let mut d = Document::new();
        let (a, b) = (pt(&mut d, 0.0, 0.0), pt(&mut d, 5.0, 0.0));
        let h = line(&mut d, a, b);
        let (p, q) = (pt(&mut d, 12.0, -5.0), pt(&mut d, 12.0, 5.0));
        let wall = line(&mut d, p, q);
        let (p2, q2) = (pt(&mut d, 20.0, -5.0), pt(&mut d, 20.0, 5.0));
        line(&mut d, p2, q2);
        let plan = extend(&d, h, v(4.0, 0.0)).unwrap();
        let out = run(d.clone(), &plan);
        assert!(out.line(h).unwrap().b.approx_eq(v(12.0, 0.0), 1e-9));
        assert!(out.constraints().values().any(|c| matches!(c, Constraint::PointOnCurve { point, curve } if *point == b && *curve == wall)));
        // Nothing behind the start.
        assert_eq!(extend(&d, h, v(1.0, 0.0)), Err(ModifyError::NoBoundary));
        // A joined end can't be extended.
        let r = pt(&mut d, 5.0, 3.0);
        line(&mut d, b, r);
        assert_eq!(extend(&d, h, v(4.0, 0.0)), Err(ModifyError::EndConnected));
    }

    #[test]
    fn extend_an_arc_round_to_a_line() {
        let mut d = Document::new();
        let c0 = pt(&mut d, 0.0, 0.0);
        let (s, e) = (pt(&mut d, 10.0, 0.0), pt(&mut d, 0.0, 10.0));
        let a = add(
            &mut d,
            Geom::Arc {
                center: c0,
                start: s,
                end: e,
            },
        );
        let (p, q) = (pt(&mut d, -20.0, 0.0), pt(&mut d, -5.0, 0.0));
        line(&mut d, p, q);
        let out = run(d.clone(), &extend(&d, a, v(1.0, 9.0)).unwrap());
        assert!(out.arc(a).unwrap().end.approx_eq(v(-10.0, 0.0), 1e-9));
    }

    #[test]
    fn modified_sketches_solve_without_conflict() {
        // Constrained like a drawn sketch: horizontal and vertical lines, the
        // verticals' feet fixed. Trim, break and extend each re-solve cleanly
        // as one undo step.
        let (mut d, h, vs) = crossed();
        let mut cons = vec![Constraint::Horizontal { line: h }];
        for vl in vs {
            cons.push(Constraint::Vertical { line: vl });
        }
        for c in cons {
            let id = ConstraintId::new(d.alloc_id().unwrap());
            d.constraints.insert(id, c);
        }
        for (op, at) in [
            (
                trim as fn(&Document, EntityId, Vec2) -> Result<Plan, ModifyError>,
                v(15.0, 0.0),
            ),
            (split, v(25.0, 0.0)),
            (trim, v(2.0, 0.0)),
        ] {
            let mut sk = crate::Sketcher::new(d.clone());
            sk.apply(&op(&d, h, at).unwrap().command).unwrap();
            assert!(sk.undo().is_ok());
            assert_eq!(sk.document().line(h), d.line(h));
        }
        let plan = trim(&d, h, v(15.0, 0.0)).unwrap();
        let mut sk = crate::Sketcher::new(d);
        sk.apply(&plan.command).unwrap();
        assert!(
            sk.document()
                .line(h)
                .unwrap()
                .b
                .approx_eq(v(10.0, 0.0), 1e-6)
        );
    }

    #[test]
    fn unsupported_and_unknown() {
        let mut d = Document::new();
        let p = pt(&mut d, 0.0, 0.0);
        assert_eq!(
            trim(&d, p, v(0.0, 0.0)),
            Err(ModifyError::Unsupported(EntityKind::Point))
        );
        assert_eq!(
            trim(&d, EntityId::new(999), v(0.0, 0.0)),
            Err(ModifyError::UnknownEntity(EntityId::new(999)))
        );
    }
}
