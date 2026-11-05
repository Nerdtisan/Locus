// SPDX-License-Identifier: AGPL-3.0-or-later

//! Option C: a Locus-owned exact boolean on line/arc loops.
//!
//! 1. split every segment at every intersection, T-junction and overlap end
//!    (vertices snapped within `EPS`), 2. drop duplicate (coincident) edges,
//! 3. classify each edge by the winding number of each operand just left and
//!    right of its midpoint, 4. keep edges where the result changes, oriented
//!    with the result on the left, 5. trace loops (sharpest left turn at
//!    branch vertices), 6. merge pieces that share a carrier line or circle.
//!
//! Steps 1–2 and 5 are the planar arrangement that region detection (#121)
//! needs anyway.

use crate::geom::{P, Seg, Shape, p, winding};
use std::collections::HashMap;
use std::f64::consts::PI;

/// Vertex snapping and on-curve tolerance, mm (= SOLVER_TOL, SPEC §4).
pub const EPS: f64 = 1e-6;
/// Offset of the side probes used to classify an edge, mm.
const PROBE: f64 = 1e-4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Union,
    Intersect,
    Subtract,
}

impl Op {
    fn eval(self, a: bool, b: bool) -> bool {
        match self {
            Op::Union => a || b,
            Op::Intersect => a && b,
            Op::Subtract => a && !b,
        }
    }
}

struct Pool {
    pts: Vec<P>,
    grid: HashMap<(i64, i64), Vec<usize>>,
}

impl Pool {
    const CELL: f64 = EPS * 4.0;
    fn key(q: P) -> (i64, i64) {
        ((q.x / Self::CELL).floor() as i64, (q.y / Self::CELL).floor() as i64)
    }
    fn get(&mut self, q: P) -> usize {
        let (kx, ky) = Self::key(q);
        for dx in -1..=1 {
            for dy in -1..=1 {
                if let Some(v) = self.grid.get(&(kx + dx, ky + dy)) {
                    for &i in v {
                        if self.pts[i].dist(q) <= EPS {
                            return i;
                        }
                    }
                }
            }
        }
        self.pts.push(q);
        let i = self.pts.len() - 1;
        self.grid.entry((kx, ky)).or_default().push(i);
        i
    }
}

/// Points where two segments cross or touch (not overlaps: those are found
/// from endpoints lying on the other segment).
fn intersect(s: &Seg, o: &Seg) -> Vec<P> {
    let cands: Vec<P> = match (*s, *o) {
        (Seg::Line { a, b }, Seg::Line { a: c, b: d }) => {
            let (r, q) = (b - a, d - c);
            let den = r.cross(q);
            if den.abs() <= 1e-12 * r.len() * q.len() {
                vec![]
            } else {
                vec![a + r * ((c - a).cross(q) / den)]
            }
        }
        (Seg::Line { a, b }, Seg::Arc { c, r, .. }) | (Seg::Arc { c, r, .. }, Seg::Line { a, b }) => {
            line_circle(a, b, c, r)
        }
        (Seg::Arc { c: c0, r: r0, .. }, Seg::Arc { c: c1, r: r1, .. }) => circle_circle(c0, r0, c1, r1),
    };
    cands.into_iter().filter(|q| s.dist(*q) <= EPS && o.dist(*q) <= EPS).collect()
}

fn line_circle(a: P, b: P, c: P, r: f64) -> Vec<P> {
    let u = (b - a).unit();
    let foot = a + u * (c - a).dot(u);
    let h = c.dist(foot);
    if h > r + EPS {
        vec![]
    } else if h >= r - EPS {
        vec![foot]
    } else {
        let k = (r * r - h * h).sqrt();
        vec![foot - u * k, foot + u * k]
    }
}

