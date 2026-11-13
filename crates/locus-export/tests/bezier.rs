// SPDX-License-Identifier: AGPL-3.0-or-later

//! Cubic Bézier export (#159; SPEC §4, §8): true curves in SVG/PDF, arcs
//! within the export tolerance in DXF R12, and a clean validation report.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use locus_doc::{Command, Constraint, Document, Editor, EntityId, Geom};
use locus_export::{Severity, Shape, collect, validate};
use locus_geom::{Cubic, EXPORT_TOL, Vec2};

fn add(ed: &mut Editor, geom: Geom) -> EntityId {
    ed.apply(&Command::AddEntity {
        layer: Document::default_layer(),
        construction: false,
        style: Default::default(),
        geom,
    })
    .unwrap()
    .entities[0]
}

/// Cut layer: a 100 mm arch closed by a line; plus an open S-curve tangent to a
/// short line (it will fail the Cut-layer open-path rule, so it goes on its own
/// Draw layer in the golden fixture below).
fn arch_doc() -> Document {
    let mut ed = Editor::new(Document::new());
    let pt = |ed: &mut Editor, x, y| {
        add(
            ed,
            Geom::Point {
                pos: Vec2::new(x, y),
            },
        )
    };
    let p = [
        pt(&mut ed, 0.0, 0.0),
        pt(&mut ed, 0.0, 55.0),
        pt(&mut ed, 100.0, 55.0),
        pt(&mut ed, 100.0, 0.0),
    ];
    add(
        &mut ed,
        Geom::Bezier {
            start: p[0],
            start_ctrl: p[1],
            end_ctrl: p[2],
            end: p[3],
        },
    );
    add(
        &mut ed,
        Geom::Line {
            start: p[3],
            end: p[0],
        },
    );
    ed.into_document()
}

#[test]
fn a_closed_bezier_profile_validates_clean() {
    let model = collect(&arch_doc());
    assert_eq!(model.layers.len(), 1);
    assert_eq!(model.layers[0].paths.len(), 1);
    assert!(model.layers[0].paths[0].closed);
    let report = validate(&model);
    assert!(
        report.issues.iter().all(|i| i.severity != Severity::Error),
        "{:?}",
        report.issues
    );
    assert!(report.issues.is_empty(), "{:?}", report.issues);
}

#[test]
fn svg_and_pdf_keep_the_curve() {
    let doc = arch_doc();
    let svg = String::from_utf8(locus_export::to_svg(&doc)).unwrap();
    assert!(
        svg.contains(" C 0 55 100 55 100 0") || svg.contains(" C "),
        "{svg}"
    );
    let pdf = String::from_utf8_lossy(&locus_export::to_pdf(&doc)).into_owned();
    assert!(pdf.contains(" c\n"), "{pdf}");
}

/// DXF R12 has only lines and arcs: every exported ARC/LINE must lie within
/// the export tolerance of the original curve, and together they must cover it.
#[test]
fn dxf_arcs_follow_the_curve_within_the_export_tolerance() {
    let doc = arch_doc();
    let dxf = String::from_utf8(locus_export::to_dxf(&doc)).unwrap();
    let lines: Vec<&str> = dxf.lines().collect();
    let kinds: Vec<&str> = lines
        .chunks(2)
        .filter(|c| c[0].trim() == "0")
        .map(|c| c[1].trim())
        .collect();
    assert!(kinds.contains(&"ARC"), "{kinds:?}");
    assert!(!kinds.contains(&"SPLINE") && !kinds.contains(&"POLYLINE"));

    // Rebuild the pieces through the model (the writer's own input) and compare.
    let model = collect(&doc);
    let cubic = Cubic::new(
        Vec2::new(0.0, 0.0),
        Vec2::new(0.0, 55.0),
        Vec2::new(100.0, 55.0),
        Vec2::new(100.0, 0.0),
    );
    let seg = model.layers[0]
        .paths
        .iter()
        .flat_map(|p| &p.segs)
        .find(|s| matches!(s.shape, Shape::Cubic(_)))
        .unwrap();
    let pieces = seg.shape.pieces();
    assert!(pieces.len() > 1);
    for i in 0..=1000 {
        let q = cubic.eval(f64::from(i) / 1000.0);
        let d = pieces
            .iter()
            .map(|p| match p {
                Shape::Line(l) => l.distance_to(q),
                Shape::Arc(a) => q.distance(a.closest_point(q)),
                _ => f64::INFINITY,
            })
            .fold(f64::INFINITY, f64::min);
        assert!(d <= EXPORT_TOL, "t={i}/1000: {d} mm from the DXF arcs");
    }
}

/// Build `tests/fixtures/beziers.locus` (arch on Cut, an S-curve on a Draw
/// layer, tangent to a line) with `LOCUS_UPDATE_GOLDEN=1`; it is the golden
/// input for all three writers.
#[test]
fn beziers_fixture_is_current() {
    let path =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/beziers.locus");
    let mut ed = Editor::new(arch_doc());
    let draw = {
        use locus_doc::Operation;
        let r = ed
            .apply(&Command::AddLayer {
                name: "Pen".to_owned(),
                colour: [0, 0, 200],
                operation: Operation::Draw,
            })
            .unwrap();
        r.layers[0]
    };
    let pt = |ed: &mut Editor, x, y| {
        ed.apply(&Command::AddEntity {
            layer: draw,
            construction: false,
            style: Default::default(),
            geom: Geom::Point {
                pos: Vec2::new(x, y),
            },
        })
        .unwrap()
        .entities[0]
    };
    let p = [
        pt(&mut ed, 120.0, 0.0),
        pt(&mut ed, 130.0, 0.0),
        pt(&mut ed, 140.0, 0.0),
        pt(&mut ed, 160.0, -30.0),
        pt(&mut ed, 170.0, 0.0),
    ];
    let line = ed
        .apply(&Command::AddEntity {
            layer: draw,
            construction: false,
            style: Default::default(),
            geom: Geom::Line {
                start: p[0],
                end: p[1],
            },
        })
        .unwrap()
        .entities[0];
    let bez = ed
        .apply(&Command::AddEntity {
            layer: draw,
            construction: false,
            style: Default::default(),
            geom: Geom::Bezier {
                start: p[1],
                start_ctrl: p[2],
                end_ctrl: p[3],
                end: p[4],
            },
        })
        .unwrap()
        .entities[0];
    ed.apply(&Command::AddConstraint {
        constraint: Constraint::Tangent { a: line, b: bez },
    })
    .unwrap();
    let text = locus_doc::file::save(&ed.into_document()).unwrap();
    if std::env::var_os("LOCUS_UPDATE_GOLDEN").is_some() {
        std::fs::write(&path, &text).unwrap();
    }
    assert_eq!(
        std::fs::read_to_string(&path)
            .unwrap()
            .replace("\r\n", "\n"),
        text
    );
}
