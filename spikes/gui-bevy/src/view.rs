// SPDX-License-Identifier: AGPL-3.0-or-later
//! The canvas view: the millimetre world (f64, y up, SPEC §4) and the canvas
//! screen space (logical pixels from the canvas centre, y up — exactly the
//! world space of the canvas's Bevy camera).
//!
//! Every world→screen conversion happens here in f64, relative to the view
//! centre, and only the small screen-space result is cast to f32 for the GPU.
//! That keeps 0.001 mm views precise anywhere on a 2 m sheet; f32 world
//! coordinates would not (f32 spacing near 2000 mm is ~1.2e-4 mm).

use bevy::math::DVec2;

/// The smallest visible extent: the canvas's shorter side spans at least this.
pub const MIN_EXTENT_MM: f64 = 0.001;
/// The largest visible extent: the canvas's longer side spans at most this.
pub const MAX_EXTENT_MM: f64 = 2000.0;

/// Pan/zoom state of the canvas.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    /// World point at the canvas centre, in mm.
    pub center: DVec2,
    /// Zoom: logical pixels per millimetre.
    pub px_per_mm: f64,
    /// Canvas size in logical pixels.
    pub size: DVec2,
}

impl Default for View {
    fn default() -> Self {
        Self {
            center: DVec2::ZERO,
            px_per_mm: 4.0,
            size: DVec2::new(800.0, 600.0),
        }
    }
}

impl View {
    /// World (mm) → screen (logical px from the canvas centre, y up).
    pub fn to_screen(self, p: DVec2) -> DVec2 {
        (p - self.center) * self.px_per_mm
    }

    /// Screen (logical px from the canvas centre, y up) → world (mm).
    pub fn to_world(self, s: DVec2) -> DVec2 {
        self.center + s / self.px_per_mm
    }

    /// The allowed zoom range for the current canvas size.
    pub fn zoom_limits(&self) -> (f64, f64) {
        let lo = self.size.max_element().max(1.0) / MAX_EXTENT_MM;
        let hi = self.size.min_element().max(1.0) / MIN_EXTENT_MM;
        (lo, hi.max(lo))
    }

    /// Keeps the zoom inside the limits, e.g. after the canvas is resized.
    pub fn clamp_zoom(&mut self) {
        let (lo, hi) = self.zoom_limits();
        self.px_per_mm = self.px_per_mm.clamp(lo, hi);
    }

    /// Zooms by `factor`, keeping the world point under `anchor` fixed.
    pub fn zoom_about(&mut self, anchor: DVec2, factor: f64) {
        if !factor.is_finite() || factor <= 0.0 {
            return;
        }
        let fixed = self.to_world(anchor);
        let (lo, hi) = self.zoom_limits();
        self.px_per_mm = (self.px_per_mm * factor).clamp(lo, hi);
        self.center = fixed - anchor / self.px_per_mm;
    }

    /// Moves the view so the content follows a pointer drag of `delta` px.
    pub fn pan(&mut self, delta: DVec2) {
        self.center -= delta / self.px_per_mm;
    }

    /// Visible world rectangle (min, max) in mm.
    pub fn visible(&self) -> (DVec2, DVec2) {
        let half = self.size * 0.5 / self.px_per_mm;
        (self.center - half, self.center + half)
    }

    /// Sets the zoom so the shorter visible side is `extent_mm` (clamped).
    pub fn set_extent(&mut self, extent_mm: f64) {
        self.px_per_mm = self.size.min_element().max(1.0) / extent_mm;
        self.clamp_zoom();
    }

    /// Fits a world rectangle into the view with a small margin.
    pub fn fit(&mut self, min: DVec2, max: DVec2) {
        let span = (max - min).max(DVec2::splat(MIN_EXTENT_MM));
        self.center = (min + max) * 0.5;
        self.px_per_mm = (self.size / (span * 1.1)).min_element();
        self.clamp_zoom();
    }
}

/// One level of the adaptive grid.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridLevel {
    /// Line spacing in mm.
    pub step: f64,
    /// Opacity, 0..1.
    pub alpha: f32,
}

/// Minor lines are never closer than this many logical pixels.
const MIN_GRID_PX: f64 = 8.0;

