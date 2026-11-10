// SPDX-License-Identifier: AGPL-3.0-or-later

//! Offsets of line/arc areas and chains (SPEC §6.2 offset, §7 kerf and
//! blade offsets; [ADR 0010](../../docs/adr/0010-offsets.md)).
//!
//! The offset of an area A by d is a Minkowski sum or difference with a disc
//! of radius |d|: grow = A ∪ (∂A ⊕ D), shrink = A − (∂A ⊕ D). `∂A ⊕ D` is the
//! union of exact pieces: a band per boundary edge (a rectangle for a line,
//! an annular sector or wedge for an arc) and a disc at every corner that
//! opens a gap on the offset side. All of them are true lines and arcs, so
//! one boolean on the ADR 0008 arrangement gives the exact result, with
//! self-intersections, collapsed features and nesting resolved for free.
//! Cubic Béziers are lowered to biarcs before they get here (`locus_geom::
//! Cubic::to_pieces`), so their offsets are arcs too.

use crate::arrange::SNAP;
use crate::boolean::{BoolOp, boolean};
use crate::seg::{Area, Edge, Loop, Seg};
use core::f64::consts::{PI, TAU};
use locus_geom::{EXPORT_TOL, Vec2};
use serde::{Deserialize, Serialize};

/// Corner treatment on the side being offset.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Join {
    /// An arc about the corner: the true offset, and what a round laser beam
    /// or pen cuts.
    Round,
    /// Tangents extended to a sharp corner while the miter point stays within
    /// `limit` × |d| of the corner; round beyond that.
    Miter { limit: f64 },
}

/// Corners turning less than this (radians) are smooth: no disc needed.
const SMOOTH: f64 = 1e-9;

fn arc(center: Vec2, radius: f64, start: f64, sweep: f64, source: u64) -> Edge {
    Edge {
        seg: Seg::Arc {
            center,
            radius,
            start,
            sweep,
        },
        source,
    }
}

fn line(a: Vec2, b: Vec2, source: u64) -> Edge {
    Edge {
        seg: Seg::Line { a, b },
        source,
    }
}

fn disc(c: Vec2, r: f64, source: u64) -> Loop {
    Loop {
        edges: vec![arc(c, r, 0.0, TAU, source)],
    }
}

/// The points whose nearest point on `seg` is interior and at most `w` away,
/// as CCW loops (a full circle gives an annulus: a CCW and a CW loop).
fn band(seg: &Seg, w: f64, source: u64, out: &mut Vec<Loop>) {
    match *seg {
        Seg::Line { a, b } => {
            let Some(u) = (b - a).normalized(0.0) else {
                return;
            };
            let n = u.perp() * w;
            out.push(Loop {
                edges: vec![
                    line(a - n, b - n, source),
                    line(b - n, b + n, source),
                    line(b + n, a + n, source),
                    line(a + n, a - n, source),
                ],
            });
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
            let (ro, ri) = (radius + w, radius - w);
            if sw >= TAU - 1e-12 {
                out.push(disc(center, ro, source));
                if ri > 0.0 {
                    out.push(disc(center, ri, source).reversed());
                }
                return;
            }
            let dir = Vec2::from_angle;
            let mut edges = vec![arc(center, ro, s, sw, source)];
            if ri > 0.0 {
                edges.push(line(
                    center + dir(s + sw) * ro,
                    center + dir(s + sw) * ri,
                    source,
                ));
                edges.push(arc(center, ri, s + sw, -sw, source));
                edges.push(line(center + dir(s) * ri, center + dir(s) * ro, source));
            } else {
                // The offset passes the centre: a wedge.
                edges.push(line(center + dir(s + sw) * ro, center, source));
                edges.push(line(center, center + dir(s) * ro, source));
            }
            out.push(Loop { edges });
        }
    }
}

/// Signed turn (radians, CCW > 0) from the end of `a` into the start of `b`,
/// and the two tangents.
fn turn(a: &Seg, b: &Seg) -> (f64, Vec2, Vec2) {
    let (t1, t2) = (a.tangent(1.0), b.tangent(0.0));
    (t1.cross(t2).atan2(t1.dot(t2)), t1, t2)
}

