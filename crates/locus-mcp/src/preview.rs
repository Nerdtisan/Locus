// SPDX-License-Identifier: AGPL-3.0-or-later

//! `render_preview` (SPEC §16.3, #231): a PNG of the document or a region of
//! it, so the AI can see its result.
//!
//! Headless, and styled like the canvas in its light theme: geometry that can
//! still move in blue, fully constrained geometry near-black, construction
//! geometry purple and dashed, closed regions (§6.3) lightly shaded, hidden
//! layers left out, Bézier handles hidden. Constraint glyphs and dimension
//! labels are not drawn (they need the GUI's fonts).

use core::f64::consts::TAU;

use locus_doc::{EntityId, Geom, Sketcher};
use locus_geom::Vec2;
use serde_json::{Value, json};
use tiny_skia::{
    Color, FillRule, LineCap, LineJoin, Paint, PathBuilder, Pixmap, Stroke, StrokeDash, Transform,
};

use crate::dto::PreviewArgs;
use crate::error::ToolError;

pub const DEFAULT_WIDTH: u32 = 800;
pub const DEFAULT_HEIGHT: u32 = 600;
/// Largest side, so a preview stays a few hundred kB at most.
pub const MAX_SIDE: u32 = 2048;
pub const MIN_SIDE: u32 = 16;

// The canvas's light palette (locus-app `render.rs`).
const BACKGROUND: (u8, u8, u8) = (250, 250, 250);
const FREE: (u8, u8, u8) = (25, 100, 200);
const CONSTRAINED: (u8, u8, u8) = (20, 20, 20);
const CONSTRUCTION: (u8, u8, u8) = (150, 60, 190);
const REGION_ALPHA: u8 = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Style {
    Free,
    Constrained,
    Construction,
}

impl Style {
    fn rgb(self) -> (u8, u8, u8) {
        match self {
            Style::Free => FREE,
            Style::Constrained => CONSTRAINED,
            Style::Construction => CONSTRUCTION,
        }
    }
}

/// World-space drawing: polylines, dots and shaded regions.
#[derive(Default)]
struct Drawing {
    lines: Vec<(Style, Vec<Vec2>)>,
    dots: Vec<(Style, Vec2)>,
    regions: Vec<Vec<Vec<Vec2>>>,
}

fn arc_points(center: Vec2, r: f64, start: f64, sweep: f64) -> Vec<Vec2> {
    let n = ((sweep.abs() / TAU) * 128.0).ceil().clamp(4.0, 256.0) as usize;
    (0..=n)
        .map(|i| {
            let a = start + sweep * (i as f64) / (n as f64);
            Vec2::new(center.x + r * a.cos(), center.y + r * a.sin())
        })
        .collect()
}

fn cubic_points(c: &locus_geom::Cubic) -> Vec<Vec2> {
    (0..=32).map(|i| c.eval(f64::from(i) / 32.0)).collect()
}

fn collect(sk: &Sketcher) -> Drawing {
    let doc = sk.document();
    let st = sk.state();
    let mut d = Drawing::default();
    // Bézier handles show only for a selected curve in the app: hide them.
    let handles: std::collections::BTreeSet<EntityId> = doc
        .entities()
        .iter()
        .filter_map(|(_, e)| match e.geom {
            Geom::Bezier {
                start_ctrl,
                end_ctrl,
                ..
            } => Some([start_ctrl, end_ctrl]),
            _ => None,
        })
        .flatten()
        .collect();
    for (id, e) in doc.entities().iter() {
        if doc.layer(e.layer).is_some_and(|l| !l.visible) || handles.contains(&id) {
            continue;
        }
        let style = if e.construction {
            Style::Construction
        } else if st.fully_constrained.contains(&id) {
            Style::Constrained
        } else {
            Style::Free
        };
        match &e.geom {
            Geom::Point { pos } => d.dots.push((style, *pos)),
            Geom::Line { .. } => {
                if let Some(l) = doc.line(id) {
                    d.lines.push((style, vec![l.a, l.b]));
                }
            }
            Geom::Circle { .. } => {
                if let Some(c) = doc.circle(id) {
                    d.lines
                        .push((style, arc_points(c.center, c.radius, 0.0, TAU)));
                }
            }
            Geom::Arc { .. } => {
                if let Some(a) = doc.arc(id) {
                    d.lines.push((
                        style,
                        arc_points(a.center, a.radius(), a.start_angle(), a.sweep()),
                    ));
                }
            }
            Geom::Bezier { .. } => {
                if let Some(c) = doc.cubic(id) {
                    d.lines.push((style, cubic_points(&c)));
                }
            }
            Geom::Text { .. } => {
                let Some(g) = doc.text_geometry(id) else {
                    continue;
                };
                for c in &g.contours {
                    let mut pts = vec![c.start];
                    let mut from = c.start;
                    for s in &c.segs {
                        match *s {
                            locus_text::Seg::Line { end } => pts.push(end),
                            locus_text::Seg::Cubic { c0, c1, end } => {
                                let cu = locus_geom::Cubic::new(from, c0, c1, end);
                                pts.extend(cubic_points(&cu).into_iter().skip(1));
                            }
                        }
                        from = s.end();
                    }
                    if c.closed {
                        pts.push(c.start);
                    }
                    d.lines.push((style, pts));
                }
            }
        }
    }
    for r in sk.regions() {
        let lp = |l: &locus_region::Loop| -> Vec<Vec2> {
            let mut pts = Vec::new();
            for e in &l.edges {
                match e.seg {
                    locus_region::Seg::Line { a, b } => pts.extend([a, b]),
                    locus_region::Seg::Arc {
                        center,
                        radius,
                        start,
                        sweep,
                    } => pts.extend(arc_points(center, radius, start, sweep)),
                }
            }
            pts
        };
        let mut loops = vec![lp(&r.outer)];
        loops.extend(r.holes.iter().map(lp));
        d.regions.push(loops);
    }
    d
}

