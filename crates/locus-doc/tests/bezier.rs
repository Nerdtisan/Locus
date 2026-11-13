// SPDX-License-Identifier: AGPL-3.0-or-later

//! Cubic Bézier entity: G1 tangency through the solver (#159; SPEC §4, §5).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use locus_doc::{Command, Constraint, DocError, Document, EntityId, Geom, Sketcher};
use locus_geom::Vec2;

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

fn pos(s: &Sketcher, id: EntityId) -> Vec2 {
    s.document().point_pos(id).unwrap()
}

fn bezier(s: &mut Sketcher, p: [EntityId; 4]) -> EntityId {
    add(
        s,
        Geom::Bezier {
            start: p[0],
            start_ctrl: p[1],
            end_ctrl: p[2],
            end: p[3],
        },
    )
}

/// Sine of the angle between two vectors (0 when parallel).
fn sin(a: Vec2, b: Vec2) -> f64 {
    a.cross(b) / (a.length() * b.length())
}

#[test]
fn bezier_is_tangent_to_a_line_while_the_far_end_is_dragged() {
    let mut s = Sketcher::new(Document::new());
    let a = pt(&mut s, 0.0, 0.0);
    let j = pt(&mut s, 10.0, 0.0);
    let line = add(&mut s, Geom::Line { start: a, end: j });
    let h1 = pt(&mut s, 14.0, 0.5);
    let h2 = pt(&mut s, 20.0, 8.0);
    let e = pt(&mut s, 25.0, 10.0);
    let b = bezier(&mut s, [j, h1, h2, e]);
    s.apply(&Command::AddConstraint {
        constraint: Constraint::Fix { point: a },
    })
    .unwrap();
    s.apply(&Command::AddConstraint {
        constraint: Constraint::Tangent { a: line, b },
    })
    .unwrap();
    let tangent_error = |s: &Sketcher| sin(pos(s, j) - pos(s, a), pos(s, h1) - pos(s, j)).abs();
    assert!(tangent_error(&s) < 1e-6, "tangent after the constraint");

    // Drag the Bézier's far end and then its handle: G1 must hold in every frame.
    for target in [h2, e, h1] {
        s.begin_drag(target).unwrap();
        for k in 1..=20 {
            let at = pos(&s, target) + Vec2::new(0.3 * f64::from(k), -0.7 * f64::from(k));
            s.drag_to(at).unwrap();
            assert!(tangent_error(&s) < 1e-5, "frame {k} dragging {target}");
        }
        s.end_drag().unwrap();
    }
}

#[test]
fn bezier_is_tangent_to_an_arc_and_to_another_bezier() {
    let mut s = Sketcher::new(Document::new());
    let c = pt(&mut s, 0.0, 0.0);
    let start = pt(&mut s, 10.0, 0.0);
    let j = pt(&mut s, 0.0, 10.0);
    let arc = add(
        &mut s,
        Geom::Arc {
            center: c,
            start,
            end: j,
        },
    );
    // Leave the arc end heading left (tangent to the circle there).
    let h1 = pt(&mut s, -3.0, 10.5);
    let h2 = pt(&mut s, -8.0, 15.0);
    let m = pt(&mut s, -12.0, 12.0);
    let b1 = bezier(&mut s, [j, h1, h2, m]);
    let g1 = pt(&mut s, -16.0, 9.0);
    let g2 = pt(&mut s, -20.0, 5.0);
    let e = pt(&mut s, -22.0, 0.0);
    let b2 = bezier(&mut s, [m, g1, g2, e]);
    for constraint in [
        Constraint::Fix { point: c },
        Constraint::Tangent { a: arc, b: b1 },
        Constraint::Tangent { a: b1, b: b2 },
    ] {
        s.apply(&Command::AddConstraint { constraint }).unwrap();
    }
    let check = |s: &Sketcher| {
        let radial = pos(s, j) - pos(s, c);
        let at_arc = radial.dot(pos(s, h1) - pos(s, j)).abs() / radial.length();
        let at_joint = sin(pos(s, m) - pos(s, h2), pos(s, g1) - pos(s, m)).abs();
        (at_arc, at_joint)
    };
    let (a, b) = check(&s);
    assert!(a < 1e-6 && b < 1e-6, "{a} {b}");
    s.begin_drag(e).unwrap();
    for k in 1..=15 {
        s.drag_to(Vec2::new(-22.0 - f64::from(k), f64::from(k) * 1.5))
            .unwrap();
        let (a, b) = check(&s);
        assert!(a < 1e-5 && b < 1e-5, "frame {k}: {a} {b}");
    }
    s.end_drag().unwrap();
}

#[test]
fn tangent_without_a_shared_end_or_to_a_circle_is_refused() {
    let mut s = Sketcher::new(Document::new());
    let p = [
        pt(&mut s, 0.0, 0.0),
        pt(&mut s, 3.0, 3.0),
        pt(&mut s, 6.0, 3.0),
        pt(&mut s, 9.0, 0.0),
    ];
    let b = bezier(&mut s, p);
    let (x, y) = (pt(&mut s, 20.0, 0.0), pt(&mut s, 30.0, 0.0));
    let line = add(&mut s, Geom::Line { start: x, end: y });
    let c = pt(&mut s, 0.0, 20.0);
    let circle = add(
        &mut s,
        Geom::Circle {
            center: c,
            radius: 3.0,
        },
    );
    for other in [line, circle] {
        let err = s
            .apply(&Command::AddConstraint {
                constraint: Constraint::Tangent { a: b, b: other },
            })
            .unwrap_err();
        assert!(
            matches!(err, DocError::Invalid(_) | DocError::WrongKind { .. }),
            "{err:?}"
        );
    }
}

#[test]
fn only_the_limited_constraint_set_applies_to_a_bezier() {
    let mut s = Sketcher::new(Document::new());
    let p = [
        pt(&mut s, 0.0, 0.0),
        pt(&mut s, 3.0, 3.0),
        pt(&mut s, 6.0, 3.0),
        pt(&mut s, 9.0, 0.0),
    ];
    let b = bezier(&mut s, p);
    // Coincident and Fix act on its points ...
    let q = pt(&mut s, 12.0, 0.0);
    for constraint in [
        Constraint::Coincident { a: p[3], b: q },
        Constraint::Fix { point: p[0] },
    ] {
        s.apply(&Command::AddConstraint { constraint }).unwrap();
    }
    assert!(pos(&s, q).distance(pos(&s, p[3])) < 1e-6);
    // ... curve-wide constraints do not apply (SPEC §4: limited set).
    for constraint in [
        Constraint::PointOnCurve { point: q, curve: b },
        Constraint::Radius {
            curve: b,
            dim: locus_doc::Dimension::driving(5.0),
        },
    ] {
        let err = s.apply(&Command::AddConstraint { constraint }).unwrap_err();
        assert!(matches!(err, DocError::WrongKind { .. }), "{err:?}");
    }
}
