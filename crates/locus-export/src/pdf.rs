// SPDX-License-Identifier: AGPL-3.0-or-later

//! Vector PDF writer: true scale, one optional content group per layer
//! (SPEC §8 Formats, #131).
//!
//! Hand-written rather than `printpdf`/`krilla`: the output is a single page of
//! stroked paths, so the whole writer is a few dozen lines, has no dependencies
//! (nothing for `cargo deny` or wasm32 to object to) and contains no timestamps
//! or random IDs, so golden tests are byte-stable. Neither crate is needed
//! until text or images arrive (M3).
//!
//! The page is sized to the drawing's bounds in millimetres (1 pt = 25.4/72 mm),
//! origin at the bounds' lower-left; PDF is Y-up like the document.

use crate::order::order;
use crate::pipeline::{ExportModel, PathSeg, Shape, collect};
use crate::svg::num;
use locus_doc::Document;
use locus_geom::{Arc, Vec2};
use std::f64::consts::FRAC_PI_2;
use std::fmt::Write as _;

const PT_PER_MM: f64 = 72.0 / 25.4;
/// Minimum page edge, mm, so an empty or one-line drawing still has a valid page.
const MIN_PAGE_MM: f64 = 1.0;
/// Stroke width, mm (matches the SVG writer).
const STROKE_MM: f64 = 0.1;

/// Document → PDF bytes: collect, order, write. No validation (see `validate`).
#[must_use]
pub fn to_pdf(doc: &Document) -> Vec<u8> {
    let mut model = collect(doc);
    order(&mut model);
    to_pdf_model(&model)
}

/// Write a prepared model as a one-page PDF 1.5.
#[must_use]
pub fn to_pdf_model(model: &ExportModel) -> Vec<u8> {
    let (min, w, h) = match model.bounds() {
        Some(b) => (b.min, b.width(), b.height()),
        None => (Vec2::ZERO, 0.0, 0.0),
    };
    let (w, h) = (w.max(MIN_PAGE_MM), h.max(MIN_PAGE_MM));
    let map = |p: Vec2| Vec2::new((p.x - min.x) * PT_PER_MM, (p.y - min.y) * PT_PER_MM);

    let layers: Vec<_> = model
        .layers
        .iter()
        .filter(|l| !l.paths.is_empty())
        .collect();

    // Object numbers: 1 catalog, 2 pages, 3 page, 4 content, then one OCG per layer.
    let ocg = |i: usize| 5 + i;

    let mut content = String::new();
    let _ = writeln!(content, "{} w 1 J 1 j", num(STROKE_MM * PT_PER_MM));
    for (i, l) in layers.iter().enumerate() {
        let [r, g, b] = l.colour.map(|c| f64::from(c) / 255.0);
        let _ = writeln!(content, "/OC /L{i} BDC");
        let _ = writeln!(content, "{} {} {} RG", num(r), num(g), num(b));
        for p in &l.paths {
            let s = map(p.start());
            let _ = writeln!(content, "{} {} m", num(s.x), num(s.y));
            for seg in &p.segs {
                write_seg(&mut content, seg, &map);
            }
            content.push_str(if p.closed { "h S\n" } else { "S\n" });
        }
        content.push_str("EMC\n");
    }

    let ids: Vec<String> = (0..layers.len())
        .map(|i| format!("{} 0 R", ocg(i)))
        .collect();
    let ids = ids.join(" ");
    let props: String = (0..layers.len())
        .map(|i| format!("/L{i} {} 0 R ", ocg(i)))
        .collect();

    let mut objs: Vec<String> = vec![
        format!(
            "<< /Type /Catalog /Pages 2 0 R /OCProperties << /OCGs [{ids}] /D << /Order [{ids}] /ON [{ids}] >> >> >>"
        ),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {} {}] /Contents 4 0 R /Resources << /Properties << {props}>> >> >>",
            num(w * PT_PER_MM),
            num(h * PT_PER_MM)
        ),
        format!(
            "<< /Length {} >>\nstream\n{content}endstream",
            content.len()
        ),
    ];
    for l in &layers {
        objs.push(format!(
            "<< /Type /OCG /Name {} >>",
            pdf_string(&format!("{} ({})", l.name, l.operation.kind().name()))
        ));
    }

    let mut out = b"%PDF-1.5\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offsets = Vec::with_capacity(objs.len());
    for (i, body) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    let mut tail = format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1);
    for o in &offsets {
        let _ = writeln!(tail, "{o:010} 00000 n ");
    }
    let _ = write!(
        tail,
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
        objs.len() + 1
    );
    out.extend_from_slice(tail.as_bytes());
    out
}

