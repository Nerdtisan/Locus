// SPDX-License-Identifier: AGPL-3.0-or-later

//! Layer commands (#115; SPEC §4 Layer rules 1, 2, 5; §11).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use locus_doc::{
    Command, Constraint, Dimension, DocError, Document, DrawParams, EntityId, Geom, LayerId,
    LayerProps, Operation, OperationKind, Sketcher,
};
use locus_geom::Vec2;

fn add_layer(s: &mut Sketcher, name: &str, operation: Operation) -> LayerId {
    s.apply(&Command::AddLayer {
        name: name.to_owned(),
        colour: [1, 2, 3],
        operation,
    })
    .unwrap()
    .layers[0]
}

fn pt(s: &mut Sketcher, layer: LayerId, x: f64, y: f64) -> EntityId {
    s.apply(&Command::AddEntity {
        layer,
        construction: false,
        geom: Geom::Point {
            pos: Vec2::new(x, y),
        },
    })
    .unwrap()
    .entities[0]
}

#[test]
fn add_layer_assigns_id_and_order_and_undoes_exactly() {
    let mut s = Sketcher::new(Document::new());
    let before = s.document().clone();
    let id = add_layer(&mut s, "Score", Operation::Score);
    let l = s.document().layer(id).unwrap();
    assert_eq!(l.name, "Score");
    assert_eq!(l.export_order, 1);
    assert!(l.visible && !l.locked && l.export_enabled);
    assert!(id.get() < s.document().next_id());
    s.undo().unwrap();
    assert!(s.document().layer(id).is_none());
    // Redo restores the same ID.
    s.redo().unwrap();
    assert!(s.document().layer(id).is_some());
    s.undo().unwrap();
    assert_eq!(s.document().layers(), before.layers());
}

#[test]
fn bad_layers_are_rejected_without_a_trace() {
    let mut s = Sketcher::new(Document::new());
    let bad_name = Command::AddLayer {
        name: "  ".to_owned(),
        colour: [0; 3],
        operation: Operation::Score,
    };
    assert!(matches!(s.apply(&bad_name), Err(DocError::Invalid(_))));
    let bad_op = Command::AddLayer {
        name: "x".to_owned(),
        colour: [0; 3],
        operation: Operation::Draw(DrawParams {
            pen_number: 0,
            pen_width: 0.4,
        }),
    };
    assert!(matches!(s.apply(&bad_op), Err(DocError::BadValue { .. })));
    assert_eq!(s.document().layers().len(), 1);
    assert!(!s.editor().can_undo());
    let unknown = Command::SetLayerProps {
        id: LayerId::new(999),
        props: LayerProps::default(),
    };
    assert!(matches!(s.apply(&unknown), Err(DocError::UnknownLayer(_))));
}

#[test]
fn set_props_changes_only_named_fields_and_undoes_exactly() {
    let mut s = Sketcher::new(Document::new());
    let l = Document::default_layer();
    let before = s.document().layer(l).unwrap().clone();
    s.apply(&Command::SetLayerProps {
        id: l,
        props: LayerProps {
            name: Some("Outline".to_owned()),
            colour: Some([9, 8, 7]),
            operation: Some(OperationKind::Engrave.default_operation()),
            export_order: Some(5),
            visible: Some(false),
            export_enabled: Some(false),
        },
    })
    .unwrap();
    let after = s.document().layer(l).unwrap();
    assert_eq!(after.name, "Outline");
    assert_eq!(after.colour, [9, 8, 7]);
    assert_eq!(after.operation.kind(), OperationKind::Engrave);
    assert_eq!(after.export_order, 5);
    assert!(!after.visible && !after.export_enabled);
    assert!(!after.locked);
    s.undo().unwrap();
    assert_eq!(s.document().layer(l).unwrap(), &before);
    // A no-op edit leaves no history.
    s.apply(&Command::SetLayerProps {
        id: l,
        props: LayerProps::default(),
    })
    .unwrap();
    assert!(!s.editor().can_undo());
}

