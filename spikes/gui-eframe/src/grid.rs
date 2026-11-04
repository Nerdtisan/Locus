// SPDX-License-Identifier: AGPL-3.0-or-later
//! Adaptive decade grid: minor lines every 10^n mm, major every 10^(n+1) mm,
//! with the minor lines fading in as they spread apart (Fusion-style).

use crate::view::{View, V2};
use egui::{Color32, Painter, Rect, Stroke};

/// Minor lines closer than this many points are not drawn.
const MIN_MINOR_PT: f64 = 8.0;
/// Minor lines are fully opaque once this far apart.
const FULL_MINOR_PT: f64 = 40.0;

/// Minor grid step in mm for a zoom level, and how opaque the minor lines are.
pub fn minor_step(pt_per_mm: f64) -> (f64, f32) {
    let n = (MIN_MINOR_PT / pt_per_mm).log10().ceil();
    let step = 10f64.powf(n);
    let px = step * pt_per_mm;
    let t = ((px - MIN_MINOR_PT) / (FULL_MINOR_PT - MIN_MINOR_PT)).clamp(0.0, 1.0);
    (step, t as f32)
}

pub fn draw(painter: &Painter, view: &View, rect: Rect, dark: bool) {
    let (step, fade) = minor_step(view.pt_per_mm);
    let lo = view.to_world(rect.left_bottom(), rect);
    let hi = view.to_world(rect.right_top(), rect);
    let base = if dark { 255 } else { 0 };
    let minor_c = Color32::from_rgba_unmultiplied(base, base, base, (18.0 * fade) as u8);
    let major_c = Color32::from_rgba_unmultiplied(base, base, base, 40);
    let axis_c = Color32::from_rgba_unmultiplied(base, base, base, 90);

    // Index-based iteration (k * step) avoids drift from repeated addition.
    let lines = |from: f64, to: f64| {
        let k0 = (from / step).floor() as i64;
        let k1 = (to / step).ceil() as i64;
        k0..=k1
    };
    for k in lines(lo.x, hi.x) {
        let x = k as f64 * step;
        let c = if k == 0 {
            axis_c
        } else if k % 10 == 0 {
            major_c
        } else if fade > 0.0 {
            minor_c
        } else {
            continue;
        };
        let sx = view.to_screen(V2::new(x, 0.0), rect).x;
        painter.line_segment(
            [egui::pos2(sx, rect.top()), egui::pos2(sx, rect.bottom())],
            Stroke::new(1.0, c),
        );
    }
    for k in lines(lo.y, hi.y) {
        let y = k as f64 * step;
        let c = if k == 0 {
            axis_c
        } else if k % 10 == 0 {
            major_c
        } else if fade > 0.0 {
            minor_c
        } else {
            continue;
        };
        let sy = view.to_screen(V2::new(0.0, y), rect).y;
        painter.line_segment(
            [egui::pos2(rect.left(), sy), egui::pos2(rect.right(), sy)],
            Stroke::new(1.0, c),
        );
    }
}

/// Human label for a step in mm, e.g. "1 µm", "10 mm", "1 m".
pub fn label(step_mm: f64) -> String {
    if step_mm < 0.999e-3 {
        format!("{} nm", (step_mm * 1.0e6).round())
    } else if step_mm < 0.999 {
        format!("{} µm", (step_mm * 1.0e3).round())
    } else if step_mm < 999.0 {
        format!("{} mm", step_mm.round())
    } else {
        format!("{} m", (step_mm / 1000.0).round())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_is_a_decade_at_least_min_spacing() {
        for &z in &[1e-3, 0.3, 1.0, 7.5, 123.0, 6e5] {
            let (s, _) = minor_step(z);
            assert!(s * z >= MIN_MINOR_PT - 1e-9, "zoom {z}");
            assert!(s * z < MIN_MINOR_PT * 10.0 + 1e-9, "zoom {z}");
            let lg = s.log10();
            assert!((lg - lg.round()).abs() < 1e-9);
        }
    }

    #[test]
    fn labels() {
        assert_eq!(label(0.001), "1 µm");
        assert_eq!(label(0.1), "100 µm");
        assert_eq!(label(10.0), "10 mm");
        assert_eq!(label(1000.0), "1 m");
    }
}
