// SPDX-License-Identifier: AGPL-3.0-or-later

//! Variable-width strokes (#426; ADR 0019 decision 3, spike S1 and its
//! decisions). The pipeline the spike measured (`spikes/stroke/src/build.rs`):
//!
//! 1. Each side, C(t) ± n(t)·w(t), is fitted with Hermite cubics within T/2,
//!    per band: a span of one centreline piece, cut where the tangent has
//!    turned 90° and where a side has a cusp (its offset passes the centre of
//!    curvature). Derivatives are taken inside the band, never across a cut.
//! 2. Each band (right side forward, left side back) is lowered to biarcs
//!    within T/2 and normalised alone, so a swallowtail of one band can't
//!    cancel the cover of another.
//! 3. One `locus-region` union takes the bands, the join pieces at corners
//!    and the caps: the cleaned outline in lines and arcs (option (a)).
//! 4. Option (b), owner-approved: every cleaned run that came from one fitted
//!    cubic maps back to that cubic (whole, or a sub-curve snapped to the
//!    run's ends), unless the sub-curve strays more than T from the run,
//!    where the side is itself an arc and the arcs are the better answer.
//!
//! The canvas doesn't run this: it samples the sides into a triangle strip
//! (spike S3), see [`mesh`].

use crate::profile::WidthProfile;
use crate::{Cap, Chain, Corner, Join, Limits, StrokeError, corner, polygon};
use core::f64::consts::{FRAC_PI_2, PI, TAU};
use locus_geom::{Cubic, Line, Piece, Vec2};
use locus_region::{Area, BoolOp, Edge, Loop, Seg, boolean, union_all};

/// Corners turning less than this (radians) are smooth: no join piece.
const SMOOTH: f64 = 1e-6;
/// Deepest bisection of one side span while fitting.
const MAX_DEPTH: u32 = 22;
/// Samples per side span checked against a fitted cubic.
const CHECK: usize = 10;
const POLY: usize = 32;
/// Samples per arc searched for a side cusp.
const CUSP_SAMPLES: usize = 256;

/// One piece of an outline: an exact line or arc, or a fitted cubic.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OutPiece {
    Seg(Seg),
    Cubic(Cubic),
}

impl OutPiece {
    #[must_use]
    pub fn start(&self) -> Vec2 {
        match self {
            Self::Seg(s) => s.start(),
            Self::Cubic(c) => c.p0,
        }
    }

    #[must_use]
    pub fn end(&self) -> Vec2 {
        match self {
            Self::Seg(s) => s.end(),
            Self::Cubic(c) => c.p1,
        }
    }
}

/// A stroke's outline for drawing and editing (option (b)): closed loops,
/// CCW outer and CW holes, of lines, arcs and cubics. Where loops of
/// separate strokes (dashes, extra strokes) overlap, fill them non-zero.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Outline {
    pub loops: Vec<Vec<OutPiece>>,
}

impl Outline {
    /// An exact line/arc outline as is.
    #[must_use]
    pub fn from_area(a: &Area) -> Self {
        Self {
            loops: a
                .loops
                .iter()
                .map(|l| l.edges.iter().map(|e| OutPiece::Seg(e.seg)).collect())
                .collect(),
        }
    }

    /// Pieces in every loop.
    #[must_use]
    pub fn len(&self) -> usize {
        self.loops.iter().map(Vec::len).sum()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.loops.iter().all(Vec::is_empty)
    }
}

/// A chain with its width profile over the length fractions `u0..u1` (a
/// dash takes its part of the whole path's profile).
pub(crate) struct Profiled<'a> {
    chain: &'a Chain,
    /// Length before each piece.
    cum: Vec<f64>,
    lens: Vec<f64>,
    total: f64,
    half: f64,
    profile: &'a WidthProfile,
    u0: f64,
    u1: f64,
}

