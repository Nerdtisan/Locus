// SPDX-License-Identifier: AGPL-3.0-or-later
//! Canvas state and rendering. Everything is drawn with Bevy gizmos in canvas
//! screen space (logical px from the canvas centre), recomputed each frame
//! from the f64 mm geometry, so precision never depends on f32 world
//! coordinates (see `view.rs`).

use bevy::math::DVec2;
use bevy::prelude::*;
use bevy_egui::egui;

use crate::doc::Line;
use crate::view::{self, View};

pub const BACKGROUND: Color = Color::srgb(0.085, 0.09, 0.105);
const GRID: (f32, f32, f32) = (0.62, 0.66, 0.76);
const AXIS_X: Color = Color::srgba(0.85, 0.35, 0.35, 0.8);
const AXIS_Y: Color = Color::srgba(0.35, 0.75, 0.4, 0.8);
const LINE: Color = Color::srgb(0.86, 0.88, 0.92);
const HOVER: Color = Color::srgb(1.0, 0.62, 0.15);
const SELECTED: Color = Color::srgb(0.25, 0.7, 1.0);
const RUBBER: Color = Color::srgb(1.0, 0.85, 0.3);

/// Gizmo group for highlighted and in-progress geometry (wider lines).
#[derive(Default, Reflect, GizmoConfigGroup)]
pub struct Thick;

/// The active tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Select,
    Line,
}

/// Canvas state shared by the UI (input, panels) and the renderer.
#[derive(Resource, Debug)]
pub struct Canvas {
    pub view: View,
    pub lines: Vec<Line>,
    pub tool: Tool,
    /// First point of a line being drawn (click-click).
    pub pending: Option<DVec2>,
    pub hover: Option<usize>,
    pub selected: Option<usize>,
    /// Pointer position in mm while it is over the canvas.
    pub cursor: Option<DVec2>,
    /// The canvas tab's rectangle in egui points; `None` while hidden.
    pub rect: Option<egui::Rect>,
    /// egui pixels per point (= window scale factor here).
    pub ppp: f32,
    /// Lines that survived culling last frame.
    pub drawn: usize,
}

impl Default for Canvas {
    fn default() -> Self {
        Self {
            view: View::default(),
            lines: Vec::new(),
            tool: Tool::Line,
            pending: None,
            hover: None,
            selected: None,
            cursor: None,
            rect: None,
            ppp: 1.0,
            drawn: 0,
        }
    }
}

pub fn setup_gizmos(mut store: ResMut<GizmoConfigStore>) {
    let (cfg, _) = store.config_mut::<DefaultGizmoConfigGroup>();
    cfg.line.width = 1.0;
    let (cfg, _) = store.config_mut::<Thick>();
    cfg.line.width = 2.5;
}

/// Snaps a screen coordinate to the centre of a physical pixel, so 1 px grid
/// lines stay crisp instead of smearing over two pixels. `half` is half the
/// canvas size in physical pixels.
fn snap(s: f64, half: f64, ppp: f64) -> f64 {
    ((s * ppp + half).floor() + 0.5 - half) / ppp
}

fn v2(p: DVec2) -> Vec2 {
    p.as_vec2()
}

