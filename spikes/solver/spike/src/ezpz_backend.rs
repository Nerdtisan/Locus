// SPDX-License-Identifier: AGPL-3.0-or-later

//! KittyCAD/Zoo's `ezpz` solver on the harness (issue #40). MIT; calls its
//! public API only.
//!
//! ezpz is stateless: each solve takes constraint requests plus initial guesses
//! for every variable. The adapter keeps the current values between calls, so a
//! drag frame re-solves from the previous frame's geometry.

use ezpz::datatypes::inputs::{DatumCircle, DatumDistance, DatumLineSegment, DatumPoint};
use ezpz::datatypes::{Angle as EAngle, AngleKind};
use ezpz::{Config, Constraint as E, ConstraintRequest, Id, IdGenerator, LineSide};
use solver_harness::{Backend, Constraint, Curve, Diagnosis, PointId, Sketch, Vec2};

pub struct Ezpz;

/// Harness index of a request that is not a user constraint (an arc's implicit
/// equal-radius rows, a drag hold).
const IMPLICIT: usize = usize::MAX;

pub struct State {
    /// The harness sketch it was loaded from (for the rank analysis).
    sketch: Sketch,
    reqs: Vec<ConstraintRequest>,
    /// Harness constraint per request.
    owner: Vec<usize>,
    points: Vec<DatumPoint>,
    /// Per curve: its radius variable (circles and arcs).
    radius: Vec<Option<DatumDistance>>,
    lines: Vec<Option<DatumLineSegment>>,
    centres: Vec<Option<DatumPoint>>,
    /// Current value of every variable, in id order.
    values: Vec<(Id, f64)>,
}

impl State {
    fn circle(&self, c: usize) -> Result<DatumCircle, String> {
        let (center, radius) = match self.curve_centre(c) {
            Some(x) => x,
            None => return Err(format!("curve {c} is not round")),
        };
        Ok(DatumCircle { center, radius })
    }
    fn curve_centre(&self, c: usize) -> Option<(DatumPoint, DatumDistance)> {
        let r = (*self.radius.get(c)?)?;
        let centre = self.centres.get(c).copied().flatten()?;
        Some((centre, r))
    }
    fn line(&self, l: usize) -> Result<DatumLineSegment, String> {
        self.lines
            .get(l)
            .copied()
            .flatten()
            .ok_or_else(|| format!("curve {l} is not a line"))
    }
    fn point(&self, p: usize) -> Result<DatumPoint, String> {
        self.points
            .get(p)
            .copied()
            .ok_or_else(|| format!("no point {p}"))
    }
    fn push(&mut self, owner: usize, k: E) {
        self.reqs.push(ConstraintRequest::highest_priority(k));
        self.owner.push(owner);
    }
}

fn solve_reqs(reqs: &[ConstraintRequest], values: &mut [(Id, f64)]) -> Option<ezpz::SolveOutcome> {
    let out = ezpz::solve(reqs, values.to_vec(), Config::default()).ok()?;
    for ((_, v), f) in values.iter_mut().zip(out.final_values()) {
        *v = *f;
    }
    Some(out)
}