impl<'a> Profiled<'a> {
    pub(crate) fn new(
        chain: &'a Chain,
        width: f64,
        profile: &'a WidthProfile,
        u0: f64,
        u1: f64,
    ) -> Self {
        let lens: Vec<f64> = chain.segs.iter().map(Seg::length).collect();
        let mut cum = Vec::with_capacity(lens.len());
        let mut acc = 0.0;
        for l in &lens {
            cum.push(acc);
            acc += l;
        }
        Self {
            chain,
            cum,
            lens,
            total: acc,
            half: width * 0.5,
            profile,
            u0,
            u1,
        }
    }

    fn seg(&self, k: usize) -> Option<&Seg> {
        self.chain.segs.get(k)
    }

    /// Profile parameter at piece `k`, `t`: lines and arcs are uniform in
    /// arc length, so this is exact.
    fn u(&self, k: usize, t: f64) -> f64 {
        if self.total <= 0.0 {
            return self.u0;
        }
        let (c, l) = (
            self.cum.get(k).copied().unwrap_or(0.0),
            self.lens.get(k).copied().unwrap_or(0.0),
        );
        self.u0 + (self.u1 - self.u0) * ((c + t * l) / self.total)
    }

    /// Left and right reach (mm) at piece `k`, `t`.
    fn widths(&self, k: usize, t: f64) -> (f64, f64) {
        let (l, r) = self.profile.at(self.u(k, t));
        (l * self.half, r * self.half)
    }

    /// Left and right side points at piece `k`, `t`.
    fn sides(&self, k: usize, t: f64) -> (Vec2, Vec2) {
        let Some(s) = self.seg(k) else {
            return (Vec2::new(0.0, 0.0), Vec2::new(0.0, 0.0));
        };
        let (c, n) = (s.at(t), s.tangent(t).perp());
        let (l, r) = self.widths(k, t);
        (c + n * l, c - n * r)
    }

    /// Spans of piece `k`: arcs cut every 90° of turn and at side cusps.
    fn spans(&self, k: usize) -> Vec<(f64, f64)> {
        let Some(Seg::Arc { radius, sweep, .. }) = self.seg(k).copied() else {
            return vec![(0.0, 1.0)];
        };
        let quarters = (sweep.abs() / FRAC_PI_2).ceil().clamp(1.0, 8.0) as usize;
        let mut cuts: Vec<f64> = (1..quarters).map(|i| i as f64 / quarters as f64).collect();
        // The side towards the centre (left on a CCW arc) has a cusp where
        // its reach crosses the radius.
        let inner = |t: f64| {
            let (l, r) = self.widths(k, t);
            radius - if sweep >= 0.0 { l } else { r }
        };
        let mut prev = inner(0.0);
        for i in 1..=CUSP_SAMPLES {
            let (a, b) = (
                (i - 1) as f64 / CUSP_SAMPLES as f64,
                i as f64 / CUSP_SAMPLES as f64,
            );
            let cur = inner(b);
            if prev * cur < 0.0 {
                let (mut lo, mut hi) = (a, b);
                for _ in 0..50 {
                    let m = 0.5 * (lo + hi);
                    if inner(lo) * inner(m) <= 0.0 {
                        hi = m;
                    } else {
                        lo = m;
                    }
                }
                cuts.push(0.5 * (lo + hi));
            }
            prev = cur;
        }
        cuts.retain(|t| *t > 1e-9 && *t < 1.0 - 1e-9);
        cuts.push(0.0);
        cuts.push(1.0);
        cuts.sort_by(f64::total_cmp);
        cuts.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
        cuts.windows(2)
            .filter_map(|w| match w {
                [a, b] => Some((*a, *b)),
                _ => None,
            })
            .collect()
    }
}

fn line(a: Vec2, b: Vec2) -> Option<Edge> {
    (a.distance(b) > 1e-9).then_some(Edge {
        seg: Seg::Line { a, b },
        source: 0,
    })
}

