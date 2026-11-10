// SPDX-License-Identifier: AGPL-3.0-or-later

//! Golden test for the operation transforms (SPEC §7, §8; #173–#175): a
//! document with nested parts on a kerf-compensated Cut layer, vinyl on a
//! drag-knife layer and a holed shape on a crosshatched Engrave layer goes
//! through `prepare` (as the app and CLI do) and is written as SVG and DXF.
//! Regenerate with `LOCUS_UPDATE_GOLDEN=1` after an intended change.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use locus_doc::{
    Command, Compensation, CutParams, Document, DragKnifeParams, Editor, EngraveParams, EntityId,
    Geom, LayerId, LayerProps, Operation,
};
use locus_export::{ExportModel, Shape, prepare, to_dxf_model, to_svg_model};
use locus_geom::Vec2;
use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

struct B {
    ed: Editor,
}

impl B {
    fn add(&mut self, layer: LayerId, geom: Geom) -> EntityId {
        self.ed
            .apply(&Command::AddEntity {
                layer,
                construction: false,
                geom,
            })
            .unwrap()
            .entities[0]
    }

    fn pt(&mut self, layer: LayerId, x: f64, y: f64) -> EntityId {
        self.add(
            layer,
            Geom::Point {
                pos: Vec2::new(x, y),
            },
        )
    }

    fn poly(&mut self, layer: LayerId, pts: &[(f64, f64)], closed: bool) {
        let ids: Vec<EntityId> = pts.iter().map(|&(x, y)| self.pt(layer, x, y)).collect();
        let n = ids.len();
        let m = if closed { n } else { n - 1 };
        for i in 0..m {
            self.add(
                layer,
                Geom::Line {
                    start: ids[i],
                    end: ids[(i + 1) % n],
                },
            );
        }
    }

    fn rect(&mut self, layer: LayerId, x0: f64, y0: f64, x1: f64, y1: f64) {
        self.poly(layer, &[(x0, y0), (x1, y0), (x1, y1), (x0, y1)], true);
    }

    fn circle(&mut self, layer: LayerId, x: f64, y: f64, r: f64) {
        let c = self.pt(layer, x, y);
        self.add(
            layer,
            Geom::Circle {
                center: c,
                radius: r,
            },
        );
    }

    fn layer(&mut self, name: &str, operation: Operation) -> LayerId {
        self.ed
            .apply(&Command::AddLayer {
                name: name.to_owned(),
                colour: [0, 0, 0],
                operation,
            })
            .unwrap()
            .layers[0]
    }
}

fn document() -> Document {
    let mut b = B {
        ed: Editor::new(Document::new()),
    };
    let cut = Document::default_layer();
    b.ed.apply(&Command::SetLayerProps {
        id: cut,
        props: LayerProps {
            operation: Some(Operation::Cut(CutParams {
                kerf_width: 0.2,
                compensation: Compensation::Outside,
                passes: 1,
            })),
            ..LayerProps::default()
        },
    })
    .unwrap();
    // A plate with a square hole holding a round part, and a round hole.
    b.rect(cut, 0.0, 0.0, 60.0, 40.0);
    b.rect(cut, 10.0, 10.0, 30.0, 30.0);
    b.circle(cut, 20.0, 20.0, 5.0);
    b.circle(cut, 45.0, 20.0, 3.0);

    let knife = b.layer(
        "Vinyl",
        Operation::DragKnife(DragKnifeParams { blade_offset: 0.25 }),
    );
    b.rect(knife, 70.0, 0.0, 90.0, 20.0);
    b.poly(knife, &[(70.0, 30.0), (80.0, 38.0), (90.0, 30.0)], false);

    let engrave = b.layer(
        "Fill",
        Operation::Engrave(EngraveParams {
            hatch_spacing: 1.0,
            hatch_angle: 45.0,
            crosshatch: true,
        }),
    );
    b.rect(engrave, 100.0, 0.0, 120.0, 20.0);
    b.circle(engrave, 110.0, 10.0, 5.0);
    b.ed.into_document()
}

fn layer<'a>(m: &'a ExportModel, name: &str) -> &'a locus_export::ExportLayer {
    m.layers.iter().find(|l| l.name == name).unwrap()
}

#[test]
fn operations_match_golden() {
    let doc = document();
    let prepared = prepare(&doc, false);
    assert!(!prepared.blocked(), "{:?}", prepared.report);
    let m = &prepared.model;

    // Kerf: the plate grows by half the kerf, the square hole shrinks, the
    // part in the hole grows, the round hole shrinks.
    let cut = &m.layers[0];
    let radii: Vec<f64> = cut
        .paths
        .iter()
        .flat_map(|p| &p.segs)
        .filter_map(|s| match s.shape {
            Shape::Circle(c) => Some(c.radius),
            _ => None,
        })
        .collect();
    assert!(radii.iter().any(|r| (r - 5.1).abs() < 1e-9));
    assert!(radii.iter().any(|r| (r - 2.9).abs() < 1e-9));
    let bounds = m.layers[0]
        .paths
        .iter()
        .filter_map(|p| p.bbox())
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), b| {
            (lo.min(b.min.x), hi.max(b.max.x))
        });
    assert!((bounds.0 + 0.1).abs() < 1e-9 && (bounds.1 - 60.1).abs() < 1e-9);

    // Blade offset: four swivel arcs on the square, one on the open V.
    let arcs = layer(m, "Vinyl")
        .paths
        .iter()
        .flat_map(|p| &p.segs)
        .filter(|s| matches!(s.shape, Shape::Arc(_)))
        .count();
    assert_eq!(arcs, 5);

    // Hatch: lines in both directions, none inside the round hole.
    let hatch: Vec<_> = layer(m, "Fill")
        .paths
        .iter()
        .filter(|p| !p.closed)
        .collect();
    assert!(hatch.len() > 40);
    assert!(hatch.iter().all(|p| {
        let q = p.segs[0].shape.bbox();
        let mid = q.min.lerp(q.max, 0.5);
        mid.distance(Vec2::new(110.0, 10.0)) >= 5.0 - 1e-9
    }));

    let svg = String::from_utf8(to_svg_model(m)).unwrap();
    let dxf = String::from_utf8(to_dxf_model(m)).unwrap();
    for (text, name) in [(svg, "ops.svg"), (dxf, "ops.dxf")] {
        let path = fixture(name);
        if std::env::var_os("LOCUS_UPDATE_GOLDEN").is_some() {
            std::fs::write(&path, &text).unwrap();
        }
        let golden = std::fs::read_to_string(&path)
            .unwrap()
            .replace("\r\n", "\n");
        assert_eq!(text, golden, "{name} differs");
    }
}

/// The document is never changed by an export (SPEC §11).
#[test]
fn export_leaves_the_document_alone() {
    let doc = document();
    let before = locus_doc::file::save(&doc).unwrap();
    let _ = prepare(&doc, true);
    assert_eq!(locus_doc::file::save(&doc).unwrap(), before);
}
