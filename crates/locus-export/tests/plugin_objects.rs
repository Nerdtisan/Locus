// SPDX-License-Identifier: AGPL-3.0-or-later

//! A document with a plugin object opens and exports without the plugin
//! (SPEC §15.5, #183): the cached geometry is all the export needs.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use locus_export::{Shape, prepare};
use locus_geom::Vec2;

#[test]
fn plugin_object_exports_from_its_cache() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../locus-doc/tests/fixtures/v10/plate.locus"
    );
    let doc = locus_doc::file::load(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert!(
        doc.recipes()
            .values()
            .any(|r| matches!(r.kind, locus_doc::RecipeKind::Generated { .. }))
    );
    let prepared = prepare(&doc, false);
    // The 10 mm square the plugin drew at (60, 10) is a closed path of four
    // lines (moved out by half the layer's 0.15 mm kerf, corners rounded).
    let corner = |s: &Shape| match s {
        Shape::Line(l) => [l.a, l.b]
            .iter()
            .any(|p| (*p - Vec2::new(70.0, 20.0)).length() < 0.1),
        _ => false,
    };
    let path = prepared
        .model
        .layers
        .iter()
        .flat_map(|l| &l.paths)
        .find(|p| p.segs.iter().any(|s| corner(&s.shape)))
        .expect("the plugin object's square is exported");
    assert!(path.closed);
    let lines = path
        .segs
        .iter()
        .filter(|s| matches!(s.shape, Shape::Line(_)))
        .count();
    assert_eq!(lines, 4, "four sides (plus the kerf's corner arcs)");
    let svg = String::from_utf8(locus_export::to_svg_model(&prepared.model)).unwrap();
    assert!(svg.contains("<path") || svg.contains("<polygon") || svg.contains("<rect"));
}