/// A pie sector (centre, radius, start angle, CCW sweep) as a CCW loop.
fn sector(c: Vec2, r: f64, a0: f64, sweep: f64) -> Option<Loop> {
    if r <= 1e-7 || sweep <= SMOOTH {
        return None;
    }
    let arc = |start, sweep| Edge {
        seg: Seg::Arc {
            center: c,
            radius: r,
            start,
            sweep,
        },
        source: 0,
    };
    if sweep >= TAU - 1e-12 {
        return Some(Loop {
            edges: vec![arc(0.0, TAU)],
        });
    }
    let p0 = c + Vec2::from_angle(a0) * r;
    let p1 = c + Vec2::from_angle(a0 + sweep) * r;
    let mut edges = Vec::new();
    edges.extend(line(c, p0));
    edges.push(arc(a0, sweep));
    edges.extend(line(p1, c));
    Some(Loop { edges })
}

/// `c` lowered to oriented lines and arcs within `tol`, tagged `source`.
fn lower(c: &Cubic, tol: f64, source: u64, out: &mut Vec<Edge>) {
    for (p, back) in c.to_pieces_oriented(tol) {
        let seg = match p {
            Piece::Line(l) => Seg::Line { a: l.a, b: l.b },
            Piece::Arc(a) => {
                let s = Seg::Arc {
                    center: a.center,
                    radius: a.radius(),
                    start: a.start_angle(),
                    sweep: a.sweep(),
                };
                if back { s.reversed() } else { s }
            }
        };
        out.push(Edge { seg, source });
    }
}

/// Derivative of `f` by central differences, one-sided at the span's ends:
/// never across a cut (ADR 0019, band rules).
fn deriv(f: &dyn Fn(f64) -> Vec2, t: f64, lo: f64, hi: f64) -> Vec2 {
    let h = 1e-7_f64.min(0.25 * (hi - lo));
    let (a, b) = ((t - h).max(lo), (t + h).min(hi));
    if b <= a {
        return Vec2::new(0.0, 0.0);
    }
    (f(b) - f(a)) * (1.0 / (b - a))
}

fn poly_dist(pl: &[Vec2], p: Vec2) -> f64 {
    pl.windows(2)
        .filter_map(|w| match w {
            [a, b] => Some(Line::new(*a, *b).distance_to(p)),
            _ => None,
        })
        .fold(f64::INFINITY, f64::min)
}

/// Fits `f` on [t0, t1] with Hermite cubics, each within `tol` of `f` both
/// ways (sampled). A span still failing at the depth limit becomes its chord.
/// Stops with [`StrokeError::Limit`] past `budget` cubics in `out`.
fn fit(
    f: &dyn Fn(f64) -> Vec2,
    t0: f64,
    t1: f64,
    tol: f64,
    budget: usize,
    out: &mut Vec<Cubic>,
) -> Result<(), StrokeError> {
    fit_rec(f, (t0, t1), t0, t1, tol, 0, budget, out)
}

#[allow(clippy::too_many_arguments)]
fn fit_rec(
    f: &dyn Fn(f64) -> Vec2,
    dom: (f64, f64),
    t0: f64,
    t1: f64,
    tol: f64,
    depth: u32,
    budget: usize,
    out: &mut Vec<Cubic>,
) -> Result<(), StrokeError> {
    if out.len() > budget {
        return Err(StrokeError::Limit {
            what: "outline pieces",
            count: out.len(),
        });
    }
    let dt = t1 - t0;
    let (p0, p1) = (f(t0), f(t1));
    if depth >= MAX_DEPTH {
        out.push(Cubic::new(
            p0,
            p0.lerp(p1, 1.0 / 3.0),
            p0.lerp(p1, 2.0 / 3.0),
            p1,
        ));
        return Ok(());
    }
    let (d0, d1) = (deriv(f, t0, dom.0, dom.1), deriv(f, t1, dom.0, dom.1));
    let c = Cubic::new(p0, p0 + d0 * (dt / 3.0), p1 - d1 * (dt / 3.0), p1);
    if c.p0.is_finite() && c.c0.is_finite() && c.c1.is_finite() && c.p1.is_finite() {
        let cf = c.flatten(tol * 0.05);
        let mut ok = (1..CHECK).all(|i| {
            let q = f(t0 + dt * i as f64 / CHECK as f64);
            poly_dist(&cf, q) <= tol
        });
        if ok {
            let fp: Vec<Vec2> = (0..=POLY)
                .map(|i| f(t0 + dt * i as f64 / POLY as f64))
                .collect();
            ok = (1..CHECK).all(|i| poly_dist(&fp, c.eval(i as f64 / CHECK as f64)) <= tol);
        }
        if ok {
            out.push(c);
            return Ok(());
        }
    }
    let m = t0 + dt * 0.5;
    fit_rec(f, dom, t0, m, tol, depth + 1, budget, out)?;
    fit_rec(f, dom, m, t1, tol, depth + 1, budget, out)
}

