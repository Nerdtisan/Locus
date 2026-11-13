// SPDX-License-Identifier: AGPL-3.0-or-later

//! Move, rotate and scale a selection (SPEC §6.2), respecting constraints,
//! as one compound command.
//!
//! In place, the selection's points are pinned at their transformed
//! positions and the sketch is solved on a scratch copy: other geometry
//! follows where constraints tie it to the selection. If the pinned solve
//! can't converge, the constraints in the conflicting set are returned
//! ([`ModifyError::Blocked`]) for the UI to name, and nothing changes. The
//! command then writes the solved positions, so applying it re-solves to the
//! same place.
//!
//! As a copy, new geometry is added at the transformed position with the
//! constraints that lie wholly inside the selection (dimensions scaled with
//! it; horizontal / vertical ones dropped from a rotated copy, since they no
//! longer hold). The original is untouched, so nothing can block a copy.

use crate::command::{Command, Editor};
use crate::constraint::Constraint;
use crate::document::Document;
use crate::entity::Geom;
use crate::ids::{ConstraintId, EntityId};
use crate::modify::{Builder, ModifyError, Plan, retarget};
use crate::solve::Sketcher;
use locus_geom::{SOLVER_TOL, Vec2};
use std::collections::{BTreeMap, BTreeSet};

/// A rigid motion or a uniform scale.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Transform {
    Move(Vec2),
    /// About `center`, CCW, radians.
    Rotate {
        center: Vec2,
        angle: f64,
    },
    /// About `center`, by `factor` (> 0).
    Scale {
        center: Vec2,
        factor: f64,
    },
}

impl Transform {
    /// Where `t` takes the point `p`.
    #[must_use]
    pub fn point(&self, p: Vec2) -> Vec2 {
        match *self {
            Transform::Move(d) => p + d,
            Transform::Rotate { center, angle } => {
                let (s, c) = angle.sin_cos();
                let q = p - center;
                center + Vec2::new(q.x * c - q.y * s, q.x * s + q.y * c)
            }
            Transform::Scale { center, factor } => center + (p - center) * factor,
        }
    }

    /// What `t` makes of a length `l`.
    #[must_use]
    pub fn length(&self, l: f64) -> f64 {
        match *self {
            Transform::Scale { factor, .. } => l * factor,
            _ => l,
        }
    }

    fn valid(&self) -> bool {
        match *self {
            Transform::Move(d) => d.is_finite(),
            Transform::Rotate { center, angle } => center.is_finite() && angle.is_finite(),
            Transform::Scale { center, factor } => {
                center.is_finite() && factor.is_finite() && factor > SOLVER_TOL
            }
        }
    }

    fn label(&self, copy: bool) -> &'static str {
        match (self, copy) {
            (Transform::Move(_), false) => "Move",
            (Transform::Move(_), true) => "Copy (move)",
            (Transform::Rotate { .. }, false) => "Rotate",
            (Transform::Rotate { .. }, true) => "Copy (rotate)",
            (Transform::Scale { .. }, false) => "Scale",
            (Transform::Scale { .. }, true) => "Copy (scale)",
        }
    }
}

/// The selected entities, plus every point a selected curve is built from.
fn closure(doc: &Document, sel: &[EntityId]) -> Result<BTreeSet<EntityId>, ModifyError> {
    let mut all = BTreeSet::new();
    for &id in sel {
        let e = doc.entity(id).ok_or(ModifyError::UnknownEntity(id))?;
        all.insert(id);
        all.extend(e.geom.point_refs());
    }
    if all.is_empty() {
        return Err(ModifyError::EmptySelection);
    }
    Ok(all)
}

