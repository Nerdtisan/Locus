// SPDX-License-Identifier: AGPL-3.0-or-later
//! The egui side: the dock (canvas, properties, layers stub, benchmark), the
//! status bar, canvas input and the ruler labels. egui owns all pointer input,
//! so a floating panel or a menu over the canvas blocks it for free.

use std::collections::VecDeque;

use bevy::math::DVec2;
use bevy::platform::time::Instant;
use bevy::prelude::*;
use bevy::window::{PresentMode, PrimaryWindow};
use bevy::winit::WinitSettings;
use bevy_egui::egui::{self, Color32, PointerButton};
use bevy_egui::EguiContexts;
use egui_dock::{DockArea, DockState, NodeIndex, Style, TabViewer};

use crate::canvas::{Canvas, Tool};
use crate::doc::{self, Line};
use crate::view;

/// Hit-test tolerance, in logical pixels.
const PICK_PX: f64 = 6.0;
/// Frame-time samples kept for the benchmark statistics.
const SAMPLES: usize = 600;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Canvas,
    Properties,
    Layers,
    Bench,
}

/// A layer row in the layers stub (SPEC §4 layers are out of scope for M0).
#[derive(Debug)]
pub struct Layer {
    name: &'static str,
    op: &'static str,
    color: Color32,
    visible: bool,
}

#[derive(Resource)]
pub struct Dock {
    state: DockState<Tab>,
    layers: Vec<Layer>,
}

impl Default for Dock {
    fn default() -> Self {
        let mut state = DockState::new(vec![Tab::Canvas]);
        let surface = state.main_surface_mut();
        let [_, right] =
            surface.split_right(NodeIndex::root(), 0.76, vec![Tab::Properties, Tab::Bench]);
        surface.split_below(right, 0.6, vec![Tab::Layers]);
        let layer = |name, op, color| Layer {
            name,
            op,
            color,
            visible: true,
        };
        Self {
            state,
            layers: vec![
                layer("Cut", "cut", Color32::from_rgb(230, 70, 70)),
                layer("Score", "score", Color32::from_rgb(70, 140, 230)),
                layer("Engrave", "engrave", Color32::from_rgb(90, 190, 90)),
            ],
        }
    }
}

/// A rolling window of timing samples, in ms.
#[derive(Debug, Default)]
pub struct Series(VecDeque<f64>);

impl Series {
    fn push(&mut self, ms: f64) {
        self.0.push_back(ms);
        while self.0.len() > SAMPLES {
            self.0.pop_front();
        }
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// (mean, p95) in ms.
    pub fn stats(&self) -> Option<(f64, f64)> {
        let n = self.0.len();
        if n == 0 {
            return None;
        }
        let mean = self.0.iter().sum::<f64>() / n as f64;
        let mut sorted: Vec<f64> = self.0.iter().copied().collect();
        sorted.sort_by(f64::total_cmp);
        Some((mean, sorted[((n as f64 * 0.95) as usize).min(n - 1)]))
    }
}

/// Benchmark mode and timing samples.
#[derive(Resource, Debug, Default)]
pub struct Bench {
    /// Continuous redraw with vsync off (native) while measuring.
    pub continuous: bool,
    applied: bool,
    /// Animate the view (pan +-20/+-10 mm, zoom +-25 %) so every frame is a
    /// new transform: the same motion as spike A's benchmark.
    pub auto_pan: bool,
    motion_origin: Option<(DVec2, f64)>,
    motion_frame: u64,
    /// Wall-clock interval between frames.
    pub interval: Series,
    /// Main-schedule CPU time (First to Last). Natively the render world runs
    /// in parallel (pipelined rendering) and is not included.
    pub update: Series,
    /// Our scene build only (the gizmo `draw` system).
    pub draw: Series,
    frame_start: Option<Instant>,
    /// Skip the first interval after switching modes (it spans the switch).
    skip: bool,
}

impl Bench {
    /// Drops the samples collected so far (e.g. after a warm-up).
    pub fn reset(&mut self) {
        self.interval = Series::default();
        self.update = Series::default();
        self.draw = Series::default();
    }

    fn recording(&self) -> bool {
        self.applied && !self.skip
    }

