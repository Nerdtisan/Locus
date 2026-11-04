// SPDX-License-Identifier: AGPL-3.0-or-later
//! Plain 2D maths in world millimetres (`f64`, y up; SPEC §4). No GUI types,
//! so the hit-testing built on it stays UI-agnostic (#67).

/// A 2D vector in world millimetres.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct V2 {
    pub x: f64,
    pub y: f64,
}

// `dot`/`len`/`dist` are used by hit-testing (#67); tests use them now.
#[allow(dead_code)]
impl V2 {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
    pub fn sub(self, o: Self) -> Self {
        Self::new(self.x - o.x, self.y - o.y)
    }
    pub fn add(self, o: Self) -> Self {
        Self::new(self.x + o.x, self.y + o.y)
    }
    pub fn scale(self, k: f64) -> Self {
        Self::new(self.x * k, self.y * k)
    }
    pub fn dot(self, o: Self) -> f64 {
        self.x * o.x + self.y * o.y
    }
    pub fn len(self) -> f64 {
        self.dot(self).sqrt()
    }
    pub fn dist(self, o: Self) -> f64 {
        self.sub(o).len()
    }
}