/// Transforms the selection `sel` (curves and points) by `t`, in place or as
/// a copy.
///
/// # Errors
/// [`ModifyError::Blocked`] with the conflicting constraints when they forbid
/// the move; [`ModifyError::EmptySelection`]; [`ModifyError::BadSize`] for a
/// non-finite transform or a scale ≤ 0; [`ModifyError::UnknownEntity`].
pub fn transform(
    doc: &Document,
    sel: &[EntityId],
    t: Transform,
    copy: bool,
) -> Result<Plan, ModifyError> {
    if !t.valid() {
        return Err(ModifyError::BadSize);
    }
    let all = closure(doc, sel)?;
    if copy {
        Ok(copy_plan(doc, &all, t))
    } else {
        in_place(doc, &all, t)
    }
}

/// Geometry moved by `t`; a circle's radius scaled.
fn moved(doc: &Document, id: EntityId, t: &Transform) -> Option<Geom> {
    match doc.entity(id)?.geom {
        Geom::Point { pos } => Some(Geom::Point { pos: t.point(pos) }),
        Geom::Circle { center, radius } => Some(Geom::Circle {
            center,
            radius: t.length(radius),
        }),
        _ => None,
    }
}

fn in_place(doc: &Document, all: &BTreeSet<EntityId>, t: Transform) -> Result<Plan, ModifyError> {
    // A fix holds a point where it is, so it forbids moving that point.
    let fixed: Vec<ConstraintId> = doc
        .constraints()
        .iter()
        .filter(|(_, c)| match c {
            Constraint::Fix { point } => {
                all.contains(point)
                    && doc
                        .point_pos(*point)
                        .is_some_and(|p| t.point(p).distance(p) > SOLVER_TOL)
            }
            _ => false,
        })
        .map(|(id, _)| *id)
        .collect();
    if !fixed.is_empty() {
        return Err(ModifyError::Blocked(fixed));
    }
    // Constraints wholly inside the selection that the transform itself
    // changes (#219): a rotation turns or breaks horizontal / vertical ones,
    // a scale scales dimensions. These go first, in the same command.
    let (adjust, dropped) = adjusted(doc, all, &t);
    // Scratch: move the selection, pin it, solve.
    let mut ed = Editor::new(doc.clone());
    if !adjust.is_empty() {
        ed.apply(&Command::Compound {
            label: "adjust".to_owned(),
            commands: adjust.clone(),
        })
        .map_err(|_| ModifyError::Blocked(Vec::new()))?;
    }
    // Constraints added from here on are scratch pins, not the user's.
    let first_temp = ed.document().next_id();
    let mut cmds: Vec<Command> = all
        .iter()
        .filter_map(|&id| moved(doc, id, &t).map(|geom| Command::ModifyEntity { id, geom }))
        .collect();
    for &id in all {
        match doc.entity(id).map(|e| &e.geom) {
            Some(Geom::Point { .. }) => cmds.push(Command::AddConstraint {
                constraint: Constraint::Fix { point: id },
            }),
            Some(Geom::Circle { radius, .. }) => cmds.push(Command::AddConstraint {
                constraint: Constraint::Radius {
                    curve: id,
                    dim: crate::constraint::Dimension::driving(t.length(*radius)),
                },
            }),
            _ => {}
        }
    }
    ed.apply(&Command::Compound {
        label: "pin".to_owned(),
        commands: cmds,
    })
    .map_err(|_| ModifyError::Blocked(Vec::new()))?;
    let sk = Sketcher::new(ed.into_document());
    let st = sk.state();
    let temp = |c: &ConstraintId| c.get() >= first_temp;
    if !st.converged || !st.conflicts.is_empty() {
        let mut named: Vec<ConstraintId> = st
            .conflicts
            .iter()
            .flat_map(|g| g.constraints.iter().copied())
            .filter(|c| !temp(c))
            .collect();
        named.sort_unstable();
        named.dedup();
        return Err(ModifyError::Blocked(named));
    }
    // Write the solved positions (selection at its targets, followers where
    // the solve put them).
    let solved = sk.document();
    let mut b = Builder::new(doc, *all.iter().next().ok_or(ModifyError::EmptySelection)?);
    b.commands.extend(adjust);
    for (id, e) in doc.entities().iter() {
        let Some(now) = solved.entity(id) else {
            continue;
        };
        let changed = match (&e.geom, &now.geom) {
            (Geom::Point { pos: a }, Geom::Point { pos: b }) => a.distance(*b) > 0.0,
            (Geom::Circle { radius: a, .. }, Geom::Circle { radius: b, .. }) => a != b,
            _ => false,
        };
        if changed {
            b.commands.push(Command::ModifyEntity {
                id,
                geom: now.geom.clone(),
            });
        }
    }
    Ok(b.finish(t.label(false), dropped))
}

