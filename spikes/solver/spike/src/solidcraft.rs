// SPDX-License-Identifier: AGPL-3.0-or-later

//! SolidCraft's `solidcraft-sketch` solver on the harness (issue #39).
//! Upstream: github.com/tangency-labs/solidcraft, MIT OR Apache-2.0. This file
//! only calls its public API; no upstream code is copied.

use solidcore_record::Vec2 as SVec2;
use solidcraft_sketch::{
    solve_with, Analysis, Budget, ConstraintKind as K, CurveKind, Hold, Sketch as SSketch,
    SolveReport,
};
use solver_harness::{Backend, Constraint, Curve, Diagnosis, PointId, Sketch, Vec2};

pub struct SolidCraft;

pub struct State {
    sk: SSketch,
    /// `SSketch::new()` starts with a fixed origin point; harness point `p` is
    /// SolidCraft point `p + off`.
    off: usize,
    /// SolidCraft constraint id per harness constraint.
    ids: Vec<String>,
}

fn v(p: Vec2) -> SVec2 {
    SVec2::new(p.x, p.y)
}

fn line_start(sk: &Sketch, l: usize) -> Result<usize, String> {
    match sk.curves.get(l) {
        Some(Curve::Line { a, .. }) => Ok(*a),
        _ => Err(format!("curve {l} is not a line")),
    }
}

impl SolidCraft {
    fn map(sk: &Sketch, k: &Constraint, o: usize) -> Result<K, String> {
        use Constraint::*;
        let s = |p: usize| p + o;
        Ok(match *k {
            Coincident(p, q) => K::Coincident { p: s(p), q: s(q) },
            PointOnCurve(p, c) => K::PointOnCurve { p: s(p), c },
            Horizontal(l) => K::Horizontal { l },
            Vertical(l) => K::Vertical { l },
            Parallel(a, b) => K::Parallel { a, b },
            Perpendicular(a, b) => K::Perpendicular { a, b },
            Tangent(a, b) => K::Tangent { a, b },
            Collinear(a, b) => K::Collinear { a, b },
            Concentric(a, b) => K::Concentric { a, b },
            EqualLength(a, b) | EqualRadius(a, b) => K::Equal { a, b },
            Symmetric { p, q, about } => K::Symmetric {
                p: s(p),
                q: s(q),
                l: about,
            },
            Midpoint { p, line } => K::Midpoint { p: s(p), l: line },
            Fix { p, .. } => K::Fix { p: s(p) },
            Distance { p, q, d } => K::Distance {
                p: s(p),
                q: s(q),
                value: d,
            },
            PointLineDistance { p, line, d } => K::PointLineDistance {
                p: s(p),
                l: line,
                value: d,
            },
            // No line-line kind: a parallel-line distance is the distance from one
            // line's point to the other, as the harness defines it.
            LineLineDistance { l, m, d } => K::PointLineDistance {
                p: s(line_start(sk, m)?),
                l,
                value: d,
            },
            // SolidCraft's horizontal/vertical distances are unsigned (|q.x - p.x|);
            // the fixtures only use them with q on the positive side.
            HorizontalDistance { p, q, d } => K::DistanceX {
                p: s(p),
                q: s(q),
                value: d,
            },
            VerticalDistance { p, q, d } => K::DistanceY {
                p: s(p),
                q: s(q),
                value: d,
            },
            Angle { l, m, deg } => K::Angle {
                a: l,
                b: m,
                value: deg.to_radians(),
                flip: false,
            },
            Radius { c, r } => K::Radius { c, value: r },
            Diameter { c, d } => K::Diameter { c, value: d },
        })
    }
}

fn report(st: &mut State, analysis: Analysis, hold: &Hold, frame: bool) -> SolveReport {
    let mut budget = if frame {
        Budget::frame()
    } else {
        Budget::full()
    };
    solve_with(&mut st.sk, hold, None, analysis, Some(&mut budget))
}

impl Backend for SolidCraft {
    type State = State;

    fn name(&self) -> &'static str {
        "SolidCraft"
    }

    fn load(&self, sk: &Sketch) -> Result<State, String> {
        let e = |e: solidcraft_sketch::SketchError| e.to_string();
        let mut s = SSketch::new();
        let o = s.points.len();
        let mut pos = sk.points.clone();
        // SolidCraft fixes a point where it is: put it at its target first.
        for k in &sk.constraints {
            if let Constraint::Fix { p, at } = k {
                if let Some(q) = pos.get_mut(*p) {
                    *q = *at;
                }
            }
        }
        for p in &pos {
            s.add_point(v(*p), None).map_err(e)?;
        }
        for c in &sk.curves {
            let kind = match *c {
                Curve::Line { a, b } => CurveKind::Line { a: a + o, b: b + o },
                Curve::Circle { c, r } => CurveKind::Circle { c: c + o, r },
                Curve::Arc { c, a, b } => CurveKind::Arc {
                    c: c + o,
                    a: a + o,
                    b: b + o,
                },
            };
            s.add_curve(kind, None).map_err(e)?;
        }
        let mut ids = Vec::with_capacity(sk.constraints.len());
        for k in &sk.constraints {
            ids.push(s.add_constraint(Self::map(sk, k, o)?, None).map_err(e)?);
        }
        Ok(State { sk: s, off: o, ids })
    }

    fn solve(&self, st: &mut State) -> bool {
        report(st, Analysis::StatusOnly, &Hold::default(), false).ok()
    }

    /// As SolidCraft's own drag benchmark (`examples/solve_bench.rs`): put the
    /// point at the target, hold it, solve with a frame budget.
    fn drag(&self, st: &mut State, p: PointId, to: Vec2) -> bool {
        let p = p + st.off;
        if let Some(q) = st.sk.points.get_mut(p) {
            q.pos = v(to);
        }
        let hold = Hold {
            points: vec![p],
            radii: Vec::new(),
        };
        report(st, Analysis::StatusOnly, &hold, true).ok()
    }

    fn diagnose(&self, st: &mut State) -> Diagnosis {
        let r = report(st, Analysis::Full, &Hold::default(), false);
        let index = |names: &[String]| -> Vec<usize> {
            names
                .iter()
                .filter_map(|n| st.ids.iter().position(|id| id == n))
                .collect()
        };
        Diagnosis {
            dof: r.ok().then_some(r.dof),
            conflicting: Some(index(&r.failing)),
            redundant: Some(index(&r.redundant)),
            note: None,
        }
    }

    fn read(&self, st: &State, into: &mut Sketch) {
        for (out, p) in into.points.iter_mut().zip(st.sk.points.iter().skip(st.off)) {
            *out = Vec2::new(p.pos.x, p.pos.y);
        }
        for (out, c) in into.curves.iter_mut().zip(&st.sk.curves) {
            if let (Curve::Circle { r, .. }, CurveKind::Circle { r: sr, .. }) = (out, &c.kind) {
                *r = *sr;
            }
        }
    }
}
