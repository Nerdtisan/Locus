// SPDX-License-Identifier: AGPL-3.0-or-later

//! Stroke styles and their expansion into real geometry (SPEC §4, §7, §8;
//! [ADR 0019](../../docs/adr/0019-line-brush-fill-styles.md) decision 3).
//!
//! A stroke of constant width is the path's tube (path ⊕ disc, ADR 0010)
//! with its caps and joins swapped for exact pieces: a band per piece
//! (a rectangle for a line, an annular sector for an arc), and at each end
//! and corner a disc, square, kite or bevel triangle. One `locus-region`
//! boolean then gives the outline in true lines and arcs, so it is exact.
//!
//! Three outputs:
//! * [`expand`]: the exact outline, for export, Expand to geometry and
//!   Machine preview;
//! * [`mesh`]: triangles at a canvas tolerance, with no boolean (spike S3:
//!   cached, these draw at the cost of a plain centreline);
//! * [`bounds`]: the visual bounds without expanding (page bounds, §8).
//!
//! Core crate: no GUI deps, no filesystem/thread/clock use, builds for
//! `wasm32-unknown-unknown`. Input is untrusted (a hostile `.locus` file):
//! everything is checked finite and bounded, and nothing panics.

// Tests may unwrap; library code may not (CLAUDE.md, Code Style).
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod chain;
mod dash;
mod measure;
mod multi;
mod profile;
mod style;
mod vary;

pub use chain::{Chain, chains};
pub use dash::{CORNER_TURN, Dash, DashAlign, MAX_DASH_LEN, MAX_DASH_PAIRS, dashes};
pub use measure::PathMap;
pub use multi::{LayerStroke, MAX_STROKE_LAYERS, StrokeLayer, layer_strokes, offset_chain};
pub use profile::{
    MAX_SIDE_FACTOR, MAX_WIDTH_POINTS, MIN_WIDTH_POINTS, ProfilePreset, WidthPoint, WidthProfile,
};
pub use style::{Align, Cap, DEFAULT_MITER, Join, LineStyle, MAX_MITER, MIN_MITER};
pub use vary::{OutPiece, Outline};

use core::f64::consts::{PI, TAU};
use locus_geom::{Aabb, EXPORT_TOL, Vec2};
use locus_region::{Area, BoolOp, Edge, Loop, SNAP, Seg, boolean, union_all};

/// Why a stroke couldn't be expanded.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum StrokeError {
    #[error("{what} is out of range: {value}")]
    BadValue { what: &'static str, value: f64 },
    #[error("too many {what} ({count}); the limit keeps expansion bounded")]
    Limit { what: &'static str, count: usize },
}

/// Bounds on one expansion (SPEC §6.2.1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Limits {
    /// Pieces of one path.
    pub pieces: usize,
    /// Triangles of one canvas mesh.
    pub triangles: usize,
    /// Dashes of one path (placements, ADR 0019).
    pub dashes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            pieces: 200_000,
            triangles: 2_000_000,
            dashes: 10_000,
        }
    }
}

/// Largest stroke width, mm (as `locus_doc`'s `MAX_LENGTH`).
pub const MAX_WIDTH: f64 = 1000.0;

/// Turns smaller than this (radians) are smooth: no join piece.
const SMOOTH: f64 = 1e-9;

fn line(a: Vec2, b: Vec2) -> Edge {
    Edge {
        seg: Seg::Line { a, b },
        source: 0,
    }
}

fn disc(c: Vec2, r: f64) -> Loop {
    Loop {
        edges: vec![Edge {
            seg: Seg::Arc {
                center: c,
                radius: r,
                start: 0.0,
                sweep: TAU,
            },
            source: 0,
        }],
    }
}

/// A closed polygon as a CCW loop.
pub(crate) fn polygon(pts: &[Vec2]) -> Loop {
    let n = pts.len();
    let l = Loop {
        edges: (0..n)
            .filter_map(|i| {
                let (a, b) = (pts.get(i)?, pts.get((i + 1) % n)?);
                (a.distance(*b) > 0.0).then(|| line(*a, *b))
            })
            .collect(),
    };
    if l.area() < 0.0 { l.reversed() } else { l }
}

