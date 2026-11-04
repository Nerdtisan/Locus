// SPDX-License-Identifier: AGPL-3.0-or-later

//! Independent residual checker. A backend's own "solved" flag is not trusted
//! (SPEC §3: verify with our own suite): after every solve the harness writes the
//! backend's geometry back into a [`Sketch`] and measures each constraint here.

use locus_geom::Vec2;
use locus_solver::{
    ConstraintId, ConstraintKind as Constraint, CurveId, CurveKind as Curve, PointId, Sketch,
};

/// A constraint counts as satisfied below this violation (mm, or the sine of an
/// angle for direction constraints). Looser than the SPEC §4 solver tolerance
/// (1e-6 mm) by design: it checks that a backend reached *a* solution, while each
/// backend converges to its own, tighter tolerance.
pub const SATISFIED: f64 = 1e-6;

fn pt(sk: &Sketch, p: PointId) -> Option<Vec2> {
    sk.point_at(p)
}

fn line(sk: &Sketch, l: CurveId) -> Option<(Vec2, Vec2)> {
    match sk.curves.get(l.index())?.kind {
        Curve::Line { a, b } => Some((pt(sk, a)?, pt(sk, b)?)),
        _ => None,
    }
}

/// Centre and radius of a circle or arc.
fn round(sk: &Sketch, c: CurveId) -> Option<(Vec2, f64)> {
    match sk.curves.get(c.index())?.kind {
        Curve::Circle { centre: c, r } => Some((pt(sk, c)?, r)),
        Curve::Arc { centre: c, a, .. } => {
            let cc = pt(sk, c)?;
            Some((cc, pt(sk, a)?.distance(cc)))
        }
        Curve::Line { .. } => None,
    }
}

/// Unsigned distance from `p` to the infinite line through `a`, `b`.
fn line_dist(p: Vec2, (a, b): (Vec2, Vec2)) -> f64 {
    let d = b - a;
    let len = d.length();
    if len < 1e-12 {
        return p.distance(a);
    }
    (d.cross(p - a) / len).abs()
}

fn unit_cross(a: (Vec2, Vec2), b: (Vec2, Vec2)) -> Option<(f64, f64)> {
    let (da, db) = (a.1 - a.0, b.1 - b.0);
    let s = da.length() * db.length();
    (s > 1e-12).then(|| (da.cross(db) / s, da.dot(db) / s))
}

fn wrap_deg(a: f64) -> f64 {
    (a + 180.0).rem_euclid(360.0) - 180.0
}

/// How far constraint `k` is from holding, `>= 0`. `INFINITY` when it refers to
/// missing or wrong-kind geometry (never a panic: the input is untrusted).
pub fn violation(sk: &Sketch, k: &Constraint) -> f64 {
    violation_opt(sk, k).unwrap_or(f64::INFINITY)
}

