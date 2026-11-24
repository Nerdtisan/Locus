// SPDX-License-Identifier: AGPL-3.0-or-later

//! Dense-sampling oracles and the error measure.
//!
//! * `ProfileOracle`: the definition of a normal-offset stroke (#426): the
//!   union of the normal segments [R(s), L(s)] over the path (dense, every
//!   step ≤ 2 µm, as thin triangles), plus the exact join sectors and caps.
//! * `NibOracle`: the exact Minkowski sweep path ⊕ nib (S2): the union over a
//!   dense polyline of the path (sag ≤ 0.1 µm) of segment ⊕ ellipse, tested
//!   exactly in the nib's scaled frame (a segment test for a flat nib).
//!
//! The error of an outline is measured both ways:
//! * outline → truth: from each boundary sample, the distance along the
//!   outline's normal to where the oracle's membership flips (bisection);
//! * truth → outline: a probe grid; where the outline and the oracle
//!   disagree, the probe's distance to the outline boundary.

use crate::path::{Model, Nib, Path};
use core::f64::consts::{FRAC_PI_2, PI, TAU};
use locus_geom::{Aabb, Vec2};
use std::collections::HashMap;

const CELL: f64 = 0.25;

struct Grid {
    cells: HashMap<(i64, i64), Vec<u32>>,
}

impl Grid {
    fn new() -> Self {
        Self {
            cells: HashMap::new(),
        }
    }
    fn key(x: f64) -> i64 {
        (x / CELL).floor() as i64
    }
    fn insert(&mut self, bb: Aabb, id: u32) {
        for i in Self::key(bb.min.x)..=Self::key(bb.max.x) {
            for j in Self::key(bb.min.y)..=Self::key(bb.max.y) {
                self.cells.entry((i, j)).or_default().push(id);
            }
        }
    }
    fn at(&self, q: Vec2) -> &[u32] {
        self.cells
            .get(&(Self::key(q.x), Self::key(q.y)))
            .map_or(&[], Vec::as_slice)
    }
}

fn bb_of(pts: &[Vec2], pad: f64) -> Aabb {
    let mut lo = Vec2::new(f64::INFINITY, f64::INFINITY);
    let mut hi = Vec2::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
    for p in pts {
        lo = Vec2::new(lo.x.min(p.x), lo.y.min(p.y));
        hi = Vec2::new(hi.x.max(p.x), hi.y.max(p.y));
    }
    Aabb {
        min: Vec2::new(lo.x - pad, lo.y - pad),
        max: Vec2::new(hi.x + pad, hi.y + pad),
    }
}

fn in_tri(q: Vec2, a: Vec2, b: Vec2, c: Vec2) -> bool {
    let d1 = (b - a).cross(q - a);
    let d2 = (c - b).cross(q - b);
    let d3 = (a - c).cross(q - c);
    let neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(neg && pos)
}

pub trait Oracle {
    fn inside(&self, q: Vec2) -> bool;
    /// On the true set within `eps` (zero-width parts of a sweep).
    fn near(&self, q: Vec2, _eps: f64) -> bool {
        self.inside(q)
    }
    fn bbox(&self) -> Aabb;
}

/// Dense t samples of piece `k` so consecutive points of `pts(t)` move ≤ h.
fn dense_ts(f: &dyn Fn(f64) -> [Vec2; 3], h: f64) -> Vec<f64> {
    let mut ts = vec![0.0];
    let mut stack = vec![(0.0, 1.0)];
    let mut out = Vec::new();
    while let Some((a, b)) = stack.pop() {
        let (pa, pb) = (f(a), f(b));
        let mv = (0..3).map(|i| pa[i].distance(pb[i])).fold(0.0, f64::max);
        if mv > h && b - a > 1e-12 {
            let m = 0.5 * (a + b);
            stack.push((m, b));
            stack.push((a, m));
        } else {
            out.push(b);
        }
    }
    ts.extend(out);
    ts
}

pub struct ProfileOracle {
    tris: Vec<[Vec2; 3]>,
    /// (centre, radius, start angle, sweep)
    sectors: Vec<(Vec2, f64, f64, f64)>,
    grid: Grid,
    bb: Aabb,
}

