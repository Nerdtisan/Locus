// SPDX-License-Identifier: AGPL-3.0-or-later

//! Fixture shapes as `locus_region` loops (CCW).

use core::f64::consts::{FRAC_PI_2, PI, TAU};
use locus_geom::Vec2;
use locus_region::{Area, Edge, Loop, Seg};

pub fn v(x: f64, y: f64) -> Vec2 {
    Vec2::new(x, y)
}

pub fn area(loops: Vec<Loop>) -> Area {
    Area { loops }
}

fn lp(segs: Vec<Seg>) -> Loop {
    Loop { edges: segs.into_iter().enumerate().map(|(i, seg)| Edge { seg, source: i as u64 }).collect() }
}

pub fn poly(pts: &[Vec2]) -> Loop {
    let n = pts.len();
    lp((0..n).map(|i| Seg::Line { a: pts[i], b: pts[(i + 1) % n] }).collect())
}

pub fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Loop {
    poly(&[v(x0, y0), v(x1, y0), v(x1, y1), v(x0, y1)])
}

pub fn circle(c: Vec2, r: f64) -> Loop {
    lp(vec![Seg::Arc { center: c, radius: r, start: 0.0, sweep: TAU }])
}

pub fn slot(c1: Vec2, c2: Vec2, r: f64) -> Loop {
    let n = (c2 - c1).normalized(0.0).unwrap_or(v(0.0, 1.0)).perp();
    let na = n.angle();
    lp(vec![
        Seg::Line { a: c1 - n * r, b: c2 - n * r },
        Seg::Arc { center: c2, radius: r, start: na - PI, sweep: PI },
        Seg::Line { a: c2 + n * r, b: c1 + n * r },
        Seg::Arc { center: c1, radius: r, start: na, sweep: PI },
    ])
}

pub fn rounded(x0: f64, y0: f64, x1: f64, y1: f64, r: f64) -> Loop {
    let h = FRAC_PI_2;
    lp(vec![
        Seg::Line { a: v(x0 + r, y0), b: v(x1 - r, y0) },
        Seg::Arc { center: v(x1 - r, y0 + r), radius: r, start: -h, sweep: h },
        Seg::Line { a: v(x1, y0 + r), b: v(x1, y1 - r) },
        Seg::Arc { center: v(x1 - r, y1 - r), radius: r, start: 0.0, sweep: h },
        Seg::Line { a: v(x1 - r, y1), b: v(x0 + r, y1) },
        Seg::Arc { center: v(x0 + r, y1 - r), radius: r, start: h, sweep: h },
        Seg::Line { a: v(x0, y1 - r), b: v(x0, y0 + r) },
        Seg::Arc { center: v(x0 + r, y0 + r), radius: r, start: PI, sweep: h },
    ])
}

/// Star polygon: `n` points, outer radius `ro`, inner `ri` (tight inside corners).
pub fn star(c: Vec2, n: usize, ro: f64, ri: f64) -> Loop {
    let pts: Vec<Vec2> = (0..2 * n)
        .map(|i| {
            let a = i as f64 * PI / n as f64;
            c + Vec2::from_angle(a) * if i % 2 == 0 { ro } else { ri }
        })
        .collect();
    poly(&pts)
}

/// Rectangle with `k` notches of width `nw` and depth `nd` cut into its top.
pub fn comb(w: f64, h: f64, k: usize, nw: f64, nd: f64) -> Loop {
    let pitch = w / (k as f64 + 1.0);
    let mut pts = vec![v(0.0, 0.0), v(w, 0.0), v(w, h)];
    for i in (1..=k).rev() {
        let x = pitch * i as f64;
        pts.extend([v(x + nw / 2.0, h), v(x + nw / 2.0, h - nd), v(x - nw / 2.0, h - nd), v(x - nw / 2.0, h)]);
    }
    pts.push(v(0.0, h));
    poly(&pts)
}

/// Gear-ish outline: a disc with `n` round-ended teeth (arcs only).
pub fn gear(r: f64, n: usize, tooth: f64) -> Area {
    let mut loops = vec![circle(v(0.0, 0.0), r)];
    for i in 0..n {
        let a = i as f64 * TAU / n as f64;
        loops.push(circle(Vec2::from_angle(a) * r, tooth));
    }
    locus_region::union_all(&[area(loops)])
}

pub fn reversed(l: &Loop) -> Loop {
    l.reversed()
}

/// Open chain from segments.
pub fn chain(segs: Vec<Seg>) -> Vec<Edge> {
    segs.into_iter().enumerate().map(|(i, seg)| Edge { seg, source: i as u64 }).collect()
}

pub fn polyline(pts: &[Vec2]) -> Vec<Edge> {
    chain(pts.windows(2).map(|w| Seg::Line { a: w[0], b: w[1] }).collect())
}