impl Ezpz {
    fn map(st: &mut State, i: usize, k: &Constraint) -> Result<(), String> {
        use Constraint::*;
        match *k {
            Coincident(p, q) => {
                let k = E::PointsCoincident(st.point(p)?, st.point(q)?);
                st.push(i, k)
            }
            PointOnCurve(p, c) => {
                let pp = st.point(p)?;
                let k = match st.line(c) {
                    Ok(l) => E::PointLineDistance(pp, l, 0.0),
                    Err(_) => {
                        let ci = st.circle(c)?;
                        E::DistanceVar(ci.center, pp, ci.radius)
                    }
                };
                st.push(i, k)
            }
            Horizontal(l) => {
                let k = E::Horizontal(st.line(l)?);
                st.push(i, k)
            }
            Vertical(l) => {
                let k = E::Vertical(st.line(l)?);
                st.push(i, k)
            }
            Parallel(a, b) => {
                let k = E::LinesAtAngle(st.line(a)?, st.line(b)?, AngleKind::Parallel);
                st.push(i, k)
            }
            Perpendicular(a, b) => {
                let k = E::LinesAtAngle(st.line(a)?, st.line(b)?, AngleKind::Perpendicular);
                st.push(i, k)
            }
            Tangent(a, b) => {
                let k = match (st.line(a), st.line(b)) {
                    (Ok(l), Err(_)) => {
                        E::LineTangentToCircle(l, st.circle(b)?, LineSide::Undefined)
                    }
                    (Err(_), Ok(l)) => {
                        E::LineTangentToCircle(l, st.circle(a)?, LineSide::Undefined)
                    }
                    (Err(_), Err(_)) => E::CircleTangentToCircle(
                        st.circle(a)?,
                        st.circle(b)?,
                        ezpz::CircleSide::Undefined,
                    ),
                    (Ok(_), Ok(_)) => return Err("tangent between two lines".into()),
                };
                st.push(i, k)
            }
            // No collinear kind: both of `b`'s ends on `a`.
            Collinear(a, b) => {
                let (la, lb) = (st.line(a)?, st.line(b)?);
                st.push(i, E::PointLineDistance(lb.p0, la, 0.0));
                st.push(i, E::PointLineDistance(lb.p1, la, 0.0));
            }
            Concentric(a, b) => {
                let k = E::PointsCoincident(st.circle(a)?.center, st.circle(b)?.center);
                st.push(i, k)
            }
            EqualLength(a, b) => {
                let k = E::LinesEqualLength(st.line(a)?, st.line(b)?);
                st.push(i, k)
            }
            EqualRadius(a, b) => {
                let k = E::ScalarEqual(st.circle(a)?.radius.id, st.circle(b)?.radius.id);
                st.push(i, k)
            }
            Symmetric { p, q, about } => {
                let k = E::Symmetric(st.line(about)?, st.point(p)?, st.point(q)?);
                st.push(i, k)
            }
            Midpoint { p, line } => {
                let k = E::Midpoint(st.line(line)?, st.point(p)?);
                st.push(i, k)
            }
            Fix { p, at } => {
                let pp = st.point(p)?;
                st.push(i, E::Fixed(pp.id_x(), at.x));
                st.push(i, E::Fixed(pp.id_y(), at.y));
            }
            Distance { p, q, d } => {
                let k = E::Distance(st.point(p)?, st.point(q)?, d);
                st.push(i, k)
            }
            PointLineDistance { p, line, d } => {
                let k = E::PointLineDistance(st.point(p)?, st.line(line)?, d);
                st.push(i, k)
            }
            LineLineDistance { l, m, d } => {
                let k = E::PointLineDistance(st.line(m)?.p0, st.line(l)?, d);
                st.push(i, k)
            }
            // ezpz: p0.x - p1.x = d; harness: q.x - p.x = d.
            HorizontalDistance { p, q, d } => {
                let k = E::HorizontalDistance(st.point(q)?, st.point(p)?, d);
                st.push(i, k)
            }
            VerticalDistance { p, q, d } => {
                let k = E::VerticalDistance(st.point(q)?, st.point(p)?, d);
                st.push(i, k)
            }
            Angle { l, m, deg } => {
                let k = E::LinesAtAngle(
                    st.line(l)?,
                    st.line(m)?,
                    AngleKind::Other(EAngle::from_degrees(deg)),
                );
                st.push(i, k)
            }
            Radius { c, r } => {
                let k = E::Fixed(st.circle(c)?.radius.id, r);
                st.push(i, k)
            }
            Diameter { c, d } => {
                let k = E::Fixed(st.circle(c)?.radius.id, d / 2.0);
                st.push(i, k)
            }
        }
        Ok(())
    }
}

impl Backend for Ezpz {
    type State = State;