/// Turn from tangent `a` into tangent `b`, radians (+ = left).
fn turn(a: Vec2, b: Vec2) -> f64 {
    a.cross(b).atan2(a.dot(b))
}

/// The cleaned outline as lines and arcs, plus the registry of fitted
/// cubics: an edge with source `i + 1` came from `cubics[i]`.
pub(crate) struct Cleaned {
    pub area: Area,
    pub cubics: Vec<Cubic>,
}

/// Steps 1–3 of the pipeline (module docs) at tolerance `tol`.
pub(crate) fn clean(
    p: &Profiled<'_>,
    cap: Cap,
    join: Join,
    tol: f64,
    limits: &Limits,
) -> Result<Cleaned, StrokeError> {
    let (fit_tol, lower_tol) = (tol * 0.5, tol * 0.5);
    let mut cubics: Vec<Cubic> = Vec::new();
    let mut bands: Vec<Area> = Vec::new();
    let mut edges_total = 0usize;
    let n = p.chain.segs.len();
    for k in 0..n {
        for (ta, tb) in p.spans(k) {
            let fl = |t: f64| p.sides(k, t).0;
            let fr = |t: f64| p.sides(k, t).1;
            let (mut lc, mut rc) = (Vec::new(), Vec::new());
            fit(&fl, ta, tb, fit_tol, limits.pieces, &mut lc)?;
            fit(&fr, ta, tb, fit_tol, limits.pieces, &mut rc)?;
            let (Some(l0), Some(r0)) = (lc.first().map(|c| c.p0), rc.first().map(|c| c.p0))
            else {
                continue;
            };
            // Right side forward, across the end, left side back, across the
            // start: CCW.
            let mut edges = Vec::new();
            for c in &rc {
                cubics.push(*c);
                lower(c, lower_tol, cubics.len() as u64, &mut edges);
            }
            let (l1, r1) = p.sides(k, tb);
            edges.extend(line(r1, l1));
            for c in lc.iter().rev() {
                let r = Cubic::new(c.p1, c.c1, c.c0, c.p0);
                cubics.push(r);
                lower(&r, lower_tol, cubics.len() as u64, &mut edges);
            }
            edges.extend(line(l0, r0));
            edges_total += edges.len();
            if edges_total > limits.pieces {
                return Err(StrokeError::Limit {
                    what: "outline pieces",
                    count: edges_total,
                });
            }
            let band = Area {
                loops: vec![Loop { edges }],
            };
            // Normalise alone (non-zero) before the union (S1).
            bands.push(boolean(&band, &Area::default(), BoolOp::Union));
        }
    }
    let mut extra: Vec<Loop> = Vec::new();
    let joins = if p.chain.closed { n } else { n.saturating_sub(1) };
    for k in 0..joins {
        let (Some(a), Some(b)) = (p.seg(k), p.seg((k + 1) % n)) else {
            continue;
        };
        let (ta, tb) = (a.tangent(1.0), b.tangent(0.0));
        let tr = turn(ta, tb);
        if tr.abs() <= SMOOTH {
            continue;
        }
        let c = a.end();
        let (wl, wr) = p.widths(k, 1.0);
        if tr.abs() >= PI - 1e-9 {
            if join == Join::Round {
                extra.extend(sector(c, wl.max(wr), 0.0, TAU));
            }
            continue;
        }
        // The gap opens on the outer side: the right on a left turn.
        let h = if tr > 0.0 { wr } else { wl };
        match corner(c, ta, tb, h, join) {
            Some(Corner::Disc) if tr > 0.0 => extra.extend(sector(c, h, (-ta.perp()).angle(), tr)),
            Some(Corner::Disc) => extra.extend(sector(c, h, tb.perp().angle(), -tr)),
            Some(Corner::Poly(pts)) => extra.push(polygon(&pts)),
            None => {}
        }
    }
    if !p.chain.closed
        && let (Some(first), Some(last)) = (p.chain.segs.first(), p.chain.segs.last())
    {
        let ends = [
            (first.start(), -first.tangent(0.0), p.widths(0, 0.0), true),
            (last.end(), last.tangent(1.0), p.widths(n - 1, 1.0), false),
        ];
        for (c, out, (wl, wr), at_start) in ends {
            // Facing outward (`out`), the path's left side is on the right at
            // the start and on the left at the end; `ol` is left of `out`.
            let (ol, or) = if at_start { (wr, wl) } else { (wl, wr) };
            let nl = out.perp();
            match cap {
                Cap::Butt => {}
                Cap::Round => {
                    // A quarter disc per side, so each side keeps its reach.
                    extra.extend(sector(c, ol, out.angle(), FRAC_PI_2));
                    extra.extend(sector(c, or, out.angle() - FRAC_PI_2, FRAC_PI_2));
                }
                Cap::Square => {
                    let e = (ol + or) * 0.5;
                    if e > 1e-9 {
                        extra.push(polygon(&[
                            c - nl * or,
                            c - nl * or + out * e,
                            c + nl * ol + out * e,
                            c + nl * ol,
                        ]));
                    }
                }
            }
        }
    }
    bands.push(Area { loops: extra });
    let mut area = union_all(&bands);
    locus_region::offset::drop_specks(&mut area);
    Ok(Cleaned { area, cubics })
}

