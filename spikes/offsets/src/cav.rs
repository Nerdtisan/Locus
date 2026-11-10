// SPDX-License-Identifier: AGPL-3.0-or-later

//! Option B: `cavalier_contours` 0.9 offsets (polylines with bulges, round
//! joins). Holes and islands go through `Shape::parallel_offset`.

use core::f64::consts::PI;
use cavalier_contours::polyline::{PlineSource, PlineSourceMut, Polyline};
use cavalier_contours::shape_algorithms::{Shape, ShapeOffsetOptions};
use locus_geom::Vec2;
use locus_region::{Area, Edge, Loop, Seg};

fn add(pl: &mut Polyline<f64>, s: &Seg) {
    match *s {
        Seg::Line { a, .. } => pl.add(a.x, a.y, 0.0),
        Seg::Arc { sweep, .. } => {
            // A bulge covers less than a full turn: split into pieces ≤ π.
            let n = if sweep.abs() > PI { 2 } else { 1 };
            for k in 0..n {
                let piece = s.sub(k as f64 / n as f64, (k + 1) as f64 / n as f64);
                let (q, Seg::Arc { sweep: sw, .. }) = (piece.start(), piece) else { continue };
                pl.add(q.x, q.y, (sw / 4.0).tan());
            }
        }
    }
}

fn to_pline(l: &Loop) -> Polyline<f64> {
    let mut pl = Polyline::new_closed();
    for e in &l.edges {
        add(&mut pl, &e.seg);
    }
    pl
}

fn seg(a: Vec2, b: Vec2, bulge: f64) -> Seg {
    if bulge.abs() < 1e-12 {
        return Seg::Line { a, b };
    }
    // Arc through a and b sweeping 4·atan(bulge).
    let sweep = 4.0 * bulge.atan();
    let chord = b - a;
    let half = chord.length() / 2.0;
    let r = half / (sweep / 2.0).sin().abs();
    let mid = a.lerp(b, 0.5);
    let h = (r * r - half * half).max(0.0).sqrt();
    let n = chord.perp().normalized(0.0).unwrap_or(Vec2::ZERO);
    // CCW arcs (bulge > 0) less than a half turn have the centre on the left.
    let small = sweep.abs() <= PI;
    let side = if (sweep > 0.0) == small { 1.0 } else { -1.0 };
    let center = mid + n * (h * side);
    Seg::Arc { center, radius: r, start: (a - center).angle(), sweep }
}

fn from_pline(pl: &Polyline<f64>) -> Vec<Edge> {
    let n = pl.vertex_count();
    let m = if pl.is_closed() { n } else { n.saturating_sub(1) };
    (0..m)
        .map(|i| {
            let (p, q) = (pl.at(i), pl.at((i + 1) % n));
            Edge { seg: seg(Vec2::new(p.x, p.y), Vec2::new(q.x, q.y), p.bulge), source: 0 }
        })
        .collect()
}

pub fn offset(a: &Area, d: f64) -> Area {
    let shape = Shape::from_plines(a.loops.iter().map(to_pline));
    // cavalier: positive offsets a CCW loop inward.
    let r = shape.parallel_offset(-d, &ShapeOffsetOptions::new());
    let loops = r
        .ccw_plines
        .iter()
        .chain(&r.cw_plines)
        .map(|ip| Loop { edges: from_pline(&ip.polyline) })
        .collect();
    Area { loops }
}

pub fn offset_open(chain: &[Edge], d: f64) -> Vec<Vec<Edge>> {
    let mut pl = Polyline::new();
    for e in chain {
        add(&mut pl, &e.seg);
    }
    if let Some(last) = chain.last() {
        let q = last.seg.end();
        pl.add(q.x, q.y, 0.0);
    }
    // Positive offsets to the left; self-intersections removed.
    let opts = cavalier_contours::polyline::PlineOffsetOptions { handle_self_intersects: true, ..Default::default() };
    pl.parallel_offset_opt(d, &opts).iter().map(from_pline).collect()
}
