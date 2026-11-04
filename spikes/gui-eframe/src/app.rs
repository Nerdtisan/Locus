// SPDX-License-Identifier: AGPL-3.0-or-later
//! The spike app: an egui_dock layout around the canvas, plus a scripted
//! benchmark so both GUI spikes report the same numbers (#44, #46).

use crate::doc::Doc;
use crate::gpu::GpuLines;
use crate::render::{self, Renderer};
use crate::view::{View, V2};
use crate::{grid, Config};
use egui::{Color32, PointerButton, Sense, Stroke};
use egui_dock::{DockArea, DockState, NodeIndex, TabViewer};
use web_time::Instant;

/// Lines in the benchmark scene (issue #44).
pub const BENCH_LINES: usize = 5000;
const BENCH_WARMUP: u32 = 30;
const BENCH_FRAMES: u32 = 600;
/// Hit-test tolerance in screen points.
const HIT_TOL_PT: f64 = 5.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tab {
    Canvas,
    Properties,
    Layers,
    Stats,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tool {
    Select,
    Line,
}

/// Rolling window of per-frame samples, in milliseconds.
#[derive(Debug, Default)]
struct Series(Vec<f32>);

impl Series {
    const CAP: usize = 240;
    fn push(&mut self, v: f32) {
        if self.0.len() == Self::CAP {
            self.0.remove(0);
        }
        self.0.push(v);
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Summary {
    pub mean: f32,
    pub p95: f32,
}

pub fn summarise(v: &[f32]) -> Summary {
    if v.is_empty() {
        return Summary {
            mean: 0.0,
            p95: 0.0,
        };
    }
    let mut s = v.to_vec();
    s.sort_by(f32::total_cmp);
    let idx = ((s.len() as f32 * 0.95).ceil() as usize).clamp(1, s.len()) - 1;
    Summary {
        mean: s.iter().sum::<f32>() / s.len() as f32,
        p95: s[idx],
    }
}

#[derive(Debug, Clone)]
struct BenchResult {
    renderer: Renderer,
    frame: Summary,
    cpu: Summary,
    scene: Summary,
}

#[derive(Debug)]
struct Bench {
    idx: usize,
    frame: u32,
    base: View,
    dt: Vec<f32>,
    cpu: Vec<f32>,
    scene: Vec<f32>,
    results: Vec<BenchResult>,
}

struct State {
    cfg: Config,
    doc: Doc,
    view: View,
    /// Keep "fit all" applied, re-fitting when the canvas resizes, until
    /// the user pans or zooms. A one-shot fit on the first frame is wrong
    /// on the web, where the canvas is not at its final size yet.
    auto_fit: bool,
    fit_size: egui::Vec2,
    /// Frames the canvas size has been unchanged; the benchmark waits for it.
    stable_frames: u32,
    tool: Tool,
    renderer: Renderer,
    pending: Option<V2>,
    hover: Option<usize>,
    selected: Option<usize>,
    cursor_world: Option<V2>,
    gpu: Option<GpuLines>,
    adapter: String,
    // Stats
    frame_ms: Series,
    cpu_ms: Series,
    scene_ms: Series,
    hit_ms: Series,
    drawn: usize,
    bench: Option<Bench>,
    last_results: Vec<BenchResult>,
    report: String,
    /// Last canvas rectangle, so scripted screenshots can set up the view.
    canvas_rect: egui::Rect,
    /// Scripted screenshots override the pointer: there is none to hover.
    forced_cursor: Option<V2>,
    forced_hover: Option<usize>,
    #[cfg(not(target_arch = "wasm32"))]
    shots: Option<shots::Runner>,
}

pub struct App {
    dock: DockState<Tab>,
    s: State,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, cfg: Config) -> Self {
        let (gpu, adapter) = match cc.wgpu_render_state.as_ref() {
            Some(rs) => {
                crate::gpu::init(rs, cfg.msaa);
                let info = rs.adapter.get_info();
                (
                    Some(GpuLines::default()),
                    format!(
                        "{:?} · {} · {:?}",
                        info.backend, info.name, rs.target_format
                    ),
                )
            }
            None => (None, "no wgpu render state".to_owned()),
        };

        let mut dock = DockState::new(vec![Tab::Canvas]);
        let tree = dock.main_surface_mut();
        let [_canvas, right] = tree.split_right(NodeIndex::root(), 0.75, vec![Tab::Properties]);
        tree.split_below(right, 0.45, vec![Tab::Layers, Tab::Stats]);

        let mut doc = Doc::default();
        doc.generate(BENCH_LINES);
        let mut s = State {
            cfg,
            doc,
            view: View::default(),
            auto_fit: true,
            fit_size: egui::Vec2::ZERO,
            stable_frames: 0,
            tool: Tool::Line,
            renderer: Renderer::Painter,
            pending: None,
            hover: None,
            selected: None,
            cursor_world: None,
            gpu,
            adapter,
            frame_ms: Series::default(),
            cpu_ms: Series::default(),
            scene_ms: Series::default(),
            hit_ms: Series::default(),
            drawn: 0,
            bench: None,
            last_results: Vec::new(),
            report: String::new(),
            canvas_rect: egui::Rect::NOTHING,
            forced_cursor: None,
            forced_hover: None,
            #[cfg(not(target_arch = "wasm32"))]
            shots: None,
        };
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(dir) = s.cfg.shot_dir.clone() {
            s.shots = Some(shots::Runner::new(dir));
        }
        if s.cfg.bench {
            s.start_bench();
        }
        Self { dock, s }
    }
}

impl State {
    fn renderers(&self) -> Vec<Renderer> {
        Renderer::ALL
            .into_iter()
            .filter(|r| *r != Renderer::Wgpu || self.gpu.is_some())
            .collect()
    }

    fn start_bench(&mut self) {
        self.doc.generate(BENCH_LINES);
        self.auto_fit = true;
        self.fit_size = egui::Vec2::ZERO;
        self.selected = None;
        self.pending = None;
        self.bench = Some(Bench {
            idx: 0,
            frame: 0,
            base: self.view,
            dt: Vec::new(),
            cpu: Vec::new(),
            scene: Vec::new(),
            results: Vec::new(),
        });
    }

    /// Advance the benchmark by one frame. The view sweeps a pan + zoom so
    /// every frame redraws all 5 000 lines at a new transform.
    fn bench_step(&mut self, dt_ms: f32, cpu_ms: Option<f32>, scene_ms: f32) {
        let renderers = self.renderers();
        let Some(b) = self.bench.as_mut() else {
            return;
        };
        if b.frame == 0 && b.idx == 0 {
            // Start only once the layout has settled (web start-up resizes).
            if self.stable_frames < 10 {
                return;
            }
            b.base = self.view;
        }
        let r = renderers[b.idx];
        self.renderer = r;
        let t = b.frame as f64 / BENCH_FRAMES as f64 * std::f64::consts::TAU;
        self.view.center = b.base.center.add(V2::new(t.sin() * 20.0, t.cos() * 10.0));
        self.view.pt_per_mm = b.base.pt_per_mm * (1.0 + 0.25 * (2.0 * t).sin());

        if b.frame > BENCH_WARMUP {
            b.dt.push(dt_ms);
            if let Some(c) = cpu_ms {
                b.cpu.push(c);
            }
            b.scene.push(scene_ms);
        }
        b.frame += 1;
        if b.frame > BENCH_WARMUP + BENCH_FRAMES {
            b.results.push(BenchResult {
                renderer: r,
                frame: summarise(&b.dt),
                cpu: summarise(&b.cpu),
                scene: summarise(&b.scene),
            });
            b.frame = 0;
            b.dt.clear();
            b.cpu.clear();
            b.scene.clear();
            b.idx += 1;
            if b.idx == renderers.len() {
                self.view = b.base;
                self.last_results = std::mem::take(&mut b.results);
                self.bench = None;
                self.report = self.format_report();
                crate::emit_report(&self.report);
            }
        }
    }

    fn format_report(&self) -> String {
        let mut s = format!(
            "spike-eframe benchmark · {} lines · {} frames/renderer · vsync {} · msaa {}\nadapter: {}\n\
             | renderer | frame interval mean / p95 (ms) | eframe CPU mean / p95 (ms) | scene build mean / p95 (ms) |\n\
             |---|---|---|---|\n",
            BENCH_LINES,
            BENCH_FRAMES,
            if self.cfg.vsync { "on" } else { "off" },
            self.cfg.msaa,
            self.adapter
        );
        for r in &self.last_results {
            s += &format!(
                "| {} | {:.2} / {:.2} | {:.2} / {:.2} | {:.2} / {:.2} |\n",
                r.renderer.name(),
                r.frame.mean,
                r.frame.p95,
                r.cpu.mean,
                r.cpu.p95,
                r.scene.mean,
                r.scene.p95
            );
        }
        s
    }

    fn canvas_ui(&mut self, ui: &mut egui::Ui) -> f32 {
        let (resp, painter) = ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
        let rect = resp.rect;
        self.canvas_rect = rect;
        let dark = ui.visuals().dark_mode;
        painter.rect_filled(
            rect,
            0.0,
            if dark {
                Color32::from_gray(24)
            } else {
                Color32::from_gray(250)
            },
        );

        if rect.size() == self.fit_size {
            self.stable_frames = self.stable_frames.saturating_add(1);
        } else {
            self.stable_frames = 0;
        }
        let bench_running = self
            .bench
            .as_ref()
            .is_some_and(|b| b.idx > 0 || b.frame > 0);
        if self.auto_fit && !bench_running && rect.size() != self.fit_size && rect.width() > 10.0 {
            if let Some((lo, hi)) = self.doc.bounds() {
                self.view.fit(lo, hi, rect);
            }
        }
        self.fit_size = rect.size();

        // ── Input: zoom about cursor, middle/right-drag pan ─────────────────
        let hover_pos = resp.hover_pos();
        if let Some(hp) = hover_pos {
            let (scroll, zoom) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
            let factor = zoom as f64 * (scroll as f64 * 0.0025).exp();
            if factor != 1.0 && self.bench.is_none() {
                self.view.zoom_about(hp, factor, rect);
                self.auto_fit = false;
            }
        }
        if resp.dragged_by(PointerButton::Middle) || resp.dragged_by(PointerButton::Secondary) {
            self.view.pan_points(resp.drag_delta());
            self.auto_fit = false;
        }

        self.cursor_world = hover_pos.map(|p| self.view.to_world(p, rect));
        if self.forced_cursor.is_some() {
            self.cursor_world = self.forced_cursor;
        }

        // ── Hit-test (brute force over all lines) ───────────────────────────
        let t0 = Instant::now();
        self.hover = self
            .cursor_world
            .and_then(|w| self.doc.hit_test(w, HIT_TOL_PT / self.view.pt_per_mm));
        self.hit_ms.push(t0.elapsed().as_secs_f32() * 1000.0);
        if self.forced_hover.is_some() {
            self.hover = self.forced_hover;
        }

        // ── Clicks ──────────────────────────────────────────────────────────
        if resp.clicked() {
            if let Some(w) = self.cursor_world {
                match self.tool {
                    Tool::Line => {
                        // Click-click; chains until Esc or right-click.
                        if let Some(a) = self.pending {
                            self.doc.add_line(a, w);
                        }
                        self.pending = Some(w);
                    }
                    Tool::Select => self.selected = self.hover,
                }
            }
        }
        if resp.secondary_clicked() {
            self.pending = None;
        }
        ui.input(|i| {
            if i.key_pressed(egui::Key::Escape) {
                self.pending = None;
                self.selected = None;
            }
        });

        // ── Draw ────────────────────────────────────────────────────────────
        let painter = painter.with_clip_rect(rect);
        grid::draw(&painter, &self.view, rect, dark);

        let t0 = Instant::now();
        let ppp = ui.ctx().pixels_per_point();
        self.drawn = match (self.renderer, self.gpu.as_mut()) {
            (Renderer::Wgpu, Some(gpu)) => {
                gpu.paint(
                    &painter,
                    &self.doc,
                    &self.view,
                    rect,
                    ppp,
                    self.hover,
                    self.selected,
                    render::hover_color(dark),
                );
                self.doc.lines.len()
            }
            (Renderer::Lyon, _) => render::draw_lyon(&painter, &self.doc, &self.view, rect),
            _ => render::draw_painter(&painter, &self.doc, &self.view, rect),
        };
        let scene_ms = t0.elapsed().as_secs_f32() * 1000.0;

        if self.renderer != Renderer::Wgpu || self.gpu.is_none() {
            if let Some(i) = self.hover {
                let c = render::hover_color(dark);
                render::draw_highlight(&painter, &self.doc, &self.view, rect, i, c);
            }
            if let Some(i) = self.selected {
                let c = Color32::from_rgb(255, 184, 26);
                render::draw_highlight(&painter, &self.doc, &self.view, rect, i, c);
            }
        }

        // Rubber band for the line tool: a thicker preview line and a dot on the
        // start point, as spike B draws it.
        if let Some(a) = self.pending {
            let rubber = ui.visuals().strong_text_color();
            let sa = self.view.to_screen(a, rect);
            if let Some(w) = self.cursor_world {
                painter.line_segment([sa, self.view.to_screen(w, rect)], Stroke::new(2.0, rubber));
            }
            painter.circle_stroke(sa, 4.0, Stroke::new(2.0, rubber));
        }

        // Coordinate labels on the major grid lines, over the geometry (as spike B).
        grid::rulers(&painter, &self.view, rect);

        // HUD text: exercises small-text rendering over the canvas.
        let (step, _) = grid::minor_step(self.view.pt_per_mm);
        let cursor = self.cursor_world.map_or(String::new(), |w| {
            format!("x {:.4} mm  y {:.4} mm", w.x, w.y)
        });
        painter.text(
            rect.left_top() + egui::vec2(8.0, 6.0),
            egui::Align2::LEFT_TOP,
            format!(
                "extent {:.4} mm · grid {} · {}\n{}",
                self.view.extent_mm(rect),
                grid::label(step),
                self.renderer.name(),
                cursor
            ),
            egui::FontId::monospace(12.0),
            ui.visuals().text_color(),
        );
        scene_ms
    }

    fn properties_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Tool");
            ui.selectable_value(&mut self.tool, Tool::Select, "Select");
            ui.selectable_value(&mut self.tool, Tool::Line, "Line");
        });
        ui.horizontal(|ui| {
            ui.label("Renderer");
            egui::ComboBox::from_id_salt("renderer")
                .selected_text(self.renderer.name())
                .show_ui(ui, |ui| {
                    for r in self.renderers() {
                        ui.selectable_value(&mut self.renderer, r, r.name());
                    }
                });
        });
        ui.separator();
        match self.selected.and_then(|i| self.doc.lines.get(i).copied()) {
            Some(l) => {
                ui.heading("Line");
                egui::Grid::new("props").num_columns(2).show(ui, |ui| {
                    ui.label("Start");
                    ui.monospace(format!("{:.4}, {:.4}", l.a.x, l.a.y));
                    ui.end_row();
                    ui.label("End");
                    ui.monospace(format!("{:.4}, {:.4}", l.b.x, l.b.y));
                    ui.end_row();
                    ui.label("Length");
                    ui.monospace(format!("{:.4} mm", l.b.sub(l.a).len()));
                    ui.end_row();
                    ui.label("Layer");
                    ui.label(self.doc.layers.get(l.layer).map_or("?", |ly| ly.name));
                    ui.end_row();
                });
            }
            None => {
                ui.weak("Nothing selected. Use the Select tool and click a line.");
            }
        }
        ui.separator();
        ui.weak(
            "Wheel: zoom about cursor · middle/right drag: pan · \
             Line tool: click-click, Esc / right-click ends",
        );
    }

    fn layers_ui(&mut self, ui: &mut egui::Ui) {
        let mut changed = false;
        for (i, layer) in self.doc.layers.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                changed |= ui.checkbox(&mut layer.visible, "").changed();
                let (r, p) = ui.allocate_painter(egui::vec2(14.0, 14.0), Sense::hover());
                p.rect_filled(r.rect, 2.0, layer.color);
                if ui
                    .selectable_label(self.doc.active_layer == i, layer.name)
                    .clicked()
                {
                    self.doc.active_layer = i;
                }
            });
        }
        if changed {
            self.doc.touch();
        }
    }

    fn stats_ui(&mut self, ui: &mut egui::Ui) {
        let f = summarise(&self.frame_ms.0);
        let c = summarise(&self.cpu_ms.0);
        let sc = summarise(&self.scene_ms.0);
        let h = summarise(&self.hit_ms.0);
        egui::Grid::new("stats").num_columns(2).show(ui, |ui| {
            let row = |ui: &mut egui::Ui, k: &str, v: String| {
                ui.label(k);
                ui.monospace(v);
                ui.end_row();
            };
            row(
                ui,
                "Lines",
                format!("{} ({} drawn)", self.doc.lines.len(), self.drawn),
            );
            row(ui, "Frame", format!("{:.2} / p95 {:.2} ms", f.mean, f.p95));
            row(
                ui,
                "eframe CPU",
                format!("{:.2} / p95 {:.2} ms", c.mean, c.p95),
            );
            row(
                ui,
                "Scene build",
                format!("{:.2} / p95 {:.2} ms", sc.mean, sc.p95),
            );
            row(
                ui,
                "Hit-test",
                format!("{:.3} / p95 {:.3} ms", h.mean, h.p95),
            );
            if let Some(g) = &self.gpu {
                row(ui, "GPU uploads", g.uploads.to_string());
            }
        });
        ui.small(&self.adapter);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(self.bench.is_none(), egui::Button::new("Run benchmark"))
                .clicked()
            {
                self.start_bench();
            }
            if ui.button("5 000 lines").clicked() {
                self.doc.generate(BENCH_LINES);
                self.selected = None;
            }
            if ui.button("Clear").clicked() {
                self.doc.lines.clear();
                self.doc.touch();
                self.selected = None;
            }
        });
        if let Some(b) = &self.bench {
            ui.label(format!(
                "Benchmarking {} … frame {}",
                self.renderers()[b.idx].name(),
                b.frame
            ));
        }
        if !self.report.is_empty() {
            ui.separator();
            ui.add(egui::Label::new(egui::RichText::new(&self.report).monospace().small()).wrap());
        }
    }
}

