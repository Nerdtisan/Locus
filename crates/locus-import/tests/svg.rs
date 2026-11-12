// SPDX-License-Identifier: AGPL-3.0-or-later

//! SVG import (#241): app fixtures, hostile input and fuzzing.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use locus_doc::command::Editor;
use locus_doc::document::Document;
use locus_doc::operation::Operation;
use locus_doc::reference::{ImportShape, import_shapes_command};
use locus_import::{ImportError, MAX_SHAPES, import_svg};
use proptest::prelude::*;

const INKSCAPE: &[u8] = include_bytes!("fixtures/inkscape.svg");
const AFFINITY: &[u8] = include_bytes!("fixtures/affinity.svg");
const LIGHTBURN: &[u8] = include_bytes!("fixtures/lightburn.svg");

fn count(shapes: &[ImportShape], f: impl Fn(&ImportShape) -> bool) -> usize {
    shapes.iter().filter(|s| f(s)).count()
}

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-6
}

#[test]
fn inkscape_export_is_millimetres_and_y_up() {
    let r = import_svg(INKSCAPE).unwrap();
    // rect 4 + circle 1 + closed path 4 + arc 1 + 2 curves + rotated ellipse 4.
    assert_eq!(r.shapes.len(), 16, "{:?}", r.shapes);
    assert_eq!(
        count(&r.shapes, |s| matches!(s, ImportShape::Circle { .. })),
        1
    );
    // The circle: (100,50) r15 on a 297 mm page, flipped to Y-up.
    let ImportShape::Circle { center, radius } = r
        .shapes
        .iter()
        .copied()
        .find(|s| matches!(s, ImportShape::Circle { .. }))
        .unwrap()
    else {
        unreachable!()
    };
    assert!(near(center.x, 100.0) && near(center.y, 247.0) && near(radius, 15.0));
    // `a 20,20 0 0 1 40,0` bulges up on screen = clockwise in Y-up, so the
    // counter-clockwise arc runs from the right end to the left end.
    let arc = r
        .shapes
        .iter()
        .find_map(|s| match s {
            ImportShape::Arc { center, start, end } => Some((*center, *start, *end)),
            _ => None,
        })
        .unwrap();
    assert!(near(arc.0.x, 140.0) && near(arc.0.y, 177.0));
    assert!(near(arc.1.x, 160.0) && near(arc.2.x, 120.0));
}

#[test]
fn affinity_export_scales_px_and_skips_external_use() {
    let r = import_svg(AFFINITY).unwrap();
    // 100% width/height: one user unit is one CSS px.
    let px = 25.4 / 96.0;
    // square 4 + circle 1 + two curves.
    assert_eq!(r.shapes.len(), 7, "{:?}", r.shapes);
    let radius = r
        .shapes
        .iter()
        .find_map(|s| match s {
            ImportShape::Circle { radius, .. } => Some(*radius),
            _ => None,
        })
        .unwrap();
    assert!(near(radius, 80.0 * px));
    assert!(
        r.warnings.iter().any(|w| w.contains("<use>")),
        "{:?}",
        r.warnings
    );
}

#[test]
fn lightburn_export_keeps_arcs_and_connects_paths() {
    let r = import_svg(LIGHTBURN).unwrap();
    // rect 4 + two semicircles + polyline 2.
    assert_eq!(r.shapes.len(), 8, "{:?}", r.shapes);
    assert_eq!(
        count(&r.shapes, |s| matches!(s, ImportShape::Arc { .. })),
        2
    );
    // viewBox is already mm: the first corner (10,10) → (10, 70) Y-up.
    assert!(matches!(
        r.shapes[0],
        ImportShape::Line { a, b } if near(a.x, 10.0) && near(a.y, 70.0) && near(b.x, 90.0)
    ));
}

#[test]
fn imports_as_one_locked_reference_layer() {
    let r = import_svg(LIGHTBURN).unwrap();
    let mut ed = Editor::new(Document::new());
    let cmd = import_shapes_command(ed.document(), "lightburn.svg", &r.shapes).unwrap();
    let applied = ed.apply(&cmd).unwrap();
    let l = ed.document().layer(applied.layers[0]).unwrap();
    assert!(l.locked);
    assert_eq!(l.operation, Operation::Reference);
    ed.undo().unwrap();
    assert!(ed.document().entities().is_empty());
}

#[test]
fn path_commands_and_transforms() {
    let svg =
        br#"<svg xmlns="http://www.w3.org/2000/svg" width="10mm" height="10mm" viewBox="0 0 10 10">
      <path d="M1 1 Q 3 -1 5 1 T 9 1 M 1 5 l 2 0 2 0" transform="translate(0 2) scale(1)"/>
      <path d="M 2 8 a1 1 0 00 2 0"/>
      <path d="M0 0 L 5 5 BAD 9 9"/>
    </svg>"#;
    let r = import_svg(svg).unwrap();
    // Q, T (cubics), two lines, one arc, one line before BAD stops the path.
    assert_eq!(r.shapes.len(), 6, "{:?}", r.shapes);
    assert!(!r.warnings.is_empty());
}

