// SPDX-License-Identifier: AGPL-3.0-or-later

//! Fillet and chamfer (SPEC §6.2): round or bevel the corner where two curves
//! meet, as one compound command each (SPEC §11).
//!
//! The two curves must share an end point (the corner). A fillet is a
//! tangent arc between a line or arc and a line or arc, with tangent
//! constraints and a radius dimension. A chamfer joins two lines with a new
//! line, dimensioned by its two set-backs or a set-back and an angle. Both
//! trim the curves back to the new piece.
//!
//! The sharp corner point is kept as a construction point held on both
//! curves (a "virtual sharp", as in Fusion) whenever a constraint uses it, so
//! dimensions to the corner survive and nothing is dropped (SPEC §4 rule 5).
//! A corner point nothing refers to is deleted.

use crate::command::Command;
use crate::constraint::{Constraint, Dimension};
use crate::document::Document;
use crate::entity::{EntityKind, Geom};
use crate::ids::EntityId;
use crate::modify::{Builder, ModifyError, Plan};
use core::f64::consts::{PI, TAU};
use locus_geom::{SOLVER_TOL, Vec2};

/// The whole line or whole circle a curve lies on.
#[derive(Debug, Clone, Copy)]
enum Carrier {
    Line { p: Vec2, u: Vec2 },
    Circle { c: Vec2, r: f64 },
}

impl Carrier {
    /// The nearest point of the carrier to `q`.
    fn foot(&self, q: Vec2) -> Vec2 {
        match *self {
            Carrier::Line { p, u } => p + u * (q - p).dot(u),
            Carrier::Circle { c, r } => {
                c + (q - c).normalized(0.0).unwrap_or(Vec2::new(1.0, 0.0)) * r
            }
        }
    }

    /// The carriers at distance `d` from this one.
    fn offsets(&self, d: f64) -> Vec<Carrier> {
        match *self {
            Carrier::Line { p, u } => {
                let n = u.perp() * d;
                vec![Carrier::Line { p: p + n, u }, Carrier::Line { p: p - n, u }]
            }
            Carrier::Circle { c, r } => {
                let mut out = vec![Carrier::Circle { c, r: r + d }];
                if (r - d).abs() > SOLVER_TOL {
                    out.push(Carrier::Circle {
                        c,
                        r: (r - d).abs(),
                    });
                }
                out
            }
        }
    }
}

/// Where two carriers meet.
fn meet(a: Carrier, b: Carrier) -> Vec<Vec2> {
    match (a, b) {
        (Carrier::Line { p, u }, Carrier::Line { p: q, u: w }) => {
            let den = u.cross(w);
            if den.abs() <= 1e-12 {
                return Vec::new();
            }
            vec![p + u * ((q - p).cross(w) / den)]
        }
        (Carrier::Line { p, u }, Carrier::Circle { c, r })
        | (Carrier::Circle { c, r }, Carrier::Line { p, u }) => {
            let foot = p + u * (c - p).dot(u);
            let h = c.distance(foot);
            if h > r + SOLVER_TOL {
                Vec::new()
            } else {
                let k = (r * r - h * h).max(0.0).sqrt();
                vec![foot - u * k, foot + u * k]
            }
        }
        (Carrier::Circle { c, r }, Carrier::Circle { c: c1, r: r1 }) => {
            let d = c.distance(c1);
            if d <= SOLVER_TOL || d > r + r1 + SOLVER_TOL || d < (r - r1).abs() - SOLVER_TOL {
                return Vec::new();
            }
            let u = (c1 - c) / d;
            let x = (d * d + r * r - r1 * r1) / (2.0 * d);
            let h = (r * r - x * x).max(0.0).sqrt();
            let base = c + u * x;
            vec![base + u.perp() * h, base - u.perp() * h]
        }
    }
}

