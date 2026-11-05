// SPDX-License-Identifier: AGPL-3.0-or-later

//! Option A: flatten → `i_overlay` boolean → arc refit. Also the fine
//! reference used to score every option.

use crate::geom::{P, Seg, Shape, arc_from_chord, p};
use crate::own::Op;
use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::float::overlay::FloatOverlay;
use std::f64::consts::PI;

type Contours = Vec<Vec<[f64; 2]>>;

pub fn flatten(s: &Shape, sag: f64) -> Contours {
    s.loops
        .iter()
        .map(|l| {
            let mut pts = Vec::new();
            for seg in l {
                seg.flatten(sag, &mut pts);
            }
            pts.iter().map(|q| [q.x, q.y]).collect()
        })
        .collect()
}

fn rule(op: Op) -> OverlayRule {
    match op {
        Op::Union => OverlayRule::Union,
        Op::Intersect => OverlayRule::Intersect,
        Op::Subtract => OverlayRule::Difference,
    }
}

/// Polygon boolean of flattened shapes; contours as point lists.
pub fn polygons(a: &Shape, b: &Shape, op: Op, sag: f64) -> Vec<Vec<P>> {
    let (sa, sb) = (flatten(a, sag), flatten(b, sag));
    let shapes = FloatOverlay::<[f64; 2], i64>::from_subj_and_clip(&sa, &sb).overlay(rule(op), FillRule::NonZero);
    shapes
        .into_iter()
        .flatten()
        .map(|c| c.into_iter().map(|q| p(q[0], q[1])).collect())
        .collect()
}

/// Polygons as a line-only shape (no refit).
pub fn as_lines(polys: &[Vec<P>]) -> Shape {
    Shape::new(
        polys
            .iter()
            .map(|c| (0..c.len()).map(|i| Seg::Line { a: c[i], b: c[(i + 1) % c.len()] }).collect())
            .collect(),
    )
}

pub fn boolean_refit(a: &Shape, b: &Shape, op: Op, sag: f64, fit_tol: f64) -> Shape {
    let polys = polygons(a, b, op, sag);
    Shape::new(polys.iter().map(|c| refit(c, fit_tol)).collect())
}

fn turn_at(c: &[P], i: usize) -> f64 {
    let n = c.len();
    let (a, b, d) = (c[(i + n - 1) % n], c[i], c[(i + 1) % n]);
    let (u, v) = (b - a, d - b);
    u.cross(v).atan2(u.dot(v)).abs()
}

/// Greedy line/arc fit of a closed polygon within `tol`, starting at its
/// sharpest corner. Arc endpoints are exact polygon vertices, so loops stay
/// closed.
pub fn refit(c: &[P], tol: f64) -> Vec<Seg> {
    let n = c.len();
    if n < 3 {
        return as_lines(&[c.to_vec()]).loops.remove(0);
    }
    let start = (0..n).max_by(|&x, &y| turn_at(c, x).total_cmp(&turn_at(c, y))).unwrap_or(0);
    let pts: Vec<P> = (0..=n).map(|k| c[(start + k) % n]).collect();
    let line_fits = |i: usize, j: usize| {
        let s = Seg::Line { a: pts[i], b: pts[j] };
        (i..=j).all(|k| s.dist(pts[k]) <= tol)
    };
    let arc_fit = |i: usize, j: usize| -> Option<Seg> {
        let m = (i + j) / 2;
        let (a, b, d) = (pts[i], pts[m], pts[j]);
        let den = 2.0 * (b - a).cross(d - a);
        if den.abs() < 1e-12 {
            return None;
        }
        let (ab, ad) = (b - a, d - a);
        let cen = a + p(ad.y * ab.dot(ab) - ab.y * ad.dot(ad), ab.x * ad.dot(ad) - ad.x * ab.dot(ab)) * (1.0 / den);
        let r = cen.dist(a);
        if r > 1e4 {
            return None;
        }
        let mut sweep = 0.0;
        let sign = den.signum();
        for k in i..j {
            let st = (pts[k] - cen).cross(pts[k + 1] - cen).atan2((pts[k] - cen).dot(pts[k + 1] - cen));
            if st.signum() != sign {
                return None;
            }
            sweep += st;
        }
        if sweep.abs() > 1.9 * PI || (i..=j).any(|k| (pts[k].dist(cen) - r).abs() > tol) {
            return None;
        }
        let arc = arc_from_chord(a, d, sweep);
        // Vertices and chord midpoints must both hug the arc, or a circle
        // through a rectangle's corners would pass.
        let hugs = (i..=j).all(|k| arc.dist(pts[k]) <= tol) && (i..j).all(|k| arc.dist((pts[k] + pts[k + 1]) * 0.5) <= tol);
        hugs.then_some(arc)
    };
    let mut out = Vec::new();
    let mut i = 0;
    while i < n {
        let mut jl = i + 1;
        while jl < n && line_fits(i, jl + 1) {
            jl += 1;
        }
        let mut best: Option<(usize, Seg)> = None;
        let mut j = i + 3;
        while j <= n {
            match arc_fit(i, j) {
                Some(s) => best = Some((j, s)),
                None => break,
            }
            j += 1;
        }
        match best {
            Some((ja, s)) if ja > jl => {
                out.push(s);
                i = ja;
            }
            _ => {
                out.push(Seg::Line { a: pts[i], b: pts[jl] });
                i = jl;
            }
        }
    }
    out
}