/// The points whose nearest point on `seg` is interior and at most `h` away.
fn band(seg: &Seg, h: f64, out: &mut Vec<Loop>) {
    match *seg {
        Seg::Line { a, b } => {
            let Some(u) = (b - a).normalized(0.0) else {
                return;
            };
            let n = u.perp() * h;
            out.push(polygon(&[a - n, b - n, b + n, a + n]));
        }
        Seg::Arc {
            center,
            radius,
            start,
            sweep,
        } => {
            let (s, sw) = if sweep >= 0.0 {
                (start, sweep)
            } else {
                (start + sweep, -sweep)
            };
            let (ro, ri) = (radius + h, radius - h);
            let arc = |r: f64, s0: f64, sw0: f64| Edge {
                seg: Seg::Arc {
                    center,
                    radius: r,
                    start: s0,
                    sweep: sw0,
                },
                source: 0,
            };
            if sw >= TAU - 1e-12 {
                out.push(disc(center, ro));
                if ri > 0.0 {
                    out.push(disc(center, ri).reversed());
                }
                return;
            }
            let dir = Vec2::from_angle;
            let mut edges = vec![arc(ro, s, sw)];
            if ri > 0.0 {
                edges.push(line(center + dir(s + sw) * ro, center + dir(s + sw) * ri));
                edges.push(arc(ri, s + sw, -sw));
                edges.push(line(center + dir(s) * ri, center + dir(s) * ro));
            } else {
                // The stroke passes the centre: a wedge.
                edges.push(line(center + dir(s + sw) * ro, center));
                edges.push(line(center, center + dir(s) * ro));
            }
            out.push(Loop { edges });
        }
    }
}

/// A corner at `v` from tangent `t1` into tangent `t2`, as the polygon (or
/// disc) that fills the gap on the outer side, or `None` when smooth.
pub(crate) enum Corner {
    Disc,
    Poly(Vec<Vec2>),
}

pub(crate) fn corner(v: Vec2, t1: Vec2, t2: Vec2, h: f64, join: Join) -> Option<Corner> {
    let turn = t1.cross(t2).atan2(t1.dot(t2));
    if turn.abs() <= SMOOTH {
        return None;
    }
    // The gap opens on the side away from the turn.
    let side = if turn > 0.0 { -1.0 } else { 1.0 };
    let (n1, n2) = (t1.perp() * side, t2.perp() * side);
    let (p1, p2) = (v + n1 * h, v + n2 * h);
    let reversal = turn.abs() >= PI - 1e-9;
    match join {
        Join::Round => Some(Corner::Disc),
        Join::Bevel if reversal => None,
        Join::Bevel => Some(Corner::Poly(vec![v, p1, p2])),
        Join::Miter { limit } => {
            // Miter length / width = 1 / cos(turn / 2) (SVG's ratio).
            let c = (turn.abs() * 0.5).cos();
            if reversal || c <= 0.0 || 1.0 / c > limit {
                return if reversal {
                    None
                } else {
                    Some(Corner::Poly(vec![v, p1, p2]))
                };
            }
            let bis = (n1 + n2).normalized(0.0)?;
            Some(Corner::Poly(vec![v, p1, v + bis * (h / c), p2]))
        }
    }
}

fn check(chain: &Chain, width: f64, style: &LineStyle, limits: &Limits) -> Result<(), StrokeError> {
    if !(width.is_finite() && width > 0.0 && width <= MAX_WIDTH) {
        return Err(StrokeError::BadValue {
            what: "stroke width",
            value: width,
        });
    }
    style.validate()?;
    if chain.segs.len() > limits.pieces {
        return Err(StrokeError::Limit {
            what: "path pieces",
            count: chain.segs.len(),
        });
    }
    for s in &chain.segs {
        let ok = match *s {
            Seg::Line { a, b } => a.is_finite() && b.is_finite(),
            Seg::Arc {
                center,
                radius,
                start,
                sweep,
            } => {
                center.is_finite()
                    && radius.is_finite()
                    && radius > 0.0
                    && start.is_finite()
                    && sweep.is_finite()
                    && sweep.abs() <= TAU + 1e-9
            }
        };
        if !ok {
            return Err(StrokeError::BadValue {
                what: "path geometry",
                value: f64::NAN,
            });
        }
    }
    Ok(())
}

/// The corners of a chain: (vertex, tangent in, tangent out).
fn corners(chain: &Chain) -> Vec<(Vec2, Vec2, Vec2)> {
    let n = chain.segs.len();
    let count = if chain.closed { n } else { n.saturating_sub(1) };
    (0..count)
        .filter_map(|i| {
            let (a, b) = (chain.segs.get(i)?, chain.segs.get((i + 1) % n)?);
            Some((a.end(), a.tangent(1.0), b.tangent(0.0)))
        })
        .collect()
}

/// The ends of an open chain: (point, outward tangent).
fn ends(chain: &Chain) -> Vec<(Vec2, Vec2)> {
    if chain.closed {
        return Vec::new();
    }
    match (chain.segs.first(), chain.segs.last()) {
        (Some(f), Some(l)) => vec![(f.start(), -f.tangent(0.0)), (l.end(), l.tangent(1.0))],
        _ => Vec::new(),
    }
}