/// One of the two curves at the corner.
#[derive(Debug, Clone)]
struct Side {
    id: EntityId,
    kind: EntityKind,
    carrier: Carrier,
    /// The geometry, with the corner end swapped for `with` by [`Side::to`].
    geom: Geom,
    corner: EntityId,
    corner_pos: Vec2,
    /// The far end's position.
    far: Vec2,
    /// For arcs: centre and CCW sweep, and whether the corner is the start.
    arc: Option<(Vec2, f64, bool)>,
}

impl Side {
    fn of(doc: &Document, id: EntityId, corner: EntityId) -> Result<Side, ModifyError> {
        let e = doc.entity(id).ok_or(ModifyError::UnknownEntity(id))?;
        let kind = e.geom.kind();
        let pos = |p| doc.point_pos(p).ok_or(ModifyError::UnknownEntity(p));
        let corner_pos = pos(corner)?;
        match e.geom {
            Geom::Line { start, end } => {
                let far = pos(if start == corner { end } else { start })?;
                let u = (far - corner_pos)
                    .normalized(0.0)
                    .ok_or(ModifyError::TooLarge)?;
                Ok(Side {
                    id,
                    kind,
                    carrier: Carrier::Line { p: corner_pos, u },
                    geom: e.geom.clone(),
                    corner,
                    corner_pos,
                    far,
                    arc: None,
                })
            }
            Geom::Arc { center, start, end } => {
                let arc = doc.arc(id).ok_or(ModifyError::UnknownEntity(id))?;
                let c = pos(center)?;
                let far = pos(if start == corner { end } else { start })?;
                Ok(Side {
                    id,
                    kind,
                    carrier: Carrier::Circle { c, r: arc.radius() },
                    geom: e.geom.clone(),
                    corner,
                    corner_pos,
                    far,
                    arc: Some((c, arc.sweep(), start == corner)),
                })
            }
            _ => Err(ModifyError::Unsupported(kind)),
        }
    }

    /// True when `t` lies on the curve strictly between the corner and the
    /// far end, so cutting back to it leaves a piece.
    fn holds(&self, t: Vec2) -> bool {
        match self.arc {
            None => {
                let d = self.far - self.corner_pos;
                let s = (t - self.corner_pos).dot(d) / d.length_sq();
                let tol = SOLVER_TOL / d.length();
                s > tol && s < 1.0 - tol
            }
            Some((c, sweep, from_start)) => {
                let ang = |q: Vec2| (q - c).angle();
                let rel = if from_start {
                    (ang(t) - ang(self.corner_pos)).rem_euclid(TAU)
                } else {
                    (ang(self.corner_pos) - ang(t)).rem_euclid(TAU)
                };
                let tol = SOLVER_TOL / self.carrier_radius().max(SOLVER_TOL);
                rel > tol && rel < sweep - tol
            }
        }
    }

    fn carrier_radius(&self) -> f64 {
        match self.carrier {
            Carrier::Circle { r, .. } => r,
            Carrier::Line { .. } => 0.0,
        }
    }

    /// The geometry with its corner end moved to point `with`.
    fn to(&self, with: EntityId) -> Geom {
        let sw = |p: EntityId| if p == self.corner { with } else { p };
        match self.geom {
            Geom::Line { start, end } => Geom::Line {
                start: sw(start),
                end: sw(end),
            },
            Geom::Arc { center, start, end } => Geom::Arc {
                center,
                start: sw(start),
                end: sw(end),
            },
            ref g => g.clone(),
        }
    }
}