    pub fn record_draw(&mut self, ms: f64) {
        if self.recording() {
            self.draw.push(ms);
        }
    }
}

/// View animation for frame `k`: a 600-frame cycle around `origin`.
fn motion(origin: (DVec2, f64), k: u64) -> (DVec2, f64) {
    let t = (k % 600) as f64 / 600.0 * std::f64::consts::TAU;
    let centre = origin.0 + DVec2::new(20.0 * t.sin(), 10.0 * (2.0 * t).sin());
    (centre, origin.1 * (1.0 + 0.25 * t.cos()))
}

/// Start of the main schedule.
pub fn frame_begin(mut bench: ResMut<Bench>) {
    bench.frame_start = Some(Instant::now());
}

/// End of the main schedule.
pub fn frame_end(mut bench: ResMut<Bench>) {
    if let Some(s) = bench.frame_start.take() {
        if bench.recording() {
            bench.update.push(s.elapsed().as_secs_f64() * 1000.0);
        }
    }
}

/// Applies the benchmark mode, records frame intervals and drives the motion.
pub fn bench_tick(
    time: Res<Time<Real>>,
    mut bench: ResMut<Bench>,
    mut canvas: ResMut<Canvas>,
    mut winit: ResMut<WinitSettings>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
) {
    let continuous = bench.continuous || bench.auto_pan;
    if continuous != bench.applied {
        bench.applied = continuous;
        bench.reset();
        bench.skip = true;
        *winit = if continuous {
            WinitSettings::game()
        } else {
            WinitSettings::desktop_app()
        };
        if let Ok(mut w) = windows.single_mut() {
            w.present_mode = if continuous {
                PresentMode::AutoNoVsync
            } else {
                PresentMode::AutoVsync
            };
        }
    }
    if bench.auto_pan {
        let origin = *bench
            .motion_origin
            .get_or_insert((canvas.view.center, canvas.view.px_per_mm));
        let (centre, ppm) = motion(origin, bench.motion_frame);
        bench.motion_frame += 1;
        canvas.view.center = centre;
        canvas.view.px_per_mm = ppm;
        canvas.view.clamp_zoom();
    } else if let Some((centre, ppm)) = bench.motion_origin.take() {
        canvas.view.center = centre;
        canvas.view.px_per_mm = ppm;
        bench.motion_frame = 0;
    }
    if !continuous {
        return;
    }
    if std::mem::take(&mut bench.skip) {
        return;
    }
    bench.interval.push(time.delta_secs_f64() * 1000.0);
}

pub fn ui(
    mut contexts: EguiContexts,
    mut canvas: ResMut<Canvas>,
    mut dock: ResMut<Dock>,
    mut bench: ResMut<Bench>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    egui::TopBottomPanel::bottom("status").show(ctx, |ui| status_bar(ui, &canvas));
    canvas.rect = None;
    let Dock { state, layers } = &mut *dock;
    let style = Style::from_egui(ctx.style().as_ref());
    DockArea::new(state)
        .style(style)
        .show_close_buttons(false)
        .show_add_buttons(false)
        .show_leaf_close_all_buttons(false)
        .show_leaf_collapse_buttons(false)
        .show(
            ctx,
            &mut Viewer {
                canvas: &mut canvas,
                bench: &mut bench,
                layers,
            },
        );
    Ok(())
}

struct Viewer<'a> {
    canvas: &'a mut Canvas,
    bench: &'a mut Bench,
    layers: &'a mut Vec<Layer>,
}

impl TabViewer for Viewer<'_> {
    type Tab = Tab;

    fn title(&mut self, tab: &mut Tab) -> egui::WidgetText {
        match tab {
            Tab::Canvas => "Canvas",
            Tab::Properties => "Properties",
            Tab::Layers => "Layers",
            Tab::Bench => "Benchmark",
        }
        .into()
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Tab) {
        match tab {
            Tab::Canvas => canvas_ui(ui, self.canvas),
            Tab::Properties => properties_ui(ui, self.canvas),
            Tab::Layers => layers_ui(ui, self.layers),
            Tab::Bench => bench_ui(ui, self.canvas, self.bench),
        }
    }

    fn closeable(&mut self, tab: &mut Tab) -> bool {
        *tab != Tab::Canvas
    }

    /// The canvas renders through a camera viewport in the main window, so
    /// it can't float in an egui window (the panels can).
    fn allowed_in_windows(&self, tab: &mut Tab) -> bool {
        *tab != Tab::Canvas
    }

    /// The canvas tab is see-through: Bevy renders under it.
    fn clear_background(&self, tab: &Tab) -> bool {
        *tab != Tab::Canvas
    }

    fn scroll_bars(&self, tab: &Tab) -> [bool; 2] {
        if *tab == Tab::Canvas {
            [false, false]
        } else {
            [false, true]
        }
    }
}

