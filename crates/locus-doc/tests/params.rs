// SPDX-License-Identifier: AGPL-3.0-or-later

//! Named parameters and dimension expressions (SPEC §5, #171, #172).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use locus_doc::{
    Command, Constraint, ConstraintId, Dimension, DocError, Document, EntityId, Geom, Sketcher,
    file,
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

fn set(s: &mut Sketcher, name: &str, expr: &str) -> Result<(), DocError> {
    s.apply(&Command::SetParameter {
        name: name.to_owned(),
        expr: expr.to_owned(),
    })
    .map(|_| ())
}

/// A fixed point and a second point held 10 mm away by a dimension.
fn rig() -> (Sketcher, EntityId, EntityId, ConstraintId) {
    let mut s = Sketcher::new(Document::new());
    let a = pt(&mut s, 0.0, 0.0);
    let b = pt(&mut s, 10.0, 0.0);
    s.apply(&Command::AddConstraint {
        constraint: Constraint::Fix { point: a },
    })
    .unwrap();
    let d = s
        .apply(&Command::AddConstraint {
            constraint: Constraint::Distance {
                a,
                b,
                dim: Dimension::driving(10.0),
            },
        })
        .unwrap()
        .constraints[0];
    (s, a, b, d)
}

fn x(s: &Sketcher, p: EntityId) -> f64 {
    s.document().point_pos(p).unwrap().x.abs()
}

#[test]
fn changing_a_parameter_updates_dependent_dimensions_in_one_undo_step() {
    let (mut s, _a, b, d) = rig();
    set(&mut s, "material", "3").unwrap();
    s.apply(&Command::SetDimensionExpr {
        id: d,
        expr: Some("material * 2 + 0.2".to_owned()),
    })
    .unwrap();
    assert!((x(&s, b) - 6.2).abs() < 1e-6);
    let before = s.document().clone();
    set(&mut s, "material", "6").unwrap();
    assert!((x(&s, b) - 12.2).abs() < 1e-6, "geometry follows");
    assert_eq!(s.document().parameters()["material"].value, 6.0);
    s.undo().unwrap();
    assert_eq!(*s.document(), before, "one undo restores everything");
    s.redo().unwrap();
    assert!((x(&s, b) - 12.2).abs() < 1e-6);
}

#[test]
fn rejected_expressions_change_nothing() {
    let (mut s, _a, _b, d) = rig();
    set(&mut s, "w", "5").unwrap();
    let before = s.document().clone();
    assert!(set(&mut s, "bad name", "1").is_err());
    assert!(set(&mut s, "sin", "1").is_err());
    assert!(set(&mut s, "p", "1 +").is_err());
    assert!(set(&mut s, "p", "nope * 2").is_err());
    assert!(set(&mut s, "w", "w + 1").is_err(), "self cycle");
    set(&mut s, "p", "w").unwrap();
    assert!(set(&mut s, "w", "p").is_err(), "two-step cycle");
    assert!(
        s.apply(&Command::SetDimensionExpr {
            id: d,
            expr: Some("1 / 0".to_owned())
        })
        .is_err()
    );
    s.undo().unwrap(); // set p
    assert_eq!(*s.document(), before);
}

#[test]
fn delete_is_refused_while_in_use_and_rename_updates_references() {
    let (mut s, _a, b, d) = rig();
    set(&mut s, "material", "3").unwrap();
    set(&mut s, "double", "material * 2").unwrap();
    s.apply(&Command::SetDimensionExpr {
        id: d,
        expr: Some("double + material".to_owned()),
    })
    .unwrap();
    match s.apply(&Command::RemoveParameter {
        name: "material".to_owned(),
    }) {
        Err(DocError::ParameterInUse { users, .. }) => assert_eq!(users.len(), 2),
        other => panic!("expected ParameterInUse, got {other:?}"),
    }
    s.apply(&Command::RenameParameter {
        from: "material".to_owned(),
        to: "stock".to_owned(),
    })
    .unwrap();
    assert_eq!(s.document().parameters()["double"].expr, "stock * 2");
    assert_eq!(s.document().dimension_expr(d), Some("double + stock"));
    assert!((x(&s, b) - 9.0).abs() < 1e-6);
    assert!(
        s.apply(&Command::RenameParameter {
            from: "stock".to_owned(),
            to: "double".to_owned()
        })
        .is_err()
    );
    s.undo().unwrap();
    assert!(s.document().parameters().contains_key("material"));
}

#[test]
fn a_number_or_driven_edit_drops_the_expression() {
    let (mut s, _a, _b, d) = rig();
    set(&mut s, "w", "4").unwrap();
    s.apply(&Command::SetDimensionExpr {
        id: d,
        expr: Some("w".to_owned()),
    })
    .unwrap();
    s.apply(&Command::SetDimension { id: d, value: 7.0 })
        .unwrap();
    assert_eq!(s.document().dimension_expr(d), None);
    s.apply(&Command::SetParameter {
        name: "w".to_owned(),
        expr: "9".to_owned(),
    })
    .unwrap();
    assert!(
        (s.document()
            .constraint(d)
            .unwrap()
            .dimension()
            .unwrap()
            .value
            - 7.0)
            .abs()
            < 1e-9
    );
    s.undo().unwrap();
    s.undo().unwrap();
    assert_eq!(
        s.document().dimension_expr(d),
        Some("w"),
        "undo restores the expression"
    );
}

#[test]
fn angle_expressions_are_degrees() {
    let mut s = Sketcher::new(Document::new());
    let p = [
        pt(&mut s, 0.0, 0.0),
        pt(&mut s, 10.0, 0.0),
        pt(&mut s, 0.0, 10.0),
    ];
    let l = |s: &mut Sketcher, a, b| {
        s.apply(&Command::AddEntity {
            layer: L,
            construction: false,
            geom: Geom::Line { start: a, end: b },
        })
        .unwrap()
        .entities[0]
    };
    let (l1, l2) = (l(&mut s, p[0], p[1]), l(&mut s, p[0], p[2]));
    let id = s
        .apply(&Command::AddConstraint {
            constraint: Constraint::Angle {
                a: l1,
                b: l2,
                dim: Dimension::driving(std::f64::consts::FRAC_PI_2),
            },
        })
        .unwrap()
        .constraints[0];
    s.apply(&Command::SetDimensionExpr {
        id,
        expr: Some("30 + 60".to_owned()),
    })
    .unwrap();
    let v = s
        .document()
        .constraint(id)
        .unwrap()
        .dimension()
        .unwrap()
        .value;
    assert!((v - std::f64::consts::FRAC_PI_2).abs() < 1e-9);
}

#[test]
fn expressions_round_trip_through_save_and_load() {
    let (mut s, _a, _b, d) = rig();
    set(&mut s, "material", "3").unwrap();
    s.apply(&Command::SetDimensionExpr {
        id: d,
        expr: Some("material * 2 + 0.2".to_owned()),
    })
    .unwrap();
    let text = file::save(s.document()).unwrap();
    let back = file::load(&text).unwrap();
    assert_eq!(&back, s.document());
    assert_eq!(file::save(&back).unwrap(), text);
    // A hostile expression in a file is rejected, not evaluated blindly.
    let bad = text.replace("material * 2 + 0.2", "material * ");
    assert!(file::load(&bad).is_err());
}
