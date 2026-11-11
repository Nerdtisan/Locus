// SPDX-License-Identifier: MIT OR Apache-2.0

//! HPGL exporter: a first-party example plugin (SPEC §13 M3, §15.2, #185).
//!
//! Writes HP-GL for pen plotters and vinyl cutters: one pen per layer (the
//! layer's pen number for Draw layers, otherwise pens 1–8 in layer order),
//! lines as `PA`, true arcs and circles as `AA`, in plotter units of
//! 0.025 mm. Locus's y axis points up, like HP-GL's, so coordinates are used
//! as they are. The host does the file I/O; this only turns the validated
//! export model into bytes (§15.4).

use locus_plugin_api::prelude::*;

/// HP-GL plotter units per millimetre.
const UNITS_PER_MM: f64 = 40.0;

fn xy(p: &Vec2) -> String {
    let u = |v: f64| (v * UNITS_PER_MM).round() as i64;
    format!("{},{}", u(p.x), u(p.y))
}

/// Where a segment starts, taking `reversed` into account.
fn start(seg: &Segment) -> Vec2 {
    match (&seg.shape, seg.reversed) {
        (Shape::Line(l), false) => l.a,
        (Shape::Line(l), true) => l.b,
        (Shape::Arc(a), false) => a.start,
        (Shape::Arc(a), true) => a.end,
        (Shape::Circle(c), _) => Vec2 {
            x: c.center.x + c.radius,
            y: c.center.y,
        },
    }
}

/// Counter-clockwise sweep from `a` to `b` around `c`, in degrees, (0, 360].
fn sweep(c: &Vec2, a: &Vec2, b: &Vec2) -> f64 {
    let a0 = (a.y - c.y).atan2(a.x - c.x);
    let a1 = (b.y - c.y).atan2(b.x - c.x);
    let s = (a1 - a0).rem_euclid(std::f64::consts::TAU).to_degrees();
    if s <= 0.0 { 360.0 } else { s }
}

fn segment(out: &mut String, seg: &Segment) {
    let dir = if seg.reversed { -1.0 } else { 1.0 };
    match &seg.shape {
        Shape::Line(l) => {
            let end = if seg.reversed { &l.a } else { &l.b };
            out.push_str(&format!("PA{};\n", xy(end)));
        }
        Shape::Arc(a) => {
            let deg = sweep(&a.center, &a.start, &a.end) * dir;
            out.push_str(&format!("AA{},{deg:.3};\n", xy(&a.center)));
        }
        Shape::Circle(c) => {
            out.push_str(&format!("AA{},{:.3};\n", xy(&c.center), 360.0 * dir));
        }
    }
}

fn pen(index: usize, layer: &Layer) -> u32 {
    match &layer.operation {
        Operation::Draw(d) if d.pen_number > 0 => d.pen_number,
        _ => (index % 8) as u32 + 1,
    }
}

struct Hpgl;

impl InfoGuest for Hpgl {
    fn describe() -> PluginInfo {
        PluginInfo {
            id: "io.locus.hpgl".into(),
            name: "HPGL".into(),
            version: "1.0.0".into(),
        }
    }
}

impl ExporterGuest for Hpgl {
    fn format() -> FormatInfo {
        FormatInfo {
            id: "hpgl".into(),
            name: "HPGL".into(),
            extension: "plt".into(),
            mime: "application/vnd.hp-hpgl".into(),
        }
    }

    fn export(model: ExportModel) -> Result<Vec<u8>, PluginError> {
        let mut out = String::from("IN;\n");
        for (i, layer) in model.layers.iter().enumerate() {
            if layer.paths.is_empty() {
                continue;
            }
            out.push_str(&format!("SP{};\n", pen(i, layer)));
            for path in &layer.paths {
                let Some(first) = path.segments.first() else {
                    continue;
                };
                out.push_str(&format!("PU{};\nPD;\n", xy(&start(first))));
                for seg in &path.segments {
                    segment(&mut out, seg);
                }
                out.push_str("PU;\n");
            }
        }
        out.push_str("SP0;\n");
        Ok(out.into_bytes())
    }
}

locus_plugin_api::export_plugin!(Hpgl: exporter);
