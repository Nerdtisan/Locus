// SPDX-License-Identifier: AGPL-3.0-or-later

//! Engrave modes and the overlap rule (#455, SPEC §7): Fill, Line,
//! Line + Fill and Offset fill, each with even-odd or union overlaps, on two
//! overlapping squares. Regenerate after an intended change with
//! `LOCUS_UPDATE_GOLDEN=1 cargo test -p locus-export --test engrave_modes_golden`
//! and review the diff of `tests/fixtures/engrave/`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use locus_doc::{
    Command, Document, Editor, EngraveMode, EngraveParams, EntityId, Geom, LayerProps, Operation,
    Overlap, Style,
};
use locus_export::{ExportModel, Shape, prepare, to_svg_model};
use locus_geom::Vec2;
use std::path::PathBuf;

fn square(ed: &mut Editor, x0: f64, y0: f64, side: f64) {
    let layer = Document::default_layer();
    let pts: Vec<EntityId> = [(0.0, 0.0), (side, 0.0), (side, side), (0.0, side)]
        .iter()
        .map(|&(x, y)| {
            ed.apply(&Command::AddEntity {
                layer,
                construction: false,
                geom: Geom::Point {
                    pos: Vec2::new(x0 + x, y0 + y),
                },
                style: Style::default(),
            })
            .unwrap()
            .entities[0]
        })
        .collect();
    for i in 0..4 {
        ed.apply(&Command::AddEntity {
            layer,
            construction: false,
            geom: Geom::Line {
                start: pts[i],
                end: pts[(i + 1) % 4],
            },
            style: Style::default(),
        })
        .unwrap();
    }
}

/// Two 10 mm squares overlapping by 5 × 5 mm, on an Engrave layer.
fn doc(mode: EngraveMode, overlap: Overlap) -> Document {
    let mut ed = Editor::new(Document::new());
    ed.apply(&Command::SetLayerProps {
        id: Document::default_layer(),
        props: LayerProps {
            operation: Some(Operation::Engrave(EngraveParams {
                hatch_spacing: 1.0,
                mode,
                overlap,
                ..EngraveParams::default()
            })),
            ..LayerProps::default()
        },
    })
    .unwrap();
    square(&mut ed, 0.0, 0.0, 10.0);
    square(&mut ed, 5.0, 5.0, 10.0);
    ed.into_document()
}

fn model(mode: EngraveMode, overlap: Overlap) -> ExportModel {
    prepare(&doc(mode, overlap), false).model
}

fn golden(name: &str, got: &[u8]) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/engrave")
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
fn every_mode_and_overlap_matches_its_golden() {
    for mode in EngraveMode::ALL {
        for overlap in Overlap::ALL {
            let name = format!("{mode:?}-{overlap:?}.svg").to_lowercase();
            golden(&name, &to_svg_model(&model(mode, overlap)));
        }
    }
}

/// Hatch length inside a horizontal band, for telling overlaps apart.
fn hatch_in(m: &ExportModel, y: f64) -> f64 {
    m.layers[0]
        .paths
        .iter()
        .filter(|p| !p.closed)
        .flat_map(|p| &p.segs)
        .filter_map(|s| match s.shape {
            Shape::Line(l) if (l.a.y - y).abs() < 1e-9 && (l.b.y - y).abs() < 1e-9 => {
                Some(l.length())
            }
            _ => None,
        })
        .sum()
}

#[test]
fn modes_keep_or_drop_the_outline_and_the_hatch() {
    let closed = |m: &ExportModel| m.layers[0].paths.iter().filter(|p| p.closed).count();
    let open = |m: &ExportModel| m.layers[0].paths.iter().filter(|p| !p.closed).count();
    let fill = model(EngraveMode::Fill, Overlap::EvenOdd);
    assert_eq!(closed(&fill), 0, "Fill has no outline");
    assert!(open(&fill) > 0, "Fill hatches");
    let line = model(EngraveMode::Line, Overlap::EvenOdd);
    assert_eq!(closed(&line), 2, "Line keeps both outlines");
    assert_eq!(open(&line), 0, "Line has no hatch");
    let both = model(EngraveMode::LineFill, Overlap::EvenOdd);
    assert_eq!(closed(&both), 2);
    assert_eq!(open(&both), open(&fill));
    let rings = model(EngraveMode::OffsetFill, Overlap::EvenOdd);
    assert_eq!(open(&rings), 0, "Offset fill has no hatch lines");
    assert!(closed(&rings) > 2, "concentric rings");
}

/// Even-odd leaves the 5 × 5 overlap empty; Union engraves it.
#[test]
fn the_overlap_rule_decides_whether_the_overlap_is_engraved() {
    // The row y = 7 crosses the overlap (x 5..10) and both squares.
    let even = hatch_in(&model(EngraveMode::Fill, Overlap::EvenOdd), 7.0);
    let union = hatch_in(&model(EngraveMode::Fill, Overlap::Union), 7.0);
    assert!((even - 10.0).abs() < 1e-6, "0..5 and 10..15: {even}");
    assert!((union - 15.0).abs() < 1e-6, "0..15 in one: {union}");
}