/// Decimal places that resolve `step` mm.
fn decimals(step: f64) -> usize {
    (-step.log10()).ceil().clamp(0.0, 9.0) as usize
}

fn fmt_mm(v: f64, step: f64) -> String {
    let d = decimals(step);
    let s = format!("{v:.d$}");
    // No "-0.00".
    if s.trim_start_matches('-')
        .chars()
        .all(|c| c == '0' || c == '.')
    {
        s.trim_start_matches('-').to_owned()
    } else {
        s
    }
}

fn canvas_ui(ui: &mut egui::Ui, c: &mut Canvas) {
    let rect = ui.max_rect();
    c.rect = Some(rect);
    c.ppp = ui.ctx().pixels_per_point();
    c.view.size = DVec2::new(f64::from(rect.width()), f64::from(rect.height()));
    c.view.clamp_zoom();

    let resp = ui.interact(
        rect,
        ui.id().with("locus_canvas"),
        egui::Sense::click_and_drag(),
    );
    let centre = rect.center();
    let to_screen =
        |p: egui::Pos2| DVec2::new(f64::from(p.x - centre.x), f64::from(centre.y - p.y));

    if resp.dragged_by(PointerButton::Middle) || resp.dragged_by(PointerButton::Secondary) {
        let d = resp.drag_delta();
        c.view.pan(DVec2::new(f64::from(d.x), -f64::from(d.y)));
    }

    if let Some(p) = resp.hover_pos() {
        let s = to_screen(p);
        let (scroll, pinch) = ui.input(|i| (i.raw_scroll_delta.y, i.zoom_delta()));
        // Ctrl+wheel and pinch arrive as zoom_delta; a plain wheel as scroll.
        let factor = if pinch != 1.0 {
            f64::from(pinch)
        } else {
            (f64::from(scroll) * 0.0025).exp()
        };
        if factor != 1.0 {
            c.view.zoom_about(s, factor);
        }
        let w = c.view.to_world(s);
        c.cursor = Some(w);
        c.hover = if c.pending.is_none() {
            doc::hit_test(&c.lines, w, PICK_PX / c.view.px_per_mm)
        } else {
            None
        };
        if c.tool == Tool::Line {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        }
    } else {
        c.cursor = None;
        c.hover = None;
    }

    if resp.clicked_by(PointerButton::Primary) {
        // Where the button was released, not where the pointer is now: several
        // input events can land in one frame (seen on the web build), and the
        // pointer may have moved on since the click.
        let release = ui.input(|i| {
            i.events.iter().rev().find_map(|e| match e {
                egui::Event::PointerButton {
                    pos,
                    button: PointerButton::Primary,
                    pressed: false,
                    ..
                } => Some(*pos),
                _ => None,
            })
        });
        if let Some(p) = release.map(|r| c.view.to_world(to_screen(r))).or(c.cursor) {
            match c.tool {
                Tool::Select => {
                    c.selected = doc::hit_test(&c.lines, p, PICK_PX / c.view.px_per_mm);
                }
                Tool::Line => {
                    if let Some(a) = c.pending {
                        // Ignore a zero-length line (double click on one spot).
                        if (p - a).length() * c.view.px_per_mm > 0.5 {
                            c.lines.push(Line { a, b: p });
                        }
                    }
                    // Chained: the end of one line starts the next.
                    c.pending = Some(p);
                }
            }
        }
    }
    if resp.clicked_by(PointerButton::Secondary) {
        c.pending = None;
    }
    if !ui.ctx().wants_keyboard_input() {
        let (esc, l, s, del, f) = ui.input(|i| {
            (
                i.key_pressed(egui::Key::Escape),
                i.key_pressed(egui::Key::L),
                i.key_pressed(egui::Key::S),
                i.key_pressed(egui::Key::Delete),
                i.key_pressed(egui::Key::F),
            )
        });
        // Esc ends the line chain; a second Esc drops to Select.
        if esc && c.pending.take().is_none() {
            c.tool = Tool::Select;
            c.selected = None;
        }
        if l {
            c.tool = Tool::Line;
        }
        if s {
            c.tool = Tool::Select;
            c.pending = None;
        }
        if del {
            delete_selected(c);
        }
        if f {
            fit_all(c);
        }
    }

    rulers(ui, c, rect);
}

