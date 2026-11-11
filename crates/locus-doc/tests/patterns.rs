// SPDX-License-Identifier: AGPL-3.0-or-later

//! Linear and circular patterns (SPEC §6.2.1, #168, #169, ADR 0011).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use locus_doc::{
    Axis, Command, Constraint, DocError, Document, EntityId, Geom, LinearDir, RecipeId, RecipeKind,
    Sketcher,
};
use locus_geom::Vec2;

const L: locus_doc::LayerId = Document::default_layer();

fn pt(s: &mut Sketcher, x: f64, y: f64) -> EntityId {
    s.apply(&Command::AddEntity {
        layer: L,
        construction: false,
        geom: Geom::Point {
            pos: Vec2::new(x, y),
        },
    })
    .unwrap()
    .entities[0]
}

fn dir(axis: Axis, count: &str, spacing: &str) -> LinearDir {
    LinearDir {
        axis,
        count: count.to_owned(),
        spacing: spacing.to_owned(),
        symmetric: false,
    }
}

fn add(s: &mut Sketcher, kind: RecipeKind, sources: &[EntityId]) -> Result<RecipeId, DocError> {
    s.apply(&Command::AddRecipe {
        kind,
        sources: sources.to_vec(),
    })
    .map(|a| a.recipes[0])
}

/// Positions of a recipe's live copies of its (single) source point.
fn copies(s: &Sketcher, r: RecipeId) -> Vec<Vec2> {
    s.document()
        .recipe(r)
        .unwrap()
        .instances()
        .map(|e| s.document().point_pos(e).unwrap())
        .collect()
}

fn near(a: Vec2, x: f64, y: f64) -> bool {
    (a.x - x).abs() < 1e-6 && (a.y - y).abs() < 1e-6
}

fn set(s: &mut Sketcher, n: &str, e: &str) -> Result<(), DocError> {
    s.apply(&Command::SetParameter {
        name: n.to_owned(),
        expr: e.to_owned(),
    })
    .map(|_| ())
}

#[test]
fn linear_pattern_follows_the_source_and_parameters() {
    let mut s = Sketcher::new(Document::new());
    set(&mut s, "n", "3").unwrap();
    set(&mut s, "pitch", "10").unwrap();
    let p = pt(&mut s, 5.0, 5.0);
    let r = add(
        &mut s,
        RecipeKind::Linear {
            dir1: dir(Axis::X, "n", "pitch"),
            dir2: None,
        },
        &[p],
    )
    .unwrap();
    let c = copies(&s, r);
    assert_eq!(c.len(), 2);
    assert!(near(c[0], 15.0, 5.0) && near(c[1], 25.0, 5.0));
    // Editing the source moves every copy.
    s.apply(&Command::ModifyEntity {
        id: p,
        geom: Geom::Point {
            pos: Vec2::new(5.0, 8.0),
        },
    })
    .unwrap();
    assert!(near(copies(&s, r)[1], 25.0, 8.0));
    // A parameter change adds a copy, in one undo step; undo restores exactly.
    let before = s.document().clone();
    set(&mut s, "n", "4").unwrap();
    assert_eq!(copies(&s, r).len(), 3);
    s.undo().unwrap();
    // Everything is back exactly; only the ID counter, which never goes
    // back (ADR 0003), has moved on.
    let d = s.document();
    assert_eq!(d.entities(), before.entities());
    assert_eq!(d.recipes(), before.recipes());
    assert_eq!(d.parameters(), before.parameters());
    s.redo().unwrap();
    assert_eq!(copies(&s, r).len(), 3);
}

#[test]
fn shrinking_then_growing_reuses_the_recipes_own_ids() {
    let mut s = Sketcher::new(Document::new());
    set(&mut s, "n", "4").unwrap();
    let p = pt(&mut s, 0.0, 0.0);
    let r = add(
        &mut s,
        RecipeKind::Linear {
            dir1: dir(Axis::Y, "n", "2"),
            dir2: None,
        },
        &[p],
    )
    .unwrap();
    let ids: Vec<_> = s.document().recipe(r).unwrap().instances().collect();
    set(&mut s, "n", "2").unwrap();
    assert_eq!(copies(&s, r).len(), 1);
    let next = s.document().next_id();
    // Nothing else may be handed those IDs while they are unused.
    let other = pt(&mut s, 50.0, 50.0);
    assert!(other.get() >= next);
    set(&mut s, "n", "4").unwrap();
    let again: Vec<_> = s.document().recipe(r).unwrap().instances().collect();
    assert_eq!(ids, again, "same IDs come back");
}

#[test]
fn grid_symmetric_and_suppress() {
    let mut s = Sketcher::new(Document::new());
    let p = pt(&mut s, 0.0, 0.0);
    let mut d1 = dir(Axis::X, "3", "10");
    d1.symmetric = true;
    let r = add(
        &mut s,
        RecipeKind::Linear {
            dir1: d1,
            dir2: Some(dir(Axis::Y, "2", "5")),
        },
        &[p],
    )
    .unwrap();
    // 3 x 2 grid minus the source.
    assert_eq!(copies(&s, r).len(), 5);
    assert!(copies(&s, r).iter().any(|c| near(*c, -10.0, 0.0)));
    assert!(copies(&s, r).iter().any(|c| near(*c, 10.0, 5.0)));
    s.apply(&Command::SetInstanceSuppressed {
        id: r,
        index: 0,
        suppressed: true,
    })
    .unwrap();
    assert_eq!(copies(&s, r).len(), 4);
    s.undo().unwrap();
    assert_eq!(copies(&s, r).len(), 5);
    assert!(
        s.apply(&Command::SetInstanceSuppressed {
            id: r,
            index: 99,
            suppressed: true
        })
        .is_err()
    );
}

