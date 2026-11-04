// SPDX-License-Identifier: AGPL-3.0-or-later

//! Levenberg–Marquardt on the system's residuals (Nielsen's damping update,
//! ADR 0007).
//!
//! Damping is `λI`, not Marquardt's `λ·diag(JᵀJ)`: `(JᵀJ + λI)⁻¹Jᵀ` tends to the
//! minimum-norm step as λ shrinks, so under-constrained geometry moves as little
//! as possible (what a drag should feel like) and the system stays positive
//! definite whatever the DOF.

use crate::ldl::LinSolve;
use crate::sketch::Sketch;
use crate::system::System;

/// Converged when every residual is below this (all rows are in mm): four
/// orders tighter than `SOLVER_TOL` (SPEC §4), which LM's quadratic
/// convergence makes nearly free.
pub(crate) const TOL: f64 = 1e-10;
/// Iteration cap for a full solve.
pub(crate) const MAX_ITER: usize = 100;
/// Iteration cap for one drag frame (#63). A reachable target converges in
/// a handful of iterations from the previous frame; an unreachable one would
/// otherwise creep towards its least-squares point for the full cap.
pub(crate) const DRAG_ITER: usize = 30;
/// λ starts tiny: long chains have soft modes with σ² ≈ 1e-7, and a start of
/// 1e-6 over-damped them (measured: 29 iterations per 1,000-curve drag frame,
/// against 4 at 1e-9). λ rises by itself when a step fails or a pivot does.
const LAMBDA_START: f64 = 1e-9;
const LAMBDA_MIN: f64 = 1e-15;
const LAMBDA_MAX: f64 = 1e16;

#[derive(Clone, Debug, Default)]
pub(crate) struct Outcome {
    pub converged: bool,
    pub iterations: usize,
    /// Residuals at the end (the least-squares point when not converged).
    pub r: Vec<f64>,
}

fn max_abs(v: &[f64]) -> f64 {
    v.iter().fold(0.0f64, |m, x| {
        if x.is_nan() {
            f64::INFINITY
        } else {
            m.max(x.abs())
        }
    })
}

fn half_sq(v: &[f64]) -> f64 {
    0.5 * v.iter().map(|x| x * x).sum::<f64>()
}

/// `g = Jᵀr`.
fn jt_r(m: &System, j: &[f64], r: &[f64], g: &mut [f64]) {
    g.iter_mut().for_each(|v| *v = 0.0);
    for b in &m.blocks {
        let nv = b.vars.len();
        for k in 0..b.rows {
            let rk = r[b.r0 + k];
            for (a, &u) in b.vars.iter().enumerate() {
                g[u] += j[b.j0 + k * nv + a] * rk;
            }
        }
    }
}

/// Solve in place from `x`.
pub(crate) fn solve<L: LinSolve>(
    m: &System,
    sk: &Sketch,
    lin: &mut L,
    x: &mut [f64],
    max_iter: usize,
) -> Outcome {
    let (mut r, mut j) = (vec![0.0; m.m], vec![0.0; m.nj]);
    let (mut rn, mut jn) = (vec![0.0; m.m], vec![0.0; m.nj]);
    let mut g = vec![0.0; m.n];
    let mut delta = vec![0.0; m.n];
    let mut xn = x.to_vec();
    m.eval(sk, x, &mut r, Some(&mut j));
    let mut f = half_sq(&r);
    let mut lambda = LAMBDA_START;
    let mut nu = 2.0;
    let mut it = 0;
    while it < max_iter {
        if max_abs(&r) < TOL {
            return Outcome {
                converged: true,
                iterations: it,
                r,
            };
        }
        it += 1;
        jt_r(m, &j, &r, &mut g);
        // A stationary point with residual left: least-squares minimum of an
        // inconsistent system.
        if max_abs(&g) < 1e-14 {
            break;
        }
        let accepted = loop {
            if lambda > LAMBDA_MAX {
                break false;
            }
            if !lin.factor(m, &j, lambda) {
                lambda *= nu;
                nu *= 2.0;
                continue;
            }
            for (d, gi) in delta.iter_mut().zip(&g) {
                *d = -gi;
            }
            lin.solve(&mut delta);
            for ((a, b), d) in xn.iter_mut().zip(x.iter()).zip(&delta) {
                *a = b + d;
            }
            m.eval(sk, &xn, &mut rn, Some(&mut jn));
            let fnew = half_sq(&rn);
            // Predicted decrease of the quadratic model.
            let pred: f64 = 0.5
                * delta
                    .iter()
                    .zip(&g)
                    .map(|(d, gi)| d * (lambda * d - gi))
                    .sum::<f64>();
            let rho = (f - fnew) / pred;
            if fnew.is_finite() && rho > 0.0 {
                x.copy_from_slice(&xn);
                std::mem::swap(&mut r, &mut rn);
                std::mem::swap(&mut j, &mut jn);
                let shrink = (1.0 - (2.0 * rho - 1.0).powi(3)).max(1.0 / 3.0);
                lambda = (lambda * shrink).max(LAMBDA_MIN);
                nu = 2.0;
                let stalled = f - fnew <= 1e-13 * f;
                f = fnew;
                break !stalled;
            }
            lambda *= nu;
            nu *= 2.0;
        };
        if !accepted {
            break;
        }
    }
    Outcome {
        converged: max_abs(&r) < TOL,
        iterations: it,
        r,
    }
}