fn circle_circle(c0: P, r0: f64, c1: P, r1: f64) -> Vec<P> {
    let d = c0.dist(c1);
    if d <= EPS || d > r0 + r1 + EPS || d < (r0 - r1).abs() - EPS {
        return vec![];
    }
    let u = (c1 - c0) * (1.0 / d);
    let a = (d * d + r0 * r0 - r1 * r1) / (2.0 * d);
    let h2 = r0 * r0 - a * a;
    let base = c0 + u * a;
    if h2 <= (2.0 * EPS * r0) {
        // Tangent within tolerance: one touching point on c0's circle.
        return vec![c0 + u * (r0 * a.signum())];
    }
    let h = h2.sqrt();
    vec![base + u.perp() * h, base - u.perp() * h]
}

#[derive(Clone, Copy)]
struct Edge {
    seg: Seg,
    v0: usize,
    v1: usize,
}

fn overlaps(a: (P, P), b: (P, P)) -> bool {
    a.0.x <= b.1.x + EPS && b.0.x <= a.1.x + EPS && a.0.y <= b.1.y + EPS && b.0.y <= a.1.y + EPS
}

/// Splits all segments against each other and returns unique edges.
fn arrangement(segs: &[Seg]) -> (Vec<Edge>, Vec<P>) {
    // Arcs longer than a half-turn are halved so pieces never wrap.
    let segs: Vec<Seg> = segs
        .iter()
        .flat_map(|s| match *s {
            Seg::Arc { sweep, .. } if sweep.abs() > PI => vec![s.sub(0.0, 0.5), s.sub(0.5, 1.0)],
            _ => vec![*s],
        })
        .collect();
    let bbs: Vec<(P, P)> = segs.iter().map(Seg::bbox).collect();
    let mut cuts: Vec<Vec<P>> = vec![Vec::new(); segs.len()];
    for i in 0..segs.len() {
        for j in i + 1..segs.len() {
            if !overlaps(bbs[i], bbs[j]) {
                continue;
            }
            for q in intersect(&segs[i], &segs[j]) {
                cuts[i].push(q);
                cuts[j].push(q);
            }
            for q in [segs[j].start(), segs[j].end()] {
                if segs[i].dist(q) <= EPS {
                    cuts[i].push(q);
                }
            }
            for q in [segs[i].start(), segs[i].end()] {
                if segs[j].dist(q) <= EPS {
                    cuts[j].push(q);
                }
            }
        }
    }
    let mut pool = Pool { pts: Vec::new(), grid: HashMap::new() };
    let mut edges: Vec<Edge> = Vec::new();
    let mut seen: HashMap<(usize, usize), Vec<usize>> = HashMap::new();
    for (s, cs) in segs.iter().zip(&cuts) {
        let len = s.len();
        if len <= EPS {
            continue;
        }
        // Snap each cut to the pool first so the piece ends are shared vertices.
        let mut ts: Vec<(f64, usize)> = vec![(0.0, pool.get(s.start())), (1.0, pool.get(s.end()))];
        for q in cs {
            let t = s.param_of(*q);
            if t * len > EPS && (1.0 - t) * len > EPS {
                ts.push((t, pool.get(*q)));
            }
        }
        ts.sort_by(|a, b| a.0.total_cmp(&b.0));
        ts.dedup_by(|b, a| a.1 == b.1);
        for w in ts.windows(2) {
            let ((t0, v0), (t1, v1)) = (w[0], w[1]);
            let seg = s.sub(t0, t1);
            if v0 == v1 && seg.len() < 10.0 * EPS {
                continue;
            }
            let key = (v0.min(v1), v0.max(v1));
            let mid = seg.at(0.5);
            let dup = seen.get(&key).is_some_and(|ids| {
                ids.iter().any(|&k| edges[k].seg.is_arc() == seg.is_arc() && edges[k].seg.at(0.5).dist(mid) <= 10.0 * EPS)
            });
            if !dup {
                seen.entry(key).or_default().push(edges.len());
                edges.push(Edge { seg, v0, v1 });
            }
        }
    }
    (edges, pool.pts)
}

