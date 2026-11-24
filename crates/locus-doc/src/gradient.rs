// SPDX-License-Identifier: AGPL-3.0-or-later
//! Gradient fills on regions (#436, ADR 0019 decision 1, M6 design §6).
//!
//! A gradient lives in its region's **frame**: the region's bounding box
//! measured along axes turned by `frame_angle` (and mirrored by
//! `frame_flip`). Its `from` and `to` points are in that frame, 0..1 on each
//! axis. The frame is always refitted from the region's current geometry
//! and never stored, so a gradient follows any move or scale. Rotate and
//! Mirror commands turn or flip the frame when **Rotate with shape** is on
//! (the default, owner decision 2026-11-24), so the gradient turns with the
//! shape; with it off the frame stays page-aligned.
//!
//! One sampler ([`Gradient::colour`]) serves the canvas, PNG and the
//! conic bands of SVG/PDF, so every view agrees.

use crate::error::DocError;
use locus_geom::Vec2;
use serde::{Deserialize, Serialize};

/// Most colour stops.
pub const MAX_STOPS: usize = 32;
/// Bands a conic gradient is drawn with in SVG and PDF (no native conic in
/// SVG 1.1 or PDF types 2/3), ADR 0019 decision 4.
pub const CONIC_BANDS: usize = 64;

/// The shape of the colour change.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum GradientKind {
    /// Along the line `from` → `to`.
    Linear,
    /// Circles about `from`, reaching the last stop at `to`.
    Radial,
    /// Ellipses about `from`: `to` sets the major radius and direction,
    /// `ratio` (0..=1] the minor axis.
    Elliptical { ratio: f64 },
    /// Round `from`, starting in the direction of `to`, counter-clockwise.
    Conic,
}

/// One colour stop.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Stop {
    /// 0..=1 along the gradient.
    pub pos: f64,
    pub colour: [u8; 3],
    /// 0..=1.
    pub opacity: f64,
}

/// What machines do with a gradient (ADR 0019 decision 4).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub enum GradientMachine {
    /// Visual only: plotters and lasers ignore it.
    #[default]
    None,
    /// Variable-density hatch by luminance, `min`..`max` mm apart.
    Hatch {
        min_spacing: f64,
        max_spacing: f64,
        angle: f64,
    },
    /// Laser power by luminance, where the profile supports greyscale.
    Power,
}

fn is_zero(v: &f64) -> bool {
    *v == 0.0
}
fn is_false(v: &bool) -> bool {
    !*v
}
fn is_true(v: &bool) -> bool {
    *v
}
fn yes() -> bool {
    true
}
fn is_none(m: &GradientMachine) -> bool {
    *m == GradientMachine::None
}

/// A region's gradient fill.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Gradient {
    pub kind: GradientKind,
    /// In the frame, 0..1 per axis (may run a little outside).
    pub from: Vec2,
    pub to: Vec2,
    /// Sorted by `pos`, 2..=[`MAX_STOPS`].
    pub stops: Vec<Stop>,
    #[serde(default, skip_serializing_if = "is_none")]
    pub machine: GradientMachine,
    /// Turn and flip with Rotate and Mirror (default on).
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub rotate_with_shape: bool,
    /// The frame's turn, radians.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub frame_angle: f64,
    #[serde(default, skip_serializing_if = "is_false")]
    pub frame_flip: bool,
}

impl Gradient {
    /// Black to white, left to right: what the Inspector starts with,
    /// from `colour` to white.
    #[must_use]
    pub fn linear_from(colour: [u8; 3]) -> Self {
        Self {
            kind: GradientKind::Linear,
            from: Vec2::new(0.0, 0.5),
            to: Vec2::new(1.0, 0.5),
            stops: vec![
                Stop {
                    pos: 0.0,
                    colour,
                    opacity: 1.0,
                },
                Stop {
                    pos: 1.0,
                    colour: [255, 255, 255],
                    opacity: 1.0,
                },
            ],
            machine: GradientMachine::None,
            rotate_with_shape: true,
            frame_angle: 0.0,
            frame_flip: false,
        }
    }