/// The shared end point of `a` and `b`, and both sides.
fn corner(doc: &Document, a: EntityId, b: EntityId) -> Result<(Side, Side), ModifyError> {
    let ends = |id: EntityId| -> Result<Vec<EntityId>, ModifyError> {
        let e = doc.entity(id).ok_or(ModifyError::UnknownEntity(id))?;
        match e.geom {
            Geom::Line { start, end } | Geom::Arc { start, end, .. } => Ok(vec![start, end]),
            _ => Err(ModifyError::Unsupported(e.geom.kind())),
        }
    };
    let (ea, eb) = (ends(a)?, ends(b)?);
    if a == b {
        return Err(ModifyError::NoCorner);
    }
    let v = ea
        .iter()
        .copied()
        .find(|p| eb.contains(p))
        .ok_or(ModifyError::NoCorner)?;
    Ok((Side::of(doc, a, v)?, Side::of(doc, b, v)?))
}

/// After both curves leave the corner: delete it if nothing else uses it;
/// otherwise hold it on both carriers (the virtual sharp), so whatever is
/// attached to it (dimensions, other curves) stays where the corner is. It
/// becomes construction when no other curve uses it.
fn settle_corner(b: &mut Builder<'_>, sa: &Side, sb: &Side, keep: bool) {
    let v = sa.corner;
    let doc = b.doc;
    let (ents, cons) = doc.dependents(v);
    let shared = ents.iter().any(|e| *e != sa.id && *e != sb.id);
    if !shared && cons.is_empty() && !keep {
        b.commands.push(crate::Command::DeleteEntity { id: v });
        return;
    }
    if !shared && doc.entity(v).is_some_and(|e| !e.construction) {
        b.commands
            .push(crate::Command::ToggleConstruction { id: v });
    }
    for curve in [sa.id, sb.id] {
        b.constrain(Constraint::PointOnCurve { point: v, curve });
    }
}

fn positive(x: f64) -> Result<f64, ModifyError> {
    if x.is_finite() && x > SOLVER_TOL {
        Ok(x)
    } else {
        Err(ModifyError::BadSize)
    }
}

/// Rounds the corner where `a` and `b` (lines or arcs) meet with a tangent
/// arc of `radius`: both are cut back to the tangent points, the arc is
/// tangent to both and carries a radius dimension.
///
/// # Errors
/// [`ModifyError::NoCorner`] when they share no end point,
/// [`ModifyError::TooLarge`] when the arc would not fit,
/// [`ModifyError::BadSize`], [`ModifyError::Unsupported`].
pub fn fillet(doc: &Document, a: EntityId, b: EntityId, radius: f64) -> Result<Plan, ModifyError> {
    let r = positive(radius)?;
    let (sa, sb) = corner(doc, a, b)?;
    let mut best: Option<(f64, Vec2, Vec2, Vec2)> = None;
    for oa in sa.carrier.offsets(r) {
        for ob in sb.carrier.offsets(r) {
            for c in meet(oa, ob) {
                let (t1, t2) = (sa.carrier.foot(c), sb.carrier.foot(c));
                let fits = (c.distance(t1) - r).abs() <= 1e-6 * r.max(1.0)
                    && (c.distance(t2) - r).abs() <= 1e-6 * r.max(1.0);
                if fits && sa.holds(t1) && sb.holds(t2) {
                    let d = c.distance(sa.corner_pos);
                    if best.is_none_or(|(bd, ..)| d < bd) {
                        best = Some((d, c, t1, t2));
                    }
                }
            }
        }
    }
    let (_, c, t1, t2) = best.ok_or(ModifyError::TooLarge)?;
    let mut bld = Builder::new(doc, a);
    let p1 = bld.add(Geom::Point { pos: t1 });
    let p2 = bld.add(Geom::Point { pos: t2 });
    let pc = bld.add(Geom::Point { pos: c });
    bld.commands.push(crate::Command::ModifyEntity {
        id: a,
        geom: sa.to(p1),
    });
    bld.commands.push(crate::Command::ModifyEntity {
        id: b,
        geom: sb.to(p2),
    });
    // The minor arc from one tangent point to the other, CCW.
    let (start, end) = if (t1 - c).cross(t2 - c) > 0.0 {
        (p1, p2)
    } else {
        (p2, p1)
    };
    let arc = bld.add(Geom::Arc {
        center: pc,
        start,
        end,
    });
    bld.constrain(Constraint::Tangent { a, b: arc });
    bld.constrain(Constraint::Tangent { a: b, b: arc });
    bld.constrain(Constraint::Radius {
        curve: arc,
        dim: Dimension::driving(r),
    });
    settle_corner(&mut bld, &sa, &sb, false);
    Ok(bld.finish("Fillet", Vec::new()))
}

