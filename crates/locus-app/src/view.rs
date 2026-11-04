// SPDX-License-Identifier: AGPL-3.0-or-later
//! World ↔ screen mapping for the canvas (ported from the eframe spike, ADR 0001).
//!
//! World units are millimetres in `f64` (SPEC §4), y up. Screen units are egui
//! points, y down. Everything is computed in `f64` and only narrowed to `f32`
//! at the very end, relative to the canvas centre, so a 0.001 mm view of a
//! point 2 m from the origin stays exact.

use locus_geom::Vec2;

/// Smallest and largest visible extent across the canvas's shorter side
/// (issue #44: 0.001 mm to 2 m).
pub const MIN_EXTENT_MM: f64 = 0.001;
pub const MAX_EXTENT_MM: f64 = 2000.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    /// World point shown at the centre of the canvas.
    pub center: Vec2,
    /// Zoom: screen points per millimetre.
    pub pt_per_mm: f64,
}

impl Default for View {
    fn default() -> Self {
        Self {
            center: Vec2::new(150.0, 100.0),
            pt_per_mm: 2.0,
        }
    }
}

impl View {
    /// World → screen, as an offset in points from the canvas centre.
    pub fn to_screen_rel(self, p: Vec2) -> (f64, f64) {
        let d = p - self.center;
        (d.x * self.pt_per_mm, -d.y * self.pt_per_mm)
    }

    pub fn to_screen(self, p: Vec2, rect: egui::Rect) -> egui::Pos2 {
        let (x, y) = self.to_screen_rel(p);
        let c = rect.center();
        egui::pos2((c.x as f64 + x) as f32, (c.y as f64 + y) as f32)
    }

    pub fn to_world(self, s: egui::Pos2, rect: egui::Rect) -> Vec2 {
        let c = rect.center();
        let dx = (s.x - c.x) as f64 / self.pt_per_mm;
        let dy = -(s.y - c.y) as f64 / self.pt_per_mm;
        self.center + Vec2::new(dx, dy)
    }

    /// Zoom range allowed for a canvas of this size: the short side shows at
    /// least `MIN_EXTENT_MM` and the long side at most `MAX_EXTENT_MM`.
    pub fn zoom_limits(rect: egui::Rect) -> (f64, f64) {
        let short = rect.width().min(rect.height()).max(1.0) as f64;
        let long = rect.width().max(rect.height()).max(1.0) as f64;
        (long / MAX_EXTENT_MM, short / MIN_EXTENT_MM)
    }

    /// Visible world extent across the shorter side, in mm.
    pub fn extent_mm(&self, rect: egui::Rect) -> f64 {
        rect.width().min(rect.height()) as f64 / self.pt_per_mm
    }

    /// Zoom by `factor`, keeping the world point under `anchor` fixed.
    pub fn zoom_about(&mut self, anchor: egui::Pos2, factor: f64, rect: egui::Rect) {
        let before = self.to_world(anchor, rect);
        let (lo, hi) = Self::zoom_limits(rect);
        self.pt_per_mm = (self.pt_per_mm * factor).clamp(lo, hi);
        let after = self.to_world(anchor, rect);
        self.center += before - after;
    }

    pub fn pan_points(&mut self, delta: egui::Vec2) {
        self.center += Vec2::new(
            -delta.x as f64 / self.pt_per_mm,
            delta.y as f64 / self.pt_per_mm,
        );
    }

    /// Fit a world bounding box into the canvas with a small margin.
    pub fn fit(&mut self, min: Vec2, max: Vec2, rect: egui::Rect) {
        let size = max - min;
        let w = size.x.max(MIN_EXTENT_MM);
        let h = size.y.max(MIN_EXTENT_MM);
        let k = (rect.width() as f64 / w).min(rect.height() as f64 / h) * 0.9;
        let (lo, hi) = Self::zoom_limits(rect);
        self.pt_per_mm = k.clamp(lo, hi);
        self.center = min + size * 0.5;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect() -> egui::Rect {
        egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(800.0, 600.0))
    }

    #[test]
    fn zoom_keeps_anchor_fixed() {
        let r = rect();
        let mut v = View::default();
        let anchor = egui::pos2(123.0, 456.0);
        let w0 = v.to_world(anchor, r);
        v.zoom_about(anchor, 37.0, r);
        let w1 = v.to_world(anchor, r);
        assert!((w0 - w1).length() < 1e-9);
    }

    #[test]
    fn zoom_clamps_to_extents() {
        let r = rect();
        let mut v = View::default();
        v.zoom_about(r.center(), 1e12, r);
        assert!((v.extent_mm(r) - MIN_EXTENT_MM).abs() < 1e-9);
        v.zoom_about(r.center(), 1e-12, r);
        // Zoomed out, the long side (800 pt wide) is the one held at 2 m.
        assert!((r.width() as f64 / v.pt_per_mm - MAX_EXTENT_MM).abs() < 1e-6);
    }

    #[test]
    fn deep_zoom_far_from_origin_round_trips() {
        let r = rect();
        let mut v = View {
            center: Vec2::new(1_999.123_456, -1500.0),
            pt_per_mm: 1.0,
        };
        v.zoom_about(r.center(), 1e9, r);
        let p = Vec2::new(1_999.123_456 + 0.000_1, -1500.0);
        let s = v.to_screen(p, r);
        let back = v.to_world(s, r);
        // One screen point at this zoom is 1/600 µm; allow a tenth of that.
        assert!((back - p).length() < 0.1 / v.pt_per_mm);
    }

    #[test]
    fn fit_centres_the_box() {
        let r = rect();
        let mut v = View::default();
        v.fit(Vec2::new(0.0, 0.0), Vec2::new(100.0, 50.0), r);
        assert!(v.center.distance(Vec2::new(50.0, 25.0)) < 1e-9);
        let s = v.to_screen(Vec2::new(100.0, 50.0), r);
        assert!(r.contains(s));
    }
}
