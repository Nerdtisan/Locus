// SPDX-License-Identifier: AGPL-3.0-or-later

//! Plain SVG 1.1 writer (SPEC §8 Formats, M1: no validation yet).
//!
//! Output is true scale: user units are millimetres (`viewBox` equals the
//! `width`/`height` in mm). The document is Y-up; SVG is Y-down, so Y is
//! flipped about the top of the bounds when writing coordinates. Construction
//! geometry, layers with export disabled and bare points are not written.

use locus_doc::{Document, Geom, Layer, LayerId};
use locus_geom::{Aabb, Vec2};
use std::f64::consts::{PI, TAU};
use std::fmt::Write as _;

/// Canvas size for a document with nothing to export (a zero-size SVG is invalid).
const EMPTY_SIZE_MM: f64 = 1.0;

enum Shape {
    Line(Vec2, Vec2),
    Circle(Vec2, f64),
    Arc(Vec2, Vec2, Vec2),
}

/// Render the document's exportable geometry as SVG bytes (UTF-8).
///
/// Geometry that references a missing point is skipped rather than failing:
/// an exporter must not panic on a document it did not validate (SPEC §8).
#[must_use]
pub fn to_svg(doc: &Document) -> Vec<u8> {
    let mut bounds: Option<Aabb> = None;
    let mut layers: Vec<(&Layer, LayerId, Vec<Shape>)> = doc
        .layers()
        .iter()
        .filter(|(_, l)| l.export_enabled)
        .map(|(id, l)| (l, *id, Vec::new()))
        .collect();
    layers.sort_by_key(|(l, id, _)| (l.export_order, id.get()));

    for (eid, e) in doc.entities().iter() {
        if e.construction {
            continue;
        }
        let shape = match &e.geom {
            Geom::Point { .. } => continue,
            Geom::Line { .. } => doc.line(eid).map(|l| (Shape::Line(l.a, l.b), l.bbox())),
            Geom::Circle { .. } => doc
                .circle(eid)
                .map(|c| (Shape::Circle(c.center, c.radius), c.bbox())),
            Geom::Arc { .. } => doc
                .arc(eid)
                .map(|a| (Shape::Arc(a.center, a.start, a.end), a.bbox())),
        };
        let Some((shape, bb)) = shape else { continue };
        let Some((_, _, shapes)) = layers.iter_mut().find(|(_, id, _)| *id == e.layer) else {
            continue;
        };
        shapes.push(shape);
        bounds = Some(bounds.map_or(bb, |b| b.union(bb)));
    }

    let (min, w, h) = match bounds {
        Some(b) => (b.min, b.width(), b.height()),
        None => (Vec2::ZERO, EMPTY_SIZE_MM, EMPTY_SIZE_MM),
    };
    // A single horizontal/vertical line has zero height; keep the canvas valid.
    let (w, h) = (w.max(EMPTY_SIZE_MM * 1e-3), h.max(EMPTY_SIZE_MM * 1e-3));
    let top = min.y + h;
    // Document point → SVG user space (origin at the bounds' top-left).
    let map = |p: Vec2| Vec2::new(p.x - min.x, top - p.y);

    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    let _ = writeln!(
        out,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" version=\"1.1\" width=\"{w}mm\" height=\"{h}mm\" viewBox=\"0 0 {w} {h}\">",
        w = num(w),
        h = num(h)
    );
    for (layer, id, shapes) in &layers {
        if shapes.is_empty() {
            continue;
        }
        let _ = writeln!(
            out,
            "  <g id=\"layer-{id}\" data-name=\"{name}\" data-operation=\"{op:?}\" fill=\"none\" stroke=\"#{r:02x}{g:02x}{b:02x}\" stroke-width=\"0.1\">",
            name = escape(&layer.name),
            op = layer.operation,
            r = layer.colour[0],
            g = layer.colour[1],
            b = layer.colour[2],
        );
        for s in shapes {
            write_shape(&mut out, s, &map);
        }
        out.push_str("  </g>\n");
    }
    out.push_str("</svg>\n");
    out.into_bytes()
}

fn write_shape(out: &mut String, s: &Shape, map: &impl Fn(Vec2) -> Vec2) {
    match *s {
        Shape::Line(a, b) => {
            let (a, b) = (map(a), map(b));
            let _ = writeln!(
                out,
                "    <line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\"/>",
                num(a.x),
                num(a.y),
                num(b.x),
                num(b.y)
            );
        }
        Shape::Circle(c, r) => {
            let c = map(c);
            let _ = writeln!(
                out,
                "    <circle cx=\"{}\" cy=\"{}\" r=\"{}\"/>",
                num(c.x),
                num(c.y),
                num(r)
            );
        }
        Shape::Arc(c, s, e) => {
            let r = s.distance(c);
            // CCW sweep in the Y-up document. The end is projected onto the
            // circle (SPEC §4: only its angle matters) so the SVG arc is exact.
            let a0 = (s - c).angle();
            let a1 = (e - c).angle();
            let sweep = (a1 - a0).rem_euclid(TAU);
            let large = u8::from(sweep > PI);
            let end = c + Vec2::from_angle(a1) * r;
            let (ms, me) = (map(s), map(end));
            // The Y flip turns CCW into clockwise on screen: sweep-flag 0.
            let _ = writeln!(
                out,
                "    <path d=\"M {} {} A {r} {r} 0 {large} 0 {} {}\"/>",
                num(ms.x),
                num(ms.y),
                num(me.x),
                num(me.y),
                r = num(r),
            );
        }
    }
}

/// Fixed 4-decimal (0.1 µm) formatting with trailing zeros trimmed, so output
/// is stable across platforms and never prints `-0`.
fn num(v: f64) -> String {
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

    #[test]
    fn num_trims() {
        assert_eq!(num(10.0), "10");
        assert_eq!(num(-0.00001), "0");
        assert_eq!(num(1.23456), "1.2346");
    }
}