    /// The gradient's invariants (it is untrusted input).
    ///
    /// # Errors
    /// [`DocError::Invalid`] or [`DocError::BadValue`] saying what is wrong.
    pub fn validate(&self) -> Result<(), DocError> {
        let bad = |what: &'static str, value: f64| DocError::BadValue { what, value };
        let n = self.stops.len();
        if !(2..=MAX_STOPS).contains(&n) {
            return Err(DocError::Invalid(format!(
                "a gradient has 2 to {MAX_STOPS} stops"
            )));
        }
        for p in [self.from, self.to] {
            for v in [p.x, p.y] {
                if !(v.is_finite() && v.abs() <= 100.0) {
                    return Err(bad("gradient point", v));
                }
            }
        }
        if self.from.distance(self.to) < 1e-9 {
            return Err(DocError::Invalid(
                "a gradient's start and end must differ".to_owned(),
            ));
        }
        let mut last = 0.0;
        for s in &self.stops {
            if !(s.pos.is_finite() && (0.0..=1.0).contains(&s.pos) && s.pos >= last) {
                return Err(bad("gradient stop position", s.pos));
            }
            if !(s.opacity.is_finite() && (0.0..=1.0).contains(&s.opacity)) {
                return Err(bad("gradient stop opacity", s.opacity));
            }
            last = s.pos;
        }
        if let GradientKind::Elliptical { ratio } = self.kind
            && !(ratio.is_finite() && ratio > 0.0 && ratio <= 1.0)
        {
            return Err(bad("gradient ratio", ratio));
        }
        if !self.frame_angle.is_finite() || self.frame_angle.abs() > 1.0e3 {
            return Err(bad("gradient angle", self.frame_angle));
        }
        if let GradientMachine::Hatch {
            min_spacing,
            max_spacing,
            angle,
        } = self.machine
            && !(min_spacing.is_finite()
                && max_spacing.is_finite()
                && angle.is_finite()
                && min_spacing > 0.0
                && max_spacing >= min_spacing
                && max_spacing <= 1.0e3)
        {
            return Err(bad("gradient hatch spacing", min_spacing));
        }
        Ok(())
    }

    /// The colour and opacity at gradient parameter `t` (clamped to 0..=1),
    /// interpolated between stops.
    #[must_use]
    pub fn colour(&self, t: f64) -> ([f64; 3], f64) {
        let t = if t.is_finite() {
            t.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let c = |s: &Stop| (s.colour.map(f64::from), s.opacity);
        let (Some(first), Some(last)) = (self.stops.first(), self.stops.last()) else {
            return ([0.0; 3], 0.0);
        };
        if t <= first.pos {
            return c(first);
        }
        for w in self.stops.windows(2) {
            let (a, b) = (&w[0], &w[1]);
            if t <= b.pos {
                let span = b.pos - a.pos;
                let k = if span > 0.0 { (t - a.pos) / span } else { 1.0 };
                let (ca, oa) = c(a);
                let (cb, ob) = c(b);
                return (
                    [0, 1, 2].map(|i| ca[i] + (cb[i] - ca[i]) * k),
                    oa + (ob - oa) * k,
                );
            }
        }
        c(last)
    }

    /// The gradient parameter at frame point `p` (0..1 per axis).
    #[must_use]
    pub fn param(&self, p: Vec2) -> f64 {
        let (a, b) = (self.from, self.to);
        let d = b - a;
        match self.kind {
            GradientKind::Linear => {
                let l2 = d.length_sq();
                if l2 > 0.0 { (p - a).dot(d) / l2 } else { 0.0 }
            }
            GradientKind::Radial => {
                let r = d.length();
                if r > 0.0 { p.distance(a) / r } else { 0.0 }
            }
            GradientKind::Elliptical { ratio } => {
                let r = d.length();
                if r <= 0.0 {
                    return 0.0;
                }
                let u = d * (1.0 / r);
                let q = p - a;
                let (x, y) = (q.dot(u), q.cross(u));
                ((x / r).powi(2) + (y / (r * ratio.max(1e-6))).powi(2)).sqrt()
            }
            GradientKind::Conic => {
                let q = p - a;
                let ang = q.y.atan2(q.x) - d.y.atan2(d.x);
                ang.rem_euclid(std::f64::consts::TAU) / std::f64::consts::TAU
            }
        }
    }
}

/// `g`'s frame on region `r` (#436): fitted to the region's outer loop,
/// sampled along each edge, at the gradient's angle and flip.
#[must_use]
pub fn region_frame(r: &locus_region::Region, g: &Gradient) -> Option<Frame> {
    let pts: Vec<Vec2> = r
        .outer
        .edges
        .iter()
        .flat_map(|e| (0..=8).map(move |k| e.seg.at(f64::from(k) / 8.0)))
        .collect();
    Frame::fit(&pts, g.frame_angle, g.frame_flip)
}

/// The colour and opacity a gradient paints at page point `w` in `frame`,
/// times the fill's own `opacity`: `([r, g, b] 0..=255, alpha 0..=1)`.
#[must_use]
pub fn paint_at(g: &Gradient, frame: &Frame, opacity: f64, w: Vec2) -> ([f64; 3], f64) {
    let (c, a) = g.colour(g.param(frame.to_frame(w)));
    (c, a * opacity.clamp(0.0, 1.0))
}

/// A region's gradient frame on the page: the box of `points` measured
/// along axes turned by `angle`, mirrored across its x axis with `flip`.
/// Frame point (u, v) is `origin + ax·u + ay·v`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    pub origin: Vec2,
    pub ax: Vec2,
    pub ay: Vec2,
}