/// Major-grid labels along the top and left edges, drawn by egui over Bevy.
fn rulers(ui: &egui::Ui, c: &Canvas, rect: egui::Rect) {
    let painter = ui.painter_at(rect);
    let step = view::grid_levels(c.view.px_per_mm)[1].step;
    let (min, max) = c.view.visible();
    let font = egui::FontId::monospace(11.0);
    let color = Color32::from_gray(150);
    let centre = rect.center();
    let range = |lo: f64, hi: f64| (lo / step).ceil() as i64..=(hi / step).floor() as i64;
    let ri = range(min.x, max.x);
    if ri.end() - ri.start() < 200 {
        for i in ri {
            let x = i as f64 * step;
            let sx = centre.x + c.view.to_screen(DVec2::new(x, 0.0)).x as f32;
            painter.text(
                egui::pos2(sx + 3.0, rect.top() + 2.0),
                egui::Align2::LEFT_TOP,
                fmt_mm(x, step),
                font.clone(),
                color,
            );
        }
    }
    let ri = range(min.y, max.y);
    if ri.end() - ri.start() < 200 {
        for i in ri {
            let y = i as f64 * step;
            let sy = centre.y - c.view.to_screen(DVec2::new(0.0, y)).y as f32;
            painter.text(
                egui::pos2(rect.left() + 3.0, sy - 2.0),
                egui::Align2::LEFT_BOTTOM,
                fmt_mm(y, step),
                font.clone(),
                color,
            );
        }
    }
}

fn delete_selected(c: &mut Canvas) {
    if let Some(i) = c.selected.take() {
        if i < c.lines.len() {
            c.lines.remove(i);
        }
        c.hover = None;
    }
}

pub fn fit_all(c: &mut Canvas) {
    let mut it = c.lines.iter().flat_map(|l| [l.a, l.b]);
    if let Some(first) = it.next() {
        let (min, max) = it.fold((first, first), |(lo, hi), p| (lo.min(p), hi.max(p)));
        c.view.fit(min, max);
    } else {
        c.view.center = DVec2::ZERO;
        c.view.set_extent(100.0);
    }
}

fn status_bar(ui: &mut egui::Ui, c: &Canvas) {
    ui.horizontal(|ui| {
        let px = 1.0 / c.view.px_per_mm;
        ui.label(match c.tool {
            Tool::Select => "Select",
            Tool::Line => "Line",
        });
        ui.separator();
        match c.cursor {
            Some(p) => ui.monospace(format!("x {}  y {} mm", fmt_mm(p.x, px), fmt_mm(p.y, px))),
            None => ui.monospace("x —  y —"),
        };
        ui.separator();
        let (min, max) = c.view.visible();
        let ext = max - min;
        ui.monospace(format!(
            "view {} × {} mm",
            fmt_mm(ext.x, ext.x / 1000.0),
            fmt_mm(ext.y, ext.y / 1000.0)
        ));
        ui.separator();
        ui.label(format!("{} lines", c.lines.len()));
        ui.separator();
        ui.weak("L line · S select · Esc cancel · Del delete · F fit · wheel zoom · middle/right-drag pan");
    });
}

fn properties_ui(ui: &mut egui::Ui, c: &mut Canvas) {
    ui.horizontal(|ui| {
        ui.label("Tool");
        ui.selectable_value(&mut c.tool, Tool::Select, "Select");
        ui.selectable_value(&mut c.tool, Tool::Line, "Line");
    });
    ui.separator();
    let px = 1.0 / c.view.px_per_mm;
    let Some(i) = c.selected.filter(|&i| i < c.lines.len()) else {
        ui.weak("Nothing selected. Select tool + click a line.");
        return;
    };
    ui.strong(format!("Line #{i}"));
    let d = decimals(px).max(3);
    // Editing here bypasses a command API on purpose: the spike has no
    // document crate yet (SPEC §11 applies from M1).
    let line = &mut c.lines[i];
    egui::Grid::new("line_props").num_columns(3).show(ui, |ui| {
        for (name, p) in [("Start", &mut line.a), ("End", &mut line.b)] {
            ui.label(name);
            ui.add(
                egui::DragValue::new(&mut p.x)
                    .speed(px)
                    .max_decimals(d)
                    .suffix(" mm"),
            );
            ui.add(
                egui::DragValue::new(&mut p.y)
                    .speed(px)
                    .max_decimals(d)
                    .suffix(" mm"),
            );
            ui.end_row();
        }
    });
    let dir = line.b - line.a;
    ui.label(format!("Length  {} mm", fmt_mm(line.length(), px)));
    ui.label(format!("Angle   {:.3}°", dir.y.atan2(dir.x).to_degrees()));
    if ui.button("Delete").clicked() {
        delete_selected(c);
    }
}