/// The tube of half-width `h` with `cap` and `join`, as loops for one
/// non-zero union.
fn tube(chain: &Chain, h: f64, cap: Cap, join: Join) -> Vec<Loop> {
    let mut loops = Vec::new();
    for s in &chain.segs {
        band(s, h, &mut loops);
    }
    for (v, t1, t2) in corners(chain) {
        match corner(v, t1, t2, h, join) {
            Some(Corner::Disc) => loops.push(disc(v, h)),
            Some(Corner::Poly(p)) => loops.push(polygon(&p)),
            None => {}
        }
    }
    for (p, out) in ends(chain) {
        match cap {
            Cap::Butt => {}
            Cap::Round => loops.push(disc(p, h)),
            Cap::Square => {
                let n = out.perp() * h;
                let f = out * h;
                loops.push(polygon(&[p - n, p + f - n, p + f + n, p + n]));
            }
        }
    }
    loops
}

/// The area a closed chain encloses (non-zero), as a CCW-outer [`Area`].
fn enclosed(chain: &Chain) -> Area {
    let l = Loop {
        edges: chain
            .segs
            .iter()
            .map(|s| Edge { seg: *s, source: 0 })
            .collect(),
    };
    boolean(&Area { loops: vec![l] }, &Area::default(), BoolOp::Union)
}

/// The exact outline of `chain` stroked at `width` mm with `style`: CCW outer
/// loops, CW holes, true lines and arcs. Open chains are always centred;
/// [`Align::Inside`] and [`Align::Outside`] put a closed chain's whole width
/// on one side (the join then shapes only that side).
///
/// # Errors
/// [`StrokeError`] for a bad width, style or geometry, or a chain over
/// `limits`.
pub fn expand(
    chain: &Chain,
    width: f64,
    style: &LineStyle,
    limits: &Limits,
) -> Result<Area, StrokeError> {
    check(chain, width, style, limits)?;
    if chain.segs.is_empty() {
        return Ok(Area::default());
    }
    // Extra strokes (#429): the union of every stroke's outline.
    if !style.extra.is_empty() {
        let mut loops = Vec::new();
        for ls in layer_strokes(chain, width, style, limits)? {
            for c in &ls.chains {
                loops.extend(expand(c, ls.width, &ls.line, limits)?.loops);
            }
        }
        return Ok(boolean(&Area { loops }, &Area::default(), BoolOp::Union));
    }
    // A width profile (#426): the cleaned variable-width outline, centred.
    if let Some(prof) = style.varying() {
        let mut areas = Vec::new();
        for (c, u0, u1) in profiled_runs(chain, style, limits)? {
            let p = vary::Profiled::new(&c, width, prof, u0, u1);
            areas.push(vary::clean(&p, style.cap, style.join, EXPORT_TOL, limits)?.area);
        }
        let mut out = union_all(&areas);
        locus_region::offset::drop_specks(&mut out);
        return Ok(out);
    }
    // A dashed stroke is the union of its dashes, each capped and joined
    // like a stroke of its own, always centred.
    if let Some(d) = style.dash {
        let solid = LineStyle {
            dash: None,
            align: Align::Centre,
            ..style.clone()
        };
        let mut loops = Vec::new();
        for c in dashes(chain, &d, limits)? {
            let h = width * 0.5;
            loops.extend(tube(&c, h, solid.cap, solid.join));
        }
        let mut out = boolean(&Area { loops }, &Area::default(), BoolOp::Union);
        locus_region::offset::drop_specks(&mut out);
        return Ok(out);
    }
    let align = if chain.closed {
        style.align
    } else {
        Align::Centre
    };
    let h = match align {
        Align::Centre => width * 0.5,
        Align::Inside | Align::Outside => width,
    };
    let t = Area {
        loops: tube(chain, h, style.cap, style.join),
    };
    let mut out = match align {
        Align::Centre => boolean(&t, &Area::default(), BoolOp::Union),
        Align::Inside => boolean(&enclosed(chain), &t, BoolOp::Intersect),
        Align::Outside => boolean(&t, &enclosed(chain), BoolOp::Subtract),
    };
    locus_region::offset::drop_specks(&mut out);
    Ok(out)
}

/// The runs a profiled stroke draws, each with its part of the profile
/// (`u0..u1` of the whole path's length): every dash, or the whole chain.
fn profiled_runs(
    chain: &Chain,
    style: &LineStyle,
    limits: &Limits,
) -> Result<Vec<(Chain, f64, f64)>, StrokeError> {
    let Some(d) = &style.dash else {
        return Ok(vec![(chain.clone(), 0.0, 1.0)]);
    };
    let lens: Vec<f64> = chain.segs.iter().map(Seg::length).collect();
    let total: f64 = lens.iter().sum();
    if total.is_nan() || total <= 0.0 {
        return Ok(Vec::new());
    }
    Ok(dash::dash_spans(chain, d, limits)?
        .into_iter()
        .map(|(a, b)| (dash::slice(chain, &lens, a, b), a / total, b / total))
        .collect())
}

