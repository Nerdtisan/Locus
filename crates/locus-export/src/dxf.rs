// SPDX-License-Identifier: AGPL-3.0-or-later

//! DXF R12 (AC1009) writer (SPEC §8 Formats, #130).
//!
//! Hand-written rather than the `dxf` crate: R12 needs only the LINE, ARC and
//! CIRCLE entities plus a LAYER table, which is ~100 lines, adds no dependency
//! (so nothing for `cargo deny` or the wasm32 build to object to) and gives
//! byte-stable output for golden tests.
//!
//! The document is Y-up in millimetres, exactly DXF's model space, so no
//! transform is applied. DXF arcs are always counter-clockwise, matching
//! SPEC §4; path direction is therefore not encoded (ordering still is).

use crate::order::order;
use crate::pipeline::{ExportModel, Shape, collect};
use crate::svg::num;
use locus_doc::Document;
use locus_geom::Vec2;
use std::collections::BTreeSet;
use std::fmt::Write as _;

/// AutoCAD Color Index entries used to approximate a layer colour, with their RGB.
const ACI: [(u8, [u8; 3]); 9] = [
    (1, [255, 0, 0]),
    (2, [255, 255, 0]),
    (3, [0, 255, 0]),
    (4, [0, 255, 255]),
    (5, [0, 0, 255]),
    (6, [255, 0, 255]),
    (7, [0, 0, 0]),
    (8, [128, 128, 128]),
    (9, [192, 192, 192]),
];

/// Document → DXF bytes: collect, order, write. No validation (see `validate`).
#[must_use]
pub fn to_dxf(doc: &Document) -> Vec<u8> {
    let mut model = collect(doc);
    order(&mut model);
    to_dxf_model(&model)
}

/// Write a prepared model as DXF R12.
#[must_use]
pub fn to_dxf_model(model: &ExportModel) -> Vec<u8> {
    let layers: Vec<&crate::pipeline::ExportLayer> = model
        .layers
        .iter()
        .filter(|l| !l.paths.is_empty())
        .collect();
    let names = unique_names(&layers);

    let mut o = String::new();
    // HEADER. $INSUNITS/$MEASUREMENT postdate R12 but readers ignore unknown
    // variables and LightBurn/CAD packages use them to pick millimetres.
    pair(&mut o, 0, "SECTION");
    pair(&mut o, 2, "HEADER");
    pair(&mut o, 9, "$ACADVER");
    pair(&mut o, 1, "AC1009");
    pair(&mut o, 9, "$INSUNITS");
    pair(&mut o, 70, "4");
    pair(&mut o, 9, "$MEASUREMENT");
    pair(&mut o, 70, "1");
    pair(&mut o, 0, "ENDSEC");

    // TABLES: the one linetype the layers reference, then the layers.
    pair(&mut o, 0, "SECTION");
    pair(&mut o, 2, "TABLES");
    pair(&mut o, 0, "TABLE");
    pair(&mut o, 2, "LTYPE");
    pair(&mut o, 70, "1");
    pair(&mut o, 0, "LTYPE");
    pair(&mut o, 2, "CONTINUOUS");
    pair(&mut o, 70, "0");
    pair(&mut o, 3, "Solid line");
    pair(&mut o, 72, "65");
    pair(&mut o, 73, "0");
    pair(&mut o, 40, "0");
    pair(&mut o, 0, "ENDTAB");
    pair(&mut o, 0, "TABLE");
    pair(&mut o, 2, "LAYER");
    pair(&mut o, 70, &layers.len().to_string());
    for (l, name) in layers.iter().zip(&names) {
        pair(&mut o, 0, "LAYER");
        pair(&mut o, 2, name);
        pair(&mut o, 70, "0");
        pair(&mut o, 62, &aci(l.colour).to_string());
        pair(&mut o, 6, "CONTINUOUS");
    }
    pair(&mut o, 0, "ENDTAB");
    pair(&mut o, 0, "ENDSEC");

    pair(&mut o, 0, "SECTION");
    pair(&mut o, 2, "ENTITIES");
    for (l, name) in layers.iter().zip(&names) {
        for p in &l.paths {
            for s in &p.segs {
                // R12 has no splines: a Bézier becomes a chain of lines and
                // arcs within the export tolerance (SPEC §8), in travel order.
                let shapes = match s.shape {
                    Shape::Cubic(_) => {
                        let mut v = s.shape.pieces();
                        if s.reversed {
                            v.reverse();
                        }
                        v
                    }
                    other => vec![other],
                };
                for shape in shapes {
                    match shape {
                        Shape::Cubic(_) => {}
                        Shape::Line(l) => {
                            // Honour the path direction for lines (arcs are CCW-only).
                            let (a, b) = if matches!(s.shape, Shape::Cubic(_)) {
                                if s.reversed { (l.b, l.a) } else { (l.a, l.b) }
                            } else {
                                (s.start(), s.end())
                            };
                            entity(&mut o, "LINE", name);
                            point(&mut o, 10, a);
                            point(&mut o, 11, b);
                        }
                        Shape::Arc(a) => {
                            entity(&mut o, "ARC", name);
                            point(&mut o, 10, a.center);
                            pair(&mut o, 40, &num(a.radius()));
                            pair(
                                &mut o,
                                50,
                                &num(a.start_angle().to_degrees().rem_euclid(360.0)),
                            );
                            pair(
                                &mut o,
                                51,
                                &num(a.end_angle().to_degrees().rem_euclid(360.0)),
                            );
                        }
                        Shape::Circle(c) => {
                            entity(&mut o, "CIRCLE", name);
                            point(&mut o, 10, c.center);
                            pair(&mut o, 40, &num(c.radius));
                        }
                    }
                }
            }
        }
    }
    pair(&mut o, 0, "ENDSEC");
    pair(&mut o, 0, "EOF");
    o.into_bytes()
}