/// Parameter of the point of `c` closest to `p` (sampled, then Newton).
fn closest_t(c: &Cubic, p: Vec2) -> f64 {
    let mut best = (f64::INFINITY, 0.0);
    for i in 0..=64 {
        let t = f64::from(i) / 64.0;
        let d = c.eval(t).distance(p);
        if d < best.0 {
            best = (d, t);
        }
    }
    let mut t = best.1;
    for _ in 0..8 {
        let d1 = c.derivative(t);
        let h = 1e-6;
        let d2 = (c.derivative((t + h).min(1.0)) - c.derivative((t - h).max(0.0))) * (0.5 / h);
        let r = c.eval(t) - p;
        let gg = d1.dot(d1) + r.dot(d2);
        if gg.abs() < 1e-18 {
            break;
        }
        t = (t - r.dot(d1) / gg).clamp(0.0, 1.0);
    }
    t
}

/// The part of `c` between parameters `t0` ≤ `t1`.
fn sub(c: &Cubic, t0: f64, t1: f64) -> Cubic {
    if t0 >= 1.0 {
        return Cubic::new(c.p1, c.p1, c.p1, c.p1);
    }
    let (_, right) = c.split(t0);
    let u = ((t1 - t0) / (1.0 - t0)).clamp(0.0, 1.0);
    right.split(u).0
}