/// Direction leaving the start of `s` (a short chord, so tangent ties are
/// broken by curvature).
fn out_dir(s: &Seg) -> P {
    let t = (1e-3 / s.len()).min(0.25);
    (s.at(t) - s.start()).unit()
}

fn in_dir(s: &Seg) -> P {
    let t = (1e-3 / s.len()).min(0.25);
    (s.end() - s.at(1.0 - t)).unit()
}

pub fn boolean(a: &Shape, b: &Shape, op: Op) -> Shape {
    let segs: Vec<Seg> = a.segs().chain(b.segs()).copied().collect();
    let (edges, _pts) = arrangement(&segs);
    let mut kept: Vec<Edge> = Vec::new();
    for e in &edges {
        let m = e.seg.at(0.5);
        let n = e.seg.tangent(0.5).perp() * PROBE.min(e.seg.len() * 0.1);
        let side = |q: P| op.eval(winding(&a.loops, q) != 0, winding(&b.loops, q) != 0);
        let (l, r) = (side(m + n), side(m - n));
        if l != r {
            kept.push(if l { *e } else { Edge { seg: e.seg.rev(), v0: e.v1, v1: e.v0 } });
        }
    }
    trace(&kept)
}

fn trace(kept: &[Edge]) -> Shape {
    let mut out: HashMap<usize, Vec<usize>> = HashMap::new();
    for (i, e) in kept.iter().enumerate() {
        out.entry(e.v0).or_default().push(i);
    }
    let mut used = vec![false; kept.len()];
    let mut loops = Vec::new();
    for first in 0..kept.len() {
        if used[first] {
            continue;
        }
        used[first] = true;
        let mut lp = vec![kept[first].seg];
        let mut cur = first;
        while kept[cur].v1 != kept[first].v0 {
            let din = in_dir(&kept[cur].seg);
            let next = out.get(&kept[cur].v1).and_then(|cands| {
                cands
                    .iter()
                    .copied()
                    .filter(|&k| !used[k])
                    .max_by(|&x, &y| turn(din, &kept[x].seg).total_cmp(&turn(din, &kept[y].seg)))
            });
            let Some(nx) = next else { break };
            used[nx] = true;
            lp.push(kept[nx].seg);
            cur = nx;
        }
        loops.push(merge(lp));
    }
    Shape::new(loops)
}

fn turn(din: P, s: &Seg) -> f64 {
    let d = out_dir(s);
    din.cross(d).atan2(din.dot(d))
}

/// Joins consecutive pieces that lie on one line or one circle.
fn merge(mut lp: Vec<Seg>) -> Vec<Seg> {
    let join = |x: &Seg, y: &Seg| -> Option<Seg> {
        match (*x, *y) {
            (Seg::Line { a, b }, Seg::Line { a: _, b: d }) => {
                let (u, v) = ((b - a).unit(), (d - b).unit());
                (u.cross(v).abs() < 1e-9 && u.dot(v) > 0.0).then_some(Seg::Line { a, b: d })
            }
            (Seg::Arc { c, r, a0, sweep }, Seg::Arc { c: c1, r: r1, sweep: s1, .. }) => {
                (c.dist(c1) <= 10.0 * EPS && (r - r1).abs() <= 10.0 * EPS && sweep.signum() == s1.signum())
                    .then_some(Seg::Arc { c, r, a0, sweep: sweep + s1 })
            }
            _ => None,
        }
    };
    let mut changed = true;
    while changed && lp.len() > 1 {
        changed = false;
        let n = lp.len();
        for i in 0..n {
            let j = (i + 1) % n;
            if let Some(m) = join(&lp[i], &lp[j]) {
                lp[i] = m;
                lp.remove(j);
                changed = true;
                break;
            }
        }
    }
    lp
}

#[must_use]
pub fn origin() -> P {
    p(0.0, 0.0)
}
