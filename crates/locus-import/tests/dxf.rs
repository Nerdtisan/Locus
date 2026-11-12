// SPDX-License-Identifier: AGPL-3.0-or-later

//! DXF import (#242): app fixtures, hostile input and fuzzing.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use locus_doc::command::Editor;
use locus_doc::document::Document;
use locus_doc::operation::Operation;
use locus_doc::reference::{ImportShape, import_shapes_command};
use locus_import::{ImportError, MAX_INPUT_BYTES, import_dxf};
use proptest::prelude::*;

const LIBRECAD: &[u8] = include_bytes!("fixtures/librecad.dxf");
const FUSION: &[u8] = include_bytes!("fixtures/fusion.dxf");
const SILHOUETTE: &[u8] = include_bytes!("fixtures/silhouette.dxf");

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-6
}

fn count(shapes: &[ImportShape], f: impl Fn(&ImportShape) -> bool) -> usize {
    shapes.iter().filter(|s| f(s)).count()
}

#[test]
fn librecad_lwpolyline_bulge_is_a_true_arc() {
    let r = import_dxf(LIBRECAD).unwrap();
    // line + circle + arc + (bulge arc + 3 lines).
    assert_eq!(r.shapes.len(), 7, "{:?}", r.shapes);
    // bulge 1 from (30,0) to (40,0): a counter-clockwise semicircle about (35,0).
    let found = r.shapes.iter().any(|s| {
        matches!(s, ImportShape::Arc { center, start, end }
            if near(center.x, 35.0) && near(center.y, 0.0)
                && near(start.x, 30.0) && near(end.x, 40.0))
    });
    assert!(found, "{:?}", r.shapes);
    // The ARC entity: centre (0,20), 0°→90°.
    assert!(r.shapes.iter().any(|s| {
        matches!(s, ImportShape::Arc { center, start, end }
            if near(center.y, 20.0) && near(start.x, 5.0) && near(end.y, 25.0))
    }));
}

#[test]
fn fusion_blocks_are_flattened_and_splines_approximated() {
    let r = import_dxf(FUSION).unwrap();
    // 2 + 4 (array of two) + 12 spline segments + 4 ellipse pieces.
    assert_eq!(r.shapes.len(), 22, "{:?}", r.shapes);
    // INSERT rotated 90° at (100,0): the block line (0,0)-(5,0) → (100,0)-(100,5).
    assert!(r.shapes.iter().any(|s| {
        matches!(s, ImportShape::Line { a, b }
            if near(a.x, 100.0) && near(a.y, 0.0) && near(b.x, 100.0) && near(b.y, 5.0))
    }));
    // Array copy 20 mm along X: circle at (2,2) → (22,52).
    assert!(r.shapes.iter().any(|s| {
        matches!(s, ImportShape::Circle { center, radius }
            if near(center.x, 22.0) && near(center.y, 52.0) && near(*radius, 1.0))
    }));
    assert_eq!(
        count(&r.shapes, |s| matches!(s, ImportShape::Cubic { .. })),
        4
    );
    assert!(r.warnings.iter().any(|w| w.contains("SPLINE")));
    assert!(r.warnings.iter().any(|w| w.contains("TEXT")));
}

#[test]
fn spline_endpoints_are_the_control_polygon_ends() {
    let r = import_dxf(FUSION).unwrap();
    // The clamped cubic starts at (0,80) and ends at (30,80).
    assert!(r.shapes.iter().any(|s| {
        matches!(s, ImportShape::Line { a, .. } if near(a.x, 0.0) && near(a.y, 80.0))
    }));
    assert!(r.shapes.iter().any(|s| {
        matches!(s, ImportShape::Line { b, .. } if near(b.x, 30.0) && near(b.y, 80.0))
    }));
}

#[test]
fn silhouette_r12_inches_and_negative_bulge() {
    let r = import_dxf(SILHOUETTE).unwrap();
    // closed polyline: arc + 3 lines, then circle and line.
    assert_eq!(r.shapes.len(), 6, "{:?}", r.shapes);
    // $INSUNITS 1: inches → mm. Bulge -1 from (0,0) to (1,0) is clockwise
    // over the top, i.e. counter-clockwise from (25.4,0) back to (0,0).
    assert!(r.shapes.iter().any(|s| {
        matches!(s, ImportShape::Arc { center, start, end }
            if near(center.x, 12.7) && near(center.y, 0.0)
                && near(start.x, 25.4) && near(end.x, 0.0))
    }));
    assert!(
        r.shapes
            .iter()
            .any(|s| { matches!(s, ImportShape::Circle { radius, .. } if near(*radius, 12.7)) })
    );
}

#[test]
fn imports_as_one_locked_reference_layer() {
    let r = import_dxf(LIBRECAD).unwrap();
    let mut ed = Editor::new(Document::new());
    let cmd = import_shapes_command(ed.document(), "a.dxf", &r.shapes).unwrap();
    let applied = ed.apply(&cmd).unwrap();
    let l = ed.document().layer(applied.layers[0]).unwrap();
    assert!(l.locked);
    assert_eq!(l.operation, Operation::Reference);
}

