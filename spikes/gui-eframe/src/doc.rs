// SPDX-License-Identifier: AGPL-3.0-or-later
//! Spike stand-in for the document: lines plus a layer stub.
//!
//! The real model and its command API live in `locus-doc` (SPEC §4, §11); the
//! spike mutates this struct directly because it is throwaway.

use crate::view::V2;
use egui::Color32;

#[derive(Clone, Copy, Debug)]
pub struct Line {
    pub a: V2,
    pub b: V2,
    pub layer: usize,
}

#[derive(Clone, Debug)]
pub struct Layer {
    pub name: &'static str,
    pub color: Color32,
    pub visible: bool,
}

#[derive(Debug)]
pub struct Doc {
    pub lines: Vec<Line>,
    pub layers: Vec<Layer>,
    pub active_layer: usize,
    /// Bumped on every change so GPU buffers know when to re-upload.
    pub revision: u64,
}

impl Default for Doc {
    fn default() -> Self {
        Self {
            lines: Vec::new(),
            layers: vec![
                Layer {
                    name: "Cut",
                    color: Color32::from_rgb(230, 70, 60),
                    visible: true,
                },
                Layer {
                    name: "Score",
                    color: Color32::from_rgb(60, 140, 230),
                    visible: true,
                },
                Layer {
                    name: "Engrave",
                    color: Color32::from_rgb(60, 180, 90),
                    visible: true,
                },
            ],
            active_layer: 0,
            revision: 0,
        }
    }
}

/// Distance from `p` to segment `a`–`b`.
pub fn dist_to_segment(p: V2, a: V2, b: V2) -> f64 {
    let ab = b.sub(a);
    let len2 = ab.dot(ab);
    let t = if len2 > 0.0 {
        (p.sub(a).dot(ab) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    p.sub(a.add(ab.scale(t))).len()
}

impl Doc {
    pub fn touch(&mut self) {
        self.revision += 1;
    }

    pub fn add_line(&mut self, a: V2, b: V2) {
        self.lines.push(Line {
            a,
            b,
            layer: self.active_layer,
        });
        self.touch();
    }

    pub fn is_visible(&self, l: &Line) -> bool {
        self.layers.get(l.layer).is_some_and(|ly| ly.visible)
    }

    pub fn color(&self, l: &Line) -> Color32 {
        self.layers
            .get(l.layer)
            .map_or(Color32::GRAY, |ly| ly.color)
    }

    /// Nearest visible line within `tol_mm` of `p`. Brute force on purpose: the
    /// spike measures what a naive scan costs at 5 000 lines.
    pub fn hit_test(&self, p: V2, tol_mm: f64) -> Option<usize> {
        let mut best: Option<(usize, f64)> = None;
        for (i, l) in self.lines.iter().enumerate() {
            if !self.is_visible(l) {
                continue;
            }
            let d = dist_to_segment(p, l.a, l.b);
            if d <= tol_mm && best.is_none_or(|(_, bd)| d < bd) {
                best = Some((i, d));
            }
        }
        best.map(|(i, _)| i)
    }

    /// Replace the drawing with `n` pseudo-random lines over a 400 × 300 mm bed.
    /// Deterministic (fixed seed) so both GUI spikes draw the same scene.
    pub fn generate(&mut self, n: usize) {
        let mut s: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut rnd = move || {
            // xorshift64*
            s ^= s >> 12;
            s ^= s << 25;
            s ^= s >> 27;
            (s.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
        };
        self.lines.clear();
        for i in 0..n {
            let a = V2::new(rnd() * 400.0, rnd() * 300.0);
            let ang = rnd() * std::f64::consts::TAU;
            let len = 2.0 + rnd() * 40.0;
            let b = a.add(V2::new(ang.cos() * len, ang.sin() * len));
            self.lines.push(Line {
                a,
                b,
                layer: i % self.layers.len(),
            });
        }
        self.touch();
    }

    pub fn bounds(&self) -> Option<(V2, V2)> {
        let mut it = self.lines.iter().flat_map(|l| [l.a, l.b]);
        let first = it.next()?;
        Some(it.fold((first, first), |(lo, hi), p| {
            (
                V2::new(lo.x.min(p.x), lo.y.min(p.y)),
                V2::new(hi.x.max(p.x), hi.y.max(p.y)),
            )
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segment_distance() {
        let a = V2::new(0.0, 0.0);
        let b = V2::new(10.0, 0.0);
        assert_eq!(dist_to_segment(V2::new(5.0, 3.0), a, b), 3.0);
        assert_eq!(dist_to_segment(V2::new(-4.0, 3.0), a, b), 5.0);
        assert_eq!(dist_to_segment(V2::new(1.0, 1.0), a, a), 2f64.sqrt());
    }

    #[test]
    fn hit_test_picks_nearest_visible() {
        let mut d = Doc::default();
        d.add_line(V2::new(0.0, 0.0), V2::new(10.0, 0.0));
        d.add_line(V2::new(0.0, 1.0), V2::new(10.0, 1.0));
        assert_eq!(d.hit_test(V2::new(5.0, 0.8), 0.5), Some(1));
        assert_eq!(d.hit_test(V2::new(5.0, 5.0), 0.5), None);
        d.layers[0].visible = false;
        assert_eq!(d.hit_test(V2::new(5.0, 0.2), 0.5), None);
    }

    #[test]
    fn generate_is_deterministic() {
        let mut a = Doc::default();
        let mut b = Doc::default();
        a.generate(5000);
        b.generate(5000);
        assert_eq!(a.lines.len(), 5000);
        assert!(a
            .lines
            .iter()
            .zip(&b.lines)
            .all(|(x, y)| x.a == y.a && x.b == y.b));
    }
}