/// Step 4 (module docs): the cleaned outline with its runs mapped back to
/// the cubics they came from, within `tol`.
pub(crate) fn map_back(cl: &Cleaned, tol: f64) -> Outline {
    let mut out = Outline::default();
    for l in &cl.area.loops {
        let es = &l.edges;
        let m = es.len();
        let at = |i: usize| es.get(i % m.max(1));
        // Start at a source change, so no run wraps around the loop.
        let start = (0..m)
            .find(|&i| at(i).map(|e| e.source) != at(i + m - 1).map(|e| e.source))
            .unwrap_or(0);
        let mut pieces = Vec::new();
        let mut i = 0;
        while i < m {
            let Some(e0) = at(start + i).copied() else {
                break;
            };
            let mut j = i + 1;
            while j < m && at(start + j).map(|e| e.source) == Some(e0.source) {
                j += 1;
            }
            let run: Vec<Edge> = (i..j).filter_map(|q| at(start + q).copied()).collect();
            i = j;
            let segs = || run.iter().map(|e| OutPiece::Seg(e.seg));
            let cubic = (e0.source as usize)
                .checked_sub(1)
                .and_then(|x| cl.cubics.get(x))
                .copied();
            let (Some(c), Some(first), Some(last)) = (cubic, run.first(), run.last()) else {
                pieces.extend(segs());
                continue;
            };
            let (p, q) = (first.seg.start(), last.seg.end());
            let full = p.distance(c.p0) < 1e-6 && q.distance(c.p1) < 1e-6;
            let mut s = if full {
                c
            } else {
                let (tp, tq) = (closest_t(&c, p), closest_t(&c, q));
                if tp <= tq {
                    sub(&c, tp, tq)
                } else {
                    let r = sub(&c, tq, tp);
                    Cubic::new(r.p1, r.c1, r.c0, r.p0)
                }
            };
            // Snap the ends to the run's, which its neighbours share.
            s = Cubic::new(p, s.c0 + (p - s.p0), s.c1 + (q - s.p1), q);
            let fine = s.flatten(tol * 0.02);
            let worst_run = run
                .iter()
                .flat_map(|e| (0..=8).map(move |k| e.seg.at(f64::from(k) / 8.0)))
                .map(|x| poly_dist(&fine, x))
                .fold(0.0, f64::max);
            let worst_cubic = (1..16)
                .map(|k| s.eval(f64::from(k) / 16.0))
                .map(|x| {
                    run.iter()
                        .map(|e| e.seg.distance(x))
                        .fold(f64::INFINITY, f64::min)
                })
                .fold(0.0, f64::max);
            if worst_run.max(worst_cubic) <= tol {
                pieces.push(OutPiece::Cubic(s));
            } else {
                pieces.extend(segs());
            }
        }
        out.loops.push(pieces);
    }
    out
}

/// Polyline parameters of `f` on [t0, t1] within `tol`, start excluded (the
/// canvas: no fit, no cleanup).
fn flatten(f: &dyn Fn(f64) -> Vec2, t0: f64, t1: f64, tol: f64, out: &mut Vec<f64>) {
    flat_rec(f, (t0, f(t0)), (t1, f(t1)), tol, 0, out);
}

fn flat_rec(
    f: &dyn Fn(f64) -> Vec2,
    a: (f64, Vec2),
    b: (f64, Vec2),
    tol: f64,
    depth: u32,
    out: &mut Vec<f64>,
) {
    let m = 0.5 * (a.0 + b.0);
    let pm = f(m);
    let chord = Line::new(a.1, b.1);
    let ok = depth >= 2
        && chord.distance_to(pm) <= tol
        && chord.distance_to(f(0.5 * (a.0 + m))) <= tol
        && chord.distance_to(f(0.5 * (m + b.0))) <= tol;
    if ok || depth >= 16 {
        out.push(b.0);
        return;
    }
    flat_rec(f, a, (m, pm), tol, depth + 1, out);
    flat_rec(f, (m, pm), b, tol, depth + 1, out);
}

