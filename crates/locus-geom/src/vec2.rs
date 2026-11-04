// SPDX-License-Identifier: AGPL-3.0-or-later

//! 2D vector / point in millimetres.

use core::ops::{Add, AddAssign, Div, Mul, Neg, Sub, SubAssign};
use serde::{Deserialize, Serialize};

/// A 2D vector. Also used as a point; see [`Point2`].
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Vec2 {
    pub x: f64,
    pub y: f64,
}

/// A position in the sketch plane (mm). Same representation as [`Vec2`].
pub type Point2 = Vec2;

impl Vec2 {
    pub const ZERO: Vec2 = Vec2 { x: 0.0, y: 0.0 };

    #[must_use]
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    /// Unit vector at `angle` radians (CCW from +x).
    #[must_use]
    pub fn from_angle(angle: f64) -> Self {
        let (s, c) = angle.sin_cos();
        Self::new(c, s)
    }

    #[must_use]
    pub fn dot(self, o: Self) -> f64 {
        self.x * o.x + self.y * o.y
    }

    /// 2D cross product (z component of the 3D cross product).
    #[must_use]
    pub fn cross(self, o: Self) -> f64 {
        self.x * o.y - self.y * o.x
    }

    #[must_use]
    pub fn length_sq(self) -> f64 {
        self.dot(self)
    }

    #[must_use]
    pub fn length(self) -> f64 {
        self.x.hypot(self.y)
    }

    #[must_use]
    pub fn distance(self, o: Self) -> f64 {
        (self - o).length()
    }

    /// Unit vector, or `None` when the length is not above `eps` (degenerate).
    #[must_use]
    pub fn normalized(self, eps: f64) -> Option<Self> {
        let l = self.length();
        (l.is_finite() && l > eps).then(|| self / l)
    }

    /// Rotated 90° counter-clockwise.
    #[must_use]
    pub fn perp(self) -> Self {
        Self::new(-self.y, self.x)
    }

    /// Angle in radians, CCW from +x, in (-π, π].
    #[must_use]
    pub fn angle(self) -> f64 {
        self.y.atan2(self.x)
    }

    #[must_use]
    pub fn rotated(self, angle: f64) -> Self {
        let (s, c) = angle.sin_cos();
        Self::new(self.x * c - self.y * s, self.x * s + self.y * c)
    }

    #[must_use]
    pub fn lerp(self, o: Self, t: f64) -> Self {
        self + (o - self) * t
    }

    /// True when both coordinates are finite (no NaN / inf).
    #[must_use]
    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }

    /// True when within `tol` of `o`.
    #[must_use]
    pub fn approx_eq(self, o: Self, tol: f64) -> bool {
        self.distance(o) <= tol
    }
}

impl Add for Vec2 {
    type Output = Vec2;
    fn add(self, o: Vec2) -> Vec2 {
        Vec2::new(self.x + o.x, self.y + o.y)
    }
}
impl Sub for Vec2 {
    type Output = Vec2;
    fn sub(self, o: Vec2) -> Vec2 {
        Vec2::new(self.x - o.x, self.y - o.y)
    }
}
impl Mul<f64> for Vec2 {
    type Output = Vec2;
    fn mul(self, k: f64) -> Vec2 {
        Vec2::new(self.x * k, self.y * k)
    }
}
impl Div<f64> for Vec2 {
    type Output = Vec2;
    fn div(self, k: f64) -> Vec2 {
        Vec2::new(self.x / k, self.y / k)
    }
}
impl Neg for Vec2 {
    type Output = Vec2;
    fn neg(self) -> Vec2 {
        Vec2::new(-self.x, -self.y)
    }
}
impl AddAssign for Vec2 {
    fn add_assign(&mut self, o: Vec2) {
        *self = *self + o;
    }
}
impl SubAssign for Vec2 {
    fn sub_assign(&mut self, o: Vec2) {
        *self = *self - o;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basics() {
        let a = Vec2::new(3.0, 4.0);
        assert_eq!(a.length(), 5.0);
        assert_eq!(a.dot(Vec2::new(1.0, 0.0)), 3.0);
        assert_eq!(Vec2::new(1.0, 0.0).cross(Vec2::new(0.0, 1.0)), 1.0);
        assert_eq!(a.perp(), Vec2::new(-4.0, 3.0));
        assert!(Vec2::ZERO.normalized(1e-12).is_none());
        assert!(Vec2::new(f64::NAN, 0.0).normalized(1e-12).is_none());
        let r = Vec2::new(1.0, 0.0).rotated(core::f64::consts::FRAC_PI_2);
        assert!(r.approx_eq(Vec2::new(0.0, 1.0), 1e-12));
    }
}
