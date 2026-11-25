// SPDX-License-Identifier: AGPL-3.0-or-later

//! Width profiles (#426, ADR 0019 decision 1): the stroke's width along the
//! path, as width points at fractions of its length. Between points the
//! width eases with a smoothstep, so the outline is smooth (C1) through each
//! point, the look Illustrator's width tool gives.

use crate::StrokeError;
use serde::{Deserialize, Serialize};

/// Fewest and most width points in a profile.
pub const MIN_WIDTH_POINTS: usize = 2;
pub const MAX_WIDTH_POINTS: usize = 64;
/// Largest side factor: a side reaches at most this × half the width.
pub const MAX_SIDE_FACTOR: f64 = 20.0;

/// One width point: at `t` (0..=1 of the path's length), the left and right
/// sides reach `left` and `right` × half the stroke width from the path
/// (left of travel is +). 1 / 1 is the plain width.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WidthPoint {
    pub t: f64,
    pub left: f64,
    pub right: f64,
}

impl WidthPoint {
    #[must_use]
    pub const fn new(t: f64, left: f64, right: f64) -> Self {
        Self { t, left, right }
    }

    /// A symmetric point: both sides at `f`.
    #[must_use]
    pub const fn even(t: f64, f: f64) -> Self {
        Self::new(t, f, f)
    }
}

fn is_none_u64(v: &Option<u64>) -> bool {
    v.is_none()
}

/// A stroke's width along its path (#426). Points are sorted by `t`, the
/// first at 0 and the last at 1.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WidthProfile {
    pub points: Vec<WidthPoint>,
    /// The seed of a **Pressure** preset, so the Inspector can name it and
    /// offer Reseed; cleared when the points are edited.
    #[serde(default, skip_serializing_if = "is_none_u64")]
    pub seed: Option<u64>,
}

/// The built-in profiles (design §3). Each is a set of width points, so
/// editing one makes it a custom profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfilePreset {
    Uniform,
    TaperBoth,
    TaperStart,
    TaperEnd,
    /// Thick middle.
    Bulge,
    /// Thin middle.
    Pinch,
    /// A smooth random profile from a seed, like a pen pressed by hand.
    Pressure,
}

impl ProfilePreset {
    pub const ALL: [Self; 7] = [
        Self::Uniform,
        Self::TaperBoth,
        Self::TaperStart,
        Self::TaperEnd,
        Self::Bulge,
        Self::Pinch,
        Self::Pressure,
    ];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Uniform => "Uniform",
            Self::TaperBoth => "Taper both ends",
            Self::TaperStart => "Taper start",
            Self::TaperEnd => "Taper end",
            Self::Bulge => "Bulge (thick middle)",
            Self::Pinch => "Pinch (thin middle)",
            Self::Pressure => "Pressure (simulated)",
        }
    }

    /// The preset's profile; `seed` is used by [`Self::Pressure`] only.
    #[must_use]
    pub fn profile(self, seed: u64) -> WidthProfile {
        let e = WidthPoint::even;
        let points = match self {
            Self::Uniform => vec![e(0.0, 1.0), e(1.0, 1.0)],
            Self::TaperBoth => vec![e(0.0, 0.0), e(0.2, 1.0), e(0.8, 1.0), e(1.0, 0.0)],
            Self::TaperStart => vec![e(0.0, 0.0), e(0.3, 1.0), e(1.0, 1.0)],
            Self::TaperEnd => vec![e(0.0, 1.0), e(0.7, 1.0), e(1.0, 0.0)],
            Self::Bulge => vec![e(0.0, 0.4), e(0.5, 1.6), e(1.0, 0.4)],
            Self::Pinch => vec![e(0.0, 1.4), e(0.5, 0.3), e(1.0, 1.4)],
            Self::Pressure => {
                let mut rng = SplitMix64(seed);
                let n = 8;
                (0..=n)
                    .map(|i| {
                        let t = f64::from(i) / f64::from(n);
                        // Light at the ends, as a pen lands and lifts.
                        let f = if i == 0 || i == n {
                            0.15
                        } else {
                            0.55 + 0.9 * rng.unit()
                        };
                        e(t, f)
                    })
                    .collect()
            }
        };
        WidthProfile {
            points,
            seed: (self == Self::Pressure).then_some(seed),
        }
    }
}

/// A small deterministic generator (ADR 0019: Locus-owned SplitMix64, no
/// `rand`), the same on every target.
pub(crate) struct SplitMix64(pub u64);

impl SplitMix64 {
    pub(crate) fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in [0, 1).
    pub(crate) fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}

impl WidthProfile {
    /// The preset this profile is, if it is one unedited.
    #[must_use]
    pub fn preset(&self) -> Option<ProfilePreset> {
        ProfilePreset::ALL
            .into_iter()
            .find(|p| *p == ProfilePreset::Pressure && self.seed.is_some() || *self == p.profile(0))
    }

    /// Is every point 1 / 1: the plain width?
    #[must_use]
    pub fn is_uniform(&self) -> bool {
        self.points
            .iter()
            .all(|p| p.left == 1.0 && p.right == 1.0)
    }