/// The stroke's outline for drawing and editing (SVG/PDF, Expand to
/// geometry): as [`expand`], except that variable-width spans (#426) keep
/// the fitted cubics within `tol` mm (ADR 0019 option (b), owner-approved).
/// Constant-width strokes are exact lines and arcs. Outlines of separate
/// strokes (extra strokes, dashes) are listed apart: fill them non-zero.
///
/// # Errors
/// As [`expand`].
pub fn outline(
    chain: &Chain,
    width: f64,
    style: &LineStyle,
    tol: f64,
    limits: &Limits,
) -> Result<Outline, StrokeError> {
    check(chain, width, style, limits)?;
    let tol = if tol.is_finite() {
        tol.clamp(1e-6, 1.0)
    } else {
        EXPORT_TOL
    };
    if !style.extra.is_empty() {
        let mut out = Outline::default();
        for ls in layer_strokes(chain, width, style, limits)? {
            for c in &ls.chains {
                out.loops
                    .extend(outline(c, ls.width, &ls.line, tol, limits)?.loops);
            }
        }
        return Ok(out);
    }
    let Some(prof) = style.varying() else {
        return Ok(Outline::from_area(&expand(chain, width, style, limits)?));
    };
    let mut out = Outline::default();
    for (c, u0, u1) in profiled_runs(chain, style, limits)? {
        let p = vary::Profiled::new(&c, width, prof, u0, u1);
        let cleaned = vary::clean(&p, style.cap, style.join, tol, limits)?;
        out.loops.extend(vary::map_back(&cleaned, tol).loops);
    }
    Ok(out)
}

/// The visual bounds of the stroke, without expanding it: the pieces grown
/// by the stroke's reach, plus miter tips and square-cap corners. Never
/// smaller than the outline, so a page never clips it (§8, #450).
#[must_use]
pub fn bounds(chain: &Chain, width: f64, style: &LineStyle) -> Option<Aabb> {
    if !style.extra.is_empty() {
        // Every stroke's bounds, at its offset.
        let strokes = layer_strokes(chain, width, style, &Limits::default()).ok()?;
        return strokes
            .iter()
            .flat_map(|ls| {
                ls.chains
                    .iter()
                    .filter_map(|c| bounds(c, ls.width, &ls.line))
            })
            .reduce(Aabb::union);
    }
    let w = if width.is_finite() {
        width.max(0.0)
    } else {
        0.0
    };
    // A width profile (#426) reaches at most its largest side factor; a
    // square cap's corner √2 of that; a miter's tip as for that width.
    if let Some(prof) = style.varying() {
        let h = w * 0.5 * prof.max_factor();
        let r = if style.cap == Cap::Square {
            h * core::f64::consts::SQRT_2
        } else {
            h
        };
        let bb = chain.segs.iter().map(Seg::bbox).reduce(Aabb::union)?;
        let mut bb = Aabb::from_points(bb.min - Vec2::new(r, r), bb.max + Vec2::new(r, r));
        for (v, t1, t2) in corners(chain) {
            if let Some(Corner::Poly(p)) = corner(v, t1, t2, h, style.join) {
                for q in p {
                    bb = bb.include(q);
                }
            }
        }
        return Some(bb);
    }
    // Dashed strokes are always centred (see `expand`).
    let align = if chain.closed && style.dash.is_none() {
        style.align
    } else {
        Align::Centre
    };
    let h = match align {
        Align::Centre => w * 0.5,
        Align::Inside => 0.0,
        Align::Outside => w,
    };
    let mut bb = chain.segs.iter().map(Seg::bbox).reduce(Aabb::union)?;
    // Square caps on dashes sit anywhere along the path: their corners reach
    // h·√2 from it.
    let r = if style.dash.is_some() && style.cap == Cap::Square {
        h * core::f64::consts::SQRT_2
    } else {
        h
    };
    bb = Aabb::from_points(bb.min - Vec2::new(r, r), bb.max + Vec2::new(r, r));
    if h > 0.0 {
        for (v, t1, t2) in corners(chain) {
            if let Some(Corner::Poly(p)) = corner(v, t1, t2, h, style.join) {
                for q in p {
                    bb = bb.include(q);
                }
            }
        }
        if style.cap == Cap::Square {
            for (p, out) in ends(chain) {
                let (n, f) = (out.perp() * h, out * h);
                bb = bb.include(p + f - n).include(p + f + n);
            }
        }
    }
    Some(bb)
}

