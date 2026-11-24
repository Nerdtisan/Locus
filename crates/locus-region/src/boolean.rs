// SPDX-License-Identifier: AGPL-3.0-or-later

//! Exact booleans on line/arc areas (SPEC §6.3, ADR 0008): classify every
//! edge of the operands' arrangement by each operand's winding number just
//! left and right of it, keep the edges where the result changes (result on
//! the left), trace them into loops and merge pieces on one line or circle.
//! Arcs stay arcs; no flattening, no refit.

use crate::arrange::{GEdge, arrange, in_dir, merge_carriers, out_dir};
use crate::seg::{Area, Edge, Loop};
use locus_geom::{Aabb, EXPORT_TOL, Vec2};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Largest offset of the side probes that classify an edge, mm (10× under
/// `EXPORT_TOL`, ADR 0008). A probe is pulled in closer when another edge
/// passes nearer than this, so a cusp where two curves leave a point
/// tangentially (every offset corner has them, ADR 0010) is still classified
/// on the right side of both.
const PROBE: f64 = 1e-4;

/// Smallest probe offset: far above f64 resolution at sketch coordinates.
const MIN_PROBE: f64 = 1e-9;

/// Loops with less area than this (mm²) are slivers, dropped from results.
const MIN_AREA: f64 = 1e-9;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum BoolOp {
    Union,
    Subtract,
    Intersect,
}

impl BoolOp {
    fn keep(self, a: bool, b: bool) -> bool {
        match self {
            BoolOp::Union => a || b,
            BoolOp::Subtract => a && !b,
            BoolOp::Intersect => a && b,
        }
    }
}

/// `a op b`. Inputs are combined by the non-zero rule, so overlapping loops
/// within one operand are fine. The result's outer loops run CCW, holes CW;
/// each edge keeps the source of the piece it came from.
#[must_use]
pub fn boolean(a: &Area, b: &Area, op: BoolOp) -> Area {
    let input: Vec<Edge> = a.edges().chain(b.edges()).copied().collect();
    let arr = arrange(&input);
    let (fa, fb) = (Boxed::new(a), Boxed::new(b));
    let near = Near::new(&arr.edges);
    // Per arrangement edge: not kept, kept forward, or kept reversed.
    let mut state: Vec<Option<bool>> = Vec::with_capacity(arr.edges.len());
    for (i, e) in arr.edges.iter().enumerate() {
        let seg = e.edge.seg;
        let m = seg.at(0.5);
        let reach = PROBE.min(seg.length() * 0.1);
        let probe = (0.5 * near.distance(i, m, reach)).clamp(MIN_PROBE, reach);
        let n = seg.tangent(0.5).perp() * probe;
        let inside = |q: Vec2| op.keep(fa.contains(q), fb.contains(q));
        let (left, right) = (inside(m + n), inside(m - n));
        state.push((left != right).then_some(left));
    }
    repair_micro(&arr.edges, &mut state);
    if std::env::var("DBG_BOOL").is_ok() {
        let mut bal: HashMap<usize, i32> = HashMap::new();
        for (e, s) in arr.edges.iter().zip(&state) {
            match s {
                Some(true) => {
                    *bal.entry(e.v0).or_default() += 1;
                    *bal.entry(e.v1).or_default() -= 1;
                }
                Some(false) => {
                    *bal.entry(e.v1).or_default() += 1;
                    *bal.entry(e.v0).or_default() -= 1;
                }
                None => {}
            }
        }
        for (v, b) in &bal {
            if *b != 0 {
                eprintln!("unbalanced v{v} {:?} bal {b}", arr.verts[*v]);
                for (k, (e, s)) in arr.edges.iter().zip(&state).enumerate() {
                    if e.v0 == *v || e.v1 == *v {
                        eprintln!(
                            "   e{k} v{}->v{} len {:.3e} state {:?} {:?}",
                            e.v0,
                            e.v1,
                            e.edge.seg.length(),
                            s,
                            e.edge.seg
                        );
                    }
                }
            }
        }
    }
    let kept: Vec<GEdge> = arr
        .edges
        .iter()
        .zip(&state)
        .filter_map(|(e, s)| match s {
            Some(true) => Some(*e),
            Some(false) => Some(GEdge {
                edge: Edge {
                    seg: e.edge.seg.reversed(),
                    source: e.edge.source,
                },
                v0: e.v1,
                v1: e.v0,
            }),
            None => None,
        })
        .collect();
    let loops = trace(&kept)
        .into_iter()
        .filter(|l| l.area().abs() > MIN_AREA)
        .collect();
    Area { loops }
}

/// Edges shorter than this may be re-classified to keep the result closed:
/// changing one moves the boundary by less than the export tolerance.
const MICRO: f64 = EXPORT_TOL;

/// Repairs classification inconsistencies on micro-edges. The kept edges of a
/// consistent classification enter and leave every vertex equally often. An
/// edge only a few nm–µm long (near-tangent pieces that cross right next to
/// a shared vertex leave one) has probes so close to other edges that its
/// classification can come out wrong, which leaves an open chain that
/// `trace` would drop with its whole loop. Each micro-edge takes whichever
/// state (dropped, forward, reversed) best balances its two vertices;
/// bounded passes, and a no-op when everything already balances.
fn repair_micro(edges: &[GEdge], state: &mut [Option<bool>]) {
    let mut bal: HashMap<usize, i32> = HashMap::new();
    let flow = |e: &GEdge, s: Option<bool>| match s {
        Some(true) => Some((e.v0, e.v1)),
        Some(false) => Some((e.v1, e.v0)),
        None => None,
    };
    for (e, s) in edges.iter().zip(state.iter()) {
        if let Some((from, to)) = flow(e, *s) {
            *bal.entry(from).or_default() += 1;
            *bal.entry(to).or_default() -= 1;
        }
    }
    if bal.values().all(|&b| b == 0) {
        return;
    }
    for _ in 0..4 {
        let mut changed = false;
        for (e, s) in edges.iter().zip(state.iter_mut()) {
            if e.edge.seg.length() > MICRO || e.v0 == e.v1 {
                continue;
            }
            let b0 = bal.get(&e.v0).copied().unwrap_or(0);
            let b1 = bal.get(&e.v1).copied().unwrap_or(0);
            if b0 == 0 && b1 == 0 {
                continue;
            }
            // Balances with this edge's contribution taken out.
            let (mut c0, mut c1) = (b0, b1);
            match *s {
                Some(true) => (c0, c1) = (c0 - 1, c1 + 1),
                Some(false) => (c0, c1) = (c0 + 1, c1 - 1),
                None => {}
            }
            let cost = |x: i32, y: i32| x.abs() + y.abs();
            let best = [
                (None, cost(c0, c1)),
                (Some(true), cost(c0 + 1, c1 - 1)),
                (Some(false), cost(c0 - 1, c1 + 1)),
            ]
            .into_iter()
            .min_by_key(|&(_, c)| c)
            .map_or(*s, |(st, _)| st);
            if best != *s
                && cost(b0, b1)
                    > cost(c0, c1)
                        .min(cost(c0 + 1, c1 - 1))
                        .min(cost(c0 - 1, c1 + 1))
            {
                let (n0, n1) = match best {
                    Some(true) => (c0 + 1, c1 - 1),
                    Some(false) => (c0 - 1, c1 + 1),
                    None => (c0, c1),
                };
                bal.insert(e.v0, n0);
                bal.insert(e.v1, n1);
                *s = best;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
}

/// Uniform grid over bounding boxes, so a probe only looks at the items
/// (loops or edges) whose box is near it.
struct BoxGrid {
    cell: f64,
    cells: HashMap<(i64, i64), Vec<usize>>,
    /// Items too big for the grid: every query visits them.
    overflow: Vec<usize>,
}

impl BoxGrid {
    /// Upper bound on the cells one item is entered in.
    const MAX_CELLS: i64 = 1024;

    fn new(bbs: &[Aabb]) -> Self {
        let span = bbs
            .iter()
            .copied()
            .reduce(Aabb::union)
            .map_or(1.0, |t| t.width().max(t.height()));
        let n = (bbs.len().max(1) as f64).sqrt();
        let cell = (span / n).max(PROBE * 4.0);
        let mut grid = Self {
            cell,
            cells: HashMap::new(),
            overflow: Vec::new(),
        };
        for (i, bb) in bbs.iter().enumerate() {
            let (lo, hi) = (grid.key(bb.min), grid.key(bb.max));
            if (hi.0 - lo.0 + 1).saturating_mul(hi.1 - lo.1 + 1) > Self::MAX_CELLS {
                grid.overflow.push(i);
                continue;
            }
            for x in lo.0..=hi.0 {
                for y in lo.1..=hi.1 {
                    grid.cells.entry((x, y)).or_default().push(i);
                }
            }
        }
        grid
    }

    fn key(&self, q: Vec2) -> (i64, i64) {
        // Saturating casts: coordinates are finite and bounded.
        (
            (q.x / self.cell).floor() as i64,
            (q.y / self.cell).floor() as i64,
        )
    }

    /// Items whose box may hold `q`, each once.
    fn at(&self, q: Vec2) -> impl Iterator<Item = usize> + '_ {
        let cell = self.cells.get(&self.key(q)).map_or(&[][..], Vec::as_slice);
        cell.iter().chain(&self.overflow).copied()
    }

    /// Items whose box may come within `r` of `q` (possibly repeated).
    fn near(&self, q: Vec2, r: f64) -> impl Iterator<Item = usize> + '_ {
        let (lo, hi) = (self.key(q - Vec2::new(r, r)), self.key(q + Vec2::new(r, r)));
        (lo.0..=hi.0)
            .flat_map(move |x| (lo.1..=hi.1).map(move |y| (x, y)))
            .filter_map(|k| self.cells.get(&k))
            .flatten()
            .chain(&self.overflow)
            .copied()
    }
}

/// An operand with its loops in a grid. A loop's winding number is zero
/// outside its box, so a probe only walks the loops whose box holds it.
struct Boxed<'a> {
    /// Each loop with its box and its edges' boxes.
    loops: Vec<(&'a Loop, Aabb, Vec<Aabb>)>,
    grid: BoxGrid,
}

impl<'a> Boxed<'a> {
    fn new(a: &'a Area) -> Self {
        let loops: Vec<(&Loop, Aabb, Vec<Aabb>)> = a
            .loops
            .iter()
            .filter_map(|l| {
                let edges: Vec<Aabb> = l.edges.iter().map(|e| e.seg.bbox()).collect();
                let bb = edges.iter().copied().reduce(Aabb::union)?;
                Some((l, bb, edges))
            })
            .collect();
        let bbs: Vec<Aabb> = loops.iter().map(|(_, bb, _)| *bb).collect();
        Self {
            grid: BoxGrid::new(&bbs),
            loops,
        }
    }

    fn contains(&self, q: Vec2) -> bool {
        self.grid
            .at(q)
            .filter(|&i| self.loops[i].1.contains(q, 0.0))
            .map(|i| self.loops[i].0.winding_boxed(q, Some(&self.loops[i].2)))
            .sum::<i32>()
            != 0
    }
}

/// The arrangement's edges in a grid, for the distance from a probe's base
/// to the nearest other edge.
struct Near<'a> {
    edges: &'a [GEdge],
    grid: BoxGrid,
}

impl<'a> Near<'a> {
    fn new(edges: &'a [GEdge]) -> Self {
        let bbs: Vec<Aabb> = edges.iter().map(|e| e.edge.seg.bbox()).collect();
        Self {
            edges,
            grid: BoxGrid::new(&bbs),
        }
    }

    /// Distance from `q` to the nearest edge other than `skip`, or `reach`
    /// when none is closer.
    fn distance(&self, skip: usize, q: Vec2, reach: f64) -> f64 {
        self.grid
            .near(q, reach)
            .filter(|&k| k != skip)
            .map(|k| self.edges[k].edge.seg.distance(q))
            .fold(reach, f64::min)
    }
}

/// Union of several areas.
#[must_use]
pub fn union_all(areas: &[Area]) -> Area {
    // One arrangement for everything: the non-zero rule already unions
    // overlapping loops, so this is `all ∪ ∅`.
    let all = Area {
        loops: areas.iter().flat_map(|a| a.loops.iter().cloned()).collect(),
    };
    boolean(&all, &Area::default(), BoolOp::Union)
}

/// Chains result edges (result on the left) into closed loops. Where several
/// leave one vertex (loops touching at a point), take the sharpest left turn
/// so each touching loop closes on its own.
fn trace(kept: &[GEdge]) -> Vec<Loop> {
    let mut out: HashMap<usize, Vec<usize>> = HashMap::new();
    for (i, e) in kept.iter().enumerate() {
        out.entry(e.v0).or_default().push(i);
    }
    let turn = |din: Vec2, k: usize| {
        let d = out_dir(&kept[k].edge.seg);
        din.cross(d).atan2(din.dot(d))
    };
    let mut used = vec![false; kept.len()];
    let mut loops = Vec::new();
    for first in 0..kept.len() {
        if used[first] {
            continue;
        }
        used[first] = true;
        let mut edges = vec![kept[first].edge];
        let mut cur = first;
        let mut closed = true;
        while kept[cur].v1 != kept[first].v0 {
            let din = in_dir(&kept[cur].edge.seg);
            let next = out.get(&kept[cur].v1).and_then(|c| {
                c.iter()
                    .copied()
                    .filter(|&k| !used[k])
                    .max_by(|&x, &y| turn(din, x).total_cmp(&turn(din, y)))
            });
            let Some(nx) = next else {
                // Only possible on a classification inconsistency; an open
                // chain is never returned as a loop.
                closed = false;
                break;
            };
            used[nx] = true;
            edges.push(kept[nx].edge);
            cur = nx;
        }
        if closed {
            loops.push(Loop {
                edges: merge_carriers(edges, false),
            });
        }
    }
    loops
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::seg::Seg;
    use core::f64::consts::{FRAC_PI_2, PI, TAU};

    fn v(x: f64, y: f64) -> Vec2 {
        Vec2::new(x, y)
    }

    fn lp(segs: Vec<Seg>) -> Area {
        Area {
            loops: vec![Loop {
                edges: segs
                    .into_iter()
                    .map(|seg| Edge { seg, source: 1 })
                    .collect(),
            }],
        }
    }

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Area {
        let (a, b, c, d) = (v(x0, y0), v(x1, y0), v(x1, y1), v(x0, y1));
        lp(vec![
            Seg::Line { a, b },
            Seg::Line { a: b, b: c },
            Seg::Line { a: c, b: d },
            Seg::Line { a: d, b: a },
        ])
    }

    fn circle(c: Vec2, r: f64) -> Area {
        lp(vec![Seg::Arc {
            center: c,
            radius: r,
            start: 0.0,
            sweep: TAU,
        }])
    }

    fn slot(c1: Vec2, c2: Vec2, r: f64) -> Area {
        let n = (c2 - c1).normalized(0.0).unwrap().perp();
        let na = n.angle();
        lp(vec![
            Seg::Line {
                a: c1 - n * r,
                b: c2 - n * r,
            },
            Seg::Arc {
                center: c2,
                radius: r,
                start: na - PI,
                sweep: PI,
            },
            Seg::Line {
                a: c2 + n * r,
                b: c1 + n * r,
            },
            Seg::Arc {
                center: c1,
                radius: r,
                start: na,
                sweep: PI,
            },
        ])
    }

    fn rounded(x0: f64, y0: f64, x1: f64, y1: f64, r: f64) -> Area {
        let h = FRAC_PI_2;
        lp(vec![
            Seg::Line {
                a: v(x0 + r, y0),
                b: v(x1 - r, y0),
            },
            Seg::Arc {
                center: v(x1 - r, y0 + r),
                radius: r,
                start: -h,
                sweep: h,
            },
            Seg::Line {
                a: v(x1, y0 + r),
                b: v(x1, y1 - r),
            },
            Seg::Arc {
                center: v(x1 - r, y1 - r),
                radius: r,
                start: 0.0,
                sweep: h,
            },
            Seg::Line {
                a: v(x1 - r, y1),
                b: v(x0 + r, y1),
            },
            Seg::Arc {
                center: v(x0 + r, y1 - r),
                radius: r,
                start: h,
                sweep: h,
            },
            Seg::Line {
                a: v(x0, y1 - r),
                b: v(x0, y0 + r),
            },
            Seg::Arc {
                center: v(x0 + r, y0 + r),
                radius: r,
                start: PI,
                sweep: h,
            },
        ])
    }

    /// (arcs, lines, loops)
    fn counts(a: &Area) -> (usize, usize, usize) {
        let arcs = a.edges().filter(|e| e.seg.is_arc()).count();
        (arcs, a.edges().count() - arcs, a.loops.len())
    }

    fn closed(a: &Area) -> bool {
        a.loops.iter().all(|l| {
            let n = l.edges.len();
            (0..n).all(|i| {
                l.edges[i]
                    .seg
                    .end()
                    .distance(l.edges[(i + 1) % n].seg.start())
                    <= 1e-9
            })
        })
    }

    /// The ADR 0008 fixtures, with hand-counted minimal results. An internal
    /// tangency comes back as an outer loop and a hole touching it at a point.
    #[test]
    fn adr_fixtures_are_exact_and_minimal() {
        use BoolOp::*;
        type Case = (&'static str, Area, Area, BoolOp, (usize, usize, usize), f64);
        let cases: Vec<Case> = vec![
            (
                "rect ∪ circle on corner",
                rect(0.0, 0.0, 20.0, 10.0),
                circle(v(20.0, 10.0), 5.0),
                Union,
                (1, 4, 1),
                200.0 + 0.75 * PI * 25.0,
            ),
            (
                "lens",
                circle(v(0.0, 0.0), 10.0),
                circle(v(12.0, 0.0), 10.0),
                Intersect,
                (2, 0, 1),
                2.0 * (100.0 * (0.6f64).acos() - 6.0 * 8.0),
            ),
            (
                "shared edge",
                rect(0.0, 0.0, 10.0, 10.0),
                rect(10.0, 0.0, 20.0, 10.0),
                Union,
                (0, 4, 1),
                200.0,
            ),
            (
                "partly shared edge",
                rect(0.0, 0.0, 10.0, 10.0),
                rect(10.0, 5.0, 20.0, 15.0),
                Union,
                (0, 8, 1),
                200.0,
            ),
            (
                "touching corner",
                rect(0.0, 0.0, 10.0, 10.0),
                rect(10.0, 10.0, 20.0, 20.0),
                Union,
                (0, 8, 2),
                200.0,
            ),
            (
                "notch",
                rect(0.0, 0.0, 20.0, 10.0),
                rect(5.0, 5.0, 15.0, 10.0),
                Subtract,
                (0, 8, 1),
                150.0,
            ),
            (
                "annulus",
                circle(v(0.0, 0.0), 10.0),
                circle(v(0.0, 0.0), 5.0),
                Subtract,
                (2, 0, 2),
                75.0 * PI,
            ),
            (
                "external tangent",
                circle(v(0.0, 0.0), 5.0),
                circle(v(10.0, 0.0), 5.0),
                Union,
                (2, 0, 2),
                50.0 * PI,
            ),
            (
                "internal tangent",
                circle(v(0.0, 0.0), 10.0),
                circle(v(5.0, 0.0), 5.0),
                Subtract,
                (2, 0, 2),
                75.0 * PI,
            ),
            (
                "coincident cap",
                slot(v(0.0, 0.0), v(20.0, 0.0), 5.0),
                circle(v(20.0, 0.0), 5.0),
                Union,
                (2, 2, 1),
                200.0 + 25.0 * PI,
            ),
            (
                "same circle",
                circle(v(3.0, 4.0), 7.0),
                circle(v(3.0, 4.0), 7.0),
                Union,
                (1, 0, 1),
                49.0 * PI,
            ),
            (
                "tangent at a point",
                rect(0.0, 0.0, 10.0, 10.0),
                circle(v(5.0, 15.0), 5.0),
                Intersect,
                (0, 0, 0),
                0.0,
            ),
            (
                "slot hole",
                rounded(0.0, 0.0, 40.0, 20.0, 4.0),
                slot(v(10.0, 10.0), v(30.0, 10.0), 3.0),
                Subtract,
                (6, 6, 2),
                800.0 - 16.0 * (4.0 - PI) - 120.0 - 9.0 * PI,
            ),
            (
                "disjoint intersect",
                rect(0.0, 0.0, 1.0, 1.0),
                rect(2.0, 2.0, 3.0, 3.0),
                Intersect,
                (0, 0, 0),
                0.0,
            ),
        ];
        for (name, a, b, op, want, area) in cases {
            let r = boolean(&a, &b, op);
            assert_eq!(counts(&r), want, "{name}");
            assert!(
                (r.area() - area).abs() < 1e-6,
                "{name}: area {} want {area}",
                r.area()
            );
            assert!(closed(&r), "{name}");
        }
    }

    #[test]
    fn operand_with_a_hole() {
        let mut annulus = circle(v(0.0, 0.0), 10.0);
        annulus
            .loops
            .push(circle(v(0.0, 0.0), 5.0).loops[0].reversed());
        let r = boolean(&annulus, &rect(-2.0, -12.0, 2.0, 12.0), BoolOp::Union);
        assert_eq!(counts(&r), (4, 8, 3));
        assert!(closed(&r));
    }

    #[test]
    fn union_all_merges_a_chain() {
        let cs: Vec<Area> = (0..40)
            .map(|i| circle(v(4.0 * f64::from(i), 0.0), 3.0))
            .collect();
        let r = union_all(&cs);
        assert_eq!(counts(&r), (78, 0, 1));
    }

    #[test]
    fn near_coincident_edges_leave_no_slivers() {
        let r = boolean(
            &rect(0.0, 0.0, 10.0, 10.0),
            &rect(10.0 + 1e-7, 0.0, 20.0, 10.0),
            BoolOp::Union,
        );
        assert_eq!(counts(&r), (0, 4, 1));
        let r = boolean(
            &rect(0.0, 0.0, 10.0, 10.0),
            &rect(1e-7, 1e-7, 10.0, 10.0),
            BoolOp::Subtract,
        );
        assert!(r.is_empty());
    }

    #[test]
    fn sources_follow_the_pieces() {
        let mut a = rect(0.0, 0.0, 10.0, 10.0);
        let mut b = circle(v(10.0, 10.0), 5.0);
        a.loops[0].edges.iter_mut().for_each(|e| e.source = 7);
        b.loops[0].edges[0].source = 9;
        let r = boolean(&a, &b, BoolOp::Union);
        let arc = r.edges().find(|e| e.seg.is_arc()).unwrap();
        assert_eq!(arc.source, 9);
        assert!(r.edges().filter(|e| !e.seg.is_arc()).all(|e| e.source == 7));
    }
}