    /// The side factors (left, right) at `u` (0..=1 of the length).
    #[must_use]
    pub fn at(&self, u: f64) -> (f64, f64) {
        let p = &self.points;
        let (Some(first), Some(last)) = (p.first(), p.last()) else {
            return (1.0, 1.0);
        };
        if !(u > first.t) {
            return (first.left, first.right);
        }
        for w in p.windows(2) {
            if let [a, b] = w
                && u <= b.t
            {
                let x = if b.t > a.t { (u - a.t) / (b.t - a.t) } else { 1.0 };
                let s = x * x * (3.0 - 2.0 * x);
                return (
                    a.left + (b.left - a.left) * s,
                    a.right + (b.right - a.right) * s,
                );
            }
        }
        (last.left, last.right)
    }

    /// The largest side factor anywhere: the smoothstep never overshoots its
    /// two points, so this bounds the outline (page bounds, §8).
    #[must_use]
    pub fn max_factor(&self) -> f64 {
        self.points
            .iter()
            .map(|p| p.left.max(p.right))
            .fold(0.0, f64::max)
    }

    /// Reversed along the path (the design's **Flip**).
    #[must_use]
    pub fn flipped(&self) -> Self {
        Self {
            points: self
                .points
                .iter()
                .rev()
                .map(|p| WidthPoint::new(1.0 - p.t, p.right, p.left))
                .collect(),
            seed: None,
        }
    }

    /// The left side copied to the right (the design's **Mirror**).
    #[must_use]
    pub fn mirrored(&self) -> Self {
        Self {
            points: self
                .points
                .iter()
                .map(|p| WidthPoint::new(p.t, p.left, p.left))
                .collect(),
            seed: None,
        }
    }

    /// # Errors
    /// [`StrokeError`] for too few or too many points, a `t` out of 0..=1 or
    /// out of order, ends not at 0 and 1, or a side factor out of range.
    pub fn validate(&self) -> Result<(), StrokeError> {
        let n = self.points.len();
        if !(MIN_WIDTH_POINTS..=MAX_WIDTH_POINTS).contains(&n) {
            return Err(StrokeError::Limit {
                what: "width points",
                count: n,
            });
        }
        let mut prev = -1.0;
        for p in &self.points {
            if !(p.t.is_finite() && (0.0..=1.0).contains(&p.t) && p.t >= prev) {
                return Err(StrokeError::BadValue {
                    what: "width point position",
                    value: p.t,
                });
            }
            prev = p.t;
            for f in [p.left, p.right] {
                if !(f.is_finite() && (0.0..=MAX_SIDE_FACTOR).contains(&f)) {
                    return Err(StrokeError::BadValue {
                        what: "width point width",
                        value: f,
                    });
                }
            }
        }
        let ends = (self.points.first().map(|p| p.t), self.points.last().map(|p| p.t));
        if ends != (Some(0.0), Some(1.0)) {
            return Err(StrokeError::BadValue {
                what: "width profile ends",
                value: ends.0.unwrap_or(f64::NAN),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_are_valid_and_recognised() {
        for p in ProfilePreset::ALL {
            let prof = p.profile(7);
            assert!(prof.validate().is_ok(), "{p:?}");
            assert_eq!(prof.preset(), Some(p), "{p:?}");
        }
        assert!(ProfilePreset::Uniform.profile(0).is_uniform());
    }

    #[test]
    fn pressure_is_deterministic_per_seed() {
        let a = ProfilePreset::Pressure.profile(1);
        assert_eq!(a, ProfilePreset::Pressure.profile(1));
        assert_ne!(a.points, ProfilePreset::Pressure.profile(2).points);
    }

    #[test]
    fn eases_between_points_and_holds_past_the_ends() {
        let p = ProfilePreset::TaperStart.profile(0);
        assert_eq!(p.at(0.0), (0.0, 0.0));
        assert_eq!(p.at(0.15), (0.5, 0.5));
        assert_eq!(p.at(0.9), (1.0, 1.0));
        assert_eq!(p.at(-1.0), (0.0, 0.0));
        assert_eq!(p.at(f64::NAN), (0.0, 0.0));
    }

    #[test]
    fn flip_and_mirror() {
        let p = WidthProfile {
            points: vec![WidthPoint::new(0.0, 1.0, 2.0), WidthPoint::new(1.0, 3.0, 4.0)],
            seed: None,
        };
        let f = p.flipped();
        assert_eq!(f.points[0], WidthPoint::new(0.0, 4.0, 3.0));
        assert!(f.validate().is_ok());
        assert_eq!(p.mirrored().points[1], WidthPoint::new(1.0, 3.0, 3.0));
    }

    #[test]
    fn hostile_profiles_are_refused() {
        let ok = ProfilePreset::Bulge.profile(0);
        let mut bad = vec![];
        let mut b = ok.clone();
        b.points[1].t = 2.0;
        bad.push(b);
        let mut b = ok.clone();
        b.points[1].left = f64::NAN;
        bad.push(b);
        let mut b = ok.clone();
        b.points[1].right = 21.0;
        bad.push(b);
        let mut b = ok.clone();
        b.points.truncate(1);
        bad.push(b);
        let mut b = ok.clone();
        b.points[0].t = 0.1;
        bad.push(b);
        let mut b = ok;
        b.points = vec![WidthPoint::even(0.0, 1.0); 65];
        bad.push(b);
        for b in bad {
            assert!(b.validate().is_err(), "{b:?}");
        }
    }
}
