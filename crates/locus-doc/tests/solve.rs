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

// ── Dragging (#71) ───────────────────────────────────────────────────────

fn snapshot(s: &Sketcher) -> Vec<(EntityId, Vec2)> {
    s.document()
        .entities()
        .iter()
        .filter_map(|(id, _)| s.document().point_pos(id).map(|p| (id, p)))
        .collect()
}

/// A loose rectangle: horizontal / vertical sides, so dragging a corner
/// keeps it a rectangle but lets it grow.
fn loose_rect() -> Rect {
    let mut r = rough_rect();
    let (p, l) = (r.p, r.l);
    let s = &mut r.s;
    constrain(s, Constraint::Fix { point: p[0] }).unwrap();
    for c in [
        Constraint::Horizontal { line: l[0] },
        Constraint::Horizontal { line: l[2] },
        Constraint::Vertical { line: l[1] },
        Constraint::Vertical { line: l[3] },
    ] {
        constrain(s, c).unwrap();
    }
    r
}

#[test]
fn drag_frames_move_geometry_without_recording_and_end_records_one_step() {
    let mut r = loose_rect();
    let before = snapshot(&r.s);
    let (undo_label, redo_label) = (
        r.s.editor().undo_label().map(str::to_owned),
        r.s.editor().redo_label().map(str::to_owned),
    );
    r.s.begin_drag(r.p[2]).unwrap();
    assert!(r.s.is_dragging());
    for step in 1..=10 {
        let f = r.s.drag_to(Vec2::new(10.0 + f64::from(step), 8.0)).unwrap();
        assert!(f.converged && f.at_target);
        // Frames record nothing.
        assert_eq!(r.s.editor().undo_label(), undo_label.as_deref());
        assert_eq!(r.s.editor().redo_label(), redo_label.as_deref());
    }
    // The corner follows, and the sides stay axis-aligned.
    assert!(close(pos(&r.s, r.p[2]), 20.0, 8.0));
    assert!((pos(&r.s, r.p[1]).x - 20.0).abs() < EPS);
    assert!((pos(&r.s, r.p[3]).y - 8.0).abs() < EPS);
    r.s.end_drag().unwrap();
    assert!(!r.s.is_dragging());
    assert_eq!(r.s.editor().undo_label(), Some("Drag"));
    let after = snapshot(&r.s);
    // One step: undo restores the pre-drag geometry bit for bit, redo the end.
    r.s.undo().unwrap();
    assert_eq!(snapshot(&r.s), before);
    r.s.redo().unwrap();
    assert_eq!(snapshot(&r.s), after);
}

#[test]
fn cancel_restores_the_start_exactly_and_records_nothing() {
    let mut r = loose_rect();
    let before = snapshot(&r.s);
    let undo_label = r.s.editor().undo_label().map(str::to_owned);
    r.s.begin_drag(r.p[2]).unwrap();
    r.s.drag_to(Vec2::new(30.0, 30.0)).unwrap();
    assert_ne!(snapshot(&r.s), before);
    r.s.cancel_drag();
    assert!(!r.s.is_dragging());
    assert_eq!(snapshot(&r.s), before);
    assert_eq!(r.s.editor().undo_label(), undo_label.as_deref());
    // Cancelling with nothing running is harmless.
    r.s.cancel_drag();
}

#[test]
fn a_drag_that_goes_nowhere_leaves_no_history() {
    let mut r = loose_rect();
    let undo_label = r.s.editor().undo_label().map(str::to_owned);
    r.s.begin_drag(r.p[2]).unwrap();
    r.s.end_drag().unwrap();
    assert_eq!(r.s.editor().undo_label(), undo_label.as_deref());
}

#[test]
fn a_fully_constrained_point_does_not_move() {
    let mut r = rough_rect();
    constrain_rect(&mut r);
    let before = snapshot(&r.s);
    r.s.begin_drag(r.p[2]).unwrap();
    let f = r.s.drag_to(Vec2::new(40.0, 40.0)).unwrap();
    assert!(!f.at_target, "the constraints hold it in place");
    r.s.end_drag().unwrap();
    for (id, p) in before {
        assert!(close(pos(&r.s, id), p.x, p.y), "{id} moved");
    }
}