fn pair(o: &mut String, code: i32, value: &str) {
    let _ = write!(o, "{code:>3}\n{value}\n");
}

fn entity(o: &mut String, kind: &str, layer: &str) {
    pair(o, 0, kind);
    pair(o, 8, layer);
}

/// Group codes `code`, `code+10` and `code+20` for X, Y and Z (always 0).
fn point(o: &mut String, code: i32, p: Vec2) {
    pair(o, code, &num(p.x));
    pair(o, code + 10, &num(p.y));
    pair(o, code + 20, "0");
}

/// Nearest ACI colour by RGB distance.
fn aci(rgb: [u8; 3]) -> u8 {
    let d = |c: [u8; 3]| {
        c.iter()
            .zip(rgb)
            .map(|(a, b)| (i32::from(*a) - i32::from(b)).pow(2))
            .sum::<i32>()
    };
    ACI.iter().min_by_key(|(_, c)| d(*c)).map_or(7, |(i, _)| *i)
}

/// R12 layer names: ASCII letters, digits, `$`, `-`, `_`, at most 31 characters,
/// unique case-insensitively. Anything else becomes `_`; clashes get the layer id.
fn unique_names(layers: &[&crate::pipeline::ExportLayer]) -> Vec<String> {
    let mut seen = BTreeSet::new();
    layers
        .iter()
        .map(|l| {
            let mut n: String = l
                .name
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || matches!(c, '$' | '-' | '_') {
                        c
                    } else {
                        '_'
                    }
                })
                .take(31)
                .collect();
            if n.is_empty() || n == "0" || !seen.insert(n.to_ascii_uppercase()) {
                let suffix = format!("_{}", l.id);
                n.truncate(31 - suffix.len().min(31));
                n.push_str(&suffix);
                seen.insert(n.to_ascii_uppercase());
            }
            n
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::testutil::*;
    use locus_doc::OperationKind;

    #[test]
    fn empty_model_is_a_valid_skeleton() {
        let s = String::from_utf8(to_dxf_model(&ExportModel::default())).unwrap();
        assert!(s.starts_with("  0\nSECTION"));
        assert!(s.ends_with("  0\nEOF\n"));
        assert!(s.contains("AC1009"));
    }

    #[test]
    fn rectangle_is_four_lines_on_a_named_layer() {
        let m = ExportModel {
            layers: vec![layer(OperationKind::Cut, rect(1, 0.0, 0.0, 10.0, 5.0))],
            ..Default::default()
        };
        let s = String::from_utf8(to_dxf_model(&m)).unwrap();
        assert_eq!(s.matches("  0\nLINE\n").count(), 4);
        assert!(s.contains("  8\nL\n"));
    }

    #[test]
    fn aci_picks_nearest() {
        assert_eq!(aci([250, 10, 10]), 1);
        assert_eq!(aci([0, 0, 200]), 5);
        assert_eq!(aci([0, 0, 0]), 7);
    }

    #[test]
    fn layer_names_are_sanitised_and_unique() {
        let mut a = layer(OperationKind::Cut, rect(1, 0.0, 0.0, 1.0, 1.0));
        a.name = "Cut & <score>".into();
        let mut b = a.clone();
        b.id = locus_doc::LayerId::new(2);
        let names = unique_names(&[&a, &b]);
        assert_eq!(names[0], "Cut____score_");
        assert_eq!(names[1], "Cut____score__2");
    }
}