impl ProfileOracle {
    pub fn new(path: &Path, model: &Model) -> Self {
        let mut tris = Vec::new();
        let mut all = Vec::new();
        for k in 0..path.pieces.len() {
            let f = |t: f64| {
                let (l, r) = model.sides(path, k, t);
                [path.pieces[k].at(t), l, r]
            };
            let ts = dense_ts(&f, 0.002);
            let mut prev = f(ts[0]);
            for &t in &ts[1..] {
                let cur = f(t);
                let (l0, r0, l1, r1) = (prev[1], prev[2], cur[1], cur[2]);
                tris.push([r0, r1, l1]);
                tris.push([r0, l1, l0]);
                // Also the other diagonal: a twisted quad is covered too.
                tris.push([r0, r1, l0]);
                tris.push([r1, l1, l0]);
                all.extend([l0, r0]);
                prev = cur;
            }
            all.extend([prev[1], prev[2]]);
        }
        let n = path.pieces.len();
        let mut sectors = Vec::new();
        let joins = if path.closed { n } else { n - 1 };
        for k in 0..joins {
            let (a, b) = (&path.pieces[k], &path.pieces[(k + 1) % n]);
            let (ta, tb) = (a.tangent(1.0), b.tangent(0.0));
            let tr = ta.cross(tb).atan2(ta.dot(tb));
            let (wl, wr) = model.widths(path, k, 1.0);
            let c = a.at(1.0);
            if tr > 1e-6 {
                sectors.push((c, wr, (-ta.perp()).angle(), tr));
            } else if tr < -1e-6 {
                sectors.push((c, wl, tb.perp().angle(), -tr));
            }
        }
        if !path.closed {
            let last = &path.pieces[n - 1];
            let (c0, t0) = (path.pieces[0].at(0.0), path.pieces[0].tangent(0.0));
            let (c1, t1) = (last.at(1.0), last.tangent(1.0));
            let (wl0, wr0) = model.widths(path, 0, 0.0);
            let (wl1, wr1) = model.widths(path, n - 1, 1.0);
            sectors.push((c0, wl0, t0.perp().angle(), FRAC_PI_2));
            sectors.push((c0, wr0, t0.perp().angle() + FRAC_PI_2, FRAC_PI_2));
            sectors.push((c1, wr1, t1.perp().angle() - PI, FRAC_PI_2));
            sectors.push((c1, wl1, t1.perp().angle() - FRAC_PI_2, FRAC_PI_2));
        }
        for s in &sectors {
            all.push(s.0 + Vec2::new(s.1, s.1));
            all.push(s.0 - Vec2::new(s.1, s.1));
        }
        let mut grid = Grid::new();
        for (i, t) in tris.iter().enumerate() {
            grid.insert(bb_of(t, 1e-9), i as u32);
        }
        Self {
            tris,
            sectors,
            grid,
            bb: bb_of(&all, 0.0),
        }
    }
}

impl Oracle for ProfileOracle {
    fn inside(&self, q: Vec2) -> bool {
        for &(c, r, a0, sw) in &self.sectors {
            if q.distance(c) <= r {
                let a = ((q - c).angle() - a0).rem_euclid(TAU);
                if a <= sw {
                    return true;
                }
            }
        }
        self.grid.at(q).iter().any(|&i| {
            let t = &self.tris[i as usize];
            in_tri(q, t[0], t[1], t[2])
        })
    }
    fn bbox(&self) -> Aabb {
        self.bb
    }
}

pub struct NibOracle {
    nib: Nib,
    segs: Vec<(Vec2, Vec2)>,
    grid: Grid,
    bb: Aabb,
}

impl NibOracle {
    pub fn new(path: &Path, nib: Nib) -> Self {
        let mut pts: Vec<Vec2> = Vec::new();
        for (k, p) in path.pieces.iter().enumerate() {
            let f = |t: f64| [p.at(t), p.at(t), p.at(t)];
            let ts = dense_ts(&f, 0.002);
            for (i, t) in ts.iter().enumerate() {
                if k > 0 && i == 0 {
                    continue;
                }
                pts.push(p.at(*t));
            }
        }
        if path.closed {
            pts.push(pts[0]);
        }
        let segs: Vec<(Vec2, Vec2)> = pts.windows(2).map(|w| (w[0], w[1])).collect();
        let mut grid = Grid::new();
        for (i, s) in segs.iter().enumerate() {
            grid.insert(bb_of(&[s.0, s.1], nib.a + 1e-6), i as u32);
        }
        Self {
            nib,
            segs,
            grid,
            bb: bb_of(&pts, nib.a),
        }
    }
}

fn seg_dist(p: Vec2, a: Vec2, b: Vec2) -> f64 {
    let d = b - a;
    let l = d.length_sq();
    let t = if l > 0.0 {
        ((p - a).dot(d) / l).clamp(0.0, 1.0)
    } else {
        0.0
    };
    p.distance(a + d * t)
}

fn segs_cross(p: Vec2, q: Vec2, a: Vec2, b: Vec2) -> bool {
    let d1 = (q - p).cross(a - p);
    let d2 = (q - p).cross(b - p);
    let d3 = (b - a).cross(p - a);
    let d4 = (b - a).cross(q - a);
    (d1 * d2 <= 0.0) && (d3 * d4 <= 0.0)
}

impl NibOracle {
    fn inside_grown(&self, q: Vec2, eps: f64) -> bool {
        let (u, v) = self.nib.axes();
        let (a, b) = (self.nib.a + eps, self.nib.b + eps);
        self.grid.at(q).iter().any(|&i| {
            let (p0, p1) = self.segs[i as usize];
            if b <= 1e-9 {
                segs_cross(q - u * a, q + u * a, p0, p1)
            } else {
                // Scaled frame: the nib is the unit disc.
                let tr = |x: Vec2| {
                    let d = x - q;
                    Vec2::new(d.dot(u) / a, d.dot(v) / b)
                };
                seg_dist(Vec2::ZERO, tr(p0), tr(p1)) <= 1.0
            }
        })
    }
}