impl Frame {
    /// Fit the frame to `points` (the region's outline, sampled).
    #[must_use]
    pub fn fit(points: &[Vec2], angle: f64, flip: bool) -> Option<Self> {
        let (s, c) = angle.sin_cos();
        let (ex, ey) = (Vec2::new(c, s), Vec2::new(-s, c));
        let (mut lo, mut hi) = (Vec2::new(f64::MAX, f64::MAX), Vec2::new(f64::MIN, f64::MIN));
        for p in points {
            let q = Vec2::new(p.dot(ex), p.dot(ey));
            lo = Vec2::new(lo.x.min(q.x), lo.y.min(q.y));
            hi = Vec2::new(hi.x.max(q.x), hi.y.max(q.y));
        }
        if !(lo.x.is_finite() && hi.x > lo.x && hi.y > lo.y) {
            return None;
        }
        let (w, h) = (hi.x - lo.x, hi.y - lo.y);
        let origin = ex * lo.x + ey * lo.y;
        Some(if flip {
            Self {
                origin: origin + ey * h,
                ax: ex * w,
                ay: ey * -h,
            }
        } else {
            Self {
                origin,
                ax: ex * w,
                ay: ey * h,
            }
        })
    }

    /// Frame point (u, v) on the page.
    #[must_use]
    pub fn to_page(&self, p: Vec2) -> Vec2 {
        self.origin + self.ax * p.x + self.ay * p.y
    }

    /// Page point `w` in the frame.
    #[must_use]
    pub fn to_frame(&self, w: Vec2) -> Vec2 {
        let det = self.ax.cross(self.ay);
        if det.abs() < 1e-18 {
            return Vec2::ZERO;
        }
        let q = w - self.origin;
        Vec2::new(q.cross(self.ay) / det, self.ax.cross(q) / det)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn stops_interpolate_and_clamp() {
        let g = Gradient::linear_from([0, 0, 0]);
        assert_eq!(g.colour(-1.0).0, [0.0; 3]);
        assert_eq!(g.colour(0.5).0, [127.5; 3]);
        assert_eq!(g.colour(2.0).0, [255.0; 3]);
        assert_eq!(g.param(Vec2::new(0.25, 0.9)), 0.25);
    }

    #[test]
    fn kinds_measure_as_drawn() {
        let mut g = Gradient::linear_from([0, 0, 0]);
        g.from = Vec2::new(0.5, 0.5);
        g.to = Vec2::new(1.0, 0.5);
        g.kind = GradientKind::Radial;
        assert!((g.param(Vec2::new(0.5, 1.0)) - 1.0).abs() < 1e-12);
        g.kind = GradientKind::Elliptical { ratio: 0.5 };
        assert!((g.param(Vec2::new(0.5, 0.75)) - 1.0).abs() < 1e-12);
        g.kind = GradientKind::Conic;
        assert!((g.param(Vec2::new(0.5, 1.0)) - 0.25).abs() < 1e-12);
    }

    #[test]
    fn the_frame_fits_the_box_along_its_angle() {
        let pts = [
            Vec2::new(0.0, 0.0),
            Vec2::new(4.0, 0.0),
            Vec2::new(4.0, 2.0),
            Vec2::new(0.0, 2.0),
        ];
        let f = Frame::fit(&pts, 0.0, false).unwrap();
        assert_eq!(f.to_page(Vec2::new(1.0, 1.0)), Vec2::new(4.0, 2.0));
        let back = f.to_frame(Vec2::new(2.0, 1.0));
        assert!(back.distance(Vec2::new(0.5, 0.5)) < 1e-12);
        let turned = Frame::fit(&pts, std::f64::consts::FRAC_PI_2, false).unwrap();
        // Along a quarter turn, "x" runs up the page.
        assert!(turned.ax.distance(Vec2::new(0.0, 2.0)) < 1e-12);
        let flipped = Frame::fit(&pts, 0.0, true).unwrap();
        assert!(flipped.to_page(Vec2::ZERO).distance(Vec2::new(0.0, 2.0)) < 1e-12);
    }

    #[test]
    fn bad_gradients_are_refused() {
        let ok = Gradient::linear_from([10, 20, 30]);
        assert!(ok.validate().is_ok());
        let mut g = ok.clone();
        g.stops.truncate(1);
        assert!(g.validate().is_err());
        let mut g = ok.clone();
        g.stops[0].pos = 0.9;
        g.stops[1].pos = 0.1;
        assert!(g.validate().is_err());
        let mut g = ok.clone();
        g.to = g.from;
        assert!(g.validate().is_err());
        let mut g = ok.clone();
        g.kind = GradientKind::Elliptical { ratio: f64::NAN };
        assert!(g.validate().is_err());
        let mut g = ok;
        g.stops = vec![g.stops[0]; MAX_STOPS + 1];
        assert!(g.validate().is_err());
    }
}