/// The miter kite at corner `v` between unit normals `n1` and `n2`, or `None`
/// when the miter point is farther than `limit` × `w` from the corner.
fn kite(v: Vec2, n1: Vec2, n2: Vec2, w: f64, limit: f64, source: u64) -> Option<Loop> {
    let bis = (n1 + n2).normalized(0.0)?;
    let c = bis.dot(n1);
    if c <= 0.0 || 1.0 / c > limit {
        return None;
    }
    let pts = [v, v + n1 * w, v + bis * (w / c), v + n2 * w];
    let l = Loop {
        edges: (0..4)
            .map(|i| line(pts[i], pts[(i + 1) % 4], source))
            .collect(),
    };
    Some(if l.area() < 0.0 { l.reversed() } else { l })
}

/// The corner piece at `v`, on `side` (+1 = right of travel, -1 = left):
/// a kite for a miter join that fits, else a disc. `None` when the corner
/// doesn't open a gap on that side.
fn corner(v: Vec2, a: &Seg, b: &Seg, w: f64, side: f64, join: Join, source: u64) -> Option<Loop> {
    let (t, t1, t2) = turn(a, b);
    // Points whose nearest boundary point is this corner fill its normal
    // cone, which is on `side` only when the turn is away from it. At a
    // reversal (a cusp) the turn's sign is noise and the cone is a half-disc
    // reaching both sides.
    let reversal = t.abs() >= PI - SMOOTH;
    if t.abs() <= SMOOTH || (t * side <= 0.0 && !reversal) {
        return None;
    }
    let miter = match join {
        Join::Miter { limit } if !reversal => {
            let nrm = |t: Vec2| -t.perp() * side;
            kite(v, nrm(t1), nrm(t2), w, limit, source)
        }
        _ => None,
    };
    Some(miter.unwrap_or_else(|| disc(v, w, source)))
}

/// Offsets an area (outer loops CCW, holes CW, as booleans and region
/// detection return them) by `d`: > 0 grows, < 0 shrinks. Each result edge
/// keeps the source of the boundary edge it was offset from. Loops thinner
/// than `EXPORT_TOL` are dropped (see [`drop_specks`]).
#[must_use]
pub fn offset(a: &Area, d: f64, join: Join) -> Area {
    let w = d.abs();
    if !d.is_finite() || w <= SNAP {
        return a.clone();
    }
    let side = d.signum();
    let mut tool = Vec::new();
    for l in &a.loops {
        let n = l.edges.len();
        for (i, e) in l.edges.iter().enumerate() {
            band(&e.seg, w, e.source, &mut tool);
            let nx = &l.edges[(i + 1) % n];
            if let Some(c) = corner(e.seg.end(), &e.seg, &nx.seg, w, side, join, e.source) {
                tool.push(c);
            }
        }
    }
    let mut r = if d > 0.0 {
        let mut all = a.loops.clone();
        all.extend(tool);
        boolean(&Area { loops: all }, &Area::default(), BoolOp::Union)
    } else {
        boolean(a, &Area { loops: tool }, BoolOp::Subtract)
    };
    drop_specks(&mut r);
    r
}

/// Drops loops whose mean width (2·area / perimeter) is under `EXPORT_TOL`:
/// the specks and slivers left where an offset nearly vanishes, which no
/// machine can cut (ADR 0010).
pub fn drop_specks(r: &mut Area) {
    r.loops.retain(|l| {
        let per: f64 = l.edges.iter().map(|e| e.seg.length()).sum();
        2.0 * l.area().abs() > EXPORT_TOL * per
    });
}