struct Viewer<'a> {
    s: &'a mut State,
    scene_ms: f32,
}

impl TabViewer for Viewer<'_> {
    type Tab = Tab;

    fn title(&mut self, tab: &mut Tab) -> egui::WidgetText {
        match tab {
            Tab::Canvas => "Canvas",
            Tab::Properties => "Properties",
            Tab::Layers => "Layers",
            Tab::Stats => "Stats",
        }
        .into()
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Tab) {
        match tab {
            Tab::Canvas => self.scene_ms = self.s.canvas_ui(ui),
            Tab::Properties => self.s.properties_ui(ui),
            Tab::Layers => self.s.layers_ui(ui),
            Tab::Stats => self.s.stats_ui(ui),
        }
    }

    fn is_closeable(&self, tab: &Tab) -> bool {
        *tab != Tab::Canvas
    }

    fn scroll_bars(&self, tab: &Tab) -> [bool; 2] {
        // The canvas owns the wheel for zoom.
        if *tab == Tab::Canvas {
            [false, false]
        } else {
            [true, true]
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let dt_ms = ui.input(|i| i.unstable_dt) * 1000.0;
        let cpu_ms = frame.info().cpu_usage.map(|c| c * 1000.0);
        self.s.frame_ms.push(dt_ms);
        if let Some(c) = cpu_ms {
            self.s.cpu_ms.push(c);
        }

        let mut viewer = Viewer {
            s: &mut self.s,
            scene_ms: 0.0,
        };
        DockArea::new(&mut self.dock)
            .show_leaf_collapse_buttons(false)
            .show_inside(ui, &mut viewer);
        let scene_ms = viewer.scene_ms;
        self.s.scene_ms.push(scene_ms);

        if self.s.bench.is_some() {
            self.s.bench_step(dt_ms, cpu_ms, scene_ms);
            if self.s.bench.is_none() && self.s.cfg.exit_after_bench {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        shots::step(&mut self.s, ui.ctx());
        // Repaint continuously so the stats show steady-state frame cost.
        ui.ctx().request_repaint();
    }
}

/// Scripted window captures for RESULTS.md (`--shot <dir>`), using egui's
/// own screenshot command, so no desktop control is needed.
#[cfg(not(target_arch = "wasm32"))]
mod shots {
    use super::{Renderer, State, Tool};
    use crate::view::{View, MAX_EXTENT_MM, MIN_EXTENT_MM, V2};
    use std::path::PathBuf;

    type Setup = fn(&mut State);

    /// The line the screenshots select and centre on (same index as spike B).
    const SEL: usize = 7;

    fn overview(s: &mut State, r: Renderer) {
        s.renderer = r;
        s.auto_fit = false;
        s.view = View::default();
        if let Some((lo, hi)) = s.doc.bounds() {
            s.view.fit(lo, hi, s.canvas_rect);
        }
        s.tool = Tool::Line;
        s.selected = Some(SEL);
        s.forced_hover = Some(11);
        // Rubber band from line 3's start towards line 20's midpoint.
        let l20 = s.doc.lines[20];
        s.pending = Some(s.doc.lines[3].a);
        s.forced_cursor = Some(l20.a.add(l20.b).scale(0.5));
    }

    /// Centre on line SEL's midpoint with the given short-side extent.
    fn around_selected(s: &mut State, r: Renderer, extent_mm: f64) {
        overview(s, r);
        s.pending = None;
        s.forced_cursor = None;
        s.forced_hover = None;
        let l = s.doc.lines[SEL];
        s.view.center = l.a.add(l.b).scale(0.5);
        let short = s.canvas_rect.width().min(s.canvas_rect.height()) as f64;
        s.view.pt_per_mm = short / extent_mm;
    }

    /// The smallest extent, 0.2 µm / 0.1 µm from line SEL's end: the end must
    /// land exactly that far off-centre if the f64 → screen path is precise.
    fn precision(s: &mut State, r: Renderer) {
        around_selected(s, r, MIN_EXTENT_MM);
        s.view.center = s.doc.lines[SEL].b.add(V2::new(0.000_2, 0.000_1));
    }

    fn two_metres(s: &mut State) {
        overview(s, Renderer::Painter);
        s.pending = None;
        s.forced_cursor = None;
        let long = s.canvas_rect.width().max(s.canvas_rect.height()) as f64;
        s.view.pt_per_mm = long / MAX_EXTENT_MM;
    }

    const SHOTS: &[(&str, Setup)] = &[
        ("native-overview-5000-lines-painter", |s| {
            overview(s, Renderer::Painter)
        }),
        ("native-overview-5000-lines-lyon", |s| {
            overview(s, Renderer::Lyon)
        }),
        ("native-overview-5000-lines-wgpu", |s| {
            overview(s, Renderer::Wgpu)
        }),
        ("native-aa-20mm-painter", |s| {
            around_selected(s, Renderer::Painter, 20.0)
        }),
        ("native-aa-20mm-lyon", |s| {
            around_selected(s, Renderer::Lyon, 20.0)
        }),
        ("native-aa-20mm-wgpu", |s| {
            around_selected(s, Renderer::Wgpu, 20.0)
        }),
        ("native-extent-0.001mm-painter", |s| {
            precision(s, Renderer::Painter)
        }),
        ("native-extent-0.001mm-wgpu", |s| {
            precision(s, Renderer::Wgpu)
        }),
        ("native-extent-2m", two_metres),
    ];

    #[derive(Debug)]
    pub struct Runner {
        dir: PathBuf,
        idx: usize,
        frame: u32,
        requested: bool,
    }

    impl Runner {
        pub fn new(dir: PathBuf) -> Self {
            Self {
                dir,
                idx: 0,
                frame: 0,
                requested: false,
            }
        }
    }

    /// Per shot: set up on frame 10, settle until frame 16, request a
    /// capture, then wait for egui's `Event::Screenshot`.
    pub fn step(s: &mut State, ctx: &egui::Context) {
        let Some(run) = s.shots.as_mut() else {
            return;
        };
        run.frame += 1;
        let frame = run.frame;
        let Some(&(name, setup)) = SHOTS.get(run.idx) else {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        };
        if frame < 10 {
            return;
        }
        if frame == 10 {
            setup(s);
            return;
        }
        if frame < 16 {
            return;
        }
        if !run.requested {
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
            run.requested = true;
            return;
        }
        let image = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(image) = image {
            let path = run.dir.join(format!("{name}.png"));
            if let Err(e) = save_png(&path, &image) {
                eprintln!("screenshot {name}: {e}");
            }
            run.idx += 1;
            run.frame = 9;
            run.requested = false;
        }
    }

    fn save_png(path: &std::path::Path, img: &egui::ColorImage) -> Result<(), String> {
        let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
        let [w, h] = img.size;
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w as u32, h as u32);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().map_err(|e| e.to_string())?;
        let bytes: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
        writer.write_image_data(&bytes).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_stats() {
        let v: Vec<f32> = (1..=100).map(|x| x as f32).collect();
        let s = summarise(&v);
        assert_eq!(s.mean, 50.5);
        assert_eq!(s.p95, 95.0);
        assert_eq!(summarise(&[]).mean, 0.0);
    }
}
