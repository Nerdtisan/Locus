// SPDX-License-Identifier: AGPL-3.0-or-later
//! CPU-side renderers for the canvas lines (SPEC §12.2).
//!
//! - `Painter`: one `line_segment` per line; epaint tessellates each frame with
//!   its feathered anti-aliasing.
//! - `Lyon`: lyon's stroke tessellator builds one egui mesh per layer each
//!   frame. No feathering, so edges are aliased unless MSAA is on.
//!
//! The wgpu paint-callback renderer is in `gpu.rs`.

use crate::doc::Doc;
use crate::view::View;
use egui::epaint::{Mesh, Vertex, WHITE_UV};
use egui::{Color32, Painter, Rect, Stroke};
use lyon_tessellation::math::point;
use lyon_tessellation::path::Path;
use lyon_tessellation::{
    BuffersBuilder, LineCap, StrokeOptions, StrokeTessellator, StrokeVertex, VertexBuffers,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Renderer {
    Painter,
    Lyon,
    Wgpu,
}

impl Renderer {
    pub const ALL: [Renderer; 3] = [Renderer::Painter, Renderer::Lyon, Renderer::Wgpu];
    pub fn name(self) -> &'static str {
        match self {
            Renderer::Painter => "egui painter",
            Renderer::Lyon => "lyon → egui mesh",
            Renderer::Wgpu => "wgpu callback",
        }
    }
}

/// Stroke width of a document line on screen, in points (hairline-ish, as a
/// plotter preview would show it).
pub const LINE_WIDTH_PT: f32 = 1.25;

/// Lines whose screen bounding box misses `rect`, skipped by every renderer
/// so the comparison is like for like.
fn culled(a: egui::Pos2, b: egui::Pos2, rect: Rect) -> bool {
    let r = Rect::from_two_pos(a, b).expand(LINE_WIDTH_PT * 2.0);
    !r.intersects(rect)
}

pub fn draw_painter(painter: &Painter, doc: &Doc, view: &View, rect: Rect) -> usize {
    let mut drawn = 0;
    for l in &doc.lines {
        if !doc.is_visible(l) {
            continue;
        }
        let a = view.to_screen(l.a, rect);
        let b = view.to_screen(l.b, rect);
        if culled(a, b, rect) {
            continue;
        }
        painter.line_segment([a, b], Stroke::new(LINE_WIDTH_PT, doc.color(l)));
        drawn += 1;
    }
    drawn
}

pub fn draw_lyon(painter: &Painter, doc: &Doc, view: &View, rect: Rect) -> usize {
    let mut drawn = 0;
    let mut tess = StrokeTessellator::new();
    let opts = StrokeOptions::default()
        .with_line_width(LINE_WIDTH_PT)
        .with_line_cap(LineCap::Round)
        // Screen-space tolerance in points; finer than a pixel for the caps.
        .with_tolerance(0.1);
    for (li, layer) in doc.layers.iter().enumerate() {
        if !layer.visible {
            continue;
        }
        let mut pb = Path::builder();
        for l in doc.lines.iter().filter(|l| l.layer == li) {
            let a = view.to_screen(l.a, rect);
            let b = view.to_screen(l.b, rect);
            if culled(a, b, rect) {
                continue;
            }
            pb.begin(point(a.x, a.y));
            pb.line_to(point(b.x, b.y));
            pb.end(false);
            drawn += 1;
        }
        let path = pb.build();
        let mut geom: VertexBuffers<Vertex, u32> = VertexBuffers::new();
        let color: Color32 = layer.color;
        let res = tess.tessellate_path(
            &path,
            &opts,
            &mut BuffersBuilder::new(&mut geom, |v: StrokeVertex| Vertex {
                pos: egui::pos2(v.position().x, v.position().y),
                uv: WHITE_UV,
                color,
            }),
        );
        if res.is_err() {
            // Spike: a tessellation failure just drops that layer for a frame.
            continue;
        }
        painter.add(egui::Shape::mesh(Mesh {
            indices: geom.indices,
            vertices: geom.vertices,
            ..Default::default()
        }));
    }
    drawn
}

/// Hover colour that reads on both themes (the web build follows the
/// system theme, so it is often light).
pub fn hover_color(dark: bool) -> Color32 {
    if dark {
        Color32::from_white_alpha(220)
    } else {
        Color32::from_black_alpha(200)
    }
}

/// Hover / selection overlay for the CPU renderers (the wgpu renderer does
/// this in its shader).
pub fn draw_highlight(
    painter: &Painter,
    doc: &Doc,
    view: &View,
    rect: Rect,
    idx: usize,
    color: Color32,
) {
    if let Some(l) = doc.lines.get(idx) {
        let a = view.to_screen(l.a, rect);
        let b = view.to_screen(l.b, rect);
        painter.line_segment([a, b], Stroke::new(LINE_WIDTH_PT + 2.0, color));
    }
}
