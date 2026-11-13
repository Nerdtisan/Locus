// SPDX-License-Identifier: AGPL-3.0-or-later

//! Stroke-edge dimensions (SPEC §5 Stroke-edge constraint reference, §7, #239,
//! ADR 0014): distances measured to the edge of a pen line or laser cut.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use locus_doc::{
    Command, Compensation, Constraint, CutParams, Dimension, Document, Edge, EntityId, Geom,
    LayerId, LayerProps, Operation, Sketcher,
};
use locus_geom::Vec2;

const TOL: f64 = 1e-6;

fn cut(kerf: f64) -> Operation {
    Operation::Cut(CutParams {
        kerf_width: kerf,
        compensation: Compensation::None,
        passes: 1,
    })
}

fn set_op(s: &mut Sketcher, layer: LayerId, op: Operation) -> Result<(), locus_doc::DocError> {
    s.apply(&Command::SetLayerProps {
        id: layer,
        props: LayerProps {
            operation: Some(op),
            ..LayerProps::default()
        },
    })
    .map(|_| ())
}

fn pt(s: &mut Sketcher, layer: LayerId, x: f64, y: f64) -> EntityId {
    s.apply(&Command::AddEntity {
        layer,
        construction: false,
        style: Default::default(),
        geom: Geom::Point {
            pos: Vec2::new(x, y),
        },
    })
    .unwrap()
    .entities[0]
}

fn line(s: &mut Sketcher, layer: LayerId, a: EntityId, b: EntityId) -> EntityId {
    s.apply(&Command::AddEntity {
        layer,
        construction: false,
        style: Default::default(),
        geom: Geom::Line { start: a, end: b },
    })
    .unwrap()
    .entities[0]
}

fn add(s: &mut Sketcher, c: Constraint) -> locus_doc::ConstraintId {
    s.apply(&Command::AddConstraint { constraint: c })
        .unwrap()
        .constraints[0]
}

/// Two horizontal lines, the lower one fixed, the upper one free to move
/// vertically, both on `layer`.
fn two_lines(s: &mut Sketcher, layer: LayerId) -> (EntityId, EntityId) {
    let (a, b) = (pt(s, layer, 0.0, 0.0), pt(s, layer, 20.0, 0.0));
    let (c, d) = (pt(s, layer, 0.0, 9.0), pt(s, layer, 20.0, 9.0));
    let (l0, l1) = (line(s, layer, a, b), line(s, layer, c, d));
    for k in [a, b] {
        add(s, Constraint::Fix { point: k });
    }
    add(s, Constraint::Horizontal { line: l1 });
    (l0, l1)
}

fn gap(s: &Sketcher, l0: EntityId, l1: EntityId) -> f64 {
    let y = |l: EntityId| {
        let Geom::Line { start, .. } = s.document().entity(l).unwrap().geom else {
            unreachable!()
        };
        s.document().point_pos(start).unwrap().y
    };
    y(l1) - y(l0)
}

/// The outside-edge distance between two cut lines is exact, and follows the
/// kerf when it changes (a re-solve).
#[test]
fn outside_edges_are_exact_and_follow_the_kerf() {
    let mut s = Sketcher::new(Document::new());
    let l = Document::default_layer();
    set_op(&mut s, l, cut(0.4)).unwrap();
    let (l0, l1) = two_lines(&mut s, l);
    let id = add(
        &mut s,
        Constraint::Distance {
            a: l0,
            b: l1,
            dim: Dimension::new(10.4, false, Edge::Outside),
        },
    );
    // Edge to edge 10.4 = centre 10 + kerf/2 on each side.
    assert!((gap(&s, l0, l1) - 10.0).abs() < TOL, "{}", gap(&s, l0, l1));
    set_op(&mut s, l, cut(1.0)).unwrap();
    // Centre 9.4 now: edge-to-edge still 10.4.
    assert!((gap(&s, l0, l1) - 9.4).abs() < TOL, "{}", gap(&s, l0, l1));
    // Same distance to the inside edge (the gap between the cuts).
    s.apply(&Command::SetDimensionEdge {
        id,
        edge: Edge::Inside,
    })
    .unwrap();
    assert!((gap(&s, l0, l1) - 11.4).abs() < TOL, "{}", gap(&s, l0, l1));
    s.apply(&Command::SetDimensionEdge {
        id,
        edge: Edge::Centre,
    })
    .unwrap();
    assert!((gap(&s, l0, l1) - 10.4).abs() < TOL);
    // One undo step per change.
    s.undo().unwrap();
    assert!((gap(&s, l0, l1) - 11.4).abs() < TOL);
}