/// The minor and major grid levels for a zoom. Spacings are powers of ten
/// (mm-native); the minor level fades in as it opens up, and reaches the
/// major opacity just as it becomes the next major level, so zooming never
/// pops.
pub fn grid_levels(px_per_mm: f64) -> [GridLevel; 2] {
    let k = (MIN_GRID_PX / px_per_mm).log10().ceil();
    let minor = 10f64.powf(k);
    // minor spacing is in [MIN_GRID_PX, 10 * MIN_GRID_PX) px.
    let t = ((minor * px_per_mm / MIN_GRID_PX).log10()).clamp(0.0, 1.0) as f32;
    [
        GridLevel {
            step: minor,
            alpha: 0.06 + 0.22 * t,
        },
        GridLevel {
            step: minor * 10.0,
            alpha: 0.28 + 0.22 * t,
        },
    ]
}

/// Clips segment `a`–`b` to the rectangle `min`..`max` (Liang–Barsky, f64).
pub fn clip_segment(a: DVec2, b: DVec2, min: DVec2, max: DVec2) -> Option<(DVec2, DVec2)> {
    let d = b - a;
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    for (p, q) in [
        (-d.x, a.x - min.x),
        (d.x, max.x - a.x),
        (-d.y, a.y - min.y),
        (d.y, max.y - a.y),
    ] {
        if p == 0.0 {
            if q < 0.0 {
                return None;
            }
        } else {
            let r = q / p;
            if p < 0.0 {
                t0 = t0.max(r);
            } else {
                t1 = t1.min(r);
            }
            if t0 > t1 {
                return None;
            }
        }
    }
    Some((a + d * t0, a + d * t1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> View {
        View {
            center: DVec2::new(1999.5, -1999.5),
            px_per_mm: 3.0,
            size: DVec2::new(1000.0, 800.0),
        }
    }

    #[test]
    fn zoom_keeps_the_anchor_fixed() {
        let mut v = view();
        let anchor = DVec2::new(123.0, -77.0);
        let before = v.to_world(anchor);
        v.zoom_about(anchor, 37.0);
        let after = v.to_world(anchor);
        assert!((before - after).length() < 1e-9);
    }

    #[test]
    fn zoom_is_clamped_to_the_extent_limits() {
        let mut v = view();
        v.zoom_about(DVec2::ZERO, 1e12);
        let (min, max) = v.visible();
        assert!(((max - min).min_element() - MIN_EXTENT_MM).abs() < 1e-12);
        v.zoom_about(DVec2::ZERO, 1e-12);
        let (min, max) = v.visible();
        assert!(((max - min).max_element() - MAX_EXTENT_MM).abs() < 1e-9);
    }

    #[test]
    fn micron_view_far_from_origin_round_trips() {
        let mut v = view();
        v.set_extent(MIN_EXTENT_MM);
        // One logical pixel at this zoom is 1.25e-6 mm; the round trip must
        // be far tighter than that.
        let s = DVec2::new(0.5, -0.25);
        let back = v.to_screen(v.to_world(s));
        assert!((back - s).length() < 1e-6);
    }

    #[test]
    fn grid_spacing_stays_in_pixel_band_and_fades_continuously() {
        let mut ppm = 1e-3;
        while ppm < 1e7 {
            let [minor, major] = grid_levels(ppm);
            let px = minor.step * ppm;
            assert!((MIN_GRID_PX - 1e-9..10.0 * MIN_GRID_PX + 1e-9).contains(&px));
            assert!(minor.alpha <= major.alpha);
            ppm *= 1.07;
        }
        // Just below a decade boundary the minor level is as strong as the
        // major level just above it.
        let below = grid_levels(MIN_GRID_PX / 1.0000001)[0];
        let above = grid_levels(MIN_GRID_PX * 1.0000001)[1];
        assert!((below.alpha - above.alpha).abs() < 1e-3);
    }

    #[test]
    fn clipping() {
        let (min, max) = (DVec2::ZERO, DVec2::splat(10.0));
        let c = clip_segment(DVec2::new(-5.0, 5.0), DVec2::new(15.0, 5.0), min, max);
        assert_eq!(c, Some((DVec2::new(0.0, 5.0), DVec2::new(10.0, 5.0))));
        assert_eq!(
            clip_segment(DVec2::new(-5.0, -5.0), DVec2::new(-1.0, 20.0), min, max),
            None
        );
        let inside = (DVec2::new(1.0, 2.0), DVec2::new(3.0, 4.0));
        assert_eq!(clip_segment(inside.0, inside.1, min, max), Some(inside));
    }
}
