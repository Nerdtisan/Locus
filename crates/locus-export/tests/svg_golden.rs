// SPDX-License-Identifier: AGPL-3.0-or-later

//! Layer-model golden test (the per-format fixtures live in `golden.rs`). Golden-file test: `.locus` fixture → SVG compared with the committed output (SPEC §14).
//! Regenerate with `LOCUS_UPDATE_GOLDEN=1` after an intended change.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn check(name: &str) {
    let text = std::fs::read_to_string(fixture(&format!("{name}.locus"))).unwrap();
    let doc = locus_doc::file::load(&text).unwrap();
    let svg = String::from_utf8(locus_export::to_svg(&doc)).unwrap();
    let path = fixture(&format!("{name}.svg"));
    if std::env::var_os("LOCUS_UPDATE_GOLDEN").is_some() {
        std::fs::write(&path, &svg).unwrap();
    }
    let golden = std::fs::read_to_string(&path)
        .unwrap()
        .replace("\r\n", "\n");
    assert_eq!(svg, golden);
}

/// One `<g>` per exported layer, in export order, each carrying its operation
/// and colour; Reference and export-disabled layers are absent (#119, SPEC §7).
#[test]
fn layers_match_golden() {
    use locus_doc::{Command, Editor, Geom, LayerId, LayerProps, Operation, OperationKind};
    use locus_geom::Vec2;

    let mut ed = Editor::new(locus_doc::Document::new());
    let mut layer = |name: &str, colour: [u8; 3], operation: Operation| -> LayerId {
        ed.apply(&Command::AddLayer {
            name: name.to_owned(),
            colour,
            operation,
        })
        .unwrap()
        .layers[0]
    };
    let cut = locus_doc::Document::default_layer();
    let score = layer("Score", [255, 0, 0], Operation::Score);
    let engrave = layer(
        "Engrave (off)",
        [0, 0, 255],
        OperationKind::Engrave.default_operation(),
    );
    let reference = layer("Guides", [128, 128, 128], Operation::Reference);
    let knife = layer(
        "Vinyl",
        [0, 160, 0],
        OperationKind::DragKnife.default_operation(),
    );
    ed.apply(&Command::SetLayerProps {
        id: engrave,
        props: LayerProps {
            export_enabled: Some(false),
            ..LayerProps::default()
        },
    })
    .unwrap();
    // Vinyl first in the export order, to prove ordering follows it.
    ed.apply(&Command::SetLayerProps {
        id: knife,
        props: LayerProps {
            export_order: Some(0),
            ..LayerProps::default()
        },
    })
    .unwrap();
    let mut line = |l: LayerId, a: (f64, f64), b: (f64, f64)| {
        let mut pt = |x, y| {
            ed.apply(&Command::AddEntity {
                layer: l,
                construction: false,
                geom: Geom::Point {
                    pos: Vec2::new(x, y),
                },
            })
            .unwrap()
            .entities[0]
        };
        let (s, e) = (pt(a.0, a.1), pt(b.0, b.1));
        ed.apply(&Command::AddEntity {
            layer: l,
            construction: false,
            geom: Geom::Line { start: s, end: e },
        })
        .unwrap();
    };
    line(cut, (0.0, 0.0), (40.0, 0.0));
    line(score, (0.0, 5.0), (40.0, 5.0));
    line(engrave, (0.0, 10.0), (40.0, 10.0));
    line(reference, (0.0, 15.0), (40.0, 15.0));
    line(knife, (0.0, 20.0), (40.0, 20.0));
    let doc = ed.into_document();

    let path = fixture("layers.locus");
    if std::env::var_os("LOCUS_UPDATE_GOLDEN").is_some() {
        std::fs::write(&path, locus_doc::file::save(&doc).unwrap()).unwrap();
    }
    let svg = String::from_utf8(locus_export::to_svg(&doc)).unwrap();
    assert!(!svg.contains("Guides") && !svg.contains("Engrave (off)"));
    check("layers");
}