#[test]
fn elliptical_arc_with_nonuniform_scale_becomes_beziers() {
    let svg =
        br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10" width="10" height="10">
      <g transform="scale(2 1)"><circle cx="2" cy="2" r="1"/></g></svg>"#;
    let r = import_svg(svg).unwrap();
    assert_eq!(
        count(&r.shapes, |s| matches!(s, ImportShape::Cubic { .. })),
        4
    );
}

#[test]
fn rejects_non_svg_and_empty() {
    assert_eq!(import_svg(b"hello").unwrap_err(), ImportError::BadSvg);
    assert_eq!(import_svg(b"<html/>").unwrap_err(), ImportError::BadSvg);
    assert_eq!(
        import_svg(&[0xff, 0xfe, 0x00]).unwrap_err(),
        ImportError::BadSvg
    );
    assert_eq!(
        import_svg(b"<svg xmlns='http://www.w3.org/2000/svg'/>").unwrap_err(),
        ImportError::Empty
    );
}

#[test]
fn external_entities_are_never_loaded() {
    let svg = br#"<?xml version="1.0"?>
<!DOCTYPE svg [<!ENTITY x SYSTEM "file:///etc/passwd">]>
<svg xmlns="http://www.w3.org/2000/svg"><path d="M0 0 L1 1"/><title>&x;</title></svg>"#;
    // Either rejected or parsed without the entity: never an I/O attempt.
    if let Ok(r) = import_svg(svg) {
        assert_eq!(r.shapes.len(), 1);
    }
}

#[test]
fn entity_bombs_are_refused() {
    let mut s = String::from("<?xml version=\"1.0\"?><!DOCTYPE svg [<!ENTITY a0 \"lol\">");
    for i in 1..12 {
        let prev = format!("&a{};", i - 1);
        s.push_str(&format!("<!ENTITY a{i} \"{}\">", prev.repeat(10)));
    }
    s.push_str(
        "]><svg xmlns=\"http://www.w3.org/2000/svg\"><title>&a11;</title><path d=\"M0 0L1 1\"/></svg>",
    );
    // Must terminate quickly, error or not.
    let _ = import_svg(s.as_bytes());
}

#[test]
fn too_much_geometry_is_an_error_not_a_crash() {
    let mut d = String::from("M0 0");
    for _ in 0..=MAX_SHAPES {
        d.push_str(" l1 1");
    }
    let svg = format!("<svg xmlns=\"http://www.w3.org/2000/svg\"><path d=\"{d}\"/></svg>");
    assert_eq!(
        import_svg(svg.as_bytes()).unwrap_err(),
        ImportError::TooMuchGeometry
    );
}

#[test]
fn oversized_input_is_refused() {
    let big = vec![b' '; locus_import::MAX_INPUT_BYTES + 1];
    assert_eq!(import_svg(&big).unwrap_err(), ImportError::TooLarge);
}

#[test]
fn use_expansion_is_bounded() {
    // Doubling `use` chain: 2^40 instances if followed naively.
    let mut s = String::from(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\"><defs><path id=\"p0\" d=\"M0 0L1 1\"/>",
    );
    for i in 1..40 {
        s.push_str(&format!(
            "<g id=\"p{i}\"><use xlink:href=\"#p{}\"/><use xlink:href=\"#p{}\"/></g>",
            i - 1,
            i - 1
        ));
    }
    s.push_str("</defs><use xlink:href=\"#p39\"/></svg>");
    assert!(import_svg(s.as_bytes()).is_err());
}

#[test]
fn self_referencing_use_terminates() {
    let svg =
        br##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink">
      <g id="a"><path d="M0 0L1 1"/><use xlink:href="#a"/></g></svg>"##;
    let _ = import_svg(svg);
}

#[test]
fn non_finite_and_huge_coordinates_are_dropped() {
    let svg = br#"<svg xmlns="http://www.w3.org/2000/svg"><path d="M0 0 L1e300 1e300 L5 5"/><path d="M0 0 L1e999 0"/></svg>"#;
    if let Ok(r) = import_svg(svg) {
        for s in &r.shapes {
            if let ImportShape::Line { a, b } = s {
                assert!(a.x.is_finite() && b.x.is_finite() && b.x.abs() <= 1e6);
            }
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn random_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
        let _ = import_svg(&bytes);
    }

    #[test]
    fn random_path_data_never_panics(d in "[MmLlHhVvCcSsQqTtAaZz0-9 .,eE+-]{0,200}") {
        let svg = format!("<svg xmlns=\"http://www.w3.org/2000/svg\"><path d=\"{d}\"/></svg>");
        let _ = import_svg(svg.as_bytes());
    }

    #[test]
    fn random_transforms_never_panic(t in "[a-zA-Z0-9 .,()eE+-]{0,80}") {
        let svg = format!("<svg xmlns=\"http://www.w3.org/2000/svg\"><path transform=\"{t}\" d=\"M0 0 L1 1 A1 2 30 1 1 4 4\"/></svg>");
        let _ = import_svg(svg.as_bytes());
    }

    #[test]
    fn mutated_fixtures_never_panic(
        which in 0usize..3,
        cuts in proptest::collection::vec((any::<prop::sample::Index>(), any::<u8>()), 0..8),
    ) {
        let mut b = [INKSCAPE, AFFINITY, LIGHTBURN][which].to_vec();
        for (i, v) in cuts {
            let n = b.len();
            b[i.index(n)] = v;
        }
        let _ = import_svg(&b);
    }
}