#[test]
fn edits_and_history_are_refused_while_dragging_and_bad_targets_rejected() {
    let mut r = loose_rect();
    assert!(matches!(
        r.s.begin_drag(r.l[0]),
        Err(DocError::WrongKind { .. })
    ));
    assert!(matches!(
        r.s.begin_drag(EntityId::new(9999)),
        Err(DocError::UnknownEntity(_))
    ));
    assert!(matches!(r.s.drag_to(Vec2::ZERO), Err(DocError::Invalid(_))));
    assert!(matches!(r.s.end_drag(), Err(DocError::Invalid(_))));
    r.s.begin_drag(r.p[1]).unwrap();
    assert!(matches!(r.s.begin_drag(r.p[2]), Err(DocError::Invalid(_))));
    assert!(matches!(r.s.undo(), Err(DocError::Invalid(_))));
    assert!(matches!(r.s.redo(), Err(DocError::Invalid(_))));
    assert!(matches!(
        r.s.apply(&Command::ToggleConstruction { id: r.l[0] }),
        Err(DocError::Invalid(_))
    ));
    assert!(r.s.drag_to(Vec2::new(f64::NAN, 0.0)).is_err());
    r.s.cancel_drag();
    assert!(r.s.undo().is_ok());
}

#[test]
fn drag_end_refreshes_the_solve_state() {
    let mut r = loose_rect();
    r.s.begin_drag(r.p[2]).unwrap();
    r.s.drag_to(Vec2::new(15.0, 12.0)).unwrap();
    r.s.end_drag().unwrap();
    assert!(r.s.state().converged);
    assert!(r.s.state().conflicts.is_empty());
}

#[test]
fn dragging_a_circle_centre_keeps_a_driven_dimension_measured() {
    let mut s = Sketcher::new(Document::new());
    let c = pt(&mut s, 0.0, 0.0);
    let circle = add(
        &mut s,
        Geom::Circle {
            center: c,
            radius: 5.0,
        },
    );
    let q = pt(&mut s, 20.0, 0.0);
    let dim = constrain(
        &mut s,
        Constraint::Distance {
            a: c,
            b: q,
            dim: Dimension {
                value: 20.0,
                driven: true,
            },
        },
    )
    .unwrap();
    let _ = circle;
    s.begin_drag(c).unwrap();
    s.drag_to(Vec2::new(8.0, 0.0)).unwrap();
    s.end_drag().unwrap();
    let measured = s
        .document()
        .constraint(dim)
        .unwrap()
        .dimension()
        .unwrap()
        .value;
    assert!((measured - 12.0).abs() < 1e-6, "{measured}");
    s.undo().unwrap();
    let back = s
        .document()
        .constraint(dim)
        .unwrap()
        .dimension()
        .unwrap()
        .value;
    assert!((back - 20.0).abs() < EPS);
}

#[test]
fn a_500_curve_drag_frame_is_fast_enough_to_run_every_frame() {
    // SPEC §3: < 16 ms per frame at 500 entities. The solver has its own gate
    // (tests/drag.rs); this covers the document write-back on top.
    let mut s = Sketcher::new(Document::new());
    let mut first = None;
    let mut prev: Option<EntityId> = None;
    for i in 0..500 {
        let p = pt(&mut s, f64::from(i) * 3.0, f64::from(i % 7));
        if let Some(q) = prev {
            add(&mut s, Geom::Line { start: q, end: p });
        }
        first.get_or_insert(p);
        prev = Some(p);
    }
    let start = std::time::Instant::now();
    s.begin_drag(first.unwrap()).unwrap();
    let frames = 30;
    for i in 0..frames {
        s.drag_to(Vec2::new(-f64::from(i), 5.0)).unwrap();
    }
    let per_frame = start.elapsed().as_secs_f64() * 1000.0 / f64::from(frames);
    s.end_drag().unwrap();
    eprintln!("500-curve drag: {per_frame:.2} ms/frame (document + solver)");
    assert!(per_frame < 16.0, "{per_frame} ms/frame");
}