/// Canvas triangles for `chain` at `width` and `style`, within `tol` mm of the
/// outline: a strip per piece, fans and polygons for joins and caps. No
/// boolean, so overlapping parts (tight inner corners) are covered twice;
/// that only shows with opacity below 1 (ADR 0019, S3). Appends to `out`.
///
/// # Errors
/// As [`expand`], and [`StrokeError::Limit`] past `limits.triangles`.
pub fn mesh(
    chain: &Chain,
    width: f64,
    style: &LineStyle,
    tol: f64,
    limits: &Limits,
    out: &mut Vec<[Vec2; 3]>,
) -> Result<(), StrokeError> {
    check(chain, width, style, limits)?;
    if !style.extra.is_empty() {
        for ls in layer_strokes(chain, width, style, limits)? {
            for c in &ls.chains {
                mesh(c, ls.width, &ls.line, tol, limits, out)?;
            }
        }
        return Ok(());
    }
    if let Some(prof) = style.varying() {
        let tol = if tol.is_finite() { tol.max(1e-6) } else { 1e-3 };
        for (c, u0, u1) in profiled_runs(chain, style, limits)? {
            let p = vary::Profiled::new(&c, width, prof, u0, u1);
            vary::mesh(&p, style.cap, style.join, tol, limits, out)?;
        }
        return Ok(());
    }
    if let Some(d) = style.dash {
        let solid = LineStyle {
            dash: None,
            align: Align::Centre,
            ..style.clone()
        };
        for c in dashes(chain, &d, limits)? {
            mesh(&c, width, &solid, tol, limits, out)?;
        }
        return Ok(());
    }
    let tol = if tol.is_finite() { tol.max(1e-6) } else { 1e-3 };
    let align = if chain.closed {
        style.align
    } else {
        Align::Centre
    };
    // Offsets of the two strip sides from the path, + = left of travel.
    // For a closed chain, the inside is on the left when it runs CCW.
    let ccw = enclosed_sign(chain) >= 0.0;
    let (lo, hi) = match align {
        Align::Centre => (-width * 0.5, width * 0.5),
        Align::Inside if ccw => (0.0, width),
        Align::Inside => (-width, 0.0),
        Align::Outside if ccw => (-width, 0.0),
        Align::Outside => (0.0, width),
    };
    let start = out.len();
    let budget = |out: &Vec<[Vec2; 3]>| {
        let n = out.len() - start;
        if n > limits.triangles {
            Err(StrokeError::Limit {
                what: "canvas triangles",
                count: n,
            })
        } else {
            Ok(())
        }
    };
    for s in &chain.segs {
        let steps = match *s {
            Seg::Line { .. } => 1,
            Seg::Arc { radius, sweep, .. } => {
                let r = radius + lo.abs().max(hi.abs());
                let step = 2.0 * (1.0 - (tol / r).min(1.0)).acos();
                ((sweep.abs() / step.max(1e-6)).ceil() as usize).clamp(1, limits.triangles)
            }
        };
        let side = |t: f64| {
            let (p, n) = (s.at(t), s.tangent(t).perp());
            (p + n * lo, p + n * hi)
        };
        let (mut a0, mut b0) = side(0.0);
        for k in 1..=steps {
            let (a1, b1) = side(k as f64 / steps as f64);
            out.push([a0, a1, b1]);
            out.push([a0, b1, b0]);
            (a0, b0) = (a1, b1);
        }
        budget(out)?;
    }
    let fan = |out: &mut Vec<[Vec2; 3]>, c: Vec2, r: f64, a0: f64, sweep: f64| {
        if r <= 0.0 {
            return;
        }
        let step = 2.0 * (1.0 - (tol / r).min(1.0)).acos();
        let n = ((sweep.abs() / step.max(1e-6)).ceil() as usize).clamp(1, 4096);
        let mut p = c + Vec2::from_angle(a0) * r;
        for k in 1..=n {
            let q = c + Vec2::from_angle(a0 + sweep * k as f64 / n as f64) * r;
            out.push([c, p, q]);
            p = q;
        }
    };
    for (v, t1, t2) in corners(chain) {
        let turn = t1.cross(t2).atan2(t1.dot(t2));
        // The outer side and its offset (centred: h; one-sided: only when
        // the stroke lies on the outer side).
        let outer_left = turn < 0.0;
        let h = if outer_left { hi } else { -lo };
        if h <= 0.0 {
            continue;
        }
        match corner(v, t1, t2, h, style.join) {
            Some(Corner::Disc) => {
                let side = if outer_left { 1.0 } else { -1.0 };
                let a0 = (t1.perp() * side).angle();
                fan(out, v, h, a0, turn);
            }
            Some(Corner::Poly(p)) => {
                for w in p.windows(2).skip(1) {
                    if let [a, b] = w {
                        out.push([v, *a, *b]);
                    }
                }
            }
            None => {}
        }
    }
    let h = width * 0.5;
    for (p, o) in ends(chain) {
        match style.cap {
            Cap::Butt => {}
            Cap::Round => fan(out, p, h, o.perp().angle() - PI, PI),
            Cap::Square => {
                let (n, f) = (o.perp() * h, o * h);
                out.push([p - n, p + f - n, p + f + n]);
                out.push([p - n, p + f + n, p + n]);
            }
        }
    }
    budget(out)
}