/// A rotation by a whole number of quarter turns: `Some(true)` for an odd
/// number (horizontal becomes vertical), `Some(false)` for an even one.
fn quarter_turns(angle: f64) -> Option<bool> {
    let q = angle / core::f64::consts::FRAC_PI_2;
    let n = q.round();
    // An angle tolerance of the solver's order: a typed 90° is exact.
    ((q - n).abs() < 1e-9).then(|| (n as i64).rem_euclid(2) == 1)
}

/// Commands that keep the selection's own constraints true after `t`, and
/// the constraints they remove, for the UI to name (SPEC §4 rule 5).
///
/// - Rotate by an odd number of quarter turns: horizontal and vertical swap;
///   by any other angle but a half turn they can no longer hold and go,
///   together with horizontal / vertical distances.
/// - Scale: driving dimensions (not angles, not expressions) scale with it,
///   as Fusion's Sketch Scale does.
///
/// Constraints a recipe owns are left alone: the recipe re-derives them.
fn adjusted(
    doc: &Document,
    all: &BTreeSet<EntityId>,
    t: &Transform,
) -> (Vec<Command>, Vec<ConstraintId>) {
    let mut cmds = Vec::new();
    let mut dropped = Vec::new();
    for (&id, c) in doc.constraints() {
        if doc.constraint_owner(id).is_some() || !c.references().iter().all(|r| all.contains(r)) {
            continue;
        }
        match *t {
            Transform::Move(_) => {}
            Transform::Rotate { angle, .. } => {
                let turns = quarter_turns(angle);
                let swapped = match (c, turns) {
                    (_, Some(false)) => continue,
                    (Constraint::Horizontal { line }, Some(true)) => {
                        Some(Constraint::Vertical { line: *line })
                    }
                    (Constraint::Vertical { line }, Some(true)) => {
                        Some(Constraint::Horizontal { line: *line })
                    }
                    (
                        Constraint::Horizontal { .. }
                        | Constraint::Vertical { .. }
                        | Constraint::HorizontalDistance { .. }
                        | Constraint::VerticalDistance { .. },
                        _,
                    ) => None,
                    _ => continue,
                };
                cmds.push(Command::RemoveConstraint { id });
                match swapped {
                    Some(constraint) => cmds.push(Command::AddConstraint { constraint }),
                    None => dropped.push(id),
                }
            }
            Transform::Scale { factor, .. } => {
                if matches!(c, Constraint::Angle { .. }) || doc.dimension_expr(id).is_some() {
                    continue;
                }
                if let Some(dim) = c.dimension()
                    && !dim.driven
                {
                    cmds.push(Command::SetDimension {
                        id,
                        value: dim.value * factor,
                    });
                }
            }
        }
    }
    (cmds, dropped)
}

/// Constraints that still hold on a transformed copy, adjusted to it.
fn copied(c: &Constraint, t: &Transform) -> Option<Constraint> {
    let mut c = c.clone();
    if matches!(t, Transform::Rotate { .. })
        && matches!(
            c,
            Constraint::Horizontal { .. }
                | Constraint::Vertical { .. }
                | Constraint::HorizontalDistance { .. }
                | Constraint::VerticalDistance { .. }
        )
    {
        return None;
    }
    if !matches!(c, Constraint::Angle { .. })
        && let Some(dim) = c.dimension_mut()
    {
        dim.value = t.length(dim.value);
    }
    Some(c)
}