/// Canvas triangles for a profiled chain within `tol` mm: a strip per span,
/// fans or polygons at joins and caps. Appends to `out`.
pub(crate) fn mesh(
    p: &Profiled<'_>,
    cap: Cap,
    join: Join,
    tol: f64,
    limits: &Limits,
    out: &mut Vec<[Vec2; 3]>,
) -> Result<(), StrokeError> {
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
    let n = p.chain.segs.len();
    for k in 0..n {
        for (ta, tb) in p.spans(k) {
            let fl = |t: f64| p.sides(k, t).0;
            let fr = |t: f64| p.sides(k, t).1;
            let mut ts = Vec::new();
            flatten(&fl, ta, tb, tol, &mut ts);
            flatten(&fr, ta, tb, tol, &mut ts);
            ts.sort_by(f64::total_cmp);
            ts.dedup();
            let (mut l0, mut r0) = p.sides(k, ta);
            for t in ts {
                let (l1, r1) = p.sides(k, t);
                out.push([l0, r0, r1]);
                out.push([l0, r1, l1]);
                (l0, r0) = (l1, r1);
            }
            budget(out)?;
        }
    }
    let fan = |out: &mut Vec<[Vec2; 3]>, c: Vec2, r: f64, a0: f64, sw: f64| {
        if r <= 0.0 || sw.abs() <= SMOOTH {
            return;
        }
        let step = 2.0 * (1.0 - (tol / r).min(1.0)).acos();
        let steps = ((sw.abs() / step.max(1e-6)).ceil() as usize).clamp(1, 4096);
        let mut q0 = c + Vec2::from_angle(a0) * r;
        for i in 1..=steps {
            let q = c + Vec2::from_angle(a0 + sw * i as f64 / steps as f64) * r;
            out.push([c, q0, q]);
            q0 = q;
        }
    };
    let joins = if p.chain.closed { n } else { n.saturating_sub(1) };
    for k in 0..joins {
        let (Some(a), Some(b)) = (p.seg(k), p.seg((k + 1) % n)) else {
            continue;
        };
        let (ta, tb) = (a.tangent(1.0), b.tangent(0.0));
        let tr = turn(ta, tb);
        if tr.abs() <= SMOOTH || tr.abs() >= PI - 1e-9 {
            continue;
        }
        let c = a.end();
        let (wl, wr) = p.widths(k, 1.0);
        let h = if tr > 0.0 { wr } else { wl };
        match corner(c, ta, tb, h, join) {
            Some(Corner::Disc) if tr > 0.0 => fan(out, c, h, (-ta.perp()).angle(), tr),
            Some(Corner::Disc) => fan(out, c, h, tb.perp().angle(), -tr),
            Some(Corner::Poly(pts)) => {
                for w in pts.windows(2).skip(1) {
                    if let [x, y] = w {
                        out.push([c, *x, *y]);
                    }
                }
            }
            None => {}
        }
    }
    if !p.chain.closed
        && let (Some(first), Some(last)) = (p.chain.segs.first(), p.chain.segs.last())
    {
        let ends = [
            (first.start(), -first.tangent(0.0), p.widths(0, 0.0), true),
            (last.end(), last.tangent(1.0), p.widths(n - 1, 1.0), false),
        ];
        for (c, o, (wl, wr), at_start) in ends {
            // Facing outward: at the start the path's left is on the right.
            let (ol, or) = if at_start { (wr, wl) } else { (wl, wr) };
            let nl = o.perp();
            match cap {
                Cap::Butt => {}
                Cap::Round => {
                    fan(out, c, ol, o.angle(), FRAC_PI_2);
                    fan(out, c, or, o.angle() - FRAC_PI_2, FRAC_PI_2);
                }
                Cap::Square => {
                    let e = (ol + or) * 0.5;
                    let (a, b) = (c - nl * or, c + nl * ol);
                    out.push([a, a + o * e, b + o * e]);
                    out.push([a, b + o * e, b]);
                }
            }
        }
    }
    budget(out)
}