/// How big a chamfer is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ChamferSize {
    /// The same set-back along both lines.
    Equal(f64),
    /// Set-back along the first line, then the second.
    Distances(f64, f64),
    /// Set-back along the first line, and the chamfer's angle to it
    /// (radians, inside the corner).
    DistanceAngle(f64, f64),
}

/// Bevels the corner where lines `a` and `b` meet with a new line, set back
/// from the corner as `size` says and dimensioned the same way.
///
/// # Errors
/// As [`fillet`]; arcs are [`ModifyError::Unsupported`].
pub fn chamfer(
    doc: &Document,
    a: EntityId,
    b: EntityId,
    size: ChamferSize,
) -> Result<Plan, ModifyError> {
    let (sa, sb) = corner(doc, a, b)?;
    for s in [&sa, &sb] {
        if s.kind != EntityKind::Line {
            return Err(ModifyError::Unsupported(s.kind));
        }
    }
    let v = sa.corner_pos;
    let (ua, ub) = (sa.far - v, sb.far - v);
    let (la, lb) = (ua.length(), ub.length());
    let (ua, ub) = (ua / la, ub / lb);
    let (d1, d2) = match size {
        ChamferSize::Equal(d) => (positive(d)?, positive(d)?),
        ChamferSize::Distances(d1, d2) => (positive(d1)?, positive(d2)?),
        ChamferSize::DistanceAngle(d, ang) => {
            let d = positive(d)?;
            let ang = positive(ang)?;
            // Triangle corner–T1–T2: angle `ang` at T1, `phi` at the corner.
            let phi = ua.cross(ub).atan2(ua.dot(ub)).abs();
            let third = PI - ang - phi;
            if third <= SOLVER_TOL {
                return Err(ModifyError::TooLarge);
            }
            (d, d * ang.sin() / third.sin())
        }
    };
    if d1 >= la - SOLVER_TOL || d2 >= lb - SOLVER_TOL {
        return Err(ModifyError::TooLarge);
    }
    let (t1, t2) = (v + ua * d1, v + ub * d2);
    let mut bld = Builder::new(doc, a);
    let p1 = bld.add(Geom::Point { pos: t1 });
    let p2 = bld.add(Geom::Point { pos: t2 });
    bld.commands.push(crate::Command::ModifyEntity {
        id: a,
        geom: sa.to(p1),
    });
    bld.commands.push(crate::Command::ModifyEntity {
        id: b,
        geom: sb.to(p2),
    });
    let line = bld.add(Geom::Line { start: p1, end: p2 });
    // The set-backs are measured from the corner, which stays as a virtual
    // sharp on both lines.
    settle_corner(&mut bld, &sa, &sb, true);
    let corner = sa.corner;
    bld.constrain(Constraint::Distance {
        a: corner,
        b: p1,
        dim: Dimension::driving(d1),
    });
    match size {
        ChamferSize::DistanceAngle(..) => {
            // The angle between the lines as stored: line `a` keeps its
            // direction, the chamfer runs T1 → T2.
            let da = match sa.geom {
                Geom::Line { start, .. } if start == sa.corner => ua,
                _ => -ua,
            };
            let dc = t2 - t1;
            bld.constrain(Constraint::Angle {
                a,
                b: line,
                dim: Dimension::driving(da.cross(dc).atan2(da.dot(dc))),
            });
        }
        _ => bld.constrain(Constraint::Distance {
            a: corner,
            b: p2,
            dim: Dimension::driving(d2),
        }),
    }
    Ok(bld.finish("Chamfer", Vec::new()))
}