fn bounds(d: &Drawing) -> Option<[f64; 4]> {
    let mut b = [
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    ];
    let pts = d
        .lines
        .iter()
        .flat_map(|(_, p)| p.iter())
        .chain(d.dots.iter().map(|(_, p)| p));
    for p in pts {
        b = [b[0].min(p.x), b[1].min(p.y), b[2].max(p.x), b[3].max(p.y)];
    }
    b[0].is_finite().then_some(b)
}

fn colour((r, g, b): (u8, u8, u8), a: u8) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(Color::from_rgba8(r, g, b, a));
    p.anti_alias = true;
    p
}

/// Render the PNG and describe the mapping.
///
/// # Errors
/// [`ToolError::BadArguments`] for a size or region out of range.
pub fn render(sk: &Sketcher, a: &PreviewArgs) -> Result<(Vec<u8>, Value), ToolError> {
    let w = a.width.unwrap_or(DEFAULT_WIDTH);
    let h = a.height.unwrap_or(DEFAULT_HEIGHT);
    if !(MIN_SIDE..=MAX_SIDE).contains(&w) || !(MIN_SIDE..=MAX_SIDE).contains(&h) {
        return Err(ToolError::BadArguments(format!(
            "width and height must be {MIN_SIDE}–{MAX_SIDE} px"
        )));
    }
    let d = collect(sk);
    let box_ = match a.region {
        Some(r) => {
            if !r.iter().all(|v| v.is_finite()) || r[2] <= r[0] || r[3] <= r[1] {
                return Err(ToolError::BadArguments(
                    "region must be [min_x, min_y, max_x, max_y] with max > min".into(),
                ));
            }
            r
        }
        None => {
            let b = bounds(&d).unwrap_or([-50.0, -50.0, 50.0, 50.0]);
            // A 5 % margin, and at least 1 mm so a lone point still has a view.
            let m = ((b[2] - b[0]).max(b[3] - b[1]) * 0.05).max(1.0);
            [b[0] - m, b[1] - m, b[2] + m, b[3] + m]
        }
    };
    let (fw, fh) = (f64::from(w), f64::from(h));
    let s = (fw / (box_[2] - box_[0])).min(fh / (box_[3] - box_[1]));
    let ox = (fw - (box_[2] - box_[0]) * s) / 2.0;
    let oy = (fh - (box_[3] - box_[1]) * s) / 2.0;
    // World y points up, image y down.
    let px = |p: Vec2| -> (f32, f32) {
        (
            ((p.x - box_[0]) * s + ox) as f32,
            (fh - ((p.y - box_[1]) * s + oy)) as f32,
        )
    };

    let mut pm = Pixmap::new(w, h)
        .ok_or_else(|| ToolError::BadArguments("cannot allocate the image".into()))?;
    let (r, g, b) = BACKGROUND;
    pm.fill(Color::from_rgba8(r, g, b, 255));

    let region_paint = colour(FREE, REGION_ALPHA);
    for loops in &d.regions {
        let mut pb = PathBuilder::new();
        for l in loops {
            let mut it = l.iter();
            if let Some(&p) = it.next() {
                let (x, y) = px(p);
                pb.move_to(x, y);
                for &q in it {
                    let (x, y) = px(q);
                    pb.line_to(x, y);
                }
                pb.close();
            }
        }
        if let Some(path) = pb.finish() {
            pm.fill_path(
                &path,
                &region_paint,
                FillRule::EvenOdd,
                Transform::identity(),
                None,
            );
        }
    }

    let solid = Stroke {
        width: 1.5,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Stroke::default()
    };
    let dashed = Stroke {
        dash: StrokeDash::new(vec![6.0, 4.0], 0.0),
        ..solid.clone()
    };
    for (style, pts) in &d.lines {
        let mut pb = PathBuilder::new();
        let mut it = pts.iter();
        let Some(&p) = it.next() else { continue };
        let (x, y) = px(p);
        pb.move_to(x, y);
        for &q in it {
            let (x, y) = px(q);
            pb.line_to(x, y);
        }
        if let Some(path) = pb.finish() {
            let stroke = if *style == Style::Construction {
                &dashed
            } else {
                &solid
            };
            pm.stroke_path(
                &path,
                &colour(style.rgb(), 255),
                stroke,
                Transform::identity(),
                None,
            );
        }
    }
    for (style, p) in &d.dots {
        let (x, y) = px(*p);
        if let Some(path) = PathBuilder::from_circle(x, y, 2.0) {
            pm.fill_path(
                &path,
                &colour(style.rgb(), 255),
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
    }

    let png = pm
        .encode_png()
        .map_err(|e| ToolError::Unavailable(format!("PNG encoding failed: {e}")))?;
    let info = json!({
        "width": w,
        "height": h,
        "region": box_,
        "px_per_mm": s,
        "legend": {
            "blue": "can still move (under-constrained)",
            "black": "fully constrained",
            "purple dashed": "construction",
            "shaded": "closed region",
        },
    });
    Ok((png, info))
}
