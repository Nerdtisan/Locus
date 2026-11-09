// SPDX-License-Identifier: AGPL-3.0-or-later

//! Plain SVG 1.1 writer (SPEC §8 Formats, M1: no validation yet).
//!
//! Output is true scale: user units are millimetres (`viewBox` equals the
//! `width`/`height` in mm). The document is Y-up; SVG is Y-down, so Y is
//! flipped about the top of the bounds when writing coordinates. Construction
//! geometry, layers with export disabled and bare points are not written.
//!
//! Each `<path>`/`<circle>` carries its own `fill`/`stroke`/`stroke-width`
//! instead of inheriting them from the layer `<g>`: Cricut Design Space
//! ignores group-level presentation attributes and imported a design with a
//! large black outline as blank (#134). Per-shape attributes are valid SVG 1.1
//! and read the same in LightBurn and Inkscape.

use crate::order::order;
use crate::pipeline::{ExportModel, PathSeg, Shape, collect};
use locus_doc::Document;
use locus_geom::Vec2;
use std::f64::consts::PI;
use std::fmt::Write as _;

/// Canvas size for a document with nothing to export (a zero-size SVG is invalid).
const EMPTY_SIZE_MM: f64 = 1.0;

/// Render the document's exportable geometry as SVG bytes (UTF-8).
///
/// Convenience for `to_svg_model(&order(collect(doc)))`: no validation.
/// Geometry that references a missing point is skipped rather than failing:
/// an exporter must not panic on a document it did not validate (SPEC §8).
#[must_use]
pub fn to_svg(doc: &Document) -> Vec<u8> {
    let mut model = collect(doc);
    order(&mut model);
    to_svg_model(&model)
}

/// Write an already prepared model. Each chained path becomes one `<path>`
/// (closed paths end in `Z`); circles stay `<circle>`.
#[must_use]
pub fn to_svg_model(model: &ExportModel) -> Vec<u8> {
    let (min, w, h) = match model.bounds() {
        Some(b) => (b.min, b.width(), b.height()),
        None => (Vec2::ZERO, EMPTY_SIZE_MM, EMPTY_SIZE_MM),
    };
    // A single horizontal/vertical line has zero height; keep the canvas valid.
    let (w, h) = (w.max(EMPTY_SIZE_MM * 1e-3), h.max(EMPTY_SIZE_MM * 1e-3));
    let top = min.y + h;
    // Document point → SVG user space (origin at the bounds' top-left).
    let map = |p: Vec2| Vec2::new(p.x - min.x, top - p.y);

    let mut out = String::new();
    out.push_str(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>
",
    );
    let _ = writeln!(
        out,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" version=\"1.1\" width=\"{w}mm\" height=\"{h}mm\" viewBox=\"0 0 {w} {h}\">",
        w = num(w),
        h = num(h)
    );
    for layer in &model.layers {
        if layer.paths.is_empty() {
            continue;
        }
        let _ = writeln!(
            out,
            "  <g id=\"layer-{id}\" data-name=\"{name}\" data-operation=\"{op:?}\">",
            id = layer.id,
            name = escape(&layer.name),
            op = layer.operation.kind(),
        );
        // On every shape, not the group (see the module docs: Cricut).
        let style = format!(
            "fill=\"none\" stroke=\"#{r:02x}{g:02x}{b:02x}\" stroke-width=\"0.1\"",
            r = layer.colour[0],
            g = layer.colour[1],
            b = layer.colour[2],
        );
        for p in &layer.paths {
            if let [seg] = p.segs.as_slice()
                && let Shape::Circle(c) = seg.shape
            {
                let c0 = map(c.center);
                let _ = writeln!(
                    out,
                    "    <circle {style} cx=\"{}\" cy=\"{}\" r=\"{}\"/>",
                    num(c0.x),
                    num(c0.y),
                    num(c.radius)
                );
                continue;
            }
            let start = map(p.start());
            let mut d = format!("M {} {}", num(start.x), num(start.y));
            for s in &p.segs {
                write_seg(&mut d, s, &map);
            }
            if p.closed {
                d.push_str(" Z");
            }
            let _ = writeln!(out, "    <path {style} d=\"{d}\"/>");
        }
        out.push_str(
            "  </g>
",
        );
    }
    out.push_str(
        "</svg>
",
    );
    out.into_bytes()
}