/// One-sided offset of an open chain (each edge ends where the next starts):
/// `d` > 0 to the left of travel, < 0 to the right. Returns the runs of the
/// chain's tube boundary (`chain ⊕ D`) that lie on that side and are not end
/// caps, each in the chain's direction. Self-intersections are gone: a run
/// breaks where the offset folds over itself.
#[must_use]
pub fn offset_open(chain: &[Edge], d: f64, join: Join) -> Vec<Vec<Edge>> {
    let w = d.abs();
    if chain.is_empty() || !d.is_finite() || w <= SNAP {
        return Vec::new();
    }
    // Corners use the right-hand convention of `corner`.
    let side = -d.signum();
    let mut tool = Vec::new();
    for (i, e) in chain.iter().enumerate() {
        band(&e.seg, w, e.source, &mut tool);
        if i == 0 {
            tool.push(disc(e.seg.start(), w, e.source));
        }
        match chain.get(i + 1) {
            Some(nx) => {
                let v = e.seg.end();
                // The far side needs its disc too, so the tube is whole.
                let piece = corner(v, &e.seg, &nx.seg, w, side, join, e.source)
                    .or_else(|| corner(v, &e.seg, &nx.seg, w, -side, Join::Round, e.source));
                tool.extend(piece);
            }
            None => tool.push(disc(e.seg.end(), w, e.source)),
        }
    }
    let tube = boolean(&Area { loops: tool }, &Area::default(), BoolOp::Union);
    let last = chain.len() - 1;
    let keep = |e: &Edge| -> bool {
        let m = e.seg.at(0.5);
        let mut best = (f64::INFINITY, 0usize, 0.0);
        for (i, c) in chain.iter().enumerate() {
            let tp = c.seg.param_of(m).clamp(0.0, 1.0);
            let dist = c.seg.at(tp).distance(m);
            if dist < best.0 {
                best = (dist, i, tp);
            }
        }
        let (_, i, tp) = best;
        let cap = (i == 0 && tp <= 1e-9) || (i == last && tp >= 1.0 - 1e-9);
        let foot = chain[i].seg.at(tp);
        !cap && chain[i].seg.tangent(tp).cross(m - foot) * d > 0.0
    };
    let mut runs = Vec::new();
    for l in &tube.loops {
        let n = l.edges.len();
        let k: Vec<bool> = l.edges.iter().map(keep).collect();
        // Start just after a dropped edge so no run wraps round the loop.
        let Some(s) = (0..n).find(|&i| !k[i]) else {
            runs.push(l.edges.clone());
            continue;
        };
        let mut cur: Vec<Edge> = Vec::new();
        for j in 1..=n {
            let i = (s + j) % n;
            if k[i] {
                cur.push(l.edges[i]);
            } else if !cur.is_empty() {
                runs.push(core::mem::take(&mut cur));
            }
        }
    }
    // The tube runs CCW, so its left-side boundary goes against the chain.
    if d > 0.0 {
        for r in &mut runs {
            *r = Loop { edges: r.clone() }.reversed().edges;
        }
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::f64::consts::FRAC_PI_2;

    fn v(x: f64, y: f64) -> Vec2 {
        Vec2::new(x, y)
    }

    fn poly(pts: &[Vec2]) -> Loop {
        let n = pts.len();
        Loop {
            edges: (0..n)
                .map(|i| line(pts[i], pts[(i + 1) % n], i as u64))
                .collect(),
        }
    }

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Loop {
        poly(&[v(x0, y0), v(x1, y0), v(x1, y1), v(x0, y1)])
    }

    fn area(loops: Vec<Loop>) -> Area {
        Area { loops }
    }

    fn slot(c1: Vec2, c2: Vec2, r: f64) -> Loop {
        let n = (c2 - c1).normalized(0.0).unwrap().perp();
        let na = n.angle();
        Loop {
            edges: vec![
                line(c1 - n * r, c2 - n * r, 0),
                arc(c2, r, na - PI, PI, 1),
                line(c2 + n * r, c1 + n * r, 2),
                arc(c1, r, na, PI, 3),
            ],
        }
    }

    fn rounded(x0: f64, y0: f64, x1: f64, y1: f64, r: f64) -> Loop {
        let h = FRAC_PI_2;
        Loop {
            edges: vec![
                line(v(x0 + r, y0), v(x1 - r, y0), 0),
                arc(v(x1 - r, y0 + r), r, -h, h, 1),
                line(v(x1, y0 + r), v(x1, y1 - r), 2),
                arc(v(x1 - r, y1 - r), r, 0.0, h, 3),
                line(v(x1 - r, y1), v(x0 + r, y1), 4),
                arc(v(x0 + r, y1 - r), r, h, h, 5),
                line(v(x0, y1 - r), v(x0, y0 + r), 6),
                arc(v(x0 + r, y0 + r), r, PI, h, 7),
            ],
        }
    }

    fn star(n: usize, ro: f64, ri: f64) -> Loop {
        let pts: Vec<Vec2> = (0..2 * n)
            .map(|i| {
                let a = i as f64 * PI / n as f64;
                Vec2::from_angle(a) * if i % 2 == 0 { ro } else { ri }
            })
            .collect();
        poly(&pts)
    }

    fn counts(a: &Area) -> (usize, usize, usize) {
        let arcs = a.edges().filter(|e| e.seg.is_arc()).count();
        (arcs, a.edges().count() - arcs, a.loops.len())
    }

    /// Every boundary point of `out` is |d| from ∂A.
    fn on_offset(a: &Area, d: f64, out: &Area) -> bool {
        out.edges().all(|e| {
            (0..=20).all(|k| {
                let q = e.seg.at(f64::from(k) / 20.0);
                let dist = a
                    .edges()
                    .map(|s| s.seg.distance(q))
                    .fold(f64::INFINITY, f64::min);
                (dist - d.abs()).abs() < 1e-9
            })
        })
    }

    #[test]
    fn rectangle_grows_round_and_shrinks_sharp() {
        let a = area(vec![rect(0.0, 0.0, 40.0, 20.0)]);
        let g = offset(&a, 1.0, Join::Round);
        assert_eq!(counts(&g), (4, 4, 1));
        assert!((g.area() - (800.0 + 2.0 * 60.0 + PI)).abs() < 1e-9);
        assert!(on_offset(&a, 1.0, &g));
        let s = offset(&a, -1.0, Join::Round);
        assert_eq!(counts(&s), (0, 4, 1));
        assert!((s.area() - 38.0 * 18.0).abs() < 1e-9);
        // Shrinking past half the width leaves nothing, not a sliver.
        assert!(offset(&a, -10.0, Join::Round).is_empty());
    }

    #[test]
    fn miter_keeps_sharp_corners() {
        let a = area(vec![rect(0.0, 0.0, 40.0, 20.0)]);
        let g = offset(&a, 1.0, Join::Miter { limit: 4.0 });
        assert_eq!(counts(&g), (0, 4, 1));
        assert!((g.area() - 42.0 * 22.0).abs() < 1e-9);
        // A 1° spike's miter is far over the limit: rounded instead.
        let spike = area(vec![poly(&[v(0.0, 0.0), v(100.0, 0.0), v(0.0, 1.745)])]);
        assert!(
            offset(&spike, 1.0, Join::Miter { limit: 4.0 })
                .edges()
                .any(|e| e.seg.is_arc())
        );
    }

    #[test]
    fn tight_inside_corners_and_narrow_notches() {
        let a = area(vec![star(12, 20.0, 8.0)]);
        for d in [1.0, -1.0, -3.0] {
            let r = offset(&a, d, Join::Round);
            assert_eq!(r.loops.len(), 1, "d = {d}");
            assert!(on_offset(&a, d, &r), "d = {d}");
        }
        // Notches 0.3 wide close when grown by 0.15 or more.
        let mut pts = vec![v(0.0, 0.0), v(40.0, 0.0), v(40.0, 10.0)];
        for i in (1..=9).rev() {
            let x = 4.0 * f64::from(i);
            pts.extend([
                v(x + 0.15, 10.0),
                v(x + 0.15, 6.0),
                v(x - 0.15, 6.0),
                v(x - 0.15, 10.0),
            ]);
        }
        pts.push(v(0.0, 10.0));
        let comb = area(vec![poly(&pts)]);
        for d in [0.15, 0.2] {
            let r = offset(&comb, d, Join::Round);
            assert_eq!(counts(&r).2, 1, "d = {d}");
            assert!(
                r.edges()
                    .all(|e| e.seg.at(0.5).y < 10.0 + d + 1e-9 && e.seg.at(0.5).y > -d - 1e-9)
            );
            assert!(
                !r.edges().any(|e| e.seg.distance(v(8.0, 8.0)) < 0.1),
                "notch closed"
            );
        }
    }

    #[test]
    fn small_radii_collapse_to_sharp_corners() {
        let a = area(vec![rounded(0.0, 0.0, 30.0, 20.0, 0.5)]);
        let r = offset(&a, -1.0, Join::Round);
        assert_eq!(counts(&r), (0, 4, 1));
        assert!((r.area() - 28.0 * 18.0).abs() < 1e-9);
        let g = offset(&a, 1.0, Join::Round);
        assert_eq!(counts(&g), (4, 4, 1));
        assert!(
            g.edges()
                .filter(|e| e.seg.is_arc())
                .all(|e| matches!(e.seg, Seg::Arc { radius, .. } if (radius - 1.5).abs() < 1e-12))
        );
    }

    #[test]
    fn circles_and_slots_shrink_to_nothing_cleanly() {
        let c = area(vec![disc(v(0.0, 0.0), 1.0, 0)]);
        assert_eq!(counts(&offset(&c, -0.999, Join::Round)), (1, 0, 1));
        assert!(offset(&c, -1.0, Join::Round).is_empty());
        let s = area(vec![slot(v(0.0, 0.0), v(20.0, 0.0), 3.0)]);
        assert_eq!(counts(&offset(&s, -2.9, Join::Round)), (2, 2, 1));
        assert!(offset(&s, -3.0, Join::Round).is_empty());
        assert_eq!(counts(&offset(&s, 2.0, Join::Round)), (2, 2, 1));
    }

    #[test]
    fn nested_parts_keep_their_nesting() {
        let a = area(vec![
            rounded(0.0, 0.0, 100.0, 60.0, 5.0),
            rect(20.0, 10.0, 80.0, 50.0).reversed(),
            disc(v(50.0, 30.0), 10.0, 9),
        ]);
        let r = offset(&a, 0.1, Join::Round);
        assert_eq!(r.loops.len(), 3);
        assert!(r.contains(v(50.0, 30.0)) && !r.contains(v(25.0, 30.0)) && r.contains(v(5.0, 1.0)));
        assert!(on_offset(&a, 0.1, &r));
        // Grown by 6 the island bridges the hole and splits it in two.
        let r = offset(&a, 6.0, Join::Round);
        assert_eq!(r.loops.len(), 3);
        assert!(r.contains(v(50.0, 30.0)) && !r.contains(v(28.0, 30.0)));
    }

    #[test]
    fn cusp_corner_gets_its_disc() {
        // A zero-width crack into the material that doubles back at its tip
        // (a 180° corner), as two rounded corners touching leave in a
        // boolean result.
        let a = area(vec![poly(&[
            v(0.0, 0.0),
            v(10.0, 0.0),
            v(10.0, 5.0),
            v(6.0, 5.0),
            v(6.0, 2.0),
            v(6.0, 5.0),
            v(0.0, 5.0),
        ])]);
        let r = offset(&a, -0.25, Join::Round);
        assert!(on_offset(&a, -0.25, &r));
    }

    #[test]
    fn sources_follow_the_offset_edges() {
        let a = area(vec![rect(0.0, 0.0, 10.0, 10.0)]);
        let r = offset(&a, -1.0, Join::Round);
        for e in r.edges() {
            let src = &a.loops[0].edges[usize::try_from(e.source).unwrap()].seg;
            assert!((src.distance(e.seg.at(0.5)) - 1.0).abs() < 1e-9);
        }
    }

    #[test]
    fn open_chains_offset_one_side() {
        let zig: Vec<Edge> = [v(0.0, 0.0), v(10.0, 3.0), v(1.0, 6.0), v(11.0, 9.0)]
            .windows(2)
            .enumerate()
            .map(|(i, w)| line(w[0], w[1], i as u64))
            .collect();
        for d in [1.0, -1.0] {
            let runs = offset_open(&zig, d, Join::Round);
            assert_eq!(runs.len(), 1, "d = {d}");
            let run = &runs[0];
            assert!(
                run.windows(2)
                    .all(|p| p[0].seg.end().distance(p[1].seg.start()) < 1e-9)
            );
            // Runs go the chain's way: starts near its start.
            assert!(run[0].seg.start().distance(v(0.0, 0.0)) < 1.0 + 1e-9);
            for e in run {
                let q = e.seg.at(0.5);
                let dist = zig
                    .iter()
                    .map(|s| s.seg.distance(q))
                    .fold(f64::INFINITY, f64::min);
                assert!((dist - 1.0).abs() < 1e-9);
            }
        }
        // A J hook whose r = 1 bend collapses on the inside.
        let j = vec![
            line(v(0.0, 10.0), v(0.0, 0.0), 0),
            arc(v(1.0, 0.0), 1.0, PI, PI, 1),
            line(v(2.0, 0.0), v(2.0, 5.0), 2),
        ];
        let inside = offset_open(&j, 2.0, Join::Round);
        assert_eq!(inside.len(), 1);
        assert!(
            inside[0].iter().all(
                |e| !e.seg.is_arc() || matches!(e.seg, Seg::Arc { radius, .. } if radius > 1.0)
            )
        );
        let mitred = offset_open(&zig, -1.0, Join::Miter { limit: 10.0 });
        assert!(mitred[0].iter().all(|e| !e.seg.is_arc()));
    }

    #[test]
    fn degenerate_input_is_harmless() {
        let a = area(vec![rect(0.0, 0.0, 1.0, 1.0)]);
        assert_eq!(offset(&a, f64::NAN, Join::Round), a);
        assert_eq!(offset(&a, 0.0, Join::Round), a);
        assert!(offset(&Area::default(), 1.0, Join::Round).is_empty());
        assert!(offset_open(&[], 1.0, Join::Round).is_empty());
    }
}
