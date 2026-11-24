// SPDX-License-Identifier: AGPL-3.0-or-later

//! Adaptive cubic fit of a side function, and direct flattening (canvas).

use locus_geom::{Cubic, Line, Vec2};

use std::sync::atomic::{AtomicBool, Ordering};

const MAX_DEPTH: u32 = 22;

/// Least-squares tangent lengths (Schneider) instead of plain Hermite.
pub static LEAST_SQUARES: AtomicBool = AtomicBool::new(false);

/// Cubic with ends p0, p3 and unit tangents u0, u1 whose tangent lengths
/// best fit the samples `q` (by chord-length parameters, refined by two
/// Newton reparameterisations). `None` when the system degenerates.
fn least_squares(p0: Vec2, p3: Vec2, u0: Vec2, u1: Vec2, q: &[Vec2]) -> Option<Cubic> {
    let n = q.len();
    let mut u: Vec<f64> = Vec::with_capacity(n);
    let mut acc = 0.0;
    let mut prev = p0;
    for &x in q {
        acc += prev.distance(x);
        u.push(acc);
        prev = x;
    }
    let total = acc + prev.distance(p3);
    if total <= 0.0 {
        return None;
    }
    for v in &mut u {
        *v /= total;
    }
    let mut best: Option<Cubic> = None;
    for _ in 0..3 {
        let (mut c00, mut c01, mut c11, mut x0, mut x1) = (0.0, 0.0, 0.0, 0.0, 0.0);
        for (i, &t) in u.iter().enumerate() {
            let s = 1.0 - t;
            let (b0, b1, b2, b3) = (s * s * s, 3.0 * s * s * t, 3.0 * s * t * t, t * t * t);
            let (a0, a1) = (u0 * b1, -u1 * b2);
            c00 += a0.dot(a0);
            c01 += a0.dot(a1);
            c11 += a1.dot(a1);
            let tmp = q[i] - (p0 * (b0 + b1) + p3 * (b2 + b3));
            x0 += a0.dot(tmp);
            x1 += a1.dot(tmp);
        }
        let det = c00 * c11 - c01 * c01;
        if det.abs() < 1e-18 {
            return best;
        }
        let al = (x0 * c11 - x1 * c01) / det;
        let be = (c00 * x1 - c01 * x0) / det;
        let seg = p0.distance(p3);
        if !(al > 1e-6 * seg && be > 1e-6 * seg) || !al.is_finite() || !be.is_finite() {
            return best;
        }
        let c = Cubic::new(p0, p0 + u0 * al, p3 - u1 * be, p3);
        best = Some(c);
        // Newton reparameterisation.
        for (i, t) in u.iter_mut().enumerate() {
            let d = c.eval(*t) - q[i];
            let d1 = c.derivative(*t);
            let h = 1e-6;
            let d2 =
                (c.derivative((*t + h).min(1.0)) - c.derivative((*t - h).max(0.0))) * (0.5 / h);
            let den = d1.dot(d1) + d.dot(d2);
            if den.abs() > 1e-18 {
                *t = (*t - d.dot(d1) / den).clamp(0.0, 1.0);
            }
        }
    }
    best
}
const CHECK: usize = 10;
const POLY: usize = 32;

/// Derivative by central differences, one-sided at the domain ends [0, 1].
fn deriv(f: &dyn Fn(f64) -> Vec2, t: f64, lo: f64, hi: f64) -> Vec2 {
    let h = 1e-7_f64.min(0.25 * (hi - lo));
    let (a, b) = ((t - h).max(lo), (t + h).min(hi));
    (f(b) - f(a)) * (1.0 / (b - a))
}

fn poly_dist(pl: &[Vec2], p: Vec2) -> f64 {
    pl.windows(2)
        .map(|w| Line::new(w[0], w[1]).distance_to(p))
        .fold(f64::INFINITY, f64::min)
}

