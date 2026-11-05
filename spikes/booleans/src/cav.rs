// SPDX-License-Identifier: AGPL-3.0-or-later

//! Option B: `cavalier_contours` polyline-with-bulge booleans (arc-preserving).
//! It combines two single closed polylines; operands with holes or several
//! loops are composed from repeated pline booleans by the caller.

use crate::geom::{Seg, Shape, arc_from_chord, loop_area, p, reversed};
use crate::own::Op;
use cavalier_contours::polyline::{BooleanOp, PlineSource, PlineSourceMut, Polyline};

fn to_pline(l: &[Seg]) -> Polyline<f64> {
    let mut pl = Polyline::new_closed();
    for s in l {
        match *s {
            Seg::Line { a, .. } => pl.add(a.x, a.y, 0.0),
            Seg::Arc { sweep, .. } => {
                // A bulge covers less than a full turn: split into halves.
                let n = if sweep.abs() > std::f64::consts::PI { 2 } else { 1 };
                for k in 0..n {
                    let piece = s.sub(k as f64 / n as f64, (k + 1) as f64 / n as f64);
                    let (q, Seg::Arc { sweep: sw, .. }) = (piece.start(), piece) else { unreachable!() };
                    pl.add(q.x, q.y, (sw / 4.0).tan());
                }
            }
        }
    }
    pl
}

fn from_pline(pl: &Polyline<f64>) -> Vec<Seg> {
    let n = pl.vertex_count();
    (0..n)
        .map(|i| {
            let (v, w) = (pl.at(i), pl.at((i + 1) % n));
            let (a, b) = (p(v.x, v.y), p(w.x, w.y));
            if v.bulge.abs() < 1e-12 { Seg::Line { a, b } } else { arc_from_chord(a, b, 4.0 * v.bulge.atan()) }
        })
        .collect()
}

/// `None` when an operand is not a single loop (cavalier works pline vs pline).
pub fn boolean(a: &Shape, b: &Shape, op: Op) -> Option<Shape> {
    if a.loops.len() != 1 || b.loops.len() != 1 {
        return None;
    }
    let (pa, pb) = (to_pline(&a.loops[0]), to_pline(&b.loops[0]));
    let bop = match op {
        Op::Union => BooleanOp::Or,
        Op::Intersect => BooleanOp::And,
        Op::Subtract => BooleanOp::Not,
    };
    let res = pa.boolean(&pb, bop);
    let mut loops = Vec::new();
    for r in &res.pos_plines {
        let l = from_pline(&r.pline);
        loops.push(if loop_area(&l) < 0.0 { reversed(&l) } else { l });
    }
    for r in &res.neg_plines {
        let l = from_pline(&r.pline);
        loops.push(if loop_area(&l) > 0.0 { reversed(&l) } else { l });
    }
    Some(Shape::new(loops))
}
