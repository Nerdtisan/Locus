// SPDX-License-Identifier: AGPL-3.0-or-later

//! Region fills per layer (#457, SPEC §6.3, §8): the owner's file, a red
//! filled circle on Layer 1 and an overlapping circle on Layer 2. The red
//! fill is the whole Layer 1 circle in its stored colour, and only in Layer
//! 1's file; Layer 2's file has only its circle. Both files share one page.
//!
//! Regenerate after an intended change with
//! `LOCUS_UPDATE_GOLDEN=1 cargo test -p locus-export --test per_layer_fills_golden`
//! and review the diff of `tests/fixtures/per-layer-fills/`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use locus_doc::region_fill::RegionFill;
use locus_doc::{Command, Document, Editor, Fill, Geom, Operation, Style};
use locus_export::{prepare, split_layers, to_svg_model};
use locus_geom::Vec2;
use locus_region::RegionKey;
use std::path::PathBuf;

fn doc() -> Document {
    let mut ed = Editor::new(Document::new());
    let l1 = Document::default_layer();
    let l2 = ed
        .apply(&Command::AddLayer {
            name: "Layer 2".to_owned(),
            colour: [0, 0, 255],
            operation: Operation::Draw,
        })
        .unwrap()
        .layers[0];
    let circle = |ed: &mut Editor, layer, x: f64| {
        let c = ed
            .apply(&Command::AddEntity {
                layer,
                construction: false,
                geom: Geom::Point {
                    pos: Vec2::new(x, 0.0),
                },
                style: Style::default(),
            })
            .unwrap()
            .entities[0];
        ed.apply(&Command::AddEntity {
            layer,
            construction: false,
            geom: Geom::Circle {
                center: c,
                radius: 10.0,
            },
            style: Style::default(),
        })
        .unwrap()
        .entities[0]
    };
    let c1 = circle(&mut ed, l1, 0.0);
    ed.apply(&Command::SetRegionFill {
        regions: vec![RegionKey {
            bounds: vec![c1.get()],
            index: 0,
        }],
        fill: Some(RegionFill {
            fill: Fill::Colour([243, 94, 94]),
            opacity: 1.0,
        }),
    })
    .unwrap();
    circle(&mut ed, l2, 12.0);
    ed.into_document()
}

fn golden(name: &str, got: &[u8]) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/per-layer-fills")
        .join(name);
    if std::env::var_os("LOCUS_UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, got).unwrap();
    }
    let text = |b: &[u8]| String::from_utf8_lossy(b).replace("\r\n", "\n");
    let want = std::fs::read(&path)
        .unwrap_or_else(|_| panic!("missing golden {name}; create it with LOCUS_UPDATE_GOLDEN=1"));
    assert_eq!(
        text(got),
        text(&want),
        "{name} differs from its golden copy"
    );
}

#[test]
fn each_layer_file_has_only_its_own_shapes_and_fills() {
    let model = prepare(&doc(), false).model;
    let whole = String::from_utf8(to_svg_model(&model)).unwrap();
    // One fill, the stored red, a full circle (two half arcs, no other arc).
    assert_eq!(whole.matches("fill=\"#f35e5e\"").count(), 1, "{whole}");
    assert!(!whole.contains("fill=\"#000000\""), "{whole}");
    golden("whole.svg", whole.as_bytes());
    let files = split_layers(&model);
    assert_eq!(files.len(), 2);
    let one = String::from_utf8(to_svg_model(&files[0].model)).unwrap();
    let two = String::from_utf8(to_svg_model(&files[1].model)).unwrap();
    assert!(one.contains("fill=\"#f35e5e\""), "{one}");
    assert_eq!(one.matches("<circle").count(), 1, "only Layer 1's circle");
    assert!(!two.contains("fills"), "Layer 2 has no fill: {two}");
    assert_eq!(two.matches("<circle").count(), 1, "only Layer 2's circle");
    // Same page and origin, so they line up when imported.
    let header = |s: &str| {
        s.lines()
            .find(|l| l.contains("viewBox"))
            .unwrap()
            .to_owned()
    };
    assert_eq!(header(&one), header(&two));
    golden("layer-1.svg", one.as_bytes());
    golden("layer-2.svg", two.as_bytes());
}
