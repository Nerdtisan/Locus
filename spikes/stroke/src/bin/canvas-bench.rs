// SPDX-License-Identifier: AGPL-3.0-or-later

//! S3: egui painter cost for 500 styled paths (Windows host, real GPU).
//! `canvas-bench <mode> <out.txt>`; modes:
//!   plain    – today's centrelines, `painter.line` per path (baseline)
//!   rebuild  – strip meshes transformed to screen every frame
//!   cached   – strip meshes built once in world space, drawn as `Arc<Mesh>`
//!              under a layer transform (the planned per-entity cache)
//!   scanline – cleaned outlines filled per screen row, as region fills are
//!              today (`views/canvas/regions.rs`)
//! Vsync off; 60 warm-up frames, then 600 measured. Writes one line.

use eframe::egui;
use egui::emath::TSTransform;
use egui::{Color32, Mesh, Pos2, Shape, Stroke};
use locus_geom::{EXPORT_TOL, Vec2};
use std::sync::Arc;
use std::time::Instant;
use stroke_spike::build::{self, Outline};
use stroke_spike::*;

const WARM: usize = 60;
const FRAMES: usize = 600;
/// 5 px per mm: the 200 mm field fills the window.
const ZOOM: f32 = 5.0;

struct App {
    mode: String,
    out: String,
    tris: Vec<Vec<[f32; 6]>>,
    cached: Vec<Arc<Mesh>>,
    centre: Vec<Vec<Vec2>>,
    outlines: Vec<Vec<Vec<Vec2>>>,
    frame: usize,
    t_start: Option<Instant>,
    cpu: f64,
    tri_count: usize,
}

fn col(i: usize) -> Color32 {
    Color32::from_rgb((40 + i * 37 % 160) as u8, (60 + i * 53 % 140) as u8, 120)
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let t0 = Instant::now();
        let rect = ui.max_rect();
        let painter = ui.painter().clone();
        let to =
            |x: f32, y: f32| Pos2::new(rect.left() + 10.0 + x * ZOOM, rect.top() + 10.0 + y * ZOOM);
        match self.mode.as_str() {
            "plain" => {
                for (i, c) in self.centre.iter().enumerate() {
                    let pts: Vec<Pos2> = c.iter().map(|p| to(p.x as f32, p.y as f32)).collect();
                    painter.add(Shape::line(pts, Stroke::new(1.0, col(i))));
                }
            }
            "rebuild" => {
                for (i, t) in self.tris.iter().enumerate() {
                    let mut m = Mesh::default();
                    for tr in t {
                        let b = m.vertices.len() as u32;
                        m.colored_vertex(to(tr[0], tr[1]), col(i));
                        m.colored_vertex(to(tr[2], tr[3]), col(i));
                        m.colored_vertex(to(tr[4], tr[5]), col(i));
                        m.add_triangle(b, b + 1, b + 2);
                    }
                    painter.add(m);
                }
            }
            "cached" => {
                let layer = egui::LayerId::new(egui::Order::Middle, egui::Id::new("styled"));
                ctx.set_transform_layer(
                    layer,
                    TSTransform::new(rect.left_top().to_vec2() + egui::vec2(10.0, 10.0), ZOOM),
                );
                let p = ctx.layer_painter(layer);
                for m in &self.cached {
                    p.add(Shape::Mesh(m.clone()));
                }
            }
            _ => {
                for (i, ol) in self.outlines.iter().enumerate() {
                    scanline(&painter, ol, col(i), &to);
                }
            }
        }
        let cpu = t0.elapsed().as_secs_f64();
        self.frame += 1;
        if self.frame == WARM {
            self.t_start = Some(Instant::now());
            self.cpu = 0.0;
        } else if self.frame > WARM {
            self.cpu += cpu;
        }
        if self.frame == WARM + FRAMES {
            let wall = self.t_start.map_or(0.0, |t| t.elapsed().as_secs_f64());
            let line = format!(
                "{}: {:.2} ms/frame wall, {:.2} ms/frame building shapes, {} triangles\n",
                self.mode,
                wall * 1e3 / FRAMES as f64,
                self.cpu * 1e3 / FRAMES as f64,
                self.tri_count
            );
            let _ = std::fs::write(&self.out, line);
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        ctx.request_repaint();
    }
}