    fn name(&self) -> &'static str {
        "ezpz"
    }

    fn load(&self, sk: &Sketch) -> Result<State, String> {
        let mut ids = IdGenerator::default();
        let mut values = Vec::new();
        let points: Vec<DatumPoint> = sk
            .points
            .iter()
            .map(|p| {
                let d = DatumPoint::new(&mut ids);
                values.push((d.id_x(), p.x));
                values.push((d.id_y(), p.y));
                d
            })
            .collect();
        let mut st = State {
            sketch: sk.clone(),
            reqs: Vec::new(),
            owner: Vec::new(),
            radius: Vec::new(),
            lines: Vec::new(),
            centres: Vec::new(),
            points,
            values: Vec::new(),
        };
        for c in &sk.curves {
            let (line, centre, radius) = match *c {
                Curve::Line { a, b } => (
                    Some(DatumLineSegment::new(st.point(a)?, st.point(b)?)),
                    None,
                    None,
                ),
                Curve::Circle { c, r } => {
                    let rd = DatumDistance::new(ids.next_id());
                    values.push((rd.id, r));
                    (None, Some(st.point(c)?), Some(rd))
                }
                // An arc gets its own radius variable held equal to both end
                // distances, so it can stand in as a circle for tangency, radius
                // and equal-radius constraints (ezpz has no line-arc tangency).
                Curve::Arc { c, a, b } => {
                    let rd = DatumDistance::new(ids.next_id());
                    let (cc, aa, bb) = (st.point(c)?, st.point(a)?, st.point(b)?);
                    let r0 = match (sk.points.get(a), sk.points.get(c)) {
                        (Some(pa), Some(pc)) => pa.dist(*pc),
                        _ => return Err("arc refers to a missing point".into()),
                    };
                    values.push((rd.id, r0));
                    st.push(IMPLICIT, E::DistanceVar(cc, aa, rd));
                    st.push(IMPLICIT, E::DistanceVar(cc, bb, rd));
                    (None, Some(cc), Some(rd))
                }
            };
            st.lines.push(line);
            st.centres.push(centre);
            st.radius.push(radius);
        }
        for (i, k) in sk.constraints.iter().enumerate() {
            Self::map(&mut st, i, k)?;
        }
        values.sort_by_key(|(id, _)| *id);
        st.values = values;
        Ok(st)
    }

    fn solve(&self, st: &mut State) -> bool {
        solve_reqs(&st.reqs, &mut st.values).is_some_and(|o| o.is_satisfied())
    }

    /// Hold the point at the target with two extra fixed-value requests and
    /// re-solve from the current values.
    fn drag(&self, st: &mut State, p: PointId, to: Vec2) -> bool {
        let Some(pp) = st.points.get(p).copied() else {
            return false;
        };
        let mut reqs = st.reqs.clone();
        reqs.push(ConstraintRequest::highest_priority(E::Fixed(
            pp.id_x(),
            to.x,
        )));
        reqs.push(ConstraintRequest::highest_priority(E::Fixed(
            pp.id_y(),
            to.y,
        )));
        for (id, v) in &mut st.values {
            if *id == pp.id_x() {
                *v = to.x;
            } else if *id == pp.id_y() {
                *v = to.y;
            }
        }
        solve_reqs(&reqs, &mut st.values).is_some_and(|o| o.is_satisfied())
    }

    fn diagnose(&self, st: &mut State) -> Diagnosis {
        match ezpz::solve_analysis(&st.reqs, st.values.clone(), Config::default()) {
            Ok(out) => {
                let mut conflicting: Vec<usize> = out
                    .outcome
                    .unsatisfied()
                    .iter()
                    .filter_map(|r| st.owner.get(*r).copied())
                    .filter(|o| *o != IMPLICIT)
                    .collect();
                conflicting.dedup();
                Diagnosis {
                    dof: None,
                    conflicting: Some(conflicting),
                    redundant: None,
                    note: Some(format!(
                        "{} underconstrained variables",
                        out.analysis.underconstrained().len()
                    )),
                }
            }
            Err(e) => Diagnosis {
                note: Some(format!("error: {}", e.error)),
                ..Diagnosis::default()
            },
        }
    }

    fn read(&self, st: &State, into: &mut Sketch) {
        let val = |id: Id| {
            st.values
                .binary_search_by_key(&id, |(i, _)| *i)
                .ok()
                .map(|i| st.values[i].1)
        };
        for (out, p) in into.points.iter_mut().zip(&st.points) {
            if let (Some(x), Some(y)) = (val(p.id_x()), val(p.id_y())) {
                *out = Vec2::new(x, y);
            }
        }
        for (out, r) in into.curves.iter_mut().zip(&st.radius) {
            if let (Curve::Circle { r: rr, .. }, Some(d)) = (out, r) {
                if let Some(x) = val(d.id) {
                    *rr = x;
                }
            }
        }
    }
}

/// ezpz for solving, plus the spike's Jacobian-rank analysis ([`crate::analysis`])
/// for DOF and redundancy, and ezpz's unsatisfied list (from a plain solve, not
/// the slower `solve_analysis`) for conflicts.
pub struct EzpzRank;

impl Backend for EzpzRank {
    type State = State;

    fn name(&self) -> &'static str {
        "ezpz + rank analysis"
    }
    fn load(&self, sk: &Sketch) -> Result<State, String> {
        Ezpz.load(sk)
    }
    fn solve(&self, st: &mut State) -> bool {
        Ezpz.solve(st)
    }
    fn drag(&self, st: &mut State, p: PointId, to: Vec2) -> bool {
        Ezpz.drag(st, p, to)
    }
    fn diagnose(&self, st: &mut State) -> Diagnosis {
        let mut conflicting: Vec<usize> =
            match ezpz::solve(&st.reqs, st.values.clone(), Config::default()) {
                Ok(out) => out
                    .unsatisfied()
                    .iter()
                    .filter_map(|r| st.owner.get(*r).copied())
                    .filter(|o| *o != IMPLICIT)
                    .collect(),
                Err(_) => Vec::new(),
            };
        conflicting.dedup();
        let mut sk = st.sketch.clone();
        Ezpz.read(st, &mut sk);
        let a = crate::analysis::analyse(&sk);
        Diagnosis {
            dof: conflicting.is_empty().then_some(a.dof),
            conflicting: Some(conflicting),
            redundant: Some(a.dependent),
            note: None,
        }
    }
    fn read(&self, st: &State, into: &mut Sketch) {
        Ezpz.read(st, into)
    }
}