#[test]
fn moving_constrained_entities_keeps_constraints_and_the_solve() {
    let mut s = Sketcher::new(Document::new());
    let l1 = Document::default_layer();
    let l2 = add_layer(&mut s, "Other", Operation::Score);
    let a = pt(&mut s, l1, 0.0, 0.0);
    let b = pt(&mut s, l1, 7.0, 1.0);
    s.apply(&Command::AddConstraint {
        constraint: Constraint::Fix { point: a },
    })
    .unwrap();
    s.apply(&Command::AddConstraint {
        constraint: Constraint::Distance {
            a,
            b,
            dim: Dimension::driving(10.0),
        },
    })
    .unwrap();
    let constraints = s.document().constraints().clone();
    let solved: Vec<_> = [a, b]
        .iter()
        .map(|p| s.document().point_pos(*p).unwrap())
        .collect();
    s.apply(&Command::MoveToLayer {
        entities: vec![b],
        layer: l2,
    })
    .unwrap();
    assert_eq!(s.document().entity(b).unwrap().layer, l2);
    assert_eq!(s.document().constraints(), &constraints);
    let after: Vec<_> = [a, b]
        .iter()
        .map(|p| s.document().point_pos(*p).unwrap())
        .collect();
    assert_eq!(solved, after);
    s.undo().unwrap();
    assert_eq!(s.document().entity(b).unwrap().layer, l1);
}

#[test]
fn move_is_atomic_and_validated() {
    let mut s = Sketcher::new(Document::new());
    let l1 = Document::default_layer();
    let a = pt(&mut s, l1, 0.0, 0.0);
    let l2 = add_layer(&mut s, "Other", Operation::Score);
    let bad = Command::MoveToLayer {
        entities: vec![a, EntityId::new(999)],
        layer: l2,
    };
    assert!(matches!(s.apply(&bad), Err(DocError::UnknownEntity(_))));
    assert_eq!(s.document().entity(a).unwrap().layer, l1, "rolled back");
    let bad = Command::MoveToLayer {
        entities: vec![a],
        layer: LayerId::new(999),
    };
    assert!(matches!(s.apply(&bad), Err(DocError::UnknownLayer(_))));
}

#[test]
fn hiding_a_layer_does_not_change_the_solve() {
    let mut s = Sketcher::new(Document::new());
    let l = Document::default_layer();
    let a = pt(&mut s, l, 0.0, 0.0);
    let b = pt(&mut s, l, 3.0, 4.0);
    s.apply(&Command::AddConstraint {
        constraint: Constraint::Distance {
            a,
            b,
            dim: Dimension::driving(10.0),
        },
    })
    .unwrap();
    let pos = |s: &Sketcher| s.document().point_pos(b).unwrap();
    let before = pos(&s);
    s.apply(&Command::SetLayerProps {
        id: l,
        props: LayerProps {
            visible: Some(false),
            ..LayerProps::default()
        },
    })
    .unwrap();
    assert_eq!(pos(&s), before);
}

// ── #116: lock ─────────────────────────────────────────────────────────

fn lock(s: &mut Sketcher, id: LayerId, locked: bool) -> Result<(), DocError> {
    s.apply(&Command::SetLayerLocked { id, locked }).map(|_| ())
}

/// `a` on layer 1 at the origin, `b` on layer 2, 10 mm apart.
fn two_layer_pair() -> (Sketcher, LayerId, LayerId, EntityId, EntityId) {
    let mut s = Sketcher::new(Document::new());
    let l1 = Document::default_layer();
    let l2 = add_layer(&mut s, "Other", Operation::Score);
    let a = pt(&mut s, l1, 0.0, 0.0);
    let b = pt(&mut s, l2, 7.0, 1.0);
    s.apply(&Command::AddConstraint {
        constraint: Constraint::Distance {
            a,
            b,
            dim: Dimension::driving(10.0),
        },
    })
    .unwrap();
    (s, l1, l2, a, b)
}

