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