/// Gives a fillet's or chamfer's dimensions expressions (SPEC §5, #217),
/// in the order the plan creates them: a fillet's radius; a chamfer's first
/// set-back, then its second set-back or its angle (angles in degrees, as
/// every dimension expression). `None` keeps that dimension a plain number.
/// The result is still one command: one undo step (SPEC §11).
#[must_use]
pub fn with_exprs(plan: Plan, exprs: &[Option<String>]) -> Plan {
    let Command::Compound { label, commands } = plan.command else {
        return plan;
    };
    let mut next = exprs.iter();
    let commands = commands
        .into_iter()
        .map(|cmd| match cmd {
            Command::AddConstraint { constraint } if constraint.dimension().is_some() => {
                match next.next().cloned().flatten() {
                    Some(expr) => Command::AddDimensionExpr { constraint, expr },
                    None => Command::AddConstraint { constraint },
                }
            }
            other => other,
        })
        .collect();
    Plan {
        command: Command::Compound { label, commands },
        dropped: plan.dropped,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{add, line, pt};
    use crate::{Command, Sketcher};

    fn v(x: f64, y: f64) -> Vec2 {
        Vec2::new(x, y)
    }

    /// Two lines meeting at (0,0): along +x to (20,0) and +y to (0,10).
    fn corner_doc() -> (Document, EntityId, EntityId, EntityId) {
        let mut d = Document::new();
        let o = pt(&mut d, 0.0, 0.0);
        let (x, y) = (pt(&mut d, 20.0, 0.0), pt(&mut d, 0.0, 10.0));
        let (a, b) = (line(&mut d, o, x), line(&mut d, y, o));
        (d, a, b, o)
    }

    fn arc_of(d: &Document) -> (EntityId, locus_geom::Arc) {
        d.entities()
            .iter()
            .find_map(|(id, _)| d.arc(id).map(|a| (id, a)))
            .unwrap()
    }

    #[test]
    fn fillet_two_lines() {
        let (d, a, b, o) = corner_doc();
        let plan = fillet(&d, a, b, 3.0).unwrap();
        let mut sk = Sketcher::new(d);
        sk.apply(&plan.command).unwrap();
        let doc = sk.document();
        let (_, arc) = arc_of(doc);
        assert!(arc.center.approx_eq(v(3.0, 3.0), 1e-6));
        assert!((arc.radius() - 3.0).abs() < 1e-6);
        assert!((arc.sweep() - PI / 2.0).abs() < 1e-6);
        assert!(doc.line(a).unwrap().a.approx_eq(v(3.0, 0.0), 1e-6));
        assert!(doc.line(b).unwrap().b.approx_eq(v(0.0, 3.0), 1e-6));
        // Nothing used the corner point, so it is gone.
        assert!(doc.entity(o).is_none());
    }

    #[test]
    fn changing_the_radius_resolves() {
        let (mut d, a, b, o) = corner_doc();
        // Pin the corner and the lines' directions so the radius drives.
        for c in [
            Constraint::Fix { point: o },
            Constraint::Horizontal { line: a },
            Constraint::Vertical { line: b },
        ] {
            let id = crate::ConstraintId::new(d.alloc_id().unwrap());
            d.constraints.insert(id, c);
        }
        let plan = fillet(&d, a, b, 3.0).unwrap();
        let mut sk = Sketcher::new(d);
        sk.apply(&plan.command).unwrap();
        // The corner is used (fix), so it stays as a construction point.
        assert!(sk.document().entity(o).unwrap().construction);
        let rid = sk
            .document()
            .constraints()
            .iter()
            .find_map(|(id, c)| matches!(c, Constraint::Radius { .. }).then_some(*id))
            .unwrap();
        sk.apply(&Command::SetDimension {
            id: rid,
            value: 5.0,
        })
        .unwrap();
        let doc = sk.document();
        let (_, arc) = arc_of(doc);
        assert!((arc.radius() - 5.0).abs() < 1e-5);
        assert!(arc.center.approx_eq(v(5.0, 5.0), 1e-5), "{:?}", arc.center);
        assert!(doc.line(a).unwrap().a.approx_eq(v(5.0, 0.0), 1e-5));
        assert!(doc.point_pos(o).unwrap().approx_eq(v(0.0, 0.0), 1e-9));
    }

    #[test]
    fn a_shared_corner_stays_on_both_curves() {
        // A third line ends at the corner: the corner stays a normal point,
        // held on both filleted lines, so the lines can't drift off it.
        let (mut d, a, b, o) = corner_doc();
        let far = pt(&mut d, -10.0, -10.0);
        let diag = line(&mut d, far, o);
        let plan = fillet(&d, a, b, 3.0).unwrap();
        let mut sk = Sketcher::new(d);
        sk.apply(&plan.command).unwrap();
        let doc = sk.document();
        assert!(!doc.entity(o).unwrap().construction);
        for curve in [a, b] {
            assert!(doc.constraints().values().any(|c| matches!(
                c,
                Constraint::PointOnCurve { point, curve: k } if *point == o && *k == curve
            )));
        }
        assert!(doc.line(diag).is_some());
    }

    #[test]
    fn fillet_line_and_arc() {
        // A line along y = 0 into (10,0), and an arc about (20,0) from
        // (20,10) CCW to (10,0): a right-angled corner at (10,0).
        let mut d = Document::new();
        let s0 = pt(&mut d, 0.0, 0.0);
        let k = pt(&mut d, 10.0, 0.0);
        let l = line(&mut d, s0, k);
        let c = pt(&mut d, 20.0, 0.0);
        let top = pt(&mut d, 20.0, 10.0);
        let arc = add(
            &mut d,
            Geom::Arc {
                center: c,
                start: top,
                end: k,
            },
        );
        let plan = fillet(&d, l, arc, 2.0).unwrap();
        let mut sk = Sketcher::new(d);
        sk.apply(&plan.command).unwrap();
        let doc = sk.document();
        let f = doc
            .entities()
            .iter()
            .filter_map(|(id, _)| doc.arc(id).map(|a| (id, a)))
            .find(|(id, _)| *id != arc)
            .unwrap()
            .1;
        assert!((f.radius() - 2.0).abs() < 1e-6);
        // Tangent to the line (y = 0) and outside the big circle (r 10).
        assert!((f.center.y - 2.0).abs() < 1e-6);
        assert!((f.center.distance(v(20.0, 0.0)) - 12.0).abs() < 1e-6);
        assert!(doc.line(l).unwrap().b.x < 10.0);
    }

    #[test]
    fn fillet_errors() {
        let (d, a, b, _) = corner_doc();
        assert_eq!(fillet(&d, a, b, 50.0), Err(ModifyError::TooLarge));
        assert_eq!(fillet(&d, a, b, -1.0), Err(ModifyError::BadSize));
        assert_eq!(fillet(&d, a, a, 1.0), Err(ModifyError::NoCorner));
        let mut d2 = d.clone();
        let (p, q) = (pt(&mut d2, 50.0, 50.0), pt(&mut d2, 60.0, 50.0));
        let far = line(&mut d2, p, q);
        assert_eq!(fillet(&d2, a, far, 1.0), Err(ModifyError::NoCorner));
    }

    #[test]
    fn chamfer_equal_and_two_distances() {
        let (d, a, b, o) = corner_doc();
        for (size, (d1, d2)) in [
            (ChamferSize::Equal(2.0), (2.0, 2.0)),
            (ChamferSize::Distances(4.0, 1.0), (4.0, 1.0)),
        ] {
            let plan = chamfer(&d, a, b, size).unwrap();
            assert!(plan.dropped.is_empty());
            let mut sk = Sketcher::new(d.clone());
            sk.apply(&plan.command).unwrap();
            let doc = sk.document();
            assert!(doc.line(a).unwrap().a.approx_eq(v(d1, 0.0), 1e-6));
            assert!(doc.line(b).unwrap().b.approx_eq(v(0.0, d2), 1e-6));
            // The corner stays as the dimensions' anchor.
            assert!(doc.entity(o).unwrap().construction);
            let dims = doc
                .constraints()
                .values()
                .filter(|c| matches!(c, Constraint::Distance { .. }))
                .count();
            assert_eq!(dims, 2);
        }
        assert_eq!(
            chamfer(&d, a, b, ChamferSize::Equal(15.0)),
            Err(ModifyError::TooLarge)
        );
    }

    #[test]
    fn chamfer_distance_and_angle() {
        let (d, a, b, _) = corner_doc();
        // 45° at T1 on a right-angled corner: an equal chamfer.
        let plan = chamfer(&d, a, b, ChamferSize::DistanceAngle(3.0, PI / 4.0)).unwrap();
        let mut sk = Sketcher::new(d);
        sk.apply(&plan.command).unwrap();
        let doc = sk.document();
        assert!(doc.line(b).unwrap().b.approx_eq(v(0.0, 3.0), 1e-6));
        assert!(
            doc.constraints()
                .values()
                .any(|c| matches!(c, Constraint::Angle { .. }))
        );
    }

    #[test]
    fn chamfer_sizes_can_be_expressions_that_follow_a_parameter() {
        let (d, a, b, o) = corner_doc();
        let mut sk = Sketcher::new(d.clone());
        sk.apply(&Command::SetParameter {
            name: "bevel".to_owned(),
            expr: "2".to_owned(),
        })
        .unwrap();
        let doc = sk.document().clone();
        let plan = chamfer(&doc, a, b, ChamferSize::Equal(2.0)).unwrap();
        let plan = with_exprs(plan, &[Some("bevel".to_owned()), Some("bevel".to_owned())]);
        sk.apply(&plan.command).unwrap();
        let exprs = sk
            .document()
            .constraints()
            .keys()
            .filter(|c| sk.document().dimension_expr(**c) == Some("bevel"))
            .count();
        assert_eq!(exprs, 2, "both set-backs follow the parameter");
        // Changing the parameter moves both set-backs, in one step.
        sk.apply(&Command::SetParameter {
            name: "bevel".to_owned(),
            expr: "3".to_owned(),
        })
        .unwrap();
        let doc = sk.document();
        // The sketch is free to turn, so measure the set-backs from the corner.
        let corner = doc.point_pos(o).unwrap();
        assert!((doc.line(a).unwrap().a.distance(corner) - 3.0).abs() < 1e-6);
        assert!((doc.line(b).unwrap().b.distance(corner) - 3.0).abs() < 1e-6);
        // `None` keeps a plain number; a fillet's radius is its one dimension.
        let plan = with_exprs(fillet(&d, a, b, 2.0).unwrap(), &[None]);
        let Command::Compound { commands, .. } = &plan.command else {
            panic!("a compound");
        };
        assert!(
            !commands
                .iter()
                .any(|c| matches!(c, Command::AddDimensionExpr { .. }))
        );
    }

    #[test]
    fn arcs_cannot_be_chamfered() {
        let (mut d, a, _, o) = corner_doc();
        let c = pt(&mut d, -5.0, 0.0);
        let e = pt(&mut d, -10.0, 0.0);
        let arc = add(
            &mut d,
            Geom::Arc {
                center: c,
                start: o,
                end: e,
            },
        );
        assert_eq!(
            chamfer(&d, a, arc, ChamferSize::Equal(1.0)),
            Err(ModifyError::Unsupported(EntityKind::Arc))
        );
    }
}