#[test]
fn locked_geometry_is_fixed_for_the_solver_and_refuses_edits() {
    let (mut s, l1, _l2, a, b) = two_layer_pair();
    let a_at = s.document().point_pos(a).unwrap();
    lock(&mut s, l1, true).unwrap();
    assert!(s.document().layer(l1).unwrap().locked);

    // Re-dimensioning must move the free point, never the locked one.
    let dim = *s.document().constraints().keys().next().unwrap();
    s.apply(&Command::SetDimension {
        id: dim,
        value: 25.0,
    })
    .unwrap();
    assert_eq!(s.document().point_pos(a).unwrap(), a_at);
    let d = (s.document().point_pos(b).unwrap() - a_at).length();
    assert!((d - 25.0).abs() < 1e-6, "{d}");
    assert!(s.state().fully_constrained.contains(&a));

    // Direct edits of locked geometry are refused, with no history.
    let depth = s.editor().can_undo();
    for cmd in [
        Command::ModifyEntity {
            id: a,
            geom: Geom::Point {
                pos: Vec2::new(1.0, 1.0),
            },
        },
        Command::ToggleConstruction { id: a },
        Command::AddEntity {
            layer: l1,
            construction: false,
            geom: Geom::Point { pos: Vec2::ZERO },
        },
        Command::MoveToLayer {
            entities: vec![a],
            layer: Document::default_layer(),
        },
    ] {
        assert_eq!(s.apply(&cmd), Err(DocError::LayerLocked(l1)), "{cmd:?}");
    }
    assert_eq!(depth, s.editor().can_undo());

    // Unlock is undoable and frees the geometry again.
    lock(&mut s, l1, false).unwrap();
    s.apply(&Command::ModifyEntity {
        id: a,
        geom: Geom::Point {
            pos: Vec2::new(1.0, 1.0),
        },
    })
    .unwrap();
}

#[test]
fn lock_and_unlock_undo_exactly() {
    let (mut s, l1, _, _, _) = two_layer_pair();
    let before = s.document().clone();
    lock(&mut s, l1, true).unwrap();
    s.undo().unwrap();
    assert_eq!(s.document(), &before);
    s.redo().unwrap();
    assert!(s.document().layer(l1).unwrap().locked);
}

#[test]
fn a_new_constraint_against_locked_geometry_names_the_conflict() {
    let (mut s, l1, _, a, b) = two_layer_pair();
    lock(&mut s, l1, true).unwrap();
    // `b` is 10 from the locked `a`; a second, different distance conflicts.
    let err = s
        .apply(&Command::AddConstraint {
            constraint: Constraint::Distance {
                a,
                b,
                dim: Dimension::driving(20.0),
            },
        })
        .unwrap_err();
    assert!(matches!(err, DocError::Conflict { .. }), "{err:?}");
}

#[test]
fn pre_lock_check_lists_constraints_the_lock_would_make_redundant() {
    let (mut s, l1, _, a, b) = two_layer_pair();
    // Pin `b` too: with `a` locked, the distance then adds nothing.
    s.apply(&Command::AddConstraint {
        constraint: Constraint::Fix { point: b },
    })
    .ok();
    let dist = *s.document().constraints().keys().next().unwrap();
    let report = s.check_lock(l1).unwrap();
    assert!(report.conflicts.is_empty());
    let listed: Vec<_> = report
        .redundant
        .iter()
        .flat_map(|g| g.constraints.iter().copied())
        .collect();
    assert!(listed.contains(&dist), "{report:?}");
    assert!(
        report
            .redundant
            .iter()
            .any(|g| g.entities.contains(&a) || g.entities.contains(&b))
    );
    // Nothing changed, and the lock is still allowed (redundant ≠ conflict).
    assert!(!s.document().layer(l1).unwrap().locked);
    lock(&mut s, l1, true).unwrap();
    // The lock's own constraints never leak into the user-visible state.
    for g in &s.state().redundant {
        assert!(
            g.constraints
                .iter()
                .all(|c| s.document().constraint(*c).is_some())
        );
    }
}

#[test]
fn check_lock_on_an_unconstrained_layer_is_empty() {
    let (mut s, _l1, l2, _, _) = two_layer_pair();
    let lone = add_layer(&mut s, "Lone", Operation::Reference);
    pt(&mut s, lone, 50.0, 50.0);
    assert_eq!(
        s.check_lock(lone).unwrap(),
        locus_doc::LockReport::default()
    );
    assert!(matches!(
        s.check_lock(LayerId::new(999)),
        Err(DocError::UnknownLayer(_))
    ));
    let _ = l2;
}

// ── #117: delete layer ─────────────────────────────────────────────────

