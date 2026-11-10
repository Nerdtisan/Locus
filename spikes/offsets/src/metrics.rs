// SPDX-License-Identifier: AGPL-3.0-or-later

//! Exact oracle. A round-join offset of A by d is the set of points
//! - grow (d > 0): in A, or within d of ∂A;
//! - shrink (d < 0): in A and farther than |d| from ∂A.
//!
//! Distances to lines and arcs are exact (`Seg::distance`), so no reference
//! implementation is needed. A result passes when
//! - every boundary point (sampled along it) is |d| from ∂A within EXPORT_TOL,
//! - every probe point on a grid is classified like the oracle (points within
//!   EXPORT_TOL of the true offset curve are skipped),
//! - every loop is closed, and there are no sliver loops (|area| < 1e-6 mm²).

use locus_geom::{EXPORT_TOL, Vec2};
use locus_region::{Area, Edge, Seg};

#[derive(Clone, Debug, Default)]
pub struct Score {
    pub pass: bool,
    /// Largest | dist(boundary point, ∂A) − |d| |.
    pub dev: f64,
    /// Grid probes classified wrongly.
    pub wrong: usize,
    pub open: bool,
    pub slivers: usize,
    pub arcs: usize,
    pub lines: usize,
    pub loops: usize,
    pub note: String,
}

pub fn dist(edges: &[Edge], q: Vec2) -> f64 {
    edges.iter().map(|e| e.seg.distance(q)).fold(f64::INFINITY, f64::min)
}

fn samples(s: &Seg, step: f64) -> impl Iterator<Item = Vec2> + '_ {
    let n = ((s.length() / step).ceil() as usize).clamp(1, 20_000);
    (0..=n).map(move |i| s.at(i as f64 / n as f64))
}

fn counts(edges: &[Edge], sc: &mut Score) {
    sc.arcs = edges.iter().filter(|e| e.seg.is_arc()).count();
    sc.lines = edges.len() - sc.arcs;
}

/// `step` is the boundary sampling step, `grid` the probe count per axis.
pub fn score(a: &Area, d: f64, out: &Area, step: f64, grid: usize) -> Score {
    let src: Vec<Edge> = a.edges().copied().collect();
    let w = d.abs();
    let mut sc = Score { loops: out.loops.len(), ..Default::default() };
    let all: Vec<Edge> = out.edges().copied().collect();
    counts(&all, &mut sc);
    for l in &out.loops {
        let n = l.edges.len();
        for i in 0..n {
            if l.edges[i].seg.end().distance(l.edges[(i + 1) % n].seg.start()) > 1e-6 {
                sc.open = true;
            }
        }
        if l.area().abs() < 1e-6 {
            sc.slivers += 1;
        }
    }
    for e in &all {
        for q in samples(&e.seg, step) {
            sc.dev = sc.dev.max((dist(&src, q) - w).abs());
        }
    }
    if let Some(bb) = a.loops.iter().filter_map(|l| l.bbox()).reduce(locus_geom::Aabb::union) {
        let pad = w + 0.5;
        let (x0, y0) = (bb.min.x - pad, bb.min.y - pad);
        let (sx, sy) = ((bb.max.x - bb.min.x + 2.0 * pad) / grid as f64, (bb.max.y - bb.min.y + 2.0 * pad) / grid as f64);
        for i in 0..grid {
            for j in 0..grid {
                // Irrational offsets keep probes off the fixtures' round numbers.
                let q = Vec2::new(x0 + sx * (i as f64 + 0.318_309), y0 + sy * (j as f64 + 0.577_215));
                let dq = dist(&src, q);
                if (dq - w).abs() <= EXPORT_TOL || dq <= EXPORT_TOL {
                    continue;
                }
                let inside_a = a.contains(q);
                let truth = if d > 0.0 { inside_a || dq <= w } else { inside_a && dq > w };
                if out.contains(q) != truth {
                    sc.wrong += 1;
                }
            }
        }
    }
    sc.pass = sc.dev <= EXPORT_TOL && sc.wrong == 0 && !sc.open && sc.slivers == 0;
    sc
}

/// Open chains: every output point is |d| from the chain, on the side of
/// `d`, and the runs are continuous.
pub fn score_open(chain: &[Edge], d: f64, runs: &[Vec<Edge>], step: f64) -> Score {
    let w = d.abs();
    let mut sc = Score { loops: runs.len(), ..Default::default() };
    let all: Vec<Edge> = runs.iter().flatten().copied().collect();
    counts(&all, &mut sc);
    let mut wrong_side = 0;
    for r in runs {
        for p in r.windows(2) {
            if p[0].seg.end().distance(p[1].seg.start()) > 1e-6 {
                sc.open = true;
            }
        }
    }
    for e in &all {
        for q in samples(&e.seg, step) {
            sc.dev = sc.dev.max((dist(chain, q) - w).abs());
        }
        // Side test at the midpoint against the nearest chain point.
        let m = e.seg.at(0.5);
        let (i, t) = chain
            .iter()
            .enumerate()
            .map(|(i, c)| (i, c.seg.param_of(m).clamp(0.0, 1.0)))
            .min_by(|x, y| chain[x.0].seg.at(x.1).distance(m).total_cmp(&chain[y.0].seg.at(y.1).distance(m)))
            .unwrap_or((0, 0.0));
        let foot = chain[i].seg.at(t);
        if chain[i].seg.tangent(t).cross(m - foot) * d <= 0.0 {
            wrong_side += 1;
        }
    }
    sc.wrong = wrong_side;
    sc.pass = sc.dev <= EXPORT_TOL && sc.wrong == 0 && !sc.open && !all.is_empty();
    sc
}
