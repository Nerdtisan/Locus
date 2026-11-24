// SPDX-License-Identifier: AGPL-3.0-or-later
//! Gradient fills in export (#436, ADR 0019 decision 4): stepped bands for
//! writers without a native form (PDF in this version; conic everywhere,
//! since SVG 1.1 and PDF types 2/3 have no conic). Each band is a polygon on
//! the page in its colour, listed in painting order; the writer clips them
//! to the region. The colours come from the same sampler as the canvas.

use locus_doc::gradient::{Frame, Gradient, GradientKind};
use locus_geom::Vec2;

/// Bands per gradient (ADR 0019: 64 wedges for conic; the same count reads
/// smooth for the others at print sizes).
pub const BANDS: usize = locus_doc::gradient::CONIC_BANDS;

/// One band: a polygon on the page, its colour (0..=255) and alpha (0..=1,
/// the stop's opacity times the fill's).
#[derive(Debug, Clone, PartialEq)]
pub struct Band {
    pub polygon: Vec<Vec2>,
    pub colour: [f64; 3],
    pub alpha: f64,
}

/// `g` on `frame` as bands that cover the frame, in painting order.
#[must_use]
pub fn bands(g: &Gradient, frame: &Frame, opacity: f64) -> Vec<Band> {
    let n = BANDS;
    let page = |p: Vec2| frame.to_page(p);
    let band = |polygon: Vec<Vec2>, t: f64| {
        let (colour, a) = g.colour(t);
        Band {
            polygon: polygon.into_iter().map(page).collect(),
            colour,
            alpha: a * opacity.clamp(0.0, 1.0),
        }
    };
    let (a, d) = (g.from, g.to - g.from);
    // Far enough to cover the unit frame from anywhere near it.
    const FAR: f64 = 8.0;
    let mut out = Vec::with_capacity(n + 1);
    match g.kind {
        GradientKind::Linear => {
            let len = d.length();
            if len <= 0.0 {
                return out;
            }
            let side = d.perp() * (FAR / len);
            let at = |t: f64| a + d * t;
            for k in 0..n {
                let t0 = if k == 0 { -FAR } else { k as f64 / n as f64 };
                let t1 = if k + 1 == n {
                    FAR
                } else {
                    (k + 1) as f64 / n as f64
                };
                let mid = (k as f64 + 0.5) / n as f64;
                out.push(band(
                    vec![at(t0) - side, at(t1) - side, at(t1) + side, at(t0) + side],
                    mid,
                ));
            }
        }
        GradientKind::Radial | GradientKind::Elliptical { .. } => {
            let ratio = match g.kind {
                GradientKind::Elliptical { ratio } => ratio,
                _ => 1.0,
            };
            let r = d.length();
            if r <= 0.0 {
                return out;
            }
            let u = d * (1.0 / r);
            let v = -u.perp();
            // Beyond the last ring: the last stop, over everything.
            let big = [
                Vec2::new(-FAR, -FAR),
                Vec2::new(FAR, -FAR),
                Vec2::new(FAR, FAR),
                Vec2::new(-FAR, FAR),
            ];
            out.push(band(big.to_vec(), 1.0));
            // Rings from the outside in, each a filled ellipse.
            for k in (0..n).rev() {
                let t = (k + 1) as f64 / n as f64;
                let ring: Vec<Vec2> = (0..96)
                    .map(|i| {
                        let phi = std::f64::consts::TAU * f64::from(i) / 96.0;
                        a + u * (t * r * phi.cos()) + v * (t * r * ratio * phi.sin())
                    })
                    .collect();
                out.push(band(ring, (k as f64 + 0.5) / n as f64));
            }
        }
        GradientKind::Conic => {
            let start = d.y.atan2(d.x);
            for k in 0..n {
                let (p0, p1) = (
                    start + std::f64::consts::TAU * k as f64 / n as f64,
                    start + std::f64::consts::TAU * (k + 1) as f64 / n as f64,
                );
                // A wedge, a hair wider so neighbours leave no seam.
                let e = 1e-3;
                out.push(band(
                    vec![
                        a,
                        a + Vec2::from_angle(p0 - e) * FAR,
                        a + Vec2::from_angle((p0 + p1) / 2.0) * FAR,
                        a + Vec2::from_angle(p1 + e) * FAR,
                    ],
                    (k as f64 + 0.5) / n as f64,
                ));
            }
        }
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn frame() -> Frame {
        let pts = [
            Vec2::new(0.0, 0.0),
            Vec2::new(10.0, 0.0),
            Vec2::new(10.0, 4.0),
            Vec2::new(0.0, 4.0),
        ];
        Frame::fit(&pts, 0.0, false).unwrap()
    }

    #[test]
    fn linear_bands_run_from_the_first_to_the_last_stop() {
        let g = Gradient::linear_from([0, 0, 0]);
        let b = bands(&g, &frame(), 1.0);
        assert_eq!(b.len(), BANDS);
        assert!(b[0].colour[0] < 5.0);
        assert!(b[BANDS - 1].colour[0] > 250.0);
        // The first band covers the region's left edge.
        assert!(b[0].polygon.iter().any(|p| p.x < 0.0));
    }

    #[test]
    fn round_and_conic_bands_cover_the_frame() {
        for kind in [
            GradientKind::Radial,
            GradientKind::Elliptical { ratio: 0.5 },
            GradientKind::Conic,
        ] {
            let mut g = Gradient::linear_from([0, 0, 0]);
            g.kind = kind;
            g.from = Vec2::new(0.5, 0.5);
            g.to = Vec2::new(1.0, 0.5);
            let b = bands(&g, &frame(), 0.5);
            assert!(b.len() >= BANDS, "{kind:?}");
            assert!(b.iter().all(|x| x.alpha <= 0.5 + 1e-12));
        }
    }
}
