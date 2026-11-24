// SPDX-License-Identifier: AGPL-3.0-or-later

//! Outline construction: bands per centreline span (sides fitted with cubics),
//! join wedges / nib copies at corners, caps at ends, one line/arc union for
//! cleanup (ADR 0008), then option (a) keep the biarcs or option (b) map each
//! cleaned run back to the cubic it came from.

use crate::fit;
use crate::path::{Model, Path};
use core::f64::consts::{FRAC_PI_2, PI, TAU};
use locus_geom::{Cubic, Piece as GPiece, Vec2};
use locus_region::{Area, BoolOp, Edge, Loop, Seg, boolean, union_all};

/// Corners turning less than this are smooth (no join piece).
const SMOOTH: f64 = 1e-6;

#[derive(Debug, Clone, Copy)]
pub enum Out {
    Seg(Seg),
    Cubic(Cubic),
}

/// One outline: loops of output pieces (CCW outer, CW holes).
#[derive(Debug, Clone, Default)]
pub struct Outline {
    pub loops: Vec<Vec<Out>>,
}

impl Outline {
    pub fn counts(&self) -> (usize, usize, usize) {
        let (mut l, mut a, mut c) = (0, 0, 0);
        for o in self.loops.iter().flatten() {
            match o {
                Out::Seg(Seg::Line { .. }) => l += 1,
                Out::Seg(Seg::Arc { .. }) => a += 1,
                Out::Cubic(_) => c += 1,
            }
        }
        (l, a, c)
    }

    pub fn from_area(a: &Area) -> Self {
        Self {
            loops: a
                .loops
                .iter()
                .map(|l| l.edges.iter().map(|e| Out::Seg(e.seg)).collect())
                .collect(),
        }
    }

