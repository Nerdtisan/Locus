// SPDX-License-Identifier: AGPL-3.0-or-later
//! The spike's stand-in document: a flat list of line segments in mm (f64).
//! The real document and its command API (SPEC §4, §11) are out of scope for
//! the GUI spike; this is only enough geometry to draw and hit-test.

use bevy::math::DVec2;

/// A line segment in millimetres.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Line {
    pub a: DVec2,
    pub b: DVec2,
}

impl Line {
    pub fn length(&self) -> f64 {
        (self.b - self.a).length()
    }
}

/// Distance from `p` to the segment `a`–`b`.
pub fn dist_to_segment(p: DVec2, a: DVec2, b: DVec2) -> f64 {
    let d = b - a;
    let len2 = d.length_squared();
    let t = if len2 > 0.0 {
        ((p - a).dot(d) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (p - (a + d * t)).length()
}

/// The line nearest to `p` within `tol` mm, if any. A brute-force scan: 5 000
/// lines take well under a millisecond, so a spatial index can wait.
pub fn hit_test(lines: &[Line], p: DVec2, tol: f64) -> Option<usize> {
    let mut best = None;
    let mut best_d = tol;
    for (i, l) in lines.iter().enumerate() {
        let d = dist_to_segment(p, l.a, l.b);
        if d <= best_d {
            best_d = d;
            best = Some(i);
        }
    }
    best
}

/// Seed of the benchmark scene, shared with spike A so both draw the same
/// distribution.
pub const STRESS_SEED: u64 = 0x9E37_79B9_7F4A_7C15;

/// `n` deterministic pseudo-random lines for the frame-time benchmark, the
/// same distribution as spike A: start uniform in a 400 x 300 mm area (here
/// centred on the origin), random angle, length 2 to 42 mm.
pub fn stress_lines(n: usize, seed: u64) -> Vec<Line> {
    let mut s = seed.max(1);
    let mut next = move || {
        // xorshift64*: deterministic, no dependency, good enough for a benchmark.
        s ^= s >> 12;
        s ^= s << 25;
        s ^= s >> 27;
        (s.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
    };
    (0..n)
        .map(|_| {
            let a = DVec2::new(next() * 400.0 - 200.0, next() * 300.0 - 150.0);
            let ang = next() * std::f64::consts::TAU;
            let len = 2.0 + next() * 40.0;
            Line {
                a,
                b: a + DVec2::from_angle(ang) * len,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distance_to_segment() {
        let (a, b) = (DVec2::ZERO, DVec2::new(10.0, 0.0));
        assert_eq!(dist_to_segment(DVec2::new(5.0, 3.0), a, b), 3.0);
        assert_eq!(dist_to_segment(DVec2::new(-4.0, 3.0), a, b), 5.0);
        assert_eq!(dist_to_segment(DVec2::new(1.0, 1.0), a, a), 2f64.sqrt());
    }

    #[test]
    fn hit_test_picks_the_nearest_within_tolerance() {
        let lines = [
            Line {
                a: DVec2::new(0.0, 0.0),
                b: DVec2::new(10.0, 0.0),
            },
            Line {
                a: DVec2::new(0.0, 1.0),
                b: DVec2::new(10.0, 1.0),
            },
        ];
        assert_eq!(hit_test(&lines, DVec2::new(5.0, 0.7), 0.5), Some(1));
        assert_eq!(hit_test(&lines, DVec2::new(5.0, 0.2), 0.5), Some(0));
        assert_eq!(hit_test(&lines, DVec2::new(5.0, 3.0), 0.5), None);
    }

    #[test]
    fn stress_lines_are_deterministic_and_bounded() {
        let a = stress_lines(5000, STRESS_SEED);
        assert_eq!(a, stress_lines(5000, STRESS_SEED));
        assert_eq!(a.len(), 5000);
        assert!(a.iter().all(|l| {
            let len = l.length();
            l.a.x.abs() <= 200.0
                && l.a.y.abs() <= 150.0
                && (2.0 - 1e-9..=42.0 + 1e-9).contains(&len)
        }));
    }
}
