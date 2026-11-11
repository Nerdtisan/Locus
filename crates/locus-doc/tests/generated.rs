// SPDX-License-Identifier: AGPL-3.0-or-later

//! Plugin-generated objects in documents (#183, SPEC §15.3, §15.5): one undo
//! step, a constrainable frame, cached output that needs no plugin to open,
//! regenerate, save or export, and untrusted output rejected cleanly.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use locus_doc::generated::{
    GenConstraint, GenEntity, GeneratedGeometry, PluginParam, PluginRef, PluginValue,
};
use locus_doc::{Command, Document, Editor, EntityKind, Geom, RecipeKind};
use locus_geom::Vec2;

fn square(size: f64) -> GeneratedGeometry {
    GeneratedGeometry {
        points: vec![
            Vec2::new(0.0, 0.0),
            Vec2::new(size, 0.0),
            Vec2::new(size, size),
            Vec2::new(0.0, size),
        ],
        entities: vec![
            GenEntity::Line(0, 1),
            GenEntity::Line(1, 2),
            GenEntity::Line(2, 3),
            GenEntity::Line(3, 0),
        ],
        constraints: vec![GenConstraint::Horizontal(0)],
    }
}

fn plugin(size: f64) -> PluginRef {
    PluginRef {
        id: "com.example.square".into(),
        version: "1.0.0".into(),
        params: vec![PluginParam {
            key: "size".into(),
            value: PluginValue::Number(size),
        }],
    }
}

fn add(ed: &mut Editor, origin: Vec2, size: f64) -> locus_doc::Applied {
    ed.apply(&Command::AddGenerated {
        layer: Document::default_layer(),
        origin,
        plugin: plugin(size),
        output: square(size),
    })
    .unwrap()
}

/// Positions of the object's points, in pool order.
fn points(doc: &Document) -> Vec<Vec2> {
    let (_, r) = doc.recipes().iter().next().unwrap();
    r.instances().filter_map(|id| doc.point_pos(id)).collect()
}

fn close(a: Vec2, b: Vec2) -> bool {
    (a - b).length() < 1e-9
}

#[test]
fn insert_is_one_undo_step_at_the_origin() {
    let mut ed = Editor::new(Document::new());
    let before = ed.document().clone();
    let applied = add(&mut ed, Vec2::new(100.0, 50.0), 10.0);
    assert_eq!(applied.recipes.len(), 1);

    let doc = ed.document();
    let lines = doc
        .entities()
        .iter()
        .filter(|(_, e)| e.geom.kind() == EntityKind::Line)
        .count();
    assert_eq!(lines, 4);
    let p = points(doc);
    assert!(close(p[0], Vec2::new(100.0, 50.0)));
    assert!(close(p[2], Vec2::new(110.0, 60.0)));
    // The instances are driven: not editable on their own.
    let (_, r) = doc.recipes().iter().next().unwrap();
    let inst = r.instances().next().unwrap();
    assert!(ed.apply(&Command::DeleteEntity { id: inst }).is_err());

    // One step undoes all of it. (The ID counter never goes back, ADR 0003.)
    ed.undo().unwrap();
    assert_eq!(ed.document().entities(), before.entities());
    assert!(ed.document().recipes().is_empty());
    assert!(!ed.can_undo());
}

#[test]
fn moving_or_turning_the_frame_moves_the_object() {
    let mut ed = Editor::new(Document::new());
    add(&mut ed, Vec2::new(0.0, 0.0), 10.0);
    let (_, r) = ed.document().recipes().iter().next().unwrap();
    let (origin, dir) = (r.sources[0], r.sources[1]);

    ed.apply(&Command::ModifyEntity {
        id: origin,
        geom: Geom::Point {
            pos: Vec2::new(5.0, 5.0),
        },
    })
    .unwrap();
    ed.apply(&Command::ModifyEntity {
        id: dir,
        geom: Geom::Point {
            pos: Vec2::new(5.0, 20.0),
        },
    })
    .unwrap();
    // Origin (5,5), +x now points up: local (10,0) lands at (5,15).
    let p = points(ed.document());
    assert!(close(p[0], Vec2::new(5.0, 5.0)), "{p:?}");
    assert!(close(p[1], Vec2::new(5.0, 15.0)), "{p:?}");
}

#[test]
fn editing_parameters_regenerates_and_undoes() {
    let mut ed = Editor::new(Document::new());
    add(&mut ed, Vec2::new(0.0, 0.0), 10.0);
    let (rid, r) = ed
        .document()
        .recipes()
        .iter()
        .next()
        .map(|(i, r)| (*i, r.clone()))
        .unwrap();
    let before = ed.document().clone();
    ed.apply(&Command::EditRecipe {
        id: rid,
        kind: RecipeKind::Generated {
            plugin: plugin(20.0),
            output: square(20.0),
        },
    })
    .unwrap();
    assert!(close(points(ed.document())[2], Vec2::new(20.0, 20.0)));
    assert_eq!(r.sources, ed.document().recipe(rid).unwrap().sources);
    ed.undo().unwrap();
    assert_eq!(ed.document(), &before);
}

#[test]
fn bad_output_is_refused_and_the_document_is_unchanged() {
    let mut ed = Editor::new(Document::new());
    let before = ed.document().clone();
    let mut nan = square(10.0);
    nan.points[1].x = f64::NAN;
    let mut dangling = square(10.0);
    dangling.entities.push(GenEntity::Line(0, 99));
    let mut bad_constraint = square(10.0);
    bad_constraint.constraints.push(GenConstraint::Equal(0, 42));
    let mut bad_id = plugin(10.0);
    bad_id.id = "../evil".into();
    for (output, plugin) in [
        (nan, plugin(10.0)),
        (dangling, plugin(10.0)),
        (bad_constraint, plugin(10.0)),
        (square(10.0), bad_id),
    ] {
        let r = ed.apply(&Command::AddGenerated {
            layer: Document::default_layer(),
            origin: Vec2::ZERO,
            plugin,
            output,
        });
        assert!(r.is_err());
        assert_eq!(ed.document(), &before);
        assert!(!ed.can_undo());
    }
}

#[test]
fn saves_and_loads_without_the_plugin() {
    let mut ed = Editor::new(Document::new());
    add(&mut ed, Vec2::new(30.0, 40.0), 12.5);
    let doc = ed.document().clone();
    let text = locus_doc::file::save(&doc).unwrap();
    let back = locus_doc::file::load(&text).unwrap();
    assert_eq!(back, doc);
    assert_eq!(locus_doc::file::save(&back).unwrap(), text);
}

#[test]
fn explode_leaves_plain_geometry() {
    let mut ed = Editor::new(Document::new());
    add(&mut ed, Vec2::ZERO, 10.0);
    let rid = *ed.document().recipes().keys().next().unwrap();
    ed.apply(&Command::ExplodeRecipe { id: rid }).unwrap();
    let doc = ed.document();
    assert!(doc.recipes().is_empty());
    let line = doc
        .entities()
        .iter()
        .find(|(_, e)| e.geom.kind() == EntityKind::Line)
        .map(|(id, _)| id)
        .unwrap();
    assert!(ed.apply(&Command::DeleteEntity { id: line }).is_ok());
}