#[test]
fn rejects_non_dxf_and_binary() {
    assert_eq!(import_dxf(b"hello world").unwrap_err(), ImportError::BadDxf);
    assert_eq!(import_dxf(b"").unwrap_err(), ImportError::BadDxf);
    assert_eq!(
        import_dxf(b"AutoCAD Binary DXF\r\n\x1a\0").unwrap_err(),
        ImportError::BinaryDxf
    );
    assert_eq!(
        import_dxf(b"0\nSECTION\n2\nENTITIES\n0\nENDSEC\n0\nEOF\n").unwrap_err(),
        ImportError::Empty
    );
    let big = vec![b'0'; MAX_INPUT_BYTES + 1];
    assert_eq!(import_dxf(&big).unwrap_err(), ImportError::TooLarge);
}

fn wrap(entities: &str) -> Vec<u8> {
    format!("0\nSECTION\n2\nENTITIES\n{entities}0\nENDSEC\n0\nEOF\n").into_bytes()
}

#[test]
fn block_recursion_and_bombs_are_bounded() {
    // A block that inserts itself.
    let s = "0\nSECTION\n2\nBLOCKS\n0\nBLOCK\n2\nA\n10\n0\n20\n0\n0\nLINE\n10\n0\n20\n0\n11\n1\n21\n1\n0\nINSERT\n2\nA\n0\nENDBLK\n0\nENDSEC\n0\nSECTION\n2\nENTITIES\n0\nINSERT\n2\nA\n0\nENDSEC\n0\nEOF\n";
    let _ = import_dxf(s.as_bytes());

    // Doubling chain of blocks, each inserting the previous twice.
    let mut s = String::from("0\nSECTION\n2\nBLOCKS\n");
    s.push_str("0\nBLOCK\n2\nB0\n0\nLINE\n10\n0\n20\n0\n11\n1\n21\n1\n0\nENDBLK\n");
    for i in 1..40 {
        s.push_str(&format!(
            "0\nBLOCK\n2\nB{i}\n0\nINSERT\n2\nB{p}\n0\nINSERT\n2\nB{p}\n0\nENDBLK\n",
            p = i - 1
        ));
    }
    s.push_str("0\nENDSEC\n0\nSECTION\n2\nENTITIES\n0\nINSERT\n2\nB39\n0\nENDSEC\n0\nEOF\n");
    assert!(import_dxf(s.as_bytes()).is_err());
}

#[test]
fn huge_array_is_capped() {
    let s = wrap("0\nLINE\n10\n0\n20\n0\n11\n1\n21\n0\n");
    assert!(import_dxf(&s).is_ok());
    let arr = "0\nSECTION\n2\nBLOCKS\n0\nBLOCK\n2\nA\n0\nLINE\n10\n0\n20\n0\n11\n1\n21\n0\n0\nENDBLK\n0\nENDSEC\n0\nSECTION\n2\nENTITIES\n0\nINSERT\n2\nA\n70\n1000000\n71\n1000000\n44\n2\n45\n2\n0\nENDSEC\n0\nEOF\n";
    let r = import_dxf(arr.as_bytes()).unwrap();
    assert!(r.shapes.len() <= 10_000);
}

#[test]
fn bad_numbers_skip_the_entity_not_the_file() {
    let s = wrap(
        "0\nLINE\n10\nabc\n20\n0\n11\n1\n21\n0\n0\nLINE\n10\n0\n20\n0\n11\n5\n21\n0\n0\nCIRCLE\n10\n0\n20\n0\n40\n-3\n0\nCIRCLE\n10\n0\n20\n0\n40\nNaN\n",
    );
    let r = import_dxf(&s).unwrap();
    assert_eq!(r.shapes.len(), 1);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn random_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
        let _ = import_dxf(&bytes);
    }

    #[test]
    fn random_entities_never_panic(
        kind in prop::sample::select(vec!["LINE","ARC","CIRCLE","ELLIPSE","LWPOLYLINE","POLYLINE","VERTEX","SEQEND","SPLINE","INSERT"]),
        body in proptest::collection::vec((0i32..80, "[-0-9.eE+a-zA-Z ]{0,8}"), 0..40),
    ) {
        let mut e = format!("0\n{kind}\n");
        for (c, v) in body {
            if c != 0 {
                e.push_str(&format!("{c}\n{v}\n"));
            }
        }
        let _ = import_dxf(&wrap(&e));
    }

    #[test]
    fn mutated_fixtures_never_panic(
        which in 0usize..3,
        cuts in proptest::collection::vec((any::<prop::sample::Index>(), any::<u8>()), 0..12),
    ) {
        let mut b = [LIBRECAD, FUSION, SILHOUETTE][which].to_vec();
        for (i, v) in cuts {
            let n = b.len();
            b[i.index(n)] = v;
        }
        let _ = import_dxf(&b);
    }
}