pub fn draw(
    mut canvas: ResMut<Canvas>,
    mut bench: ResMut<crate::ui::Bench>,
    mut g: Gizmos,
    mut thick: Gizmos<Thick>,
) {
    if canvas.rect.is_none() {
        return;
    }
    let started = bevy::platform::time::Instant::now();
    let v = canvas.view;
    let ppp = f64::from(canvas.ppp);
    let half_px = (v.size * ppp).round() * 0.5;
    let half = (v.size * 0.5).as_vec2();
    let (min, max) = v.visible();

    // Adaptive grid.
    for (level, lvl) in view::grid_levels(v.px_per_mm).iter().enumerate() {
        let color = Color::srgba(GRID.0, GRID.1, GRID.2, lvl.alpha);
        let range = |lo: f64, hi: f64| {
            let (a, b) = (
                (lo / lvl.step).floor() as i64,
                (hi / lvl.step).ceil() as i64,
            );
            // Bounded by the grid's minimum pixel spacing; the guard is for NaN/huge zooms.
            a..=if b - a > 4096 { a - 1 } else { b }
        };
        for i in range(min.x, max.x) {
            if i == 0 || (level == 0 && i % 10 == 0) {
                continue;
            }
            let x = snap(
                v.to_screen(DVec2::new(i as f64 * lvl.step, 0.0)).x,
                half_px.x,
                ppp,
            ) as f32;
            g.line_2d(Vec2::new(x, -half.y), Vec2::new(x, half.y), color);
        }
        for i in range(min.y, max.y) {
            if i == 0 || (level == 0 && i % 10 == 0) {
                continue;
            }
            let y = snap(
                v.to_screen(DVec2::new(0.0, i as f64 * lvl.step)).y,
                half_px.y,
                ppp,
            ) as f32;
            g.line_2d(Vec2::new(-half.x, y), Vec2::new(half.x, y), color);
        }
    }
    // Axes through the origin.
    let o = v.to_screen(DVec2::ZERO);
    if (min.y..=max.y).contains(&0.0) {
        let y = snap(o.y, half_px.y, ppp) as f32;
        g.line_2d(Vec2::new(-half.x, y), Vec2::new(half.x, y), AXIS_X);
    }
    if (min.x..=max.x).contains(&0.0) {
        let x = snap(o.x, half_px.x, ppp) as f32;
        g.line_2d(Vec2::new(x, -half.y), Vec2::new(x, half.y), AXIS_Y);
    }

    // Geometry, clipped in f64 (a few px of margin) before the f32 cast.
    let margin = DVec2::splat(4.0 / v.px_per_mm);
    let (cmin, cmax) = (min - margin, max + margin);
    let clip = |l: &Line| {
        view::clip_segment(l.a, l.b, cmin, cmax)
            .map(|(a, b)| (v2(v.to_screen(a)), v2(v.to_screen(b))))
    };
    let mut drawn = 0;
    for (i, l) in canvas.lines.iter().enumerate() {
        if Some(i) == canvas.hover || Some(i) == canvas.selected {
            continue;
        }
        if let Some((a, b)) = clip(l) {
            g.line_2d(a, b, LINE);
            drawn += 1;
        }
    }
    for (idx, color) in [(canvas.selected, SELECTED), (canvas.hover, HOVER)] {
        if let Some((a, b)) = idx.and_then(|i| canvas.lines.get(i)).and_then(clip) {
            thick.line_2d(a, b, color);
            drawn += 1;
        }
    }

    // The line being drawn.
    if let Some(p) = canvas.pending {
        let a = v.to_screen(p);
        if let Some(c) = canvas.cursor {
            let line = Line { a: p, b: c };
            if let Some((a, b)) = clip(&line) {
                thick.line_2d(a, b, RUBBER);
            }
        }
        if (a.abs() - v.size * 0.5).max_element() < 10.0 {
            thick.circle_2d(v2(a), 4.0, RUBBER);
        }
    }
    canvas.drawn = drawn;
    bench.record_draw(started.elapsed().as_secs_f64() * 1000.0);
}

#[cfg(test)]
mod tests {
    use super::snap;

    #[test]
    fn snapping_lands_on_pixel_centres() {
        // 1.5x scale, 801 physical px wide: centre at 400.5 physical px.
        let (ppp, half) = (1.5, 400.5);
        for s in [-123.456, 0.0, 0.1, 77.7] {
            let phys = snap(s, half, ppp) * ppp + half;
            assert!((phys.fract() - 0.5).abs() < 1e-9);
            assert!((snap(s, half, ppp) - s).abs() <= 1.0 / ppp);
        }
    }
}
