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
fn instances_are_driven_and_never_dragged() {
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
    // Instances take constraints since ADR 0015 (second solve phase).
    s.apply(&Command::AddConstraint {
        constraint: Constraint::Fix { point: inst },
    })
    .unwrap();
    assert!(matches!(
        s.apply(&Command::DeleteEntity { id: inst }),
        Err(DocError::RecipeInUse { .. })
    ));
    // A constraint that refers to an instance keeps the recipe from being removed.
    assert!(s.apply(&Command::RemoveRecipe { id: r }).is_err());
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

// ── Constraints onto pattern instances: the second solve phase (ADR 0015) ──

fn inst_pt(s: &Sketcher, r: RecipeId, k: usize) -> EntityId {
    s.document().recipe(r).unwrap().instances().nth(k).unwrap()
}

fn row(s: &mut Sketcher) -> (EntityId, RecipeId) {
    set(s, "pitch", "10").unwrap();
    set(s, "n", "3").unwrap();
    let p = pt(s, 0.0, 0.0);
    let r = add(
        s,
        RecipeKind::Linear {
            dir1: dir(Axis::X, "n", "pitch"),
            dir2: None,
        },
        &[p],
    )
    .unwrap();
    (p, r)
}

/// A free point tied to an instance follows it when the pattern changes.
#[test]
fn a_free_point_can_be_made_coincident_with_an_instance() {
    let mut s = Sketcher::new(Document::new());
    let (_, r) = row(&mut s);
    let inst = inst_pt(&s, r, 1);
    let at = s.document().point_pos(inst).unwrap();
    let q = pt(&mut s, 3.0, 4.0);
    s.apply(&Command::AddConstraint {
        constraint: Constraint::Coincident { a: q, b: inst },
    })
    .unwrap();
    assert!(near(s.document().point_pos(q).unwrap(), at.x, at.y));
    // The pattern changes; the constraint holds again after the re-solve.
    set(&mut s, "pitch", "15").unwrap();
    let at = s.document().point_pos(inst).unwrap();
    assert!(near(s.document().point_pos(q).unwrap(), at.x, at.y));
    // One undo step undoes the pitch and the follower together.
    s.undo().unwrap();
    let at = s.document().point_pos(inst).unwrap();
    assert!(near(s.document().point_pos(q).unwrap(), at.x, at.y));
    assert!((at.x - 10.0).abs() < 1e-6 || (at.x - 20.0).abs() < 1e-6);
}

/// Driving and driven dimensions to an instance are exact, not stale.
#[test]
fn dimensions_to_an_instance_are_enforced_and_measured() {
    let mut s = Sketcher::new(Document::new());
    let (_, r) = row(&mut s);
    let inst = inst_pt(&s, r, 1);
    let q = pt(&mut s, 3.0, 4.0);
    let driving = s
        .apply(&Command::AddConstraint {
            constraint: Constraint::Distance {
                a: q,
                b: inst,
                dim: locus_doc::Dimension::driving(5.0),
            },
        })
        .unwrap()
        .constraints[0];
    let d = |s: &Sketcher| {
        (s.document().point_pos(q).unwrap() - s.document().point_pos(inst).unwrap()).length()
    };
    assert!((d(&s) - 5.0).abs() < 1e-6, "{}", d(&s));
    // Changing the pitch keeps the distance.
    set(&mut s, "pitch", "25").unwrap();
    assert!((d(&s) - 5.0).abs() < 1e-6);
    // Switch to a reference dimension: it reports the measured value, and
    // re-measures when the pattern moves the instance.
    s.apply(&Command::SetDriven {
        id: driving,
        driven: true,
    })
    .unwrap();
    let shown = |s: &Sketcher| {
        s.document()
            .constraint(driving)
            .unwrap()
            .dimension()
            .unwrap()
            .value
    };
    assert!((shown(&s) - d(&s)).abs() < 1e-6);
    let before = shown(&s);
    set(&mut s, "pitch", "30").unwrap();
    assert!((shown(&s) - d(&s)).abs() < 1e-6);
    assert!(
        (shown(&s) - before).abs() > 1.0,
        "the measurement followed the pattern"
    );
}

/// Phase 2 holds the pattern and its sources fixed: a constraint that would
/// need them to move is refused with the document untouched.
#[test]
fn a_constraint_that_would_move_the_pattern_is_refused() {
    let mut s = Sketcher::new(Document::new());
    let (p, r) = row(&mut s);
    let inst = inst_pt(&s, r, 1);
    let f = pt(&mut s, 50.0, 50.0);
    s.apply(&Command::AddConstraint {
        constraint: Constraint::Fix { point: f },
    })
    .unwrap();
    let before = s.document().clone();
    let depth = s.editor().can_undo();
    let bad = s.apply(&Command::AddConstraint {
        constraint: Constraint::Distance {
            a: f,
            b: inst,
            dim: locus_doc::Dimension::driving(1.0),
        },
    });
    match bad {
        Err(DocError::PatternHeld {
            instances, sources, ..
        }) => {
            assert_eq!(instances, vec![inst]);
            assert_eq!(sources, vec![p]);
        }
        other => panic!("fixed point and fixed instance cannot satisfy 1 mm: {other:?}"),
    }
    // Nothing changed (a refused command still burns an ID, ADR 0003).
    assert_eq!(s.document().entities(), before.entities());
    assert_eq!(s.document().constraints(), before.constraints());
    assert_eq!(s.editor().can_undo(), depth);
    // The source never moved.
    assert!(near(s.document().point_pos(p).unwrap(), 0.0, 0.0));
}

/// Constraints onto instances survive a save and load, and the instance can
/// be removed only after the constraint is.
#[test]
fn instance_constraints_round_trip_and_guard_the_pattern() {
    let mut s = Sketcher::new(Document::new());
    let (_, r) = row(&mut s);
    let inst = inst_pt(&s, r, 1);
    let q = pt(&mut s, 1.0, 1.0);
    let c = s
        .apply(&Command::AddConstraint {
            constraint: Constraint::Coincident { a: q, b: inst },
        })
        .unwrap()
        .constraints[0];
    let text = locus_doc::file::save(s.document()).unwrap();
    let loaded = Sketcher::new(locus_doc::file::load(&text).unwrap());
    let at = loaded.document().point_pos(inst).unwrap();
    assert!(near(loaded.document().point_pos(q).unwrap(), at.x, at.y));
    // Shrinking the pattern so the instance would go away is refused while
    // the constraint refers to it (nothing is dropped silently).
    assert!(matches!(
        set(&mut s, "n", "2"),
        Err(DocError::EntityInUse { .. })
    ));
    s.apply(&Command::RemoveConstraint { id: c }).unwrap();
    set(&mut s, "n", "2").unwrap();
}

/// Dragging a source moves its copies; the follower catches up when the drag
/// ends, in the same undo step (ADR 0015).
#[test]
fn a_follower_catches_up_after_a_source_drag() {
    let mut s = Sketcher::new(Document::new());
    let (p, r) = row(&mut s);
    let inst = inst_pt(&s, r, 0);
    let q = pt(&mut s, 3.0, 4.0);
    s.apply(&Command::AddConstraint {
        constraint: Constraint::Coincident { a: q, b: inst },
    })
    .unwrap();
    s.begin_drag(p).unwrap();
    s.drag_to(Vec2::new(5.0, 7.0)).unwrap();
    s.end_drag().unwrap();
    let at = s.document().point_pos(inst).unwrap();
    assert!(near(at, 15.0, 7.0), "{at:?}");
    assert!(near(s.document().point_pos(q).unwrap(), at.x, at.y));
    s.undo().unwrap();
    let at = s.document().point_pos(inst).unwrap();
    assert!(near(s.document().point_pos(q).unwrap(), at.x, at.y));
    assert!(near(at, 10.0, 0.0), "{at:?}");
}

/// Circular patterns take constraints the same way.
#[test]
fn a_follower_tracks_a_circular_pattern_instance() {
    let mut s = Sketcher::new(Document::new());
    let c = pt(&mut s, 0.0, 0.0);
    let p = pt(&mut s, 10.0, 0.0);
    set(&mut s, "ang", "90").unwrap();
    let r = add(
        &mut s,
        RecipeKind::Circular {
            centre: c,
            count: "4".to_owned(),
            angle: "ang * 4".to_owned(),
            symmetric: false,
            rotate: true,
        },
        &[p],
    )
    .unwrap();
    let inst = inst_pt(&s, r, 0);
    let q = pt(&mut s, 1.0, 1.0);
    s.apply(&Command::AddConstraint {
        constraint: Constraint::Coincident { a: q, b: inst },
    })
    .unwrap();
    let at = s.document().point_pos(inst).unwrap();
    assert!(near(s.document().point_pos(q).unwrap(), at.x, at.y));
    // Move the pattern's centre point: the follower follows (the centre is
    // held fixed during phase 2 but moves in phase 1 through its own constraints).
    s.apply(&Command::ModifyEntity {
        id: c,
        geom: Geom::Point {
            pos: Vec2::new(5.0, 5.0),
        },
    })
    .unwrap();
    let at = s.document().point_pos(inst).unwrap();
    assert!(near(s.document().point_pos(q).unwrap(), at.x, at.y));
}
