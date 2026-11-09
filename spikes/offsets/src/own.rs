// SPDX-License-Identifier: AGPL-3.0-or-later

//! Option A: Locus-owned offsets on the ADR 0008 arrangement.
//!
//! The offset of an area A by d is a Minkowski sum or difference with a disc:
//! grow = A ∪ (∂A ⊕ D), shrink = A − (∂A ⊕ D). `∂A ⊕ D` is the union of one
//! exact "capsule" per boundary piece: a band (rectangle for a line, annular
//! sector or wedge for an arc) plus a disc at every corner. Every piece is a
//! true line or arc, so one `locus_region` boolean gives the exact result,
//! with self-intersections, collapsed features and nesting all resolved by
//! the same arrangement that ADR 0008 measured.

use core::f64::consts::TAU;
use locus_geom::Vec2;
use locus_region::{Area, BoolOp, Edge, Loop, Seg, boolean};

/// Corner treatment on the side being offset.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Join {
    /// Arc about the corner (the true offset; what a laser beam cuts).
    Round,
    /// Tangents extended to a sharp corner, as long as the miter stays under
    /// `limit` × d from the corner; round beyond that.
    Miter { limit: f64 },
}

/// Corners turning less than this (radians) count as smooth: no disc.
const SMOOTH: f64 = 1e-9;

fn arc(center: Vec2, radius: f64, start: f64, sweep: f64, source: u64) -> Edge {
    Edge { seg: Seg::Arc { center, radius, start, sweep }, source }
}

fn line(a: Vec2, b: Vec2, source: u64) -> Edge {
    Edge { seg: Seg::Line { a, b }, source }
}

fn disc(c: Vec2, r: f64, source: u64) -> Loop {
    Loop { edges: vec![arc(c, r, 0.0, TAU, source)] }
}

/// The band of points whose nearest point on `seg` is interior and at most
/// `w` away, as CCW loops (a full-circle arc gives an annulus: CCW + CW).
fn band(seg: &Seg, w: f64, source: u64, out: &mut Vec<Loop>) {
    match *seg {
        Seg::Line { a, b } => {
            let Some(u) = (b - a).normalized(0.0) else { return };
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
        Seg::Arc { center, radius, start, sweep } => {
            let (s, sw) = if sweep >= 0.0 { (start, sweep) } else { (start + sweep, -sweep) };
            let ro = radius + w;
            let ri = radius - w;
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
                edges.push(line(center + dir(s + sw) * ro, center + dir(s + sw) * ri, source));
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

/// Unit tangents arriving at the end of `a` and leaving the start of `b`.
fn corner(a: &Seg, b: &Seg) -> (Vec2, Vec2) {
    (a.tangent(1.0), b.tangent(0.0))
}

/// Miter kite at corner `v` on the side whose unit normals are `n1`, `n2`,
/// or `None` when the miter is longer than `limit` × w.
fn kite(v: Vec2, n1: Vec2, n2: Vec2, w: f64, limit: f64, source: u64) -> Option<Loop> {
    let bis = (n1 + n2).normalized(0.0)?;
    let c = bis.dot(n1);
    if c <= 0.0 || 1.0 / c > limit {
        return None;
    }
    let pts = [v, v + n1 * w, v + bis * (w / c), v + n2 * w];
    let mut l = Loop {
        edges: (0..4).map(|i| line(pts[i], pts[(i + 1) % 4], source)).collect(),
    };
    if l.area() < 0.0 {
        l = l.reversed();
    }
    Some(l)
}

/// `∂A ⊕ D(w)` plus miter kites on `side` (+1 = right of travel, i.e. outside
/// material that lies on the left; -1 = left).
fn tool(a: &Area, w: f64, side: f64, join: Join) -> Vec<Loop> {
    let mut out = Vec::new();
    for l in &a.loops {
        let n = l.edges.len();
        for (i, e) in l.edges.iter().enumerate() {
            band(&e.seg, w, e.source, &mut out);
            let nx = &l.edges[(i + 1) % n];
            let (t1, t2) = corner(&e.seg, &nx.seg);
            let turn = t1.cross(t2).atan2(t1.dot(t2));
            // Points whose nearest boundary point is this corner lie in its
            // normal cone. It is on the offset side only when the turn is
            // away from that side; otherwise those points are inside A
            // (growing) or outside it (shrinking), and need no disc. At a
            // reversal (a cusp) the turn's sign is noise and the cone is a
            // half-disc reaching both sides.
            let reversal = turn.abs() >= core::f64::consts::PI - SMOOTH;
            if turn.abs() <= SMOOTH || (turn * side <= 0.0 && !reversal) {
                continue;
            }
            let v = e.seg.end();
            let miter = match join {
                Join::Miter { limit } => {
                    let nrm = |t: Vec2| -t.perp() * side;
                    kite(v, nrm(t1), nrm(t2), w, limit, e.source)
                }
                Join::Round => None,
            };
            out.push(miter.unwrap_or_else(|| disc(v, w, e.source)));
        }
    }
    out
}

/// Offsets an area (outer loops CCW, holes CW) by `d`: > 0 grows, < 0 shrinks.
pub fn offset(a: &Area, d: f64, join: Join) -> Area {
    let w = d.abs();
    if w <= locus_region::SNAP {
        return a.clone();
    }
    let t = tool(a, w, d.signum(), join);
    if d > 0.0 {
        let mut all = a.loops.clone();
        all.extend(t);
        boolean(&Area { loops: all }, &Area::default(), BoolOp::Union)
    } else {
        boolean(a, &Area { loops: t }, BoolOp::Subtract)
    }
}

/// One-sided offset of an open chain (each edge ends where the next starts):
/// `d` > 0 to the left of travel, < 0 to the right. The boundary of the
/// chain's tube `chain ⊕ D` is cut into the runs that lie on the wanted side
/// and are not end caps, each returned in the chain's direction.
pub fn offset_open(chain: &[Edge], d: f64) -> Vec<Vec<Edge>> {
    let w = d.abs();
    if chain.is_empty() || w <= locus_region::SNAP {
        return Vec::new();
    }
    let mut t = Vec::new();
    for (i, e) in chain.iter().enumerate() {
        band(&e.seg, w, e.source, &mut t);
        if i == 0 {
            t.push(disc(e.seg.start(), w, e.source));
        }
        match chain.get(i + 1) {
            Some(nx) => {
                let (t1, t2) = corner(&e.seg, &nx.seg);
                if t1.cross(t2).atan2(t1.dot(t2)).abs() > SMOOTH {
                    t.push(disc(e.seg.end(), w, e.source));
                }
            }
            None => t.push(disc(e.seg.end(), w, e.source)),
        }
    }
    let tube = boolean(&Area { loops: t }, &Area::default(), BoolOp::Union);
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
                runs.push(std::mem::take(&mut cur));
            }
        }
    }
    // The tube runs CCW: its left-side boundary goes against the chain.
    if d > 0.0 {
        for r in &mut runs {
            *r = r.iter().rev().map(|e| Edge { seg: e.seg.reversed(), source: e.source }).collect();
        }
    }
    runs
}