fn write_seg(d: &mut String, s: &PathSeg, map: &impl Fn(Vec2) -> Vec2) {
    let end = map(s.end());
    match s.shape {
        Shape::Line(_) => {
            let _ = write!(d, " L {} {}", num(end.x), num(end.y));
        }
        Shape::Arc(a) => {
            let r = a.radius();
            // CCW sweep in the Y-up document; the arc's own sweep, whichever way it is traversed.
            let sweep = a.sweep();
            let large = u8::from(sweep > PI);
            // The Y flip turns CCW into clockwise on screen (flag 0); a reversed
            // traversal runs the other way (flag 1). The end is projected onto the
            // circle (SPEC §4: only its angle matters) so the SVG arc is exact.
            let flag = u8::from(s.reversed);
            let on_circle = if s.reversed {
                s.end()
            } else {
                a.center + Vec2::from_angle(a.end_angle()) * r
            };
            let end = if s.reversed { end } else { map(on_circle) };
            let _ = write!(
                d,
                " A {r} {r} 0 {large} {flag} {} {}",
                num(end.x),
                num(end.y),
                r = num(r),
            );
        }
        Shape::Circle(_) => {}
    }
}

/// Fixed 4-decimal (0.1 µm) formatting with trailing zeros trimmed, so output
/// is stable across platforms and never prints `-0`.
pub(crate) fn num(v: f64) -> String {
    let mut s = format!("{v:.4}");
    if s.contains('.') {
        while s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.pop();
        }
    }
    if s == "-0" { "0".to_owned() } else { s }
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_document_is_valid_svg() {
        let svg = String::from_utf8(to_svg(&Document::new())).unwrap();
        assert!(svg.contains("width=\"1mm\" height=\"1mm\" viewBox=\"0 0 1 1\""));
        assert!(!svg.contains("<g "));
    }

    /// Regression for #134: Cricut Design Space ignores styles on `<g>`, so
    /// every shape must carry its own fill and stroke.
    #[test]
    fn every_shape_carries_its_own_style() {
        use locus_doc::{Command, Editor, Geom};
        let mut ed = Editor::new(Document::new());
        let layer = Document::default_layer();
        let pt = |ed: &mut Editor, x, y| {
            ed.apply(&Command::AddEntity {
                layer,
                construction: false,
                geom: Geom::Point {
                    pos: Vec2::new(x, y),
                },
            })
            .unwrap()
            .entities[0]
        };
        let (a, b) = (pt(&mut ed, 0.0, 0.0), pt(&mut ed, 50.0, 0.0));
        let c = pt(&mut ed, 20.0, 20.0);
        for geom in [
            Geom::Line { start: a, end: b },
            Geom::Circle {
                center: c,
                radius: 5.0,
            },
        ] {
            ed.apply(&Command::AddEntity {
                layer,
                construction: false,
                geom,
            })
            .unwrap();
        }
        let svg = String::from_utf8(to_svg(ed.document())).unwrap();
        let shapes: Vec<&str> = svg
            .lines()
            .filter(|l| l.contains("<path") || l.contains("<circle"))
            .collect();
        assert_eq!(shapes.len(), 2, "{svg}");
        for s in shapes {
            assert!(
                s.contains("fill=\"none\"")
                    && s.contains("stroke=\"#")
                    && s.contains("stroke-width="),
                "shape without its own style: {s}"
            );
        }
        let group = svg.lines().find(|l| l.contains("<g ")).unwrap();
        assert!(
            !group.contains("stroke="),
            "style must not be on the group: {group}"
        );
    }

    #[test]
    fn num_trims() {
        assert_eq!(num(10.0), "10");
        assert_eq!(num(-0.00001), "0");
        assert_eq!(num(1.23456), "1.2346");
    }
}
