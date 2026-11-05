// SPDX-License-Identifier: AGPL-3.0-or-later

//! Scoring against a finely flattened reference: two-sided sampled Hausdorff
//! deviation, area error, closure, slivers and segment counts.

use crate::geom::{P, Seg, Shape, p};
use std::collections::HashMap;

/// SPEC §4 / §8 export tolerance, mm.
pub const EXPORT_TOL: f64 = 1e-3;

struct Index {
    segs: Vec<Seg>,
    cell: f64,
    grid: HashMap<(i64, i64), Vec<usize>>,
}

impl Index {
    fn new(segs: Vec<Seg>, cell: f64) -> Self {
        let mut grid: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
        for (i, s) in segs.iter().enumerate() {
            let (lo, hi) = s.bbox();
            for x in (lo.x / cell).floor() as i64..=(hi.x / cell).floor() as i64 {
                for y in (lo.y / cell).floor() as i64..=(hi.y / cell).floor() as i64 {
                    grid.entry((x, y)).or_default().push(i);
                }
            }
        }
        Self { segs, cell, grid }
    }
    fn nearest(&self, q: P) -> f64 {
        if self.segs.is_empty() {
            return f64::INFINITY;
        }
        let (kx, ky) = ((q.x / self.cell).floor() as i64, (q.y / self.cell).floor() as i64);
        let mut best = f64::INFINITY;
        for ring in 0..4i64 {
            for x in kx - ring..=kx + ring {
                for y in ky - ring..=ky + ring {
                    if (x - kx).abs() != ring && (y - ky).abs() != ring {
                        continue;
                    }
                    if let Some(v) = self.grid.get(&(x, y)) {
                        for &i in v {
                            best = best.min(self.segs[i].dist(q));
                        }
                    }
                }
            }
            if best <= ring as f64 * self.cell {
                return best;
            }
        }
        self.segs.iter().map(|s| s.dist(q)).fold(best, f64::min)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Score {
    /// Max distance from the candidate's boundary to the reference's.
    pub dev: f64,
    /// Max distance from the reference's boundary to the candidate's. Larger
    /// than `dev` when the candidate drops a sub-tolerance sliver.
    pub dev_back: f64,
    pub tol_area: f64,
    pub darea: f64,
    pub arcs: usize,
    pub lines: usize,
    pub loops: usize,
    pub closed: bool,
    pub slivers: usize,
}

impl Score {
    pub fn pass(&self) -> bool {
        self.dev <= EXPORT_TOL && self.darea <= self.tol_area && self.closed && self.slivers == 0
    }
}

pub fn poly_area(c: &[P]) -> f64 {
    (0..c.len()).map(|i| 0.5 * c[i].cross(c[(i + 1) % c.len()])).sum()
}

pub fn score(cand: &Shape, reference: &[Vec<P>]) -> Score {
    // A reference contour thinner than 1e-5 mm (2*area / perimeter) is a
    // sub-tolerance sliver of the true result; dropping it is correct.
    let reference: Vec<Vec<P>> = reference
        .iter()
        .filter(|c| {
            let per: f64 = (0..c.len()).map(|i| c[i].dist(c[(i + 1) % c.len()])).sum();
            2.0 * poly_area(c).abs() / per.max(1e-300) >= 1e-5
        })
        .cloned()
        .collect();
    let reference = &reference[..];
    let ref_segs: Vec<Seg> = reference
        .iter()
        .flat_map(|c| (0..c.len()).map(move |i| Seg::Line { a: c[i], b: c[(i + 1) % c.len()] }))
        .collect();
    let ref_area: f64 = reference.iter().map(|c| poly_area(c)).sum();
    let cand_segs: Vec<Seg> = cand.segs().copied().collect();
    let ci = Index::new(cand_segs.clone(), 0.5);
    let ri = Index::new(ref_segs, 0.5);
    let mut dev: f64 = 0.0;
    let mut dev_back: f64 = 0.0;
    if cand_segs.is_empty() != reference.is_empty() {
        dev = f64::INFINITY;
        dev_back = f64::INFINITY;
    } else {
        for q in reference.iter().flatten() {
            dev_back = dev_back.max(ci.nearest(*q));
        }
        for s in &cand_segs {
            let n = ((s.len() / 0.01).ceil() as usize).max(8);
            for k in 0..=n {
                dev = dev.max(ri.nearest(s.at(k as f64 / n as f64)));
            }
        }
    }
    let closed = cand.loops.iter().all(|l| (0..l.len()).all(|i| l[i].end().dist(l[(i + 1) % l.len()].start()) <= 1e-6));
    let slivers = cand.loops.iter().filter(|l| crate::geom::loop_area(l).abs() < 1e-6).count();
    let (arcs, lines) = cand.count();
    let ref_per: f64 = reference.iter().map(|c| (0..c.len()).map(|i| c[i].dist(c[(i + 1) % c.len()])).sum::<f64>()).sum();
    Score { dev, dev_back, tol_area: 0.5 * EXPORT_TOL * ref_per.max(cand.perimeter()), darea: (cand.area() - ref_area).abs(), arcs, lines, loops: cand.loops.len(), closed, slivers }
}

/// Fine reference: 0.1 µm sagitta, i64 snapping grid.
pub fn reference(a: &Shape, b: &Shape, op: crate::own::Op) -> Vec<Vec<P>> {
    crate::flat::polygons(a, b, op, 1e-7)
}

#[must_use]
pub fn origin() -> P {
    p(0.0, 0.0)
}