fn copy_plan(doc: &Document, all: &BTreeSet<EntityId>, t: Transform) -> Plan {
    let first = *all.iter().next().unwrap_or(&EntityId::new(0));
    let mut b = Builder::new(doc, first);
    let mut map: BTreeMap<EntityId, EntityId> = BTreeMap::new();
    // Points first, so curves can refer to their copies.
    for &id in all {
        if let Some(Geom::Point { pos }) = doc.entity(id).map(|e| &e.geom) {
            let n = b.add_like(id, Geom::Point { pos: t.point(*pos) });
            map.insert(id, n);
        }
    }
    let m = |p: &EntityId, map: &BTreeMap<EntityId, EntityId>| *map.get(p).unwrap_or(p);
    for &id in all {
        let geom = match doc.entity(id).map(|e| e.geom.clone()) {
            Some(Geom::Line { start, end }) => Geom::Line {
                start: m(&start, &map),
                end: m(&end, &map),
            },
            Some(Geom::Circle { center, radius }) => Geom::Circle {
                center: m(&center, &map),
                radius: t.length(radius),
            },
            Some(Geom::Arc { center, start, end }) => Geom::Arc {
                center: m(&center, &map),
                start: m(&start, &map),
                end: m(&end, &map),
            },
            Some(Geom::Bezier {
                start,
                start_ctrl,
                end_ctrl,
                end,
            }) => Geom::Bezier {
                start: m(&start, &map),
                start_ctrl: m(&start_ctrl, &map),
                end_ctrl: m(&end_ctrl, &map),
                end: m(&end, &map),
            },
            _ => continue,
        };
        let n = b.add_like(id, geom);
        map.insert(id, n);
    }
    for c in doc.constraints().values() {
        if c.references().iter().all(|r| all.contains(r))
            && let Some(mut k) = copied(c, &t)
        {
            for (from, to) in &map {
                k = retarget(&k, *from, *to);
            }
            b.constrain(k);
        }
    }
    b.finish(t.label(true), Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constraint::Dimension;
    use crate::testutil::{add, line, pt};
    use core::f64::consts::FRAC_PI_2;

    fn v(x: f64, y: f64) -> Vec2 {
        Vec2::new(x, y)
    }

    fn con(d: &mut Document, c: Constraint) -> ConstraintId {
        let id = ConstraintId::new(d.alloc_id().unwrap());
        d.constraints.insert(id, c);
        id
    }

    #[test]
    fn move_pulls_connected_geometry_along() {
        // Two lines joined at (10,0); move the first by (0,5).
        let mut d = Document::new();
        let (a, b, c) = (
            pt(&mut d, 0.0, 0.0),
            pt(&mut d, 10.0, 0.0),
            pt(&mut d, 20.0, 0.0),
        );
        let l1 = line(&mut d, a, b);
        let l2 = line(&mut d, b, c);
        let plan = transform(&d, &[l1], Transform::Move(v(0.0, 5.0)), false).unwrap();
        let mut sk = Sketcher::new(d);
        sk.apply(&plan.command).unwrap();
        let doc = sk.document();
        assert!(doc.line(l1).unwrap().a.approx_eq(v(0.0, 5.0), 1e-6));
        // The shared point moved with it; the far end stayed.
        assert!(doc.line(l2).unwrap().a.approx_eq(v(10.0, 5.0), 1e-6));
        assert!(doc.line(l2).unwrap().b.approx_eq(v(20.0, 0.0), 1e-6));
        // One undo step restores everything.
        sk.undo().unwrap();
        assert!(
            sk.document()
                .line(l1)
                .unwrap()
                .a
                .approx_eq(v(0.0, 0.0), 1e-9)
        );
    }

    #[test]
    fn a_fixed_point_blocks_and_is_named() {
        let mut d = Document::new();
        let (a, b) = (pt(&mut d, 0.0, 0.0), pt(&mut d, 10.0, 0.0));
        let l = line(&mut d, a, b);
        let fix = con(&mut d, Constraint::Fix { point: a });
        let r = transform(&d, &[l], Transform::Move(v(1.0, 0.0)), false);
        assert_eq!(r, Err(ModifyError::Blocked(vec![fix])));
        // A copy is not blocked, and copies the fix to the copy.
        let plan = transform(&d, &[l], Transform::Move(v(0.0, 3.0)), true).unwrap();
        let mut sk = Sketcher::new(d);
        sk.apply(&plan.command).unwrap();
        let fixes = sk
            .document()
            .constraints()
            .values()
            .filter(|c| matches!(c, Constraint::Fix { .. }))
            .count();
        assert_eq!(fixes, 2);
    }

    /// The four lines of a 2-point rectangle as the Rectangle tool makes it:
    /// shared corners, top and bottom horizontal, sides vertical.
    fn rectangle(d: &mut Document) -> ([EntityId; 4], [ConstraintId; 4]) {
        let p = [
            pt(d, 0.0, 0.0),
            pt(d, 20.0, 0.0),
            pt(d, 20.0, 10.0),
            pt(d, 0.0, 10.0),
        ];
        let l = [
            line(d, p[0], p[1]),
            line(d, p[1], p[2]),
            line(d, p[2], p[3]),
            line(d, p[3], p[0]),
        ];
        let c = [
            con(d, Constraint::Horizontal { line: l[0] }),
            con(d, Constraint::Vertical { line: l[1] }),
            con(d, Constraint::Horizontal { line: l[2] }),
            con(d, Constraint::Vertical { line: l[3] }),
        ];
        (l, c)
    }

    /// Regression (#219): rotating a rectangle in place was always refused,
    /// because its own horizontal / vertical constraints held it. They are
    /// dropped (and reported) by a free angle, in the same undo step.
    #[test]
    fn rotating_a_rectangle_drops_its_own_horizontal_and_vertical() {
        let mut d = Document::new();
        let (l, hv) = rectangle(&mut d);
        let rot = Transform::Rotate {
            center: v(10.0, 5.0),
            angle: 30f64.to_radians(),
        };
        let plan = transform(&d, &l, rot, false).expect("rotate is allowed");
        let mut want = hv.to_vec();
        want.sort_unstable();
        assert_eq!(plan.dropped, want);
        let mut sk = Sketcher::new(d);
        sk.apply(&plan.command).unwrap();
        assert!(sk.state().converged);
        let bottom = sk.document().line(l[0]).unwrap();
        let dir = bottom.b - bottom.a;
        assert!((dir.y.atan2(dir.x) - 30f64.to_radians()).abs() < 1e-6);
        assert!((dir.length() - 20.0).abs() < 1e-6);
        assert!(sk.document().constraints().is_empty());
        // One undo step brings the rectangle and its constraints back.
        sk.undo().unwrap();
        assert_eq!(sk.document().constraints().len(), 4);
        assert!(
            sk.document()
                .line(l[0])
                .unwrap()
                .b
                .approx_eq(v(20.0, 0.0), 1e-9)
        );
    }

    #[test]
    fn a_quarter_turn_swaps_horizontal_and_vertical() {
        let mut d = Document::new();
        let (l, _) = rectangle(&mut d);
        let rot = Transform::Rotate {
            center: v(0.0, 0.0),
            angle: FRAC_PI_2,
        };
        let plan = transform(&d, &l, rot, false).unwrap();
        assert!(plan.dropped.is_empty());
        let mut sk = Sketcher::new(d);
        sk.apply(&plan.command).unwrap();
        let doc = sk.document();
        assert!(doc.line(l[0]).unwrap().b.approx_eq(v(0.0, 20.0), 1e-6));
        let vertical = |line| {
            doc.constraints()
                .values()
                .any(|c| *c == Constraint::Vertical { line })
        };
        assert!(vertical(l[0]) && vertical(l[2]));
        assert_eq!(doc.constraints().len(), 4);
        // A half turn keeps them as they are.
        let half = Transform::Rotate {
            center: v(0.0, 0.0),
            angle: core::f64::consts::PI,
        };
        let plan = transform(doc, &l, half, false).unwrap();
        assert!(plan.dropped.is_empty());
    }

    #[test]
    fn rotate_is_blocked_by_outside_constraints_and_copies_without_hv() {
        let mut d = Document::new();
        let (a, b) = (pt(&mut d, 0.0, 0.0), pt(&mut d, 10.0, 0.0));
        let l = line(&mut d, a, b);
        // A fixed line elsewhere, parallel to this one, holds it.
        let (c, e) = (pt(&mut d, 0.0, 5.0), pt(&mut d, 10.0, 5.0));
        let other = line(&mut d, c, e);
        con(&mut d, Constraint::Fix { point: c });
        con(&mut d, Constraint::Fix { point: e });
        let par = con(&mut d, Constraint::Parallel { a: l, b: other });
        let rot = Transform::Rotate {
            center: v(0.0, 0.0),
            angle: 0.3,
        };
        assert!(matches!(
            transform(&d, &[l], rot, false),
            Err(ModifyError::Blocked(cs)) if cs.contains(&par)
        ));
        // A copy of a horizontal line rotated a quarter turn has no
        // horizontal constraint.
        let mut d = Document::new();
        let (a, b) = (pt(&mut d, 0.0, 0.0), pt(&mut d, 10.0, 0.0));
        let l = line(&mut d, a, b);
        con(&mut d, Constraint::Horizontal { line: l });
        let rot = Transform::Rotate {
            center: v(0.0, 0.0),
            angle: FRAC_PI_2,
        };
        let plan = transform(&d, &[l], rot, true).unwrap();
        let mut sk = Sketcher::new(d);
        let applied = sk.apply(&plan.command).unwrap();
        let copy = applied.entities.last().copied().unwrap();
        let cl = sk.document().line(copy).unwrap();
        assert!(cl.b.approx_eq(v(0.0, 10.0), 1e-6));
        assert_eq!(sk.document().constraints().len(), 1);
    }

    #[test]
    fn scale_scales_radii_and_dimensions() {
        let mut d = Document::new();
        let c0 = pt(&mut d, 10.0, 0.0);
        let circ = add(
            &mut d,
            Geom::Circle {
                center: c0,
                radius: 2.0,
            },
        );
        let s = Transform::Scale {
            center: v(0.0, 0.0),
            factor: 2.0,
        };
        let plan = transform(&d, &[circ], s, false).unwrap();
        let mut sk = Sketcher::new(d.clone());
        sk.apply(&plan.command).unwrap();
        let c = sk.document().circle(circ).unwrap();
        assert!(c.center.approx_eq(v(20.0, 0.0), 1e-6) && (c.radius - 4.0).abs() < 1e-6);
        // A radius dimension scales with it, in place and on a copy.
        con(
            &mut d,
            Constraint::Radius {
                curve: circ,
                dim: Dimension::driving(2.0),
            },
        );
        let radius_is = |doc: &Document, r: f64| {
            doc.constraints().values().any(|c| {
                matches!(
                    c,
                    Constraint::Radius { dim, .. } if (dim.value - r).abs() < 1e-12
                )
            })
        };
        let plan = transform(&d, &[circ], s, false).unwrap();
        let mut sk = Sketcher::new(d.clone());
        sk.apply(&plan.command).unwrap();
        assert!(radius_is(sk.document(), 4.0));
        assert!((sk.document().circle(circ).unwrap().radius - 4.0).abs() < 1e-6);
        let plan = transform(&d, &[circ], s, true).unwrap();
        let mut sk = Sketcher::new(d);
        sk.apply(&plan.command).unwrap();
        assert!(radius_is(sk.document(), 4.0) && radius_is(sk.document(), 2.0));
    }

    #[test]
    fn bad_input() {
        let d = Document::new();
        assert_eq!(
            transform(&d, &[], Transform::Move(v(1.0, 0.0)), false),
            Err(ModifyError::EmptySelection)
        );
        assert_eq!(
            transform(
                &d,
                &[],
                Transform::Scale {
                    center: v(0.0, 0.0),
                    factor: 0.0
                },
                false
            ),
            Err(ModifyError::BadSize)
        );
    }
}
