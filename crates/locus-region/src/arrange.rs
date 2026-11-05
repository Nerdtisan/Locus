// SPDX-License-Identifier: AGPL-3.0-or-later

//! The planar arrangement (ADR 0008): every piece split at every crossing,
//! tangency, T-junction and overlap end, vertices snapped within
//! [`SNAP`], coincident pieces kept once. Region detection traces its faces;
//! booleans classify its edges.

use crate::seg::{Edge, Seg};
use core::f64::consts::PI;
use locus_geom::{Aabb, SOLVER_TOL, Vec2};
use std::collections::HashMap;

/// Vertex snapping and on-curve tolerance (SPEC §4 solver tolerance): points
/// the solver made coincident are equal only to this.
pub const SNAP: f64 = SOLVER_TOL;

/// An edge of the arrangement, from vertex `v0` to `v1`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct GEdge {
    pub edge: Edge,
    pub v0: usize,
    pub v1: usize,
}

#[derive(Debug, Default)]
pub(crate) struct Arrangement {
    pub verts: Vec<Vec2>,
    pub edges: Vec<GEdge>,
}

/// Snapping vertex pool on a uniform grid.
struct Pool {
    pts: Vec<Vec2>,
    grid: HashMap<(i64, i64), Vec<usize>>,
}

impl Pool {
    const CELL: f64 = SNAP * 4.0;

    fn key(q: Vec2) -> (i64, i64) {
        // Saturating casts: callers only pass finite, bounded coordinates.
        (
            (q.x / Self::CELL).floor() as i64,
            (q.y / Self::CELL).floor() as i64,
        )
    }

    fn get(&mut self, q: Vec2) -> usize {
        let (kx, ky) = Self::key(q);
        for dx in -1..=1 {
            for dy in -1..=1 {
                if let Some(ids) = self.grid.get(&(kx + dx, ky + dy))
                    && let Some(&i) = ids.iter().find(|&&i| self.pts[i].distance(q) <= SNAP)
                {
                    return i;
                }
            }
        }
        self.pts.push(q);
        let i = self.pts.len() - 1;
        self.grid.entry((kx, ky)).or_default().push(i);
        i
    }
}

fn line_circle(a: Vec2, b: Vec2, c: Vec2, r: f64) -> Vec<Vec2> {
    let Some(u) = (b - a).normalized(0.0) else {
        return Vec::new();
    };
    let foot = a + u * (c - a).dot(u);
    let h = c.distance(foot);
    if h > r + SNAP {
        Vec::new()
    } else if h >= r - SNAP {
        vec![foot]
    } else {
        let k = (r * r - h * h).sqrt();
        vec![foot - u * k, foot + u * k]
    }
}

fn circle_circle(c0: Vec2, r0: f64, c1: Vec2, r1: f64) -> Vec<Vec2> {
    let d = c0.distance(c1);
    if d <= SNAP || d > r0 + r1 + SNAP || d < (r0 - r1).abs() - SNAP {
        return Vec::new();
    }
    let u = (c1 - c0) / d;
    let a = (d * d + r0 * r0 - r1 * r1) / (2.0 * d);
    let h2 = r0 * r0 - a * a;
    // A crossing whose lens is shallower than SNAP is a tangency: one point.
    if h2 <= 2.0 * SNAP * r0 {
        return vec![c0 + u * (r0 * a.signum())];
    }
    let h = h2.sqrt();
    let base = c0 + u * a;
    vec![base + u.perp() * h, base - u.perp() * h]
}

/// Points where two pieces cross or touch. Overlaps are found separately,
/// from piece ends lying on the other piece.
fn crossings(s: &Seg, o: &Seg) -> Vec<Vec2> {
    let cands = match (*s, *o) {
        (Seg::Line { a, b }, Seg::Line { a: c, b: d }) => {
            let (r, q) = (b - a, d - c);
            let den = r.cross(q);
            if den.abs() <= 1e-12 * r.length() * q.length() {
                Vec::new()
            } else {
                vec![a + r * ((c - a).cross(q) / den)]
            }
        }
        (Seg::Line { a, b }, Seg::Arc { center, radius, .. })
        | (Seg::Arc { center, radius, .. }, Seg::Line { a, b }) => {
            line_circle(a, b, center, radius)
        }
        (
            Seg::Arc {
                center: c0,
                radius: r0,
                ..
            },
            Seg::Arc {
                center: c1,
                radius: r1,
                ..
            },
        ) => circle_circle(c0, r0, c1, r1),
    };
    cands
        .into_iter()
        .filter(|q| s.distance(*q) <= SNAP && o.distance(*q) <= SNAP)
        .collect()
}