fn violation_opt(sk: &Sketch, k: &Constraint) -> Option<f64> {
    use Constraint::*;
    Some(match *k {
        Coincident(p, q) => pt(sk, p)?.distance(pt(sk, q)?),
        PointOnCurve(p, c) => {
            let pp = pt(sk, p)?;
            match line(sk, c) {
                Some(l) => line_dist(pp, l),
                None => {
                    let (cc, r) = round(sk, c)?;
                    (pp.distance(cc) - r).abs()
                }
            }
        }
        Horizontal(l) => {
            let (a, b) = line(sk, l)?;
            (b.y - a.y).abs()
        }
        Vertical(l) => {
            let (a, b) = line(sk, l)?;
            (b.x - a.x).abs()
        }
        Parallel(a, b) => unit_cross(line(sk, a)?, line(sk, b)?)?.0.abs(),
        Perpendicular(a, b) => unit_cross(line(sk, a)?, line(sk, b)?)?.1.abs(),
        Tangent(a, b) => match (line(sk, a), line(sk, b)) {
            (Some(l), None) | (None, Some(l)) => {
                let (cc, r) = round(sk, if line(sk, a).is_some() { b } else { a })?;
                (line_dist(cc, l) - r).abs()
            }
            (None, None) => {
                let (c1, r1) = round(sk, a)?;
                let (c2, r2) = round(sk, b)?;
                let d = c1.distance(c2);
                (d - (r1 + r2)).abs().min((d - (r1 - r2).abs()).abs())
            }
            (Some(_), Some(_)) => return None,
        },
        Collinear(a, b) => {
            let la = line(sk, a)?;
            let (b0, b1) = line(sk, b)?;
            line_dist(b0, la).max(line_dist(b1, la))
        }
        Concentric(a, b) => round(sk, a)?.0.distance(round(sk, b)?.0),
        EqualLength(a, b) => {
            let (a0, a1) = line(sk, a)?;
            let (b0, b1) = line(sk, b)?;
            (a0.distance(a1) - b0.distance(b1)).abs()
        }
        EqualRadius(a, b) => (round(sk, a)?.1 - round(sk, b)?.1).abs(),
        Symmetric { p, q, about } => {
            let l = line(sk, about)?;
            let (pp, qq) = (pt(sk, p)?, pt(sk, q)?);
            let dir = l.1 - l.0;
            let along = ((qq - pp).dot(dir) / dir.length().max(1e-12)).abs();
            line_dist((pp + qq) * 0.5, l).max(along)
        }
        Midpoint { p, line: l } => {
            let (a, b) = line(sk, l)?;
            pt(sk, p)?.distance((a + b) * 0.5)
        }
        Fix { p, at } => pt(sk, p)?.distance(at),
        Distance { p, q, d } => (pt(sk, p)?.distance(pt(sk, q)?) - d).abs(),
        PointLineDistance { p, line: l, d } => (line_dist(pt(sk, p)?, line(sk, l)?) - d).abs(),
        LineLineDistance { l, m, d } => (line_dist(line(sk, m)?.0, line(sk, l)?) - d).abs(),
        HorizontalDistance { p, q, d } => (pt(sk, q)?.x - pt(sk, p)?.x - d).abs(),
        VerticalDistance { p, q, d } => (pt(sk, q)?.y - pt(sk, p)?.y - d).abs(),
        Angle { l, m, rad } => {
            let (s, c) = unit_cross(line(sk, l)?, line(sk, m)?)?;
            // Report the angular error as its sine so it shares SATISFIED's scale.
            wrap_deg((s.atan2(c) - rad).to_degrees())
                .to_radians()
                .sin()
                .abs()
        }
        Radius { c, r } => (round(sk, c)?.1 - r).abs(),
        Diameter { c, d } => (2.0 * round(sk, c)?.1 - d).abs(),
    })
}

/// Implicit constraint of every arc: both ends on its circle.
pub fn arc_violation(sk: &Sketch, c: CurveId) -> f64 {
    match sk.curves.get(c.index()).map(|k| k.kind) {
        Some(Curve::Arc { centre: c, a, b }) => match (pt(sk, c), pt(sk, a), pt(sk, b)) {
            (Some(cc), Some(aa), Some(bb)) => (aa.distance(cc) - bb.distance(cc)).abs(),
            _ => f64::INFINITY,
        },
        _ => 0.0,
    }
}

/// The largest violation over all constraints and arcs, with the worst constraint
/// (`None` when it is an arc's implicit constraint, or nothing is violated).
pub fn worst(sk: &Sketch) -> (f64, Option<ConstraintId>) {
    let mut out = (0.0, None);
    for (i, k) in sk.constraints.iter().enumerate() {
        if k.driven {
            continue;
        }
        let v = violation(sk, &k.kind);
        // NaN counts as the worst violation.
        if v > out.0 || v.is_nan() {
            out = (v, Some(ConstraintId(i as u32)));
        }
    }
    for c in 0..sk.curves.len() {
        let v = arc_violation(sk, CurveId(c as u32));
        if v > out.0 || v.is_nan() {
            out = (v, None);
        }
    }
    out
}

/// Every constraint and arc holds within [`SATISFIED`].
pub fn satisfied(sk: &Sketch) -> bool {
    worst(sk).0 < SATISFIED
}
