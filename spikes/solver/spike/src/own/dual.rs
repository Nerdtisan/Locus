// SPDX-License-Identifier: AGPL-3.0-or-later

//! Forward-mode dual numbers with a fixed-size gradient. Every residual is
//! written once over [`D`] and yields its exact gradient, so the solver and the
//! rank analysis share one definition (ADR 0002 had two: ezpz's and ours).

use core::ops::{Add, Div, Mul, Neg, Sub};

/// Most unknowns one constraint touches (two lines: 8 coordinates).
pub const N: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct D {
    pub v: f64,
    pub g: [f64; N],
}

impl D {
    pub const fn c(v: f64) -> Self {
        D { v, g: [0.0; N] }
    }
    /// The unknown in gradient slot `slot` (slots past `N` act as constants).
    pub fn var(v: f64, slot: usize) -> Self {
        let mut d = D::c(v);
        if let Some(g) = d.g.get_mut(slot) {
            *g = 1.0;
        }
        d
    }
    fn map(self, v: f64, k: f64) -> Self {
        D {
            v,
            g: self.g.map(|x| x * k),
        }
    }
    pub fn sqrt(self) -> Self {
        let s = self.v.sqrt();
        // The gradient of sqrt at 0 is unbounded; a zero-length vector has no
        // direction, so contribute nothing rather than inf/NaN.
        self.map(s, if s > 0.0 { 0.5 / s } else { 0.0 })
    }
    pub fn abs(self) -> Self {
        if self.v < 0.0 {
            -self
        } else {
            self
        }
    }
    pub fn max_c(self, m: f64) -> Self {
        if self.v < m {
            D::c(m)
        } else {
            self
        }
    }
}

impl Add for D {
    type Output = D;
    fn add(mut self, o: D) -> D {
        self.v += o.v;
        for (a, b) in self.g.iter_mut().zip(o.g) {
            *a += b;
        }
        self
    }
}

impl Sub for D {
    type Output = D;
    fn sub(self, o: D) -> D {
        self + -o
    }
}

impl Neg for D {
    type Output = D;
    fn neg(self) -> D {
        self.map(-self.v, -1.0)
    }
}

impl Mul for D {
    type Output = D;
    fn mul(self, o: D) -> D {
        let mut g = [0.0; N];
        for (i, gi) in g.iter_mut().enumerate() {
            *gi = self.g[i] * o.v + self.v * o.g[i];
        }
        D { v: self.v * o.v, g }
    }
}

impl Div for D {
    type Output = D;
    fn div(self, o: D) -> D {
        let q = self.v / o.v;
        let mut g = [0.0; N];
        for (i, gi) in g.iter_mut().enumerate() {
            *gi = (self.g[i] - q * o.g[i]) / o.v;
        }
        D { v: q, g }
    }
}

impl Mul<f64> for D {
    type Output = D;
    fn mul(self, k: f64) -> D {
        self.map(self.v * k, k)
    }
}

impl Sub<f64> for D {
    type Output = D;
    fn sub(mut self, k: f64) -> D {
        self.v -= k;
        self
    }
}

/// A 2D point of duals.
#[derive(Clone, Copy, Debug)]
pub struct V2 {
    pub x: D,
    pub y: D,
}

impl V2 {
    pub fn minus(self, o: V2) -> V2 {
        V2 {
            x: self.x - o.x,
            y: self.y - o.y,
        }
    }
    pub fn mid(self, o: V2) -> V2 {
        V2 {
            x: (self.x + o.x) * 0.5,
            y: (self.y + o.y) * 0.5,
        }
    }
    pub fn dot(self, o: V2) -> D {
        self.x * o.x + self.y * o.y
    }
    pub fn cross(self, o: V2) -> D {
        self.x * o.y - self.y * o.x
    }
    pub fn len(self) -> D {
        self.dot(self).sqrt()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gradients_match_finite_differences() {
        let f = |a: D, b: D| ((a * a + b * b).sqrt() / (a - b * 3.0)).abs() * 2.0 - 1.0;
        let (a0, b0) = (1.3, -0.7);
        let d = f(D::var(a0, 0), D::var(b0, 1));
        let h = 1e-6;
        let fa = (f(D::c(a0 + h), D::c(b0)).v - f(D::c(a0 - h), D::c(b0)).v) / (2.0 * h);
        let fb = (f(D::c(a0), D::c(b0 + h)).v - f(D::c(a0), D::c(b0 - h)).v) / (2.0 * h);
        assert!((d.g[0] - fa).abs() < 1e-8 && (d.g[1] - fb).abs() < 1e-8);
    }
}