/// Builds the arrangement of `input`. Pieces shorter than [`SNAP`] and
/// non-finite pieces are dropped. When two pieces coincide, the first one
/// (lowest index) is kept with its source.
pub(crate) fn arrange(input: &[Edge]) -> Arrangement {
    // Arcs longer than a half turn are halved so no piece wraps on itself.
    let pieces: Vec<Edge> = input
        .iter()
        .filter(|e| finite(&e.seg) && e.seg.length() > SNAP)
        .flat_map(|e| match e.seg {
            Seg::Arc { sweep, .. } if sweep.abs() > PI => {
                vec![
                    Edge {
                        seg: e.seg.sub(0.0, 0.5),
                        source: e.source,
                    },
                    Edge {
                        seg: e.seg.sub(0.5, 1.0),
                        source: e.source,
                    },
                ]
            }
            _ => vec![*e],
        })
        .collect();
    let bbs: Vec<Aabb> = pieces.iter().map(|e| e.seg.bbox()).collect();
    let mut cuts: Vec<Vec<Vec2>> = vec![Vec::new(); pieces.len()];
    // Sweep over x so only pieces with overlapping boxes are paired.
    let mut order: Vec<usize> = (0..pieces.len()).collect();
    order.sort_by(|&i, &j| bbs[i].min.x.total_cmp(&bbs[j].min.x));
    for (oi, &i) in order.iter().enumerate() {
        for &j in &order[oi + 1..] {
            if bbs[j].min.x > bbs[i].max.x + SNAP {
                break;
            }
            if bbs[j].min.y > bbs[i].max.y + SNAP || bbs[i].min.y > bbs[j].max.y + SNAP {
                continue;
            }
            let (si, sj) = (&pieces[i].seg, &pieces[j].seg);
            for q in crossings(si, sj) {
                cuts[i].push(q);
                cuts[j].push(q);
            }
            for q in [sj.start(), sj.end()] {
                if si.distance(q) <= SNAP {
                    cuts[i].push(q);
                }
            }
            for q in [si.start(), si.end()] {
                if sj.distance(q) <= SNAP {
                    cuts[j].push(q);
                }
            }
        }
    }
    let mut pool = Pool {
        pts: Vec::new(),
        grid: HashMap::new(),
    };
    let mut edges: Vec<GEdge> = Vec::new();
    let mut by_ends: HashMap<(usize, usize), Vec<usize>> = HashMap::new();
    for (e, cs) in pieces.iter().zip(&cuts) {
        let len = e.seg.length();
        let mut ts: Vec<(f64, usize)> =
            vec![(0.0, pool.get(e.seg.start())), (1.0, pool.get(e.seg.end()))];
        for q in cs {
            let t = e.seg.param_of(*q);
            if t * len > SNAP && (1.0 - t) * len > SNAP {
                ts.push((t, pool.get(*q)));
            }
        }
        ts.sort_by(|a, b| a.0.total_cmp(&b.0));
        ts.dedup_by(|b, a| a.1 == b.1);
        for w in ts.windows(2) {
            let ((t0, v0), (t1, v1)) = (w[0], w[1]);
            let seg = e.seg.sub(t0, t1);
            if v0 == v1 {
                continue;
            }
            let key = (v0.min(v1), v0.max(v1));
            let mid = seg.at(0.5);
            let dup = by_ends.get(&key).is_some_and(|ids| {
                ids.iter().any(|&k| {
                    edges[k].edge.seg.is_arc() == seg.is_arc()
                        && edges[k].edge.seg.at(0.5).distance(mid) <= 10.0 * SNAP
                })
            });
            if !dup {
                by_ends.entry(key).or_default().push(edges.len());
                edges.push(GEdge {
                    edge: Edge {
                        seg,
                        source: e.source,
                    },
                    v0,
                    v1,
                });
            }
        }
    }
    Arrangement {
        verts: pool.pts,
        edges,
    }
}

fn finite(s: &Seg) -> bool {
    match *s {
        Seg::Line { a, b } => a.is_finite() && b.is_finite(),
        Seg::Arc {
            center,
            radius,
            start,
            sweep,
        } => {
            center.is_finite()
                && radius.is_finite()
                && radius > SNAP
                && start.is_finite()
                && sweep.is_finite()
        }
    }
}

/// Direction leaving the start of `s`, along a short chord so that tangent
/// pieces are told apart by curvature.
pub(crate) fn out_dir(s: &Seg) -> Vec2 {
    let t = (1e-3 / s.length()).min(0.25);
    (s.at(t) - s.start()).normalized(0.0).unwrap_or(Vec2::ZERO)
}

/// Direction arriving at the end of `s` (see [`out_dir`]).
pub(crate) fn in_dir(s: &Seg) -> Vec2 {
    let t = (1e-3 / s.length()).min(0.25);
    (s.end() - s.at(1.0 - t))
        .normalized(0.0)
        .unwrap_or(Vec2::ZERO)
}

/// Joins consecutive edges of a closed loop that lie on one line or one
/// circle (and, when `same_source`, come from the same entity).
pub(crate) fn merge_carriers(mut edges: Vec<Edge>, same_source: bool) -> Vec<Edge> {
    let join = |x: &Edge, y: &Edge| -> Option<Edge> {
        if same_source && x.source != y.source {
            return None;
        }
        let seg = match (x.seg, y.seg) {
            (Seg::Line { a, b }, Seg::Line { b: d, .. }) => {
                let (u, w) = ((b - a).normalized(0.0)?, (d - b).normalized(0.0)?);
                (u.cross(w).abs() < 1e-9 && u.dot(w) > 0.0).then_some(Seg::Line { a, b: d })?
            }
            (
                Seg::Arc {
                    center,
                    radius,
                    start,
                    sweep,
                },
                Seg::Arc {
                    center: c1,
                    radius: r1,
                    sweep: s1,
                    ..
                },
            ) => {
                let same = center.distance(c1) <= 10.0 * SNAP
                    && (radius - r1).abs() <= 10.0 * SNAP
                    && sweep.signum() == s1.signum();
                same.then_some(Seg::Arc {
                    center,
                    radius,
                    start,
                    sweep: sweep + s1,
                })?
            }
            _ => return None,
        };
        Some(Edge {
            seg,
            source: x.source,
        })
    };
    let mut changed = true;
    while changed && edges.len() > 1 {
        changed = false;
        let n = edges.len();
        for i in 0..n {
            let j = (i + 1) % n;
            if let Some(m) = join(&edges[i], &edges[j]) {
                edges[i] = m;
                edges.remove(j);
                changed = true;
                break;
            }
        }
    }
    edges
}
