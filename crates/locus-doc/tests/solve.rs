// SPDX-License-Identifier: AGPL-3.0-or-later

//! Document ↔ solver integration (#105; SPEC §5, §10, §11).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use locus_doc::{
    Command, Constraint, ConstraintId, Dimension, DocError, Document, EntityId, Geom, Sketcher,
    Status,
};
use locus_geom::Vec2;

const EPS: f64 = 1e-6;

fn add(s: &mut Sketcher, geom: Geom) -> EntityId {
    s.apply(&Command::AddEntity {
        layer: Document::default_layer(),
        construction: false,
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

fn constrain(s: &mut Sketcher, c: Constraint) -> Result<ConstraintId, DocError> {
    s.apply(&Command::AddConstraint { constraint: c })
        .map(|a| a.constraints[0])
}

fn pos(s: &Sketcher, id: EntityId) -> Vec2 {
    s.document().point_pos(id).unwrap()
}

fn close(a: Vec2, x: f64, y: f64) -> bool {
    (a.x - x).abs() < EPS && (a.y - y).abs() < EPS
}

/// A sloppy quadrilateral, four lines, and the handles of everything.
struct Rect {
    s: Sketcher,
    p: [EntityId; 4],
    l: [EntityId; 4],
}

fn rough_rect() -> Rect {
    let mut s = Sketcher::new(Document::new());
    let p = [
        pt(&mut s, 0.3, -0.2),
        pt(&mut s, 9.1, 0.4),
        pt(&mut s, 9.7, 5.6),
        pt(&mut s, -0.4, 4.9),
    ];
    let line = |s: &mut Sketcher, a, b| add(s, Geom::Line { start: a, end: b });
    let l = [
        line(&mut s, p[0], p[1]),
        line(&mut s, p[1], p[2]),
        line(&mut s, p[2], p[3]),
        line(&mut s, p[3], p[0]),
    ];
    Rect { s, p, l }
}

/// Fully constrain the rectangle: 0,0 to 20,10.
fn constrain_rect(r: &mut Rect) {
    let (p, l) = (r.p, r.l);
    let s = &mut r.s;
    // Pin the corner first: once other constraints exist, the solver may move
    // it to satisfy them (minimum-norm), so it must be fixed before they land.
    s.apply(&Command::ModifyEntity {
        id: p[0],
        geom: Geom::Point {
            pos: Vec2::new(0.0, 0.0),
        },
    })
    .unwrap();
    constrain(s, Constraint::Fix { point: p[0] }).unwrap();
    for c in [
        Constraint::Horizontal { line: l[0] },
        Constraint::Horizontal { line: l[2] },
        Constraint::Vertical { line: l[1] },
        Constraint::Vertical { line: l[3] },
    ] {
        constrain(s, c).unwrap();
    }
    constrain(
        s,
        Constraint::Distance {
            a: p[0],
            b: p[1],
            dim: Dimension::driving(20.0),
        },
    )
    .unwrap();
    constrain(
        s,
        Constraint::Distance {
            a: p[1],
            b: p[2],
            dim: Dimension::driving(10.0),
        },
    )
    .unwrap();
}

#[test]
fn constraints_drive_the_stored_geometry() {
    let mut r = rough_rect();
    assert_eq!(r.s.state().status, Status::UnderConstrained);
    constrain_rect(&mut r);
    let st = r.s.state();
    assert!(st.converged);
    assert_eq!(st.dof, 0);
    assert_eq!(st.status, Status::FullyConstrained);
    for id in r.p.iter().chain(&r.l) {
        assert!(st.fully_constrained.contains(id), "{id} should be fixed");
    }
    assert!(close(pos(&r.s, r.p[0]), 0.0, 0.0));
    assert!(close(pos(&r.s, r.p[1]), 20.0, 0.0));
    assert!(close(pos(&r.s, r.p[2]), 20.0, 10.0));
    assert!(close(pos(&r.s, r.p[3]), 0.0, 10.0));
}

#[test]
fn circles_arcs_and_radii_are_solved() {
    let mut s = Sketcher::new(Document::new());
    let c0 = pt(&mut s, 0.0, 0.0);
    let circle = add(
        &mut s,
        Geom::Circle {
            center: c0,
            radius: 3.0,
        },
    );
    let c1 = pt(&mut s, 1.0, 1.0);
    let a = pt(&mut s, 6.0, 1.0);
    let b = pt(&mut s, 1.0, 6.0);
    let arc = add(
        &mut s,
        Geom::Arc {
            center: c1,
            start: a,
            end: b,
        },
    );
    constrain(&mut s, Constraint::Concentric { a: circle, b: arc }).unwrap();
    constrain(
        &mut s,
        Constraint::Radius {
            curve: circle,
            dim: Dimension::driving(4.5),
        },
    )
    .unwrap();
    let st = s.state();
    assert!(st.converged);
    let centre0 = pos(&s, c0);
    assert!(close(pos(&s, c1), centre0.x, centre0.y), "concentric");
    match s.document().entity(circle).unwrap().geom {
        Geom::Circle { radius, .. } => assert!((radius - 4.5).abs() < EPS),
        _ => unreachable!(),
    }
}

#[test]
fn undo_restores_geometry_exactly_and_redo_reapplies_it() {
    let mut r = rough_rect();
    let entities = r.s.document().entities().clone();
    let constraints = r.s.document().constraints().clone();
    constrain(&mut r.s, Constraint::Horizontal { line: r.l[0] }).unwrap();
    let solved = r.s.document().entities().clone();
    assert_ne!(solved, entities, "the solve moved geometry");

    r.s.undo().unwrap();
    assert_eq!(r.s.document().entities(), &entities);
    assert_eq!(r.s.document().constraints(), &constraints);

    r.s.redo().unwrap();
    assert_eq!(r.s.document().entities(), &solved);
}

#[test]
fn conflicting_constraint_is_refused_and_named() {
    let mut r = rough_rect();
    constrain_rect(&mut r);
    let before = r.s.document().clone();
    let undo_label = r.s.editor().undo_label().map(str::to_owned);

    let err = constrain(
        &mut r.s,
        Constraint::Distance {
            a: r.p[3],
            b: r.p[2],
            dim: Dimension::driving(25.0),
        },
    )
    .unwrap_err();
    match err {
        DocError::Conflict {
            constraints,
            entities,
        } => {
            assert!(!constraints.is_empty());
            assert!(!entities.is_empty());
        }
        other => panic!("expected a conflict, got {other:?}"),
    }
    assert_eq!(r.s.document(), &before, "document untouched");
    assert_eq!(r.s.editor().undo_label().map(str::to_owned), undo_label);
}

#[test]
fn redundant_constraint_is_refused_but_driven_is_accepted_and_measured() {
    let mut r = rough_rect();
    constrain_rect(&mut r);
    // Top edge length is already implied by the bottom edge + the rectangle.
    let err = constrain(
        &mut r.s,
        Constraint::Distance {
            a: r.p[3],
            b: r.p[2],
            dim: Dimension::driving(20.0),
        },
    )
    .unwrap_err();
    assert!(matches!(err, DocError::Redundant { .. }), "{err:?}");

    // The same dimension as driven (reference) is fine and gets measured.
    let id = constrain(
        &mut r.s,
        Constraint::Distance {
            a: r.p[0],
            b: r.p[2],
            dim: Dimension {
                value: 1.0,
                driven: true,
            },
        },
    )
    .unwrap();
    let measured =
        r.s.document()
            .constraint(id)
            .unwrap()
            .dimension()
            .unwrap()
            .value;
    assert!(
        (measured - 500f64.sqrt()).abs() < EPS,
        "diagonal {measured}"
    );

    // Making it driving would over-constrain: refused.
    let err =
        r.s.apply(&Command::SetDriven { id, driven: false })
            .unwrap_err();
    assert!(
        matches!(err, DocError::Redundant { .. } | DocError::Conflict { .. }),
        "{err:?}"
    );
}

#[test]
fn unsolvable_dimension_change_is_rolled_back() {
    let mut s = Sketcher::new(Document::new());
    let p = [
        pt(&mut s, 0.0, 0.0),
        pt(&mut s, 3.0, 0.0),
        pt(&mut s, 1.0, 2.0),
    ];
    let dist = |s: &mut Sketcher, a, b, d| {
        constrain(
            s,
            Constraint::Distance {
                a,
                b,
                dim: Dimension::driving(d),
            },
        )
        .unwrap()
    };
    dist(&mut s, p[0], p[1], 3.0);
    dist(&mut s, p[1], p[2], 3.0);
    let third = dist(&mut s, p[2], p[0], 3.0);
    let before = s.document().clone();

    // 3 + 3 < 10: no triangle can satisfy it.
    let err = s
        .apply(&Command::SetDimension {
            id: third,
            value: 10.0,
        })
        .unwrap_err();
    assert!(matches!(err, DocError::Unsolvable { .. }), "{err:?}");
    assert_eq!(s.document(), &before);
    assert!(s.state().converged);
}