#[test]
fn delete_layer_drops_geometry_and_constraints_atomically_and_undoes_exactly() {
    let (mut s, l1, l2, a, b) = two_layer_pair();
    // A second constraint inside l2, and one that bridges to l1.
    let c = pt(&mut s, l2, 3.0, 3.0);
    s.apply(&Command::AddConstraint {
        constraint: Constraint::Distance {
            a: b,
            b: c,
            dim: Dimension::driving(4.0),
        },
    })
    .unwrap();
    let before = s.document().clone();
    let plan = s.document().layer_deletion(l2).unwrap();
    assert_eq!(plan.entities, vec![b, c]);
    assert_eq!(plan.constraints.len(), 2);
    assert_eq!(
        plan.touched_elsewhere,
        vec![a],
        "names the other-layer entity"
    );
    assert!(plan.blocked_by.is_empty());

    s.apply(&Command::DeleteLayer { id: l2 }).unwrap();
    let d = s.document();
    assert!(d.layer(l2).is_none());
    assert!(d.entity(b).is_none() && d.entity(c).is_none());
    assert!(d.entity(a).is_some());
    assert!(d.constraints().is_empty());
    assert_eq!(d.layer(l1).map(|l| l.locked), Some(false));

    // One undo brings everything back, bit for bit.
    s.undo().unwrap();
    assert_eq!(s.document(), &before);
    s.redo().unwrap();
    assert!(s.document().layer(l2).is_none());
    s.undo().unwrap();
    assert_eq!(s.document(), &before);
}

#[test]
fn delete_layer_refusals_change_nothing() {
    let (mut s, l1, l2, a, b) = two_layer_pair();
    // A line on l1 that ends at `b` (on l2): l2 can't go without it.
    let line = s
        .apply(&Command::AddEntity {
            layer: l1,
            construction: false,
            geom: Geom::Line { start: a, end: b },
        })
        .unwrap()
        .entities[0];
    let before = s.document().clone();
    assert_eq!(
        s.apply(&Command::DeleteLayer { id: l2 }),
        Err(DocError::LayerInUse {
            layer: l2,
            entities: vec![line]
        })
    );
    assert_eq!(
        s.document().layer_deletion(l2).unwrap().blocked_by,
        vec![line]
    );
    // Locked and unknown layers.
    lock(&mut s, l2, true).unwrap();
    assert_eq!(
        s.apply(&Command::DeleteLayer { id: l2 }),
        Err(DocError::LayerLocked(l2))
    );
    assert!(matches!(
        s.apply(&Command::DeleteLayer {
            id: LayerId::new(999)
        }),
        Err(DocError::UnknownLayer(_))
    ));
    lock(&mut s, l2, false).unwrap();
    assert_eq!(s.document(), &before);

    // The last layer stays.
    let mut solo = Sketcher::new(Document::new());
    assert!(matches!(
        solo.apply(&Command::DeleteLayer {
            id: Document::default_layer()
        }),
        Err(DocError::Invalid(_))
    ));
}

/// #292: a layer holding a pattern (its source and instances) could never be
/// deleted — `DeleteLayer` refused with `RecipeInUse` while the dialog still
/// offered Delete, so the layer "stayed". The recipe goes with the layer.
#[test]
fn delete_layer_takes_its_pattern_and_undoes_exactly() {
    let mut s = Sketcher::new(Document::new());
    let l2 = add_layer(&mut s, "Pattern", Operation::Reference);
    let src = pt(&mut s, l2, 5.0, 5.0);
    let r = s
        .apply(&Command::AddRecipe {
            kind: locus_doc::RecipeKind::Linear {
                dir1: locus_doc::LinearDir {
                    axis: locus_doc::Axis::X,
                    count: "3".to_owned(),
                    spacing: "10".to_owned(),
                    symmetric: false,
                },
                dir2: None,
            },
            sources: vec![src],
        })
        .unwrap()
        .recipes[0];
    let before = s.document().clone();
    let plan = s.document().layer_deletion(l2).unwrap();
    assert_eq!(plan.recipes, vec![r], "the dialog names the pattern");
    assert!(plan.entities.len() > 1, "instances are counted");

    s.apply(&Command::DeleteLayer { id: l2 }).unwrap();
    let d = s.document();
    assert!(d.layer(l2).is_none() && d.recipe(r).is_none());
    assert!(d.entity(src).is_none());
    assert!(d.entities().iter().all(|(_, e)| e.layer != l2));

    s.undo().unwrap();
    assert_eq!(s.document(), &before);
}