    /// Dense polylines (closed) for the oracle.
    pub fn polylines(&self, sag: f64) -> Vec<Vec<Vec2>> {
        self.loops
            .iter()
            .map(|l| {
                let mut pts = Vec::new();
                for o in l {
                    match o {
                        Out::Seg(s) => s.flatten_into(sag, &mut pts),
                        Out::Cubic(c) => {
                            let f = c.flatten(sag);
                            pts.extend_from_slice(&f[..f.len() - 1]);
                        }
                    }
                }
                pts
            })
            .collect()
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Stats {
    pub fitted: usize,
    pub biarcs: usize,
    pub runs: usize,
    pub runs_full: usize,
    pub runs_trimmed: usize,
    pub runs_fallback: usize,
}

#[derive(Debug, Clone, Default)]
pub struct Expansion {
    /// (a): the cleaned line/arc outline.
    pub area: Area,
    /// Cubic registry: edge source `i + 1` is `cubics[i]`.
    pub cubics: Vec<Cubic>,
    pub stats: Stats,
    /// The union's operands (for debugging).
    pub pieces: Vec<Area>,
}

fn line(a: Vec2, b: Vec2) -> Option<Edge> {
    (a.distance(b) > 1e-9).then_some(Edge {
        seg: Seg::Line { a, b },
        source: 0,
    })
}

/// A pie sector (centre, radius, start angle, CCW sweep) as a CCW loop.
pub fn sector(c: Vec2, r: f64, a0: f64, sweep: f64) -> Option<Loop> {
    if r <= 1e-7 || sweep <= SMOOTH {
        return None;
    }
    if sweep >= TAU - 1e-12 {
        return Some(Loop {
            edges: vec![Edge {
                seg: Seg::Arc {
                    center: c,
                    radius: r,
                    start: 0.0,
                    sweep: TAU,
                },
                source: 0,
            }],
        });
    }
    let p0 = c + Vec2::from_angle(a0) * r;
    let p1 = c + Vec2::from_angle(a0 + sweep) * r;
    let mut edges = Vec::new();
    edges.extend(line(c, p0));
    edges.push(Edge {
        seg: Seg::Arc {
            center: c,
            radius: r,
            start: a0,
            sweep,
        },
        source: 0,
    });
    edges.extend(line(p1, c));
    Some(Loop { edges })
}

/// Lowers a cubic to oriented line/arc pieces within `tol`, tagged `source`.
pub fn lower(c: &Cubic, tol: f64, source: u64, out: &mut Vec<Edge>) {
    for (p, back) in c.to_pieces_oriented(tol) {
        let seg = match p {
            GPiece::Line(l) => Seg::Line { a: l.a, b: l.b },
            GPiece::Arc(a) => {
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

/// Sub-spans of piece `k` whose tangent turns at most 90°, so each band is a
/// simple sweep that the non-zero rule fills correctly on its own.
fn spans(path: &Path, k: usize) -> Vec<(f64, f64)> {
    const N: usize = 64;
    let p = &path.pieces[k];
    let mut out = Vec::new();
    let (mut t0, mut acc) = (0.0, 0.0);
    let mut prev = p.tangent(0.0);
    for i in 1..=N {
        let t = i as f64 / N as f64;
        let d = p.tangent(t);
        acc += prev.cross(d).atan2(prev.dot(d));
        prev = d;
        if acc.abs() > FRAC_PI_2 && i < N {
            out.push((t0, t));
            t0 = t;
            acc = 0.0;
        }
    }
    out.push((t0, 1.0));
    out
}

/// Parameters of piece `k` where the tangent is parallel to the nib's major
/// axis: there the nib's support point swings from one end to the other (a
/// jump for a flat nib), so spans are split there.
fn nib_flips(path: &Path, k: usize, u: Vec2) -> Vec<f64> {
    const N: usize = 256;
    let p = &path.pieces[k];
    let g = |t: f64| p.tangent(t).cross(u);
    let mut out = Vec::new();
    let mut prev = g(0.0);
    for i in 1..=N {
        let (a, b) = ((i - 1) as f64 / N as f64, i as f64 / N as f64);
        let cur = g(b);
        if prev * cur < 0.0 {
            let (mut lo, mut hi) = (a, b);
            for _ in 0..50 {
                let m = 0.5 * (lo + hi);
                if g(lo) * g(m) <= 0.0 { hi = m } else { lo = m }
            }
            let t = 0.5 * (lo + hi);
            if t > 1e-9 && t < 1.0 - 1e-9 {
                out.push(t);
            }
        }
        prev = cur;
    }
    out
}

/// Parameters of piece `k` where a side curve reverses against the path (a
/// cusp of the side: the centre of curvature crosses the side point). A band
/// across a cusp can wind 0 over points its segments cover, so bands split
/// there.
fn side_cusps(path: &Path, model: &Model, k: usize) -> Vec<f64> {
    const N: usize = 256;
    let p = &path.pieces[k];
    let h = 1e-6;
    let g = |t: f64, side: usize| {
        let (a, b) = ((t - h).max(0.0), (t + h).min(1.0));
        let (sa, sb) = (model.sides(path, k, a), model.sides(path, k, b));
        let d = if side == 0 { sb.0 - sa.0 } else { sb.1 - sa.1 };
        d.dot(p.tangent(t))
    };
    let mut out = Vec::new();
    for side in 0..2 {
        let mut prev = g(0.0, side);
        for i in 1..=N {
            let (a, b) = ((i - 1) as f64 / N as f64, i as f64 / N as f64);
            let cur = g(b, side);
            if prev * cur < 0.0 {
                let (mut lo, mut hi) = (a, b);
                for _ in 0..40 {
                    let m = 0.5 * (lo + hi);
                    if g(lo, side) * g(m, side) <= 0.0 {
                        hi = m
                    } else {
                        lo = m
                    }
                }
                let t = 0.5 * (lo + hi);
                if t > 1e-9 && t < 1.0 - 1e-9 {
                    out.push(t);
                }
            }
            prev = cur;
        }
    }
    out
}

/// The nib as a closed loop at `c` (fitted with cubics, lowered; exact circle
/// for a round nib).
fn nib_loop(nib: &crate::path::Nib, c: Vec2, tol: f64) -> Option<Loop> {
    if nib.b <= 1e-7 {
        return None; // a flat nib has no area
    }
    if (nib.a - nib.b).abs() <= 1e-12 {
        return sector(c, nib.a, 0.0, TAU);
    }
    let f = |t: f64| c + nib.point(t * TAU);
    let mut cs = Vec::new();
    fit::fit(&f, 0.0, 1.0, tol * 0.5, &mut cs);
    let mut edges = Vec::new();
    for q in &cs {
        lower(q, tol * 0.5, 0, &mut edges);
    }
    Some(Loop { edges })
}

/// Turn from tangent a into tangent b.
fn turn(a: Vec2, b: Vec2) -> f64 {
    a.cross(b).atan2(a.dot(b))
}

/// Union strategy for the cleanup.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Cleanup {
    /// Normalise every band alone, then one union of everything.
    NormaliseThenUnion,
    /// One union of the raw bands (cheaper; wrong where swallowtails cancel).
    RawUnion,
}

pub fn expand(path: &Path, model: &Model, fit_tol: f64, lower_tol: f64) -> Expansion {
    expand_with(path, model, fit_tol, lower_tol, Cleanup::NormaliseThenUnion)
}

/// Builds the cleaned outline. `fit_tol` + `lower_tol` is the error budget.
pub fn expand_with(
    path: &Path,
    model: &Model,
    fit_tol: f64,
    lower_tol: f64,
    cleanup: Cleanup,
) -> Expansion {
    let mut ex = Expansion::default();
    let mut pieces: Vec<Area> = Vec::new();
    let n = path.pieces.len();
    for k in 0..n {
        let mut sp = spans(path, k);
        {
            let mut cuts: Vec<f64> = sp.iter().map(|s| s.1).collect();
            if let Some(nib) = model.nib() {
                cuts.extend(nib_flips(path, k, nib.axes().0));
            }
            cuts.extend(side_cusps(path, model, k));
            cuts.push(0.0);
            cuts.sort_by(f64::total_cmp);
            cuts.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
            sp = cuts.windows(2).map(|w| (w[0], w[1])).collect();
        }
        for (ta, tb) in sp {
            // Evaluate just inside the span, so the side function takes this
            // span's branch at a nib flip.
            let eps = if model.nib().is_some() { 1e-10 } else { 0.0 };
            let (ta, tb) = (
                ta + if ta > 0.0 { eps } else { 0.0 },
                tb - if tb < 1.0 { eps } else { 0.0 },
            );
            let fl = |t: f64| model.sides(path, k, t).0;
            let fr = |t: f64| model.sides(path, k, t).1;
            let (mut lc, mut rc) = (Vec::new(), Vec::new());
            fit::fit(&fl, ta, tb, fit_tol, &mut lc);
            fit::fit(&fr, ta, tb, fit_tol, &mut rc);
            ex.stats.fitted += lc.len() + rc.len();
            let (Some(l0), Some(r0)) = (lc.first().map(|c| c.p0), rc.first().map(|c| c.p0)) else {
                continue;
            };
            let mut edges = Vec::new();
            // R forward, across the end, L backward, across the start: CCW.
            for c in &rc {
                ex.cubics.push(*c);
                lower(c, lower_tol, ex.cubics.len() as u64, &mut edges);
            }
            let (l1, r1) = model.sides(path, k, tb);
            edges.extend(line(r1, l1));
            for c in lc.iter().rev() {
                let r = Cubic::new(c.p1, c.c1, c.c0, c.p0);
                ex.cubics.push(r);
                lower(&r, lower_tol, ex.cubics.len() as u64, &mut edges);
            }
            edges.extend(line(l0, r0));
            ex.stats.biarcs += edges.len();
            let band = Area {
                loops: vec![Loop { edges }],
            };
            pieces.push(match cleanup {
                // Normalise each band alone (non-zero), so a band's reversed
                // swallowtail can't cancel a neighbour's cover in the union.
                Cleanup::NormaliseThenUnion => boolean(&band, &Area::default(), BoolOp::Union),
                Cleanup::RawUnion => band,
            });
        }
    }
    let mut extra: Vec<Loop> = Vec::new();
    let joins = if path.closed { n } else { n.saturating_sub(1) };
    for k in 0..joins {
        let (a, b) = (&path.pieces[k], &path.pieces[(k + 1) % n]);
        let (ta, tb) = (a.tangent(1.0), b.tangent(0.0));
        let tr = turn(ta, tb);
        let c = a.at(1.0);
        if let Some(nib) = model.nib() {
            if tr.abs() > SMOOTH {
                extra.extend(nib_loop(&nib, c, fit_tol + lower_tol));
            }
            continue;
        }
        if tr.abs() <= SMOOTH {
            continue;
        }
        let (wl, wr) = model.widths(path, k, 1.0);
        if tr.abs() >= PI - 1e-9 {
            extra.extend(sector(c, wl.max(wr), 0.0, TAU));
        } else if tr > 0.0 {
            // Left turn: the gap is on the right.
            extra.extend(sector(c, wr, (-ta.perp()).angle(), tr));
        } else {
            extra.extend(sector(c, wl, tb.perp().angle(), -tr));
        }
    }
    if !path.closed && n > 0 {
        let last = &path.pieces[n - 1];
        let ends = [
            (
                path.pieces[0].at(0.0),
                path.pieces[0].tangent(0.0),
                0usize,
                0.0,
                false,
            ),
            (last.at(1.0), last.tangent(1.0), n - 1, 1.0, true),
        ];
        for (c, t, k, tt, end) in ends {
            if let Some(nib) = model.nib() {
                extra.extend(nib_loop(&nib, c, fit_tol + lower_tol));
                continue;
            }
            let (wl, wr) = model.widths(path, k, tt);
            let nl = t.perp().angle();
            if end {
                extra.extend(sector(c, wr, nl - PI, FRAC_PI_2));
                extra.extend(sector(c, wl, nl - FRAC_PI_2, FRAC_PI_2));
            } else {
                extra.extend(sector(c, wl, nl, FRAC_PI_2));
                extra.extend(sector(c, wr, nl + FRAC_PI_2, FRAC_PI_2));
            }
        }
    }
    pieces.push(Area { loops: extra });
    ex.area = union_all(&pieces);
    ex.pieces = pieces;
    ex
}

/// Parameter of the closest point of `c` to `p` (sampled, then Newton).
fn closest_t(c: &Cubic, p: Vec2) -> f64 {
    let mut best = (f64::INFINITY, 0.0);
    for i in 0..=64 {
        let t = i as f64 / 64.0;
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
        let g = r.dot(d1);
        let gg = d1.dot(d1) + r.dot(d2);
        if gg.abs() < 1e-18 {
            break;
        }
        t = (t - g / gg).clamp(0.0, 1.0);
    }
    t
}

fn sub(c: &Cubic, t0: f64, t1: f64) -> Cubic {
    if t0 >= 1.0 {
        return Cubic::new(c.p1, c.p1, c.p1, c.p1);
    }
    let (_, right) = c.split(t0);
    let u = ((t1 - t0) / (1.0 - t0)).clamp(0.0, 1.0);
    right.split(u).0
}

/// Option (b): maps every run of cleaned edges that came from one fitted cubic
/// back to that cubic (whole, or a sub-curve snapped to the run's ends).
/// Runs whose sub-curve strays more than `tol` from the run keep their arcs.
pub fn map_back(ex: &mut Expansion, tol: f64) -> Outline {
    let mut out = Outline::default();
    for l in &ex.area.loops {
        let es = &l.edges;
        let m = es.len();
        // Rotate to start at a source change so no run wraps.
        let start = (0..m)
            .find(|&i| es[i].source != es[(i + m - 1) % m].source)
            .unwrap_or(0);
        let mut pieces = Vec::new();
        let mut i = 0;
        while i < m {
            let e0 = es[(start + i) % m];
            let mut j = i + 1;
            while j < m && es[(start + j) % m].source == e0.source {
                j += 1;
            }
            let run: Vec<Edge> = (i..j).map(|q| es[(start + q) % m]).collect();
            i = j;
            if e0.source == 0 {
                pieces.extend(run.iter().map(|e| Out::Seg(e.seg)));
                continue;
            }
            ex.stats.runs += 1;
            let Some(c) = ex.cubics.get(e0.source as usize - 1).copied() else {
                pieces.extend(run.iter().map(|e| Out::Seg(e.seg)));
                continue;
            };
            let (p, q) = (run[0].seg.start(), run[run.len() - 1].seg.end());
            let (tp, tq) = (closest_t(&c, p), closest_t(&c, q));
            let full = p.distance(c.p0) < 1e-6 && q.distance(c.p1) < 1e-6;
            let mut s = if full {
                c
            } else if tp <= tq {
                sub(&c, tp, tq)
            } else {
                let r = sub(&c, tq, tp);
                Cubic::new(r.p1, r.c1, r.c0, r.p0)
            };
            // Snap the ends to the run's (shared with the neighbours).
            let (d0, d1) = (p - s.p0, q - s.p1);
            s = Cubic::new(p, s.c0 + d0, s.c1 + d1, q);
            let fine: Vec<Vec2> = s.flatten(tol * 0.02);
            let worst_run = run
                .iter()
                .flat_map(|e| (0..=8).map(move |k| e.seg.at(k as f64 / 8.0)))
                .map(|x| {
                    fine.windows(2)
                        .map(|w| locus_geom::Line::new(w[0], w[1]).distance_to(x))
                        .fold(f64::INFINITY, f64::min)
                })
                .fold(0.0, f64::max);
            let worst_cub = (1..16)
                .map(|k| s.eval(k as f64 / 16.0))
                .map(|x| {
                    run.iter()
                        .map(|e| e.seg.distance(x))
                        .fold(f64::INFINITY, f64::min)
                })
                .fold(0.0, f64::max);
            if worst_run.max(worst_cub) <= tol {
                if full {
                    ex.stats.runs_full += 1;
                } else {
                    ex.stats.runs_trimmed += 1;
                }
                pieces.push(Out::Cubic(s));
            } else {
                ex.stats.runs_fallback += 1;
                pieces.extend(run.iter().map(|e| Out::Seg(e.seg)));
            }
        }
        out.loops.push(pieces);
    }
    out
}

/// Canvas mode: no fit, no cleanup. Side polylines per piece as a triangle
/// strip, plus fans for joins and caps. Returns triangles (f32 xy).
pub fn canvas_mesh(path: &Path, model: &Model, tol: f64, tris: &mut Vec<[f32; 6]>) {
    let n = path.pieces.len();
    let push = |tris: &mut Vec<[f32; 6]>, a: Vec2, b: Vec2, c: Vec2| {
        tris.push([
            a.x as f32, a.y as f32, b.x as f32, b.y as f32, c.x as f32, c.y as f32,
        ]);
    };
    for k in 0..n {
        let fl = |t: f64| model.sides(path, k, t).0;
        let fr = |t: f64| model.sides(path, k, t).1;
        let mut ts = Vec::new();
        fit::flatten(&fl, 0.0, 1.0, tol, &mut ts);
        fit::flatten(&fr, 0.0, 1.0, tol, &mut ts);
        ts.sort_by(f64::total_cmp);
        ts.dedup();
        let (mut l0, mut r0) = model.sides(path, k, 0.0);
        for t in ts {
            let (l1, r1) = model.sides(path, k, t);
            push(tris, l0, r0, r1);
            push(tris, l0, r1, l1);
            (l0, r0) = (l1, r1);
        }
    }
    let fan = |tris: &mut Vec<[f32; 6]>, c: Vec2, r: f64, a0: f64, sw: f64| {
        if r <= 0.0 || sw <= SMOOTH {
            return;
        }
        let steps = ((sw * r / (8.0 * tol)).sqrt().ceil() as usize).clamp(1, 256);
        let mut p = c + Vec2::from_angle(a0) * r;
        for i in 1..=steps {
            let q = c + Vec2::from_angle(a0 + sw * i as f64 / steps as f64) * r;
            push(tris, c, p, q);
            p = q;
        }
    };
    let joins = if path.closed { n } else { n.saturating_sub(1) };
    for k in 0..joins {
        let (a, b) = (&path.pieces[k], &path.pieces[(k + 1) % n]);
        let (ta, tb) = (a.tangent(1.0), b.tangent(0.0));
        let tr = turn(ta, tb);
        let (wl, wr) = model.widths(path, k, 1.0);
        let c = a.at(1.0);
        if tr > SMOOTH {
            fan(tris, c, wr, (-ta.perp()).angle(), tr);
        } else if tr < -SMOOTH {
            fan(tris, c, wl, tb.perp().angle(), -tr);
        }
    }
    if !path.closed && n > 0 {
        let last = &path.pieces[n - 1];
        let (c0, t0) = (path.pieces[0].at(0.0), path.pieces[0].tangent(0.0));
        let (c1, t1) = (last.at(1.0), last.tangent(1.0));
        let (wl0, wr0) = model.widths(path, 0, 0.0);
        let (wl1, wr1) = model.widths(path, n - 1, 1.0);
        fan(tris, c0, wl0, t0.perp().angle(), FRAC_PI_2);
        fan(tris, c0, wr0, t0.perp().angle() + FRAC_PI_2, FRAC_PI_2);
        fan(tris, c1, wr1, t1.perp().angle() - PI, FRAC_PI_2);
        fan(tris, c1, wl1, t1.perp().angle() - FRAC_PI_2, FRAC_PI_2);
    }
}