/// Lines on different layers each contribute their own half stroke.
#[test]
fn each_operand_uses_its_own_layers_stroke() {
    let mut s = Sketcher::new(Document::new());
    let cut_layer = Document::default_layer();
    set_op(&mut s, cut_layer, cut(0.4)).unwrap();
    let pen = s
        .apply(&Command::AddLayer {
            name: "Pen".into(),
            colour: [0, 0, 255],
            operation: Operation::Draw,
        })
        .unwrap()
        .layers[0];
    let (a, b) = (
        pt(&mut s, cut_layer, 0.0, 0.0),
        pt(&mut s, cut_layer, 20.0, 0.0),
    );
    let (c, d) = (pt(&mut s, pen, 0.0, 9.0), pt(&mut s, pen, 20.0, 9.0));
    let (l0, l1) = (line(&mut s, cut_layer, a, b), line(&mut s, pen, c, d));
    // The pen width is the line's own style since #287 (SPEC §4).
    s.apply(&Command::SetStyle {
        entities: vec![l1],
        patch: locus_doc::StylePatch {
            width: Some(0.8),
            ..Default::default()
        },
    })
    .unwrap();
    for k in [a, b] {
        add(&mut s, Constraint::Fix { point: k });
    }
    add(&mut s, Constraint::Horizontal { line: l1 });
    add(
        &mut s,
        Constraint::Distance {
            a: l0,
            b: l1,
            dim: Dimension::new(10.0, false, Edge::Outside),
        },
    );
    // Outside: 10 = centre + 0.2 + 0.4.
    assert!((gap(&s, l0, l1) - 9.4).abs() < TOL, "{}", gap(&s, l0, l1));
    // A wider pen on the line re-solves the gap (SPEC §5, #287): 10 = centre + 0.2 + 0.8.
    s.apply(&Command::SetStyle {
        entities: vec![l1],
        patch: locus_doc::StylePatch {
            width: Some(1.6),
            ..Default::default()
        },
    })
    .unwrap();
    assert!((gap(&s, l0, l1) - 9.0).abs() < TOL, "{}", gap(&s, l0, l1));
}

/// Radius and diameter to a circle's outer / inner edge; a driven dimension
/// reports the measurement in edge terms.
#[test]
fn circles_and_driven_dimensions_measure_to_the_edge() {
    let mut s = Sketcher::new(Document::new());
    let l = Document::default_layer();
    set_op(&mut s, l, cut(0.5)).unwrap();
    let ctr = pt(&mut s, l, 0.0, 0.0);
    let circle = s
        .apply(&Command::AddEntity {
            layer: l,
            construction: false,
            style: Default::default(),
            geom: Geom::Circle {
                center: ctr,
                radius: 4.0,
            },
        })
        .unwrap()
        .entities[0];
    add(&mut s, Constraint::Fix { point: ctr });
    let r = |s: &Sketcher| match s.document().entity(circle).unwrap().geom {
        Geom::Circle { radius, .. } => radius,
        _ => unreachable!(),
    };
    let rid = add(
        &mut s,
        Constraint::Radius {
            curve: circle,
            dim: Dimension::new(5.0, false, Edge::Outside),
        },
    );
    assert!((r(&s) - 4.75).abs() < TOL, "{}", r(&s));
    s.apply(&Command::SetDimensionEdge {
        id: rid,
        edge: Edge::Inside,
    })
    .unwrap();
    assert!((r(&s) - 5.25).abs() < TOL);
    s.undo().unwrap();
    s.apply(&Command::RemoveConstraint { id: rid }).unwrap();
    // Diameter counts the stroke on both sides.
    add(
        &mut s,
        Constraint::Diameter {
            curve: circle,
            dim: Dimension::new(10.5, false, Edge::Outside),
        },
    );
    assert!((r(&s) - 5.0).abs() < TOL, "{}", r(&s));
    // A driven outside radius reports the outer edge of the circle as drawn.
    let did = add(
        &mut s,
        Constraint::Radius {
            curve: circle,
            dim: Dimension::new(1.0, true, Edge::Outside),
        },
    );
    let v = s
        .document()
        .constraint(did)
        .unwrap()
        .dimension()
        .unwrap()
        .value;
    assert!((v - 5.25).abs() < TOL, "{v}");
}

/// A stroke edge on something without an edge, or one that would need a
/// negative centreline value, is refused and leaves nothing behind.
#[test]
fn impossible_edges_are_refused() {
    let mut s = Sketcher::new(Document::new());
    let l = Document::default_layer();
    set_op(&mut s, l, cut(0.4)).unwrap();
    let (a, b) = (pt(&mut s, l, 0.0, 0.0), pt(&mut s, l, 5.0, 0.0));
    // Point to point has no stroke edge.
    let e = s.apply(&Command::AddConstraint {
        constraint: Constraint::Distance {
            a,
            b,
            dim: Dimension::new(5.0, false, Edge::Outside),
        },
    });
    assert!(e.is_err());
    let (l0, l1) = two_lines(&mut s, l);
    let depth = s.editor().can_undo();
    // Outside 0.2 needs centre -0.2: unsatisfiable.
    let e = s.apply(&Command::AddConstraint {
        constraint: Constraint::Distance {
            a: l0,
            b: l1,
            dim: Dimension::new(0.2, false, Edge::Outside),
        },
    });
    assert!(e.is_err());
    assert_eq!(s.editor().can_undo(), depth);
    // A kerf change that breaks an existing edge dimension is refused whole.
    add(
        &mut s,
        Constraint::Distance {
            a: l0,
            b: l1,
            dim: Dimension::new(1.0, false, Edge::Outside),
        },
    );
    assert!(set_op(&mut s, l, cut(5.0)).is_err());
    assert!(matches!(
        s.document().layer(l).unwrap().operation,
        Operation::Cut(p) if (p.kerf_width - 0.4).abs() < TOL
    ));
}

/// Layers without a stroke: every edge is the centreline.
#[test]
fn layers_without_a_stroke_measure_to_the_centreline() {
    let mut s = Sketcher::new(Document::new());
    let l = Document::default_layer();
    set_op(&mut s, l, Operation::Score).unwrap();
    let (l0, l1) = two_lines(&mut s, l);
    add(
        &mut s,
        Constraint::Distance {
            a: l0,
            b: l1,
            dim: Dimension::new(7.0, false, Edge::Outside),
        },
    );
    assert!((gap(&s, l0, l1) - 7.0).abs() < TOL);
}
