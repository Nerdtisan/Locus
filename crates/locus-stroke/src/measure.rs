// SPDX-License-Identifier: AGPL-3.0-or-later

//! A chain measured by arc length (#426): where a width point sits on the
//! path, and where a point near the path falls along it. The Width tool
//! places its handles and reads drags with this.

use crate::Chain;
use locus_geom::Vec2;
use locus_region::Seg;

/// A chain with the length before each piece.
#[derive(Debug, Clone)]
pub struct PathMap {
    segs: Vec<Seg>,
    cum: Vec<f64>,
    total: f64,
}

impl PathMap {
    #[must_use]
    pub fn new(chain: &Chain) -> Self {
        let mut cum = Vec::with_capacity(chain.segs.len());
        let mut acc = 0.0;
        for s in &chain.segs {
            cum.push(acc);
            let l = s.length();
            if l.is_finite() {
                acc += l;
            }
        }
        Self {
            segs: chain.segs.clone(),
            cum,
            total: acc,
        }
    }

    /// The whole length, mm.
    #[must_use]
    pub fn length(&self) -> f64 {
        self.total
    }

    /// The point and unit tangent at `u` (0..=1 of the length); `None` for
    /// an empty chain.
    #[must_use]
    pub fn at(&self, u: f64) -> Option<(Vec2, Vec2)> {
        let s = u.clamp(0.0, 1.0) * self.total;
        let k = self
            .cum
            .iter()
            .rposition(|c| *c <= s)
            .unwrap_or(0)
            .min(self.segs.len().checked_sub(1)?);
        let seg = self.segs.get(k)?;
        let l = seg.length();
        let t = if l > 0.0 {
            ((s - self.cum.get(k).copied().unwrap_or(0.0)) / l).clamp(0.0, 1.0)
        } else {
            0.0
        };
        Some((seg.at(t), seg.tangent(t)))
    }

    /// The `u` of the path point nearest `p` and `p`'s signed distance from
    /// it (+ = left of travel); `None` for an empty chain.
    #[must_use]
    pub fn project(&self, p: Vec2) -> Option<(f64, f64)> {
        let mut best: Option<(f64, f64, f64)> = None;
        for (k, seg) in self.segs.iter().enumerate() {
            let t = seg.param_of(p).clamp(0.0, 1.0);
            let q = seg.at(t);
            let d = q.distance(p);
            if best.is_none_or(|b| d < b.0) {
                let side = seg.tangent(t).cross(p - q);
                let s = self.cum.get(k).copied().unwrap_or(0.0) + t * seg.length();
                let u = if self.total > 0.0 { s / self.total } else { 0.0 };
                best = Some((d, u, d.copysign(side)));
            }
        }
        best.map(|(_, u, d)| (u, d))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measures_and_projects_along_an_l() {
        let v = Vec2::new;
        let c = Chain {
            segs: vec![
                Seg::Line {
                    a: v(0.0, 0.0),
                    b: v(10.0, 0.0),
                },
                Seg::Line {
                    a: v(10.0, 0.0),
                    b: v(10.0, 10.0),
                },
            ],
            closed: false,
        };
        let m = PathMap::new(&c);
        assert!((m.length() - 20.0).abs() < 1e-12);
        let (p, t) = m.at(0.75).unwrap();
        assert!(p.distance(v(10.0, 5.0)) < 1e-9 && t.distance(v(0.0, 1.0)) < 1e-9);
        let (u, d) = m.project(v(5.0, 2.0)).unwrap();
        assert!((u - 0.25).abs() < 1e-9 && (d - 2.0).abs() < 1e-9);
        let (_, d) = m.project(v(5.0, -1.0)).unwrap();
        assert!((d + 1.0).abs() < 1e-9);
        assert!(PathMap::new(&Chain::default()).at(0.5).is_none());
    }
}