/// Non-zero scanline fill, one quad per screen row (as regions.rs does).
fn scanline(p: &egui::Painter, loops: &[Vec<Vec2>], c: Color32, to: &dyn Fn(f32, f32) -> Pos2) {
    let (mut y0, mut y1) = (f64::INFINITY, f64::NEG_INFINITY);
    for l in loops {
        for q in l {
            y0 = y0.min(q.y);
            y1 = y1.max(q.y);
        }
    }
    let mut mesh = Mesh::default();
    let step = 1.0 / f64::from(ZOOM);
    let mut y = y0 + step * 0.5;
    let mut xs: Vec<(f64, i32)> = Vec::new();
    while y < y1 {
        xs.clear();
        for l in loops {
            let n = l.len();
            for i in 0..n {
                let (a, b) = (l[i], l[(i + 1) % n]);
                if (a.y <= y) != (b.y <= y) {
                    xs.push((
                        a.x + (y - a.y) / (b.y - a.y) * (b.x - a.x),
                        if b.y > a.y { 1 } else { -1 },
                    ));
                }
            }
        }
        xs.sort_by(|a, b| a.0.total_cmp(&b.0));
        let (mut w, mut s) = (0, 0.0);
        for &(x, d) in &xs {
            if w == 0 {
                s = x;
            }
            w += d;
            if w == 0 {
                let r = egui::Rect::from_min_max(
                    to(s as f32, (y - step * 0.5) as f32),
                    to(x as f32, (y + step * 0.5) as f32),
                );
                mesh.add_colored_rect(r, c);
            }
        }
        y += step;
    }
    p.add(mesh);
}

fn main() -> eframe::Result {
    let mut args = std::env::args().skip(1);
    let mode = args.next().unwrap_or_else(|| "cached".into());
    let out = args.next().unwrap_or_else(|| "canvas-bench.txt".into());
    let paths = random_paths(500, 42);
    let mut tris = Vec::new();
    let mut cached = Vec::new();
    let mut centre = Vec::new();
    let mut outlines = Vec::new();
    let mut tri_count = 0;
    for (i, (p, m)) in paths.iter().enumerate() {
        let mut t = Vec::new();
        build::canvas_mesh(p, m, CANVAS_TOL, &mut t);
        tri_count += t.len();
        let mut mesh = Mesh::default();
        for tr in &t {
            let b = mesh.vertices.len() as u32;
            for k in 0..3 {
                mesh.colored_vertex(Pos2::new(tr[2 * k], tr[2 * k + 1]), col(i));
            }
            mesh.add_triangle(b, b + 1, b + 2);
        }
        cached.push(Arc::new(mesh));
        tris.push(t);
        let mut c = Vec::new();
        for pc in &p.pieces {
            for k in 0..=32 {
                c.push(pc.at(f64::from(k) / 32.0));
            }
        }
        centre.push(c);
        if mode == "scanline" {
            let ex = build::expand(p, m, EXPORT_TOL * 5.0, EXPORT_TOL * 5.0);
            outlines.push(Outline::from_area(&ex.area).polylines(0.01));
        }
    }
    let mut options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1100.0, 1100.0]),
        ..Default::default()
    };
    options.wgpu_options.surface.present_mode = eframe::wgpu::PresentMode::AutoNoVsync;
    eframe::run_native(
        "canvas-bench",
        options,
        Box::new(move |_cc| {
            Ok(Box::new(App {
                mode,
                out,
                tris,
                cached,
                centre,
                outlines,
                frame: 0,
                t_start: None,
                cpu: 0.0,
                tri_count,
            }))
        }),
    )
}