/// Signed area of a closed chain (CCW > 0); 0 for open ones.
fn enclosed_sign(chain: &Chain) -> f64 {
    if !chain.closed {
        return 0.0;
    }
    chain.segs.iter().map(Seg::area_term).sum()
}

/// Whether two points are the same vertex (`locus-region`'s snap).
pub(crate) fn same(a: Vec2, b: Vec2) -> bool {
    a.distance(b) <= SNAP
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: f64, y: f64) -> Vec2 {
        Vec2::new(x, y)
    }

    fn polyline(pts: &[Vec2], closed: bool) -> Chain {
        let mut segs: Vec<Seg> = pts
            .windows(2)
            .map(|w| Seg::Line { a: w[0], b: w[1] })
            .collect();
        if closed {
            segs.push(Seg::Line {
                a: pts[pts.len() - 1],
                b: pts[0],
            });
        }
        Chain { segs, closed }
    }

    fn style(cap: Cap, join: Join, align: Align) -> LineStyle {
        LineStyle {
            cap,
            join,
            align,
            ..LineStyle::default()
        }
    }

    fn area(c: &Chain, w: f64, s: LineStyle) -> f64 {
        expand(c, w, &s, &Limits::default()).unwrap().area()
    }

    const EPS: f64 = 1e-6;

    #[test]
    fn caps_on_a_straight_line() {
        let c = polyline(&[v(0.0, 0.0), v(10.0, 0.0)], false);
        let w = 2.0;
        let a = |cap| area(&c, w, style(cap, Join::Round, Align::Centre));
        assert!((a(Cap::Butt) - 20.0).abs() < EPS);
        assert!((a(Cap::Square) - 24.0).abs() < EPS);
        assert!((a(Cap::Round) - (20.0 + PI)).abs() < EPS);
    }

    #[test]
    fn joins_on_a_closed_square() {
        let s = 10.0;
        let c = polyline(&[v(0.0, 0.0), v(s, 0.0), v(s, s), v(0.0, s)], true);
        let (w, h) = (2.0, 1.0);
        let miter = (s + w).powi(2) - (s - w).powi(2);
        let a = |join| area(&c, w, style(Cap::Butt, join, Align::Centre));
        assert!((a(Join::Miter { limit: 4.0 }) - miter).abs() < EPS);
        assert!((a(Join::Bevel) - (miter - 4.0 * h * h / 2.0)).abs() < EPS);
        assert!((a(Join::Round) - (miter - 4.0 * (h * h - PI * h * h / 4.0))).abs() < EPS);
        // A 90° corner's miter ratio is √2: under a limit of 1.2 it bevels.
        assert!((a(Join::Miter { limit: 1.2 }) - a(Join::Bevel)).abs() < EPS);
    }

    #[test]
    fn inside_and_outside_put_the_width_on_one_side() {
        let s = 10.0;
        for closed_ccw in [true, false] {
            let mut pts = vec![v(0.0, 0.0), v(s, 0.0), v(s, s), v(0.0, s)];
            if !closed_ccw {
                pts.reverse();
            }
            let c = polyline(&pts, true);
            let w = 1.0;
            let inside = area(
                &c,
                w,
                style(Cap::Butt, Join::Miter { limit: 4.0 }, Align::Inside),
            );
            let outside = area(
                &c,
                w,
                style(Cap::Butt, Join::Miter { limit: 4.0 }, Align::Outside),
            );
            assert!((inside - (s * s - (s - 2.0 * w).powi(2))).abs() < EPS);
            assert!((outside - ((s + 2.0 * w).powi(2) - s * s)).abs() < EPS);
        }
    }

    #[test]
    fn a_circle_strokes_to_an_annulus_with_true_arcs() {
        let c = Chain {
            segs: vec![Seg::Arc {
                center: v(0.0, 0.0),
                radius: 5.0,
                start: 0.0,
                sweep: TAU,
            }],
            closed: true,
        };
        let a = expand(&c, 2.0, &LineStyle::default(), &Limits::default()).unwrap();
        assert!((a.area() - PI * (36.0 - 16.0)).abs() < EPS);
        assert!(a.edges().all(|e| e.seg.is_arc()));
    }

    #[test]
    fn bounds_cover_the_outline() {
        let c = polyline(&[v(0.0, 0.0), v(10.0, 0.0), v(0.0, 1.0)], false);
        for s in [
            style(Cap::Square, Join::Miter { limit: 100.0 }, Align::Centre),
            style(Cap::Round, Join::Bevel, Align::Centre),
            style(Cap::Butt, Join::Round, Align::Centre),
        ] {
            let out = expand(&c, 1.0, &s, &Limits::default()).unwrap();
            let ob = out
                .loops
                .iter()
                .filter_map(Loop::bbox)
                .reduce(Aabb::union)
                .unwrap();
            let b = bounds(&c, 1.0, &s).unwrap();
            assert!(b.min.x <= ob.min.x + EPS && b.min.y <= ob.min.y + EPS);
            assert!(
                b.max.x >= ob.max.x - EPS && b.max.y >= ob.max.y - EPS,
                "{s:?}"
            );
        }
    }

    #[test]
    fn mesh_area_matches_a_simple_stroke() {
        let c = polyline(&[v(0.0, 0.0), v(10.0, 0.0)], false);
        let mut tris = Vec::new();
        mesh(
            &c,
            2.0,
            &style(Cap::Square, Join::Round, Align::Centre),
            0.01,
            &Limits::default(),
            &mut tris,
        )
        .unwrap();
        let a: f64 = tris
            .iter()
            .map(|t| ((t[1] - t[0]).cross(t[2] - t[0]) * 0.5).abs())
            .sum();
        assert!((a - 24.0).abs() < EPS);
    }

    fn profiled(pts: &[(f64, f64, f64)], cap: Cap, join: Join) -> LineStyle {
        LineStyle {
            cap,
            join,
            profile: Some(WidthProfile {
                points: pts
                    .iter()
                    .map(|&(t, l, r)| WidthPoint::new(t, l, r))
                    .collect(),
                seed: None,
                alone: false,
            }),
            ..LineStyle::default()
        }
    }

    /// Area of an outline, flattened finely.
    fn outline_area(o: &Outline) -> f64 {
        o.loops
            .iter()
            .map(|l| {
                let mut pts = Vec::new();
                for p in l {
                    match p {
                        OutPiece::Seg(s) => s.flatten_into(1e-5, &mut pts),
                        OutPiece::Cubic(c) => {
                            let f = c.flatten(1e-5);
                            pts.extend_from_slice(&f[..f.len() - 1]);
                        }
                    }
                }
                let n = pts.len();
                (0..n).map(|i| pts[i].cross(pts[(i + 1) % n])).sum::<f64>() * 0.5
            })
            .sum()
    }

    #[test]
    fn a_constant_profile_strokes_as_the_plain_width() {
        // Factor 2 at width 1 is a 2 mm stroke.
        let c = polyline(&[v(0.0, 0.0), v(10.0, 0.0)], false);
        let s = profiled(&[(0.0, 2.0, 2.0), (1.0, 2.0, 2.0)], Cap::Butt, Join::Round);
        assert!((area(&c, 1.0, s) - 20.0).abs() < 1e-3);
        // A closed square with round joins, against the exact stroke.
        let sq = polyline(
            &[v(0.0, 0.0), v(10.0, 0.0), v(10.0, 10.0), v(0.0, 10.0)],
            true,
        );
        let s = profiled(&[(0.0, 1.5, 1.5), (1.0, 1.5, 1.5)], Cap::Round, Join::Round);
        let exact = area(&sq, 3.0, style(Cap::Round, Join::Round, Align::Centre));
        assert!((area(&sq, 2.0, s) - exact).abs() < 0.01);
    }

    #[test]
    fn a_taper_has_the_eased_area() {
        // Width 2 easing from 0 to factor 2 (4 mm): the smoothstep averages
        // half, so 10 × 4 × ½ = 20.
        let c = polyline(&[v(0.0, 0.0), v(10.0, 0.0)], false);
        let s = profiled(&[(0.0, 0.0, 0.0), (1.0, 2.0, 2.0)], Cap::Butt, Join::Round);
        assert!(s.machine_styled() && s.varying().is_some());
        assert!((area(&c, 2.0, s.clone()) - 20.0).abs() < 0.01);
        let o = outline(&c, 2.0, &s, 0.01, &Limits::default()).unwrap();
        assert!(
            o.loops
                .iter()
                .flatten()
                .any(|p| matches!(p, OutPiece::Cubic(_)))
        );
        assert!((outline_area(&o) - 20.0).abs() < 0.05);
        let mut tris = Vec::new();
        mesh(&c, 2.0, &s, 0.001, &Limits::default(), &mut tris).unwrap();
        let a: f64 = tris
            .iter()
            .map(|t| ((t[1] - t[0]).cross(t[2] - t[0]) * 0.5).abs())
            .sum();
        assert!((a - 20.0).abs() < 0.05, "{a}");
        // Asymmetric: only the left side.
        let left = profiled(&[(0.0, 2.0, 0.0), (1.0, 2.0, 0.0)], Cap::Butt, Join::Round);
        let out = expand(&c, 2.0, &left, &Limits::default()).unwrap();
        let bb = out
            .loops
            .iter()
            .filter_map(Loop::bbox)
            .reduce(Aabb::union)
            .unwrap();
        assert!(bb.min.y > -1e-6 && (bb.max.y - 2.0).abs() < 1e-6);
    }

    #[test]
    fn a_profile_on_an_arc_and_dashes_reads_the_whole_path() {
        let arc = Chain {
            segs: vec![Seg::Arc {
                center: v(0.0, 0.0),
                radius: 5.0,
                start: 0.0,
                sweep: PI,
            }],
            closed: false,
        };
        let s = profiled(&[(0.0, 2.0, 2.0), (1.0, 2.0, 2.0)], Cap::Butt, Join::Round);
        // A half annulus between radii 3 and 7.
        assert!((area(&arc, 2.0, s.clone()) - PI / 2.0 * (49.0 - 9.0)).abs() < 0.01);
        // Dashed: a taper is read across the whole path, so the last dash
        // is the thickest.
        let c = polyline(&[v(0.0, 0.0), v(10.0, 0.0)], false);
        let mut s = profiled(&[(0.0, 0.0, 0.0), (1.0, 2.0, 2.0)], Cap::Butt, Join::Round);
        s.dash = Some(Dash::new(&[1.0, 1.0], 0.0, crate::DashAlign::Exact).unwrap());
        let out = expand(&c, 2.0, &s, &Limits::default()).unwrap();
        let tallest = out
            .loops
            .iter()
            .filter_map(Loop::bbox)
            .max_by(|a, b| a.height().total_cmp(&b.height()))
            .unwrap();
        assert!(tallest.min.x > 7.9, "{tallest:?}");
    }

    #[test]
    fn profile_bounds_cover_the_outline() {
        let c = polyline(&[v(0.0, 0.0), v(10.0, 0.0), v(0.0, 3.0)], false);
        for s in [
            profiled(
                &[(0.0, 0.2, 3.0), (0.5, 4.0, 1.0), (1.0, 1.0, 0.0)],
                Cap::Square,
                Join::Miter { limit: 10.0 },
            ),
            profiled(&[(0.0, 0.2, 3.0), (1.0, 1.0, 0.0)], Cap::Round, Join::Bevel),
        ] {
            let out = expand(&c, 1.0, &s, &Limits::default()).unwrap();
            let ob = out
                .loops
                .iter()
                .filter_map(Loop::bbox)
                .reduce(Aabb::union)
                .unwrap();
            let b = bounds(&c, 1.0, &s).unwrap();
            assert!(
                b.min.x <= ob.min.x + EPS && b.min.y <= ob.min.y + EPS,
                "{s:?}"
            );
            assert!(
                b.max.x >= ob.max.x - EPS && b.max.y >= ob.max.y - EPS,
                "{s:?}"
            );
        }
    }

    #[test]
    fn a_uniform_profile_is_the_plain_stroke() {
        let s = profiled(&[(0.0, 1.0, 1.0), (1.0, 1.0, 1.0)], Cap::Round, Join::Round);
        assert!(s.varying().is_none() && !s.machine_styled());
    }

    #[test]
    fn bad_input_is_refused_not_panicked() {
        let c = polyline(&[v(0.0, 0.0), v(f64::NAN, 0.0)], false);
        assert!(expand(&c, 1.0, &LineStyle::default(), &Limits::default()).is_err());
        let ok = polyline(&[v(0.0, 0.0), v(1.0, 0.0)], false);
        for w in [0.0, -1.0, f64::NAN, 1e9] {
            assert!(expand(&ok, w, &LineStyle::default(), &Limits::default()).is_err());
        }
        let tight = Limits {
            pieces: 0,
            ..Limits::default()
        };
        assert!(matches!(
            expand(&ok, 1.0, &LineStyle::default(), &tight),
            Err(StrokeError::Limit { .. })
        ));
    }
}
