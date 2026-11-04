// SPDX-License-Identifier: AGPL-3.0-or-later

//! Axis-aligned bounding box.

use crate::vec2::Vec2;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Aabb {
    pub min: Vec2,
    pub max: Vec2,
}

impl Aabb {
    #[must_use]
    pub fn from_point(p: Vec2) -> Self {
        Self { min: p, max: p }
    }

    #[must_use]
    pub fn from_points(a: Vec2, b: Vec2) -> Self {
        Self {
            min: Vec2::new(a.x.min(b.x), a.y.min(b.y)),
            max: Vec2::new(a.x.max(b.x), a.y.max(b.y)),
        }
    }

    #[must_use]
    pub fn include(self, p: Vec2) -> Self {
        Self {
            min: Vec2::new(self.min.x.min(p.x), self.min.y.min(p.y)),
            max: Vec2::new(self.max.x.max(p.x), self.max.y.max(p.y)),
        }
    }

    #[must_use]
    pub fn union(self, o: Self) -> Self {
        self.include(o.min).include(o.max)
    }

    /// Inclusive containment with slack `tol`.
    #[must_use]
    pub fn contains(self, p: Vec2, tol: f64) -> bool {
        p.x >= self.min.x - tol
            && p.x <= self.max.x + tol
            && p.y >= self.min.y - tol
            && p.y <= self.max.y + tol
    }

    #[must_use]
    pub fn width(self) -> f64 {
        self.max.x - self.min.x
    }

    #[must_use]
    pub fn height(self) -> f64 {
        self.max.y - self.min.y
    }
}