/// Fits `f` on [t0, t1] with Hermite cubics, each within `tol` of `f` both
/// ways (sampled). Spans that still fail at the depth limit become chords
/// (a jump of the side function, e.g. a flat nib flipping).
pub fn fit(f: &dyn Fn(f64) -> Vec2, t0: f64, t1: f64, tol: f64, out: &mut Vec<Cubic>) {
    fit_rec(f, (t0, t1), t0, t1, tol, 0, out);
}

fn fit_rec(
    f: &dyn Fn(f64) -> Vec2,
    dom: (f64, f64),
    t0: f64,
    t1: f64,
    tol: f64,
    depth: u32,
    out: &mut Vec<Cubic>,
) {
    let dt = t1 - t0;
    let (p0, p1) = (f(t0), f(t1));
    let (d0, d1) = (deriv(f, t0, dom.0, dom.1), deriv(f, t1, dom.0, dom.1));
    let mut c = Cubic::new(p0, p0 + d0 * (dt / 3.0), p1 - d1 * (dt / 3.0), p1);
    if LEAST_SQUARES.load(Ordering::Relaxed)
        && let (Some(u0), Some(u1)) = (d0.normalized(1e-12), d1.normalized(1e-12))
    {
        let q: Vec<Vec2> = (1..16).map(|i| f(t0 + dt * i as f64 / 16.0)).collect();
        if let Some(ls) = least_squares(p0, p1, u0, u1, &q) {
            c = ls;
        }
    }
    if depth >= MAX_DEPTH {
        out.push(Cubic::new(
            p0,
            p0.lerp(p1, 1.0 / 3.0),
            p0.lerp(p1, 2.0 / 3.0),
            p1,
        ));
        return;
    }
    if c.p0.is_finite() && c.c0.is_finite() && c.c1.is_finite() && c.p1.is_finite() {
        let cf = c.flatten(tol * 0.05);
        let mut ok = (1..CHECK).all(|i| {
            let q = f(t0 + dt * i as f64 / CHECK as f64);
            poly_dist(&cf, q) <= tol
        });
        if ok {
            let fp: Vec<Vec2> = (0..=POLY)
                .map(|i| f(t0 + dt * i as f64 / POLY as f64))
                .collect();
            ok = (1..CHECK).all(|i| poly_dist(&fp, c.eval(i as f64 / CHECK as f64)) <= tol);
        }
        if ok {
            out.push(c);
            return;
        }
    }
    let m = t0 + dt * 0.5;
    fit_rec(f, dom, t0, m, tol, depth + 1, out);
    fit_rec(f, dom, m, t1, tol, depth + 1, out);
}

/// Polyline of `f` on [t0, t1] within `tol` (midpoint and quarter-point sag),
/// for the canvas: no fit, no cleanup. Pushes t values (start excluded).
pub fn flatten(f: &dyn Fn(f64) -> Vec2, t0: f64, t1: f64, tol: f64, out: &mut Vec<f64>) {
    flat_rec(f, t0, f(t0), t1, f(t1), tol, 0, out);
}

#[allow(clippy::too_many_arguments)]
fn flat_rec(
    f: &dyn Fn(f64) -> Vec2,
    t0: f64,
    p0: Vec2,
    t1: f64,
    p1: Vec2,
    tol: f64,
    depth: u32,
    out: &mut Vec<f64>,
) {
    let m = 0.5 * (t0 + t1);
    let pm = f(m);
    let chord = Line::new(p0, p1);
    let ok = depth >= 2
        && chord.distance_to(pm) <= tol
        && chord.distance_to(f(0.5 * (t0 + m))) <= tol
        && chord.distance_to(f(0.5 * (m + t1))) <= tol;
    if ok || depth >= MAX_DEPTH {
        out.push(t1);
        return;
    }
    flat_rec(f, t0, p0, m, pm, tol, depth + 1, out);
    flat_rec(f, m, pm, t1, p1, tol, depth + 1, out);
}