fn write_seg(out: &mut String, s: &PathSeg, map: &impl Fn(Vec2) -> Vec2) {
    match s.shape {
        Shape::Line(_) => {
            let e = map(s.end());
            let _ = writeln!(out, "{} {} l", num(e.x), num(e.y));
        }
        Shape::Arc(a) => {
            let mut curves = arc_beziers(&a);
            if s.reversed {
                curves.reverse();
                for c in &mut curves {
                    c.reverse();
                }
            }
            for [_, c1, c2, p3] in curves {
                let (c1, c2, p3) = (map(c1), map(c2), map(p3));
                let _ = writeln!(
                    out,
                    "{} {} {} {} {} {} c",
                    num(c1.x),
                    num(c1.y),
                    num(c2.x),
                    num(c2.y),
                    num(p3.x),
                    num(p3.y)
                );
            }
        }
        Shape::Cubic(c) => {
            let (c1, c2) = if s.reversed {
                (c.c1, c.c0)
            } else {
                (c.c0, c.c1)
            };
            let (c1, c2, p3) = (map(c1), map(c2), map(s.end()));
            let _ = writeln!(
                out,
                "{} {} {} {} {} {} c",
                num(c1.x),
                num(c1.y),
                num(c2.x),
                num(c2.y),
                num(p3.x),
                num(p3.y)
            );
        }
        Shape::Circle(c) => {
            // Four quarter arcs starting and ending at the angle-0 point.
            let arc = Arc::new(
                c.center,
                c.center + Vec2::new(c.radius, 0.0),
                c.center + Vec2::new(c.radius, 0.0),
            );
            for q in 0..4 {
                let a0 = f64::from(q) * FRAC_PI_2;
                for [_, c1, c2, p3] in bezier(&arc, a0, FRAC_PI_2) {
                    let (c1, c2, p3) = (map(c1), map(c2), map(p3));
                    let _ = writeln!(
                        out,
                        "{} {} {} {} {} {} c",
                        num(c1.x),
                        num(c1.y),
                        num(c2.x),
                        num(c2.y),
                        num(p3.x),
                        num(p3.y)
                    );
                }
            }
        }
    }
}

/// Cubic Béziers `[p0, c1, c2, p3]` approximating the arc, ≤ 90° each.
fn arc_beziers(a: &Arc) -> Vec<[Vec2; 4]> {
    let sweep = a.sweep();
    let n = ((sweep / FRAC_PI_2).ceil() as usize).max(1);
    let step = sweep / n as f64;
    (0..n)
        .flat_map(|k| bezier(a, a.start_angle() + step * k as f64, step))
        .collect()
}

/// One Bézier for the piece of `a`'s circle from angle `a0` spanning `span` (≤ 90°).
fn bezier(a: &Arc, a0: f64, span: f64) -> [[Vec2; 4]; 1] {
    let r = a.radius();
    let k = 4.0 / 3.0 * (span / 4.0).tan();
    let (d0, d1) = (Vec2::from_angle(a0), Vec2::from_angle(a0 + span));
    let (p0, p3) = (a.center + d0 * r, a.center + d1 * r);
    [[p0, p0 + d0.perp() * (k * r), p3 - d1.perp() * (k * r), p3]]
}

/// PDF string: literal for printable ASCII, UTF-16BE hex otherwise.
fn pdf_string(s: &str) -> String {
    if s.chars().all(|c| c.is_ascii() && !c.is_ascii_control()) {
        let mut o = String::from("(");
        for c in s.chars() {
            if matches!(c, '(' | ')' | '\\') {
                o.push('\\');
            }
            o.push(c);
        }
        o.push(')');
        o
    } else {
        let mut o = String::from("<FEFF");
        for u in s.encode_utf16() {
            let _ = write!(o, "{u:04X}");
        }
        o.push('>');
        o
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::testutil::*;
    use locus_doc::OperationKind;

    fn text(b: &[u8]) -> String {
        b.iter()
            .map(|&c| if c.is_ascii() { char::from(c) } else { '?' })
            .collect()
    }

    #[test]
    fn page_is_true_scale_with_one_ocg_per_layer() {
        let mut segs = rect(1, 0.0, 0.0, 100.0, 50.0);
        segs.extend(rect(5, 10.0, 10.0, 20.0, 20.0));
        let m = ExportModel {
            layers: vec![layer(OperationKind::Cut, segs)],
        };
        let t = text(&to_pdf_model(&m));
        assert!(t.starts_with("%PDF-1.5"));
        // 100 x 50 mm = 283.4646 x 141.7323 pt
        assert!(t.contains("/MediaBox [0 0 283.4646 141.7323]"), "{t}");
        assert_eq!(t.matches("/Type /OCG").count(), 1);
        assert!(t.contains("/OC /L0 BDC"));
        assert_eq!(t.matches("h S").count(), 2);
    }

    #[test]
    fn xref_offsets_point_at_objects() {
        let m = ExportModel {
            layers: vec![layer(OperationKind::Draw, rect(1, 0.0, 0.0, 10.0, 10.0))],
        };
        let b = to_pdf_model(&m);
        let t = text(&b);
        let sx: usize = t
            .rsplit("startxref\n")
            .next()
            .unwrap()
            .lines()
            .next()
            .unwrap()
            .parse()
            .unwrap();
        assert!(
            t[sx..].starts_with("xref\n"),
            "{sx} {:?}",
            &t[sx.saturating_sub(20)..(sx + 20).min(t.len())]
        );
        let entries: Vec<&str> = t[sx..]
            .lines()
            .skip(3)
            .take_while(|l| l.ends_with(" n "))
            .collect();
        assert_eq!(entries.len(), 5);
        for (i, e) in entries.iter().enumerate() {
            let off: usize = e[..10].parse().unwrap();
            assert!(
                t[off..].starts_with(&format!("{} 0 obj", i + 1)),
                "obj {}",
                i + 1
            );
        }
    }

    #[test]
    fn empty_model_still_a_valid_page() {
        let t = text(&to_pdf_model(&ExportModel::default()));
        assert!(t.contains("/MediaBox [0 0 2.8346 2.8346]"));
        assert!(t.contains("/OCGs []"));
    }

    #[test]
    fn strings_are_escaped() {
        assert_eq!(pdf_string("a(b)\\"), "(a\\(b\\)\\\\)");
        assert_eq!(pdf_string("é"), "<FEFF00E9>");
    }
}
