// SPDX-License-Identifier: AGPL-3.0-or-later

//! 2D affine transforms: `p' = [a c; b d] p + (e, f)`.

use crate::vec2::Vec2;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Affine2 {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub e: f64,
    pub f: f64,
}

impl Affine2 {
    pub const IDENTITY: Affine2 = Affine2 {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        e: 0.0,
        f: 0.0,
    };

    #[must_use]
    pub const fn translate(t: Vec2) -> Self {
        Self {
            e: t.x,
            f: t.y,
            ..Self::IDENTITY
        }
    }

    /// Rotation by `angle` radians CCW about the origin.
    #[must_use]
    pub fn rotate(angle: f64) -> Self {
        let (s, c) = angle.sin_cos();
        Self {
            a: c,
            b: s,
            c: -s,
            d: c,
            e: 0.0,
            f: 0.0,
        }
    }

    #[must_use]
    pub const fn scale(sx: f64, sy: f64) -> Self {
        Self {
            a: sx,
            d: sy,
            ..Self::IDENTITY
        }
    }

    /// Reflection across the line through `p` with direction `dir`.
    /// `None` when `dir` is degenerate.
    #[must_use]
    pub fn mirror_about(p: Vec2, dir: Vec2) -> Option<Self> {
        let u = dir.normalized(f64::MIN_POSITIVE)?;
        let (xx, yy, xy) = (u.x * u.x, u.y * u.y, u.x * u.y);
        let lin = Self {
            a: xx - yy,
            b: 2.0 * xy,
            c: 2.0 * xy,
            d: yy - xx,
            e: 0.0,
            f: 0.0,
        };
        // Reflect about the origin line, then translate so `p` stays fixed.
        let q = lin.apply(p);
        Some(Self {
            e: p.x - q.x,
            f: p.y - q.y,
            ..lin
        })
    }

    #[must_use]
    pub fn apply(&self, p: Vec2) -> Vec2 {
        Vec2::new(
            self.a * p.x + self.c * p.y + self.e,
            self.b * p.x + self.d * p.y + self.f,
        )
    }

    /// Apply only the linear part (for direction vectors).
    #[must_use]
    pub fn apply_vector(&self, v: Vec2) -> Vec2 {
        Vec2::new(self.a * v.x + self.c * v.y, self.b * v.x + self.d * v.y)
    }

    /// `self.then(o)` applies `self` first, then `o`.
    #[must_use]
    pub fn then(&self, o: &Affine2) -> Affine2 {
        Affine2 {
            a: o.a * self.a + o.c * self.b,
            b: o.b * self.a + o.d * self.b,
            c: o.a * self.c + o.c * self.d,
            d: o.b * self.c + o.d * self.d,
            e: o.a * self.e + o.c * self.f + o.e,
            f: o.b * self.e + o.d * self.f + o.f,
        }
    }

    #[must_use]
    pub fn determinant(&self) -> f64 {
        self.a * self.d - self.b * self.c
    }

    /// Inverse, or `None` when singular.
    #[must_use]
    pub fn inverse(&self) -> Option<Affine2> {
        let det = self.determinant();
        if !det.is_finite() || det.abs() < f64::MIN_POSITIVE {
            return None;
        }
        let (a, b, c, d) = (self.d / det, -self.b / det, -self.c / det, self.a / det);
        Some(Affine2 {
            a,
            b,
            c,
            d,
            e: -(a * self.e + c * self.f),
            f: -(b * self.e + d * self.f),
        })
    }
}

impl Default for Affine2 {
    fn default() -> Self {
        Self::IDENTITY
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compose_and_invert() {
        let t = Affine2::rotate(0.7).then(&Affine2::translate(Vec2::new(3.0, -2.0)));
        let p = Vec2::new(1.5, 2.5);
        let inv = t.inverse().expect("invertible");
        assert!(inv.apply(t.apply(p)).approx_eq(p, 1e-12));
        assert!(Affine2::scale(0.0, 1.0).inverse().is_none());
    }

    #[test]
    fn mirror() {
        let m = Affine2::mirror_about(Vec2::new(0.0, 1.0), Vec2::new(1.0, 0.0)).expect("dir");
        assert!(
            m.apply(Vec2::new(2.0, 3.0))
                .approx_eq(Vec2::new(2.0, -1.0), 1e-12)
        );
        assert!(Affine2::mirror_about(Vec2::ZERO, Vec2::ZERO).is_none());
    }
}
