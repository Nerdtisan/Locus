// SPDX-License-Identifier: AGPL-3.0-or-later

//! Boolean recipes through the Sketcher (#123; SPEC §6.3, §10, §11): results
//! follow edits to their sources, keep true arcs, and report missing sources.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use core::f64::consts::PI;
use locus_doc::{Boolean, BooleanError, Command, Document, EntityId, Geom, Sketcher};
use locus_geom::Vec2;
use locus_region::{BoolOp, RegionKey, Seg};

fn add(s: &mut Sketcher, geom: Geom) -> EntityId {
    s.apply(&Command::AddEntity {
        layer: Document::default_layer(),
        construction: false,
        style: Default::default(),
        geom,
    })
    .unwrap()
    .entities[0]
}

fn pt(s: &mut Sketcher, x: f64, y: f64) -> EntityId {
    add(
        s,
        Geom::Point {
            pos: Vec2::new(x, y),
        },
    )
}

/// A 40 × 20 plate and a Ø10 circle centred on its top-right corner.
/// Returns (circle, circle centre point).
fn plate_and_circle(s: &mut Sketcher) -> (EntityId, EntityId) {
    let p = [
        pt(s, 0.0, 0.0),
        pt(s, 40.0, 0.0),
        pt(s, 40.0, 20.0),
        pt(s, 0.0, 20.0),
    ];
    for i in 0..4 {
        add(
            s,
            Geom::Line {
                start: p[i],
                end: p[(i + 1) % 4],
            },
        );
    }
    let c = pt(s, 40.0, 20.0);
    let circle = add(
        s,
        Geom::Circle {
            center: c,
            radius: 5.0,
        },
    );
    (circle, c)
}

/// Keys of the regions that contain each probe.
fn keys_at(s: &Sketcher, probes: &[(f64, f64)]) -> Vec<RegionKey> {
    probes
        .iter()
        .map(|&(x, y)| {
            let q = Vec2::new(x, y);
            s.regions()
                .iter()
                .find(|r| r.contains(q))
                .unwrap()
                .key
                .clone()
        })
        .collect()
}

fn result(s: &Sketcher) -> Result<locus_region::Area, BooleanError> {
    s.boolean_results().values().next().unwrap().clone()
}

#[test]
fn subtract_follows_source_edits_and_keeps_arcs() {
    let mut s = Sketcher::new(Document::new());
    let (circle, centre) = plate_and_circle(&mut s);
    assert_eq!(s.regions().len(), 3);
    // Plate = the faces inside the rectangle; tool = the faces inside the circle.
    let targets = keys_at(&s, &[(10.0, 10.0), (38.0, 18.0)]);
    let tools = keys_at(&s, &[(38.0, 18.0), (42.0, 22.0)]);
    let id = s
        .apply(&Command::AddBoolean {
            boolean: Boolean {
                op: BoolOp::Subtract,
                targets,
                tools,
            },
        })
        .unwrap()
        .booleans[0];
    let area = result(&s).unwrap();
    assert!((area.area() - (800.0 - 25.0 * PI / 4.0)).abs() < 1e-9);

    // Edit the source: a bigger circle. The keys survive, the result follows.
    s.apply(&Command::ModifyEntity {
        id: circle,
        geom: Geom::Circle {
            center: centre,
            radius: 8.0,
        },
    })
    .unwrap();
    let area = result(&s).unwrap();
    assert!((area.area() - (800.0 - 64.0 * PI / 4.0)).abs() < 1e-9);
    // One exact arc of radius 8 (no flattening), plus four lines.
    let arcs: Vec<f64> = area
        .edges()
        .filter_map(|e| match e.seg {
            Seg::Arc { radius, .. } => Some(radius),
            Seg::Line { .. } => None,
        })
        .collect();
    assert_eq!(arcs.len(), 1);
    assert!((arcs[0] - 8.0).abs() < 1e-12);
    assert_eq!(area.edges().count(), 5);

    // Move the circle away: its faces are gone, so the recipe reports it.
    s.apply(&Command::ModifyEntity {
        id: centre,
        geom: Geom::Point {
            pos: Vec2::new(100.0, 100.0),
        },
    })
    .unwrap();
    assert!(matches!(result(&s), Err(BooleanError::MissingRegions(m)) if !m.is_empty()));
    // Undo brings the sources, and the result, back.
    s.undo().unwrap();
    assert!(result(&s).is_ok());
    assert!(s.document().boolean(id).is_some());
}

#[test]
fn add_edit_remove_and_undo() {
    let mut s = Sketcher::new(Document::new());
    plate_and_circle(&mut s);
    let all: Vec<RegionKey> = s.regions().iter().map(|r| r.key.clone()).collect();
    let union = Boolean {
        op: BoolOp::Union,
        targets: all.clone(),
        tools: vec![],
    };
    let id = s
        .apply(&Command::AddBoolean {
            boolean: union.clone(),
        })
        .unwrap()
        .booleans[0];
    let u = result(&s).unwrap();
    assert_eq!(u.loops.len(), 1);
    assert!((u.area() - (800.0 + 75.0 * PI / 4.0)).abs() < 1e-9);

    let inter = Boolean {
        op: BoolOp::Intersect,
        targets: all[..1].to_vec(),
        tools: all.clone(),
    };
    s.apply(&Command::EditBoolean {
        id,
        boolean: inter.clone(),
    })
    .unwrap();
    assert_eq!(s.document().boolean(id), Some(&inter));
    s.undo().unwrap();
    assert_eq!(s.document().boolean(id), Some(&union));

    s.apply(&Command::RemoveBoolean { id }).unwrap();
    assert!(s.document().booleans().is_empty() && s.boolean_results().is_empty());
    s.undo().unwrap();
    assert_eq!(s.document().boolean(id), Some(&union));

    // Bad recipes are refused without a history entry.
    let before = s.editor().can_redo();
    let bad = Command::AddBoolean {
        boolean: Boolean {
            op: BoolOp::Subtract,
            targets: all,
            tools: vec![],
        },
    };
    assert!(s.apply(&bad).is_err());
    assert_eq!(s.document().booleans().len(), 1);
    assert_eq!(s.editor().can_redo(), before);
}

#[test]
fn recipes_round_trip_through_the_file() {
    let mut s = Sketcher::new(Document::new());
    plate_and_circle(&mut s);
    let all: Vec<RegionKey> = s.regions().iter().map(|r| r.key.clone()).collect();
    s.apply(&Command::AddBoolean {
        boolean: Boolean {
            op: BoolOp::Union,
            targets: all,
            tools: vec![],
        },
    })
    .unwrap();
    let text = locus_doc::file::save(s.document()).unwrap();
    let loaded = Sketcher::new(locus_doc::file::load(&text).unwrap());
    assert_eq!(loaded.document(), s.document());
    assert_eq!(loaded.boolean_results(), s.boolean_results());
    assert_eq!(locus_doc::file::save(loaded.document()).unwrap(), text);
}