#[test]
fn circular_pattern_full_and_partial() {
    let mut s = Sketcher::new(Document::new());
    let c = pt(&mut s, 0.0, 0.0);
    let p = pt(&mut s, 10.0, 0.0);
    let r = add(
        &mut s,
        RecipeKind::Circular {
            centre: c,
            count: "4".to_owned(),
            angle: "360".to_owned(),
            symmetric: false,
            rotate: true,
        },
        &[p],
    )
    .unwrap();
    let v = copies(&s, r);
    assert_eq!(v.len(), 3);
    assert!(near(v[0], 0.0, 10.0) && near(v[1], -10.0, 0.0) && near(v[2], 0.0, -10.0));
    // 90 degrees across 3 copies: 0, 45, 90.
    let q = pt(&mut s, 10.0, 0.0);
    let r2 = add(
        &mut s,
        RecipeKind::Circular {
            centre: c,
            count: "3".to_owned(),
            angle: "90".to_owned(),
            symmetric: false,
            rotate: false,
        },
        &[q],
    )
    .unwrap();
    let w = copies(&s, r2);
    assert_eq!(w.len(), 2);
    let k = std::f64::consts::FRAC_1_SQRT_2 * 10.0;
    assert!(near(w[0], k, k) && near(w[1], 0.0, 10.0));
}

#[test]
fn instances_are_driven_and_never_constrained_or_dragged() {
    let mut s = Sketcher::new(Document::new());
    let p = pt(&mut s, 0.0, 0.0);
    let r = add(
        &mut s,
        RecipeKind::Linear {
            dir1: dir(Axis::X, "3", "10"),
            dir2: None,
        },
        &[p],
    )
    .unwrap();
    let inst = s.document().recipe(r).unwrap().instances().next().unwrap();
    assert!(matches!(s.begin_drag(inst), Err(DocError::Driven { .. })));
    assert!(
        s.apply(&Command::AddConstraint {
            constraint: Constraint::Fix { point: inst }
        })
        .is_err()
    );
    assert!(matches!(
        s.apply(&Command::DeleteEntity { id: inst }),
        Err(DocError::RecipeInUse { .. })
    ));
    // A parameter the pattern uses can't be deleted; renaming follows.
    set(&mut s, "k", "3").unwrap();
    let src2 = pt(&mut s, 100.0, 100.0);
    let r2 = add(
        &mut s,
        RecipeKind::Linear {
            dir1: dir(Axis::Y, "k", "4"),
            dir2: None,
        },
        &[src2],
    )
    .unwrap();
    assert!(matches!(
        s.apply(&Command::RemoveParameter {
            name: "k".to_owned()
        }),
        Err(DocError::ParameterInUse { .. })
    ));
    s.apply(&Command::RenameParameter {
        from: "k".to_owned(),
        to: "rows".to_owned(),
    })
    .unwrap();
    match &s.document().recipe(r2).unwrap().kind {
        RecipeKind::Linear { dir1, .. } => assert_eq!(dir1.count, "rows"),
        _ => panic!(),
    }
    // Explode leaves plain geometry.
    s.apply(&Command::ExplodeRecipe { id: r }).unwrap();
    assert!(s.document().driven_by(inst).is_none());
    s.begin_drag(inst).unwrap();
    s.cancel_drag();
}

#[test]
fn limits_and_bad_expressions_are_refused() {
    let mut s = Sketcher::new(Document::new());
    let p = pt(&mut s, 0.0, 0.0);
    let big = |n: &str| RecipeKind::Linear {
        dir1: dir(Axis::X, n, "1"),
        dir2: None,
    };
    assert!(matches!(
        add(&mut s, big("20000"), &[p]),
        Err(DocError::InstanceLimit { .. })
    ));
    assert!(add(&mut s, big("10001"), &[p]).is_ok());
    let mut s = Sketcher::new(Document::new());
    let p = pt(&mut s, 0.0, 0.0);
    assert!(add(&mut s, big("0"), &[p]).is_err());
    assert!(add(&mut s, big("nope"), &[p]).is_err());
    let doc_before = s.document().clone();
    assert!(add(&mut s, big("1 /"), &[p]).is_err());
    assert_eq!(*s.document(), doc_before);
}

#[test]
fn patterns_round_trip_through_the_file() {
    let mut s = Sketcher::new(Document::new());
    let p = pt(&mut s, 1.0, 2.0);
    let c = pt(&mut s, 0.0, 0.0);
    add(
        &mut s,
        RecipeKind::Linear {
            dir1: dir(Axis::X, "3", "10"),
            dir2: Some(dir(Axis::Y, "2", "5")),
        },
        &[p],
    )
    .unwrap();
    let r = add(
        &mut s,
        RecipeKind::Circular {
            centre: c,
            count: "5".to_owned(),
            angle: "360".to_owned(),
            symmetric: false,
            rotate: true,
        },
        &[p],
    )
    .unwrap();
    s.apply(&Command::SetInstanceSuppressed {
        id: r,
        index: 1,
        suppressed: true,
    })
    .unwrap();
    let text = locus_doc::file::save(s.document()).unwrap();
    let back = locus_doc::file::load(&text).unwrap();
    assert_eq!(&back, s.document());
    assert_eq!(locus_doc::file::save(&back).unwrap(), text);
}