impl Oracle for NibOracle {
    fn inside(&self, q: Vec2) -> bool {
        self.inside_grown(q, 0.0)
    }
    fn near(&self, q: Vec2, eps: f64) -> bool {
        // A flat nib's sweep can be a bare line: test across it as well.
        let v = self.nib.axes().1 * eps;
        self.inside_grown(q, eps) || self.inside(q + v) || self.inside(q - v)
    }
    fn bbox(&self) -> Aabb {
        self.bb
    }
}

/// Result of measuring one outline against an oracle.
#[derive(Debug, Clone, Copy, Default)]
pub struct Error {
    /// Max distance from the outline boundary to the true boundary.
    pub out_to_true: f64,
    /// Max distance from a misclassified probe to the outline boundary.
    pub true_to_out: f64,
    pub samples: usize,
    pub probes: usize,
}

impl Error {
    pub fn hausdorff(&self) -> f64 {
        self.out_to_true.max(self.true_to_out)
    }
}

fn winding_inside(polys: &[Vec<Vec2>], q: Vec2) -> bool {
    let mut w = 0;
    for pl in polys {
        let n = pl.len();
        for i in 0..n {
            let (a, b) = (pl[i], pl[(i + 1) % n]);
            if (a.y <= q.y) != (b.y <= q.y) {
                let x = a.x + (q.y - a.y) / (b.y - a.y) * (b.x - a.x);
                if x > q.x {
                    w += if b.y > a.y { 1 } else { -1 };
                }
            }
        }
    }
    w != 0
}

/// Measures `polys` (the outline flattened at ≤ 10 nm) against `o`.
pub fn measure(o: &dyn Oracle, polys: &[Vec<Vec2>], probes_per_side: usize) -> Error {
    let mut e = Error::default();
    // Outline → truth.
    let total: usize = polys.iter().map(Vec::len).sum();
    let stride = (total / 20_000).max(1);
    let mut idx = 0usize;
    for pl in polys {
        let n = pl.len();
        for i in 0..n {
            idx += 1;
            if idx % stride != 0 {
                continue;
            }
            let (a, b) = (pl[i], pl[(i + 1) % n]);
            let Some(t) = (b - a).normalized(0.0) else {
                continue;
            };
            let q = a.lerp(b, 0.5);
            // Inside is on the left (CCW outer, CW holes): outward = right.
            let out = -t.perp();
            e.samples += 1;
            let mut d = flip_distance(o, q, out, 0.25);
            if d >= 0.25 {
                // No membership flip along the normal: a zero-width spur (e.g.
                // a flat nib tangent to the path) with no area to cross.
                // Bound its distance to the true set by growing the test.
                let mut e = 1e-6;
                while e < 0.25 && !o.near(q, e) {
                    e *= 1.5;
                }
                d = e.min(0.25);
            }
            e.out_to_true = e.out_to_true.max(d);
        }
    }
    // Truth → outline.
    let bb = o.bbox();
    let pad = 0.05;
    let (x0, y0) = (bb.min.x - pad, bb.min.y - pad);
    let (w, h) = (
        bb.max.x - bb.min.x + 2.0 * pad,
        bb.max.y - bb.min.y + 2.0 * pad,
    );
    let segs: Vec<(Vec2, Vec2)> = polys
        .iter()
        .flat_map(|pl| (0..pl.len()).map(move |i| (pl[i], pl[(i + 1) % pl.len()])))
        .collect();
    let n = probes_per_side;
    for j in 0..n {
        for i in 0..n {
            // Jittered so probes don't sit on axis-aligned features.
            let q = Vec2::new(
                x0 + w * (i as f64 + 0.37) / n as f64,
                y0 + h * (j as f64 + 0.61) / n as f64,
            );
            e.probes += 1;
            if o.inside(q) != winding_inside(polys, q) {
                let d = segs
                    .iter()
                    .map(|&(a, b)| seg_dist(q, a, b))
                    .fold(f64::INFINITY, f64::min);
                e.true_to_out = e.true_to_out.max(d);
            }
        }
    }
    e
}

/// Distance along `dir` (either way, up to `max`) from `q` to where the
/// oracle's membership changes.
fn flip_distance(o: &dyn Oracle, q: Vec2, dir: Vec2, max: f64) -> f64 {
    let s0 = o.inside(q);
    let step = 1e-4;
    let mut x = step;
    while x <= max {
        for sgn in [1.0, -1.0] {
            if o.inside(q + dir * (sgn * x)) != s0 {
                // Bisect in (x - step, x].
                let (mut lo, mut hi) = ((x - step).max(0.0), x);
                for _ in 0..30 {
                    let m = 0.5 * (lo + hi);
                    if o.inside(q + dir * (sgn * m)) != s0 {
                        hi = m;
                    } else {
                        lo = m;
                    }
                }
                return hi;
            }
        }
        x += step;
    }
    max
}
