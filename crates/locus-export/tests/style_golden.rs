// SPDX-License-Identifier: AGPL-3.0-or-later

//! Stroke widths and per-object style in every writer, per machine profile
//! (SPEC §7, §8; #284, #287).
//!
//! Draw layers export each object's own style; Cut, Score, Engrave and
//! Drag-knife layers export the profile's colour and hairline. Regenerate
//! after an intended change with
//! `LOCUS_UPDATE_GOLDEN=1 cargo test -p locus-export --test style_golden`
//! and review the diff of `tests/fixtures/style/`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use locus_doc::{
    Command, Document, Editor, EntityId, Fill, Geom, LayerId, OperationKind, Stroke, Style,
};
use locus_export::{MachineProfile, prepare_for, to_dxf_model, to_pdf_model, to_svg_model};
use locus_geom::Vec2;
use std::path::PathBuf;

struct B {
    ed: Editor,
}

impl B {
    fn add(&mut self, layer: LayerId, geom: Geom, style: Style) -> EntityId {
        self.ed
            .apply(&Command::AddEntity {
                layer,
                construction: false,
                geom,
                style,
            })
            .unwrap()
            .entities[0]
    }

    fn poly(&mut self, layer: LayerId, pts: &[(f64, f64)], closed: bool, style: Style) {
        let ids: Vec<EntityId> = pts
            .iter()
            .map(|&(x, y)| {
                self.add(
                    layer,
                    Geom::Point {
                        pos: Vec2::new(x, y),
                    },
                    Style::default(),
                )
            })
            .collect();
        let n = if closed { ids.len() } else { ids.len() - 1 };
        for i in 0..n {
            let (start, end) = (ids[i], ids[(i + 1) % ids.len()]);
            self.add(layer, Geom::Line { start, end }, style);
        }
    }
}

/// A Draw layer with a 0.5 mm line, a thin red line, a filled half-opaque
/// square on pen 2, and a Cut layer with a styled square (whose style the
/// machine layer ignores).
fn doc() -> Document {
    let mut b = B {
        ed: Editor::new(Document::new()),
    };
    let draw = Document::default_layer();
    let cut =
        b.ed.apply(&Command::AddLayer {
            name: "Cut".into(),
            colour: [10, 20, 30],
            operation: OperationKind::Cut.default_operation(),
        })
        .unwrap()
        .layers[0];
    let half = Style {
        width: 0.5,
        ..Style::default()
    };
    b.poly(draw, &[(0.0, 0.0), (40.0, 0.0)], false, half);
    b.poly(
        draw,
        &[(0.0, 5.0), (40.0, 5.0)],
        false,
        Style {
            stroke: Stroke::Colour([255, 0, 0]),
            width: 0.25,
            ..Style::default()
        },
    );
    b.poly(
        draw,
        &[(0.0, 10.0), (10.0, 10.0), (10.0, 20.0), (0.0, 20.0)],
        true,
        Style {
            fill: Fill::Colour([0, 0, 255]),
            opacity: 0.5,
            pen: 2,
            ..Style::default()
        },
    );
    b.poly(
        cut,
        &[(20.0, 10.0), (30.0, 10.0), (30.0, 20.0), (20.0, 20.0)],
        true,
        Style {
            stroke: Stroke::Colour([0, 255, 0]),
            width: 2.0,
            ..Style::default()
        },
    );
    b.ed.into_document()
}

fn profile(id: &str) -> Option<MachineProfile> {
    match id {
        "none" => None,
        "hairline" => {
            let mut p = locus_export::profile::find("lightburn", &[]).unwrap();
            p.id = "custom".into();
            p.hairlines.insert(OperationKind::Cut, 0.05);
            p.validate().unwrap();
            Some(p)
        }
        id => locus_export::profile::find(id, &[]),
    }
}

fn golden(name: &str, got: &[u8]) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/style")
        .join(name);
    if std::env::var_os("LOCUS_UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, got).unwrap();
    }
    let text = |b: &[u8]| {
        b.iter()
            .map(|&c| if c.is_ascii() { char::from(c) } else { '?' })
            .collect::<String>()
            .replace("\r\n", "\n")
    };
    let want = std::fs::read(&path)
        .unwrap_or_else(|_| panic!("missing golden {name}; create it with LOCUS_UPDATE_GOLDEN=1"));
    assert_eq!(
        text(got),
        text(&want),
        "{name} differs from its golden copy"
    );
}

#[test]
fn every_writer_matches_its_golden_per_profile() {
    let d = doc();
    for id in ["none", "lightburn", "cricut", "hairline"] {
        let p = profile(id);
        let model = prepare_for(&d, false, p.as_ref()).model;
        golden(&format!("{id}.svg"), &to_svg_model(&model));
        golden(&format!("{id}.pdf"), &to_pdf_model(&model));
        golden(&format!("{id}.dxf"), &to_dxf_model(&model));
    }
}

/// #284 acceptance: a 0.5 mm Draw line exports with `stroke-width="0.5"`.
#[test]
fn draw_objects_keep_their_width_and_machine_layers_their_hairline() {
    let d = doc();
    let svg = String::from_utf8(to_svg_model(&prepare_for(&d, false, None).model)).unwrap();
    assert!(svg.contains("stroke-width=\"0.5\""), "{svg}");
    assert!(svg.contains("stroke=\"#ff0000\" stroke-width=\"0.25\""));
    assert!(svg.contains("fill=\"#0000ff\""));
    assert!(svg.contains("opacity=\"0.5\" data-pen=\"2\""));
    // The Cut square ignores its green 2 mm style: layer colour, hairline.
    assert!(svg.contains("stroke=\"#0a141e\" stroke-width=\"0.1\""));
    assert!(!svg.contains("#00ff00") && !svg.contains("stroke-width=\"2\""));

    let lb = profile("hairline").unwrap();
    let svg = String::from_utf8(to_svg_model(&prepare_for(&d, false, Some(&lb)).model)).unwrap();
    assert!(
        svg.contains("stroke=\"#ff0000\" stroke-width=\"0.05\""),
        "LightBurn red cut at the profile's hairline: {svg}"
    );
}

/// Two touching lines of different styles stay two paths on a Draw layer.
#[test]
fn differently_styled_lines_are_not_chained_together() {
    let mut b = B {
        ed: Editor::new(Document::new()),
    };
    let l = Document::default_layer();
    let p: Vec<EntityId> = [(0.0, 0.0), (10.0, 0.0), (20.0, 0.0)]
        .iter()
        .map(|&(x, y)| {
            b.add(
                l,
                Geom::Point {
                    pos: Vec2::new(x, y),
                },
                Style::default(),
            )
        })
        .collect();
    b.add(
        l,
        Geom::Line {
            start: p[0],
            end: p[1],
        },
        Style::default(),
    );
    b.add(
        l,
        Geom::Line {
            start: p[1],
            end: p[2],
        },
        Style {
            width: 0.8,
            ..Style::default()
        },
    );
    let d = b.ed.into_document();
    let model = prepare_for(&d, false, None).model;
    assert_eq!(model.layers[0].paths.len(), 2);
}

/// Hairlines are validated: only machine operations, positive and bounded.
#[test]
fn bad_hairlines_are_refused() {
    let mut p = profile("lightburn").unwrap();
    for (op, w) in [
        (OperationKind::Draw, 0.1),
        (OperationKind::Cut, 0.0),
        (OperationKind::Cut, f64::NAN),
    ] {
        p.hairlines.clear();
        p.hairlines.insert(op, w);
        assert!(p.validate().is_err(), "{op:?} {w}");
    }
}