fn layers_ui(ui: &mut egui::Ui, layers: &mut [Layer]) {
    ui.weak("Stub: layers and operations arrive in M2 (SPEC §4, §7).");
    for l in layers {
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
            ui.painter().rect_filled(r, 2.0, l.color);
            ui.checkbox(&mut l.visible, l.name);
            ui.weak(l.op);
        });
    }
}

fn bench_ui(ui: &mut egui::Ui, c: &mut Canvas, b: &mut Bench) {
    ui.horizontal_wrapped(|ui| {
        if ui.button("Add 5 000 lines").clicked() {
            c.lines.extend(doc::stress_lines(5000, doc::STRESS_SEED));
            fit_all(c);
        }
        if ui.button("Clear").clicked() {
            c.lines.clear();
            c.selected = None;
            c.hover = None;
            c.pending = None;
        }
        if ui.button("Fit").clicked() {
            fit_all(c);
        }
    });
    ui.horizontal(|ui| {
        ui.label("Extent");
        if ui.button("0.001 mm").clicked() {
            c.view.set_extent(view::MIN_EXTENT_MM);
        }
        if ui.button("100 mm").clicked() {
            c.view.set_extent(100.0);
        }
        if ui.button("2 m").clicked() {
            c.view.px_per_mm = 0.0;
            c.view.clamp_zoom();
        }
    });
    ui.separator();
    ui.checkbox(&mut b.continuous, "Continuous redraw, vsync off");
    ui.checkbox(&mut b.auto_pan, "Auto-pan (implies continuous)");
    match b.interval.stats() {
        Some((mean, p95)) => {
            ui.monospace(format!("frame  mean {mean:.2} ms  p95 {p95:.2} ms"));
            ui.monospace(format!(
                "       {:.0} fps  ({} frames)",
                1000.0 / mean,
                b.interval.len()
            ));
            for (name, series) in [("update", &b.update), ("draw  ", &b.draw)] {
                if let Some((mean, p95)) = series.stats() {
                    ui.monospace(format!("{name} mean {mean:.2} ms  p95 {p95:.2} ms"));
                }
            }
        }
        None => {
            ui.weak("Turn on continuous redraw to measure.");
        }
    }
    ui.separator();
    let (min, max) = c.view.visible();
    ui.monospace(format!("lines  {} ({} drawn)", c.lines.len(), c.drawn));
    ui.monospace(format!("zoom   {:.6e} px/mm", c.view.px_per_mm));
    ui.monospace(format!("view   {:.6e} mm wide", max.x - min.x));
    ui.monospace(format!("scale  {:.2} px/pt", c.ppp));
    ui.monospace(format!(
        "build  {} {}",
        std::env::consts::ARCH,
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        }
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mm_formatting_follows_the_step() {
        assert_eq!(fmt_mm(12.0, 10.0), "12");
        assert_eq!(fmt_mm(0.125, 0.001), "0.125");
        assert_eq!(fmt_mm(-0.0000001, 0.01), "0.00");
        assert_eq!(fmt_mm(-3.5, 0.1), "-3.5");
    }

    #[test]
    fn p95_of_samples() {
        let mut s = Series::default();
        (1..=100).for_each(|i| s.push(f64::from(i)));
        assert_eq!(s.stats(), Some((50.5, 96.0)));
        assert_eq!(s.len(), 100);
    }

    #[test]
    fn motion_cycles_back_to_the_origin() {
        let o = (DVec2::new(3.0, 4.0), 2.0);
        let (c, z) = motion(o, 600);
        assert!((c - o.0).length() < 1e-9);
        assert!((z - 2.5).abs() < 1e-9);
    }
}
