// SPDX-License-Identifier: AGPL-3.0-or-later

//! Constraint model types (SPEC §5, §4 Layer rules 1).
//!
//! Constraints reference `EntityId`s only, never layers, so moving an entity
//! between layers preserves them. These are pure data; solving lives in
//! `locus-solver`.
//!
//! Angles are in **radians**, lengths in millimetres.

use crate::document::Document;
use crate::entity::{EntityKind, Geom};
use crate::error::DocError;
use crate::ids::EntityId;
use serde::{Deserialize, Serialize};

/// Which part of a stroked curve a dimension measures to (SPEC §5
/// Stroke-edge constraint reference, #239, ADR 0014).
///
/// A Draw layer's stroke is the pen width wide, a Cut layer's the kerf; other
/// layers have no stroke, so every edge equals the centreline there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Edge {
    /// The zero-width centreline (the default).
    #[default]
    Centre,
    /// The edge facing the other operand: the gap between two strokes, or a
    /// circle's smaller (inner) edge.
    Inside,
    /// The far edge: the extent across two strokes, or a circle's larger
    /// (outer) edge.
    Outside,
}

impl Edge {
    pub const ALL: [Edge; 3] = [Edge::Centre, Edge::Inside, Edge::Outside];

    #[must_use]
    pub const fn is_centre(&self) -> bool {
        matches!(self, Edge::Centre)
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Edge::Centre => "centreline",
            Edge::Inside => "inside edge",
            Edge::Outside => "outside edge",
        }
    }
}

/// A dimension value. A `driven` dimension is measured from the geometry and
/// reported, not enforced (a "reference" dimension).
///
/// `value` is measured to [`Dimension::edge`]; the solver is given the
/// centreline value (see `Document::edge_offset`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Dimension {
    pub value: f64,
    pub driven: bool,
    /// Omitted from files while it is the centreline (format v12).
    #[serde(default, skip_serializing_if = "Edge::is_centre")]
    pub edge: Edge,
}

impl Dimension {
    #[must_use]
    pub const fn driving(value: f64) -> Self {
        Self {
            value,
            driven: false,
            edge: Edge::Centre,
        }
    }

    #[must_use]
    pub const fn new(value: f64, driven: bool, edge: Edge) -> Self {
        Self {
            value,
            driven,
            edge,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Constraint {
    // ── Geometric (SPEC §5) ──────────────────────────────────────────────
    /// Two points are the same point.
    Coincident {
        a: EntityId,
        b: EntityId,
    },
    /// A point lies on a line, circle or arc.
    PointOnCurve {
        point: EntityId,
        curve: EntityId,
    },
    Horizontal {
        line: EntityId,
    },
    Vertical {
        line: EntityId,
    },
    Parallel {
        a: EntityId,
        b: EntityId,
    },
    Perpendicular {
        a: EntityId,
        b: EntityId,
    },
    /// Line–circle/arc or circle/arc–circle/arc tangency.
    Tangent {
        a: EntityId,
        b: EntityId,
    },
    Collinear {
        a: EntityId,
        b: EntityId,
    },
    /// Circles / arcs sharing a centre.
    Concentric {
        a: EntityId,
        b: EntityId,
    },
    /// Equal length (two lines) or equal radius (circles / arcs).
    Equal {
        a: EntityId,
        b: EntityId,
    },
    /// Points `a` and `b` are mirror images about line `axis`.
    Symmetric {
        a: EntityId,
        b: EntityId,
        axis: EntityId,
    },
    /// A point at the midpoint of a line.
    Midpoint {
        point: EntityId,
        line: EntityId,
    },
    /// A point is fixed at its current position.
    Fix {
        point: EntityId,
    },
    // ── Dimensional (SPEC §5) ────────────────────────────────────────────
    /// Point–point, point–line or (parallel) line–line distance.
    Distance {
        a: EntityId,
        b: EntityId,
        dim: Dimension,
    },
    HorizontalDistance {
        a: EntityId,
        b: EntityId,
        dim: Dimension,
    },
    VerticalDistance {
        a: EntityId,
        b: EntityId,
        dim: Dimension,
    },
    /// Angle between two lines, radians.
    Angle {
        a: EntityId,
        b: EntityId,
        dim: Dimension,
    },
    Radius {
        curve: EntityId,
        dim: Dimension,
    },
    Diameter {
        curve: EntityId,
        dim: Dimension,
    },
}

use EntityKind::{Arc, Bezier, Circle, Line, Point};

impl Constraint {
    /// Short stable name, used in error messages.
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Constraint::Coincident { .. } => "coincident",
            Constraint::PointOnCurve { .. } => "point-on-curve",
            Constraint::Horizontal { .. } => "horizontal",
            Constraint::Vertical { .. } => "vertical",
            Constraint::Parallel { .. } => "parallel",
            Constraint::Perpendicular { .. } => "perpendicular",
            Constraint::Tangent { .. } => "tangent",
            Constraint::Collinear { .. } => "collinear",
            Constraint::Concentric { .. } => "concentric",
            Constraint::Equal { .. } => "equal",
            Constraint::Symmetric { .. } => "symmetric",
            Constraint::Midpoint { .. } => "midpoint",
            Constraint::Fix { .. } => "fix",
            Constraint::Distance { .. } => "distance",
            Constraint::HorizontalDistance { .. } => "horizontal-distance",
            Constraint::VerticalDistance { .. } => "vertical-distance",
            Constraint::Angle { .. } => "angle",
            Constraint::Radius { .. } => "radius",
            Constraint::Diameter { .. } => "diameter",
        }
    }

    /// Every entity this constraint mentions, in a fixed order.
    #[must_use]
    pub fn references(&self) -> Vec<EntityId> {
        match self {
            Constraint::Coincident { a, b }
            | Constraint::Parallel { a, b }
            | Constraint::Perpendicular { a, b }
            | Constraint::Tangent { a, b }
            | Constraint::Collinear { a, b }
            | Constraint::Concentric { a, b }
            | Constraint::Equal { a, b }
            | Constraint::Distance { a, b, .. }
            | Constraint::HorizontalDistance { a, b, .. }
            | Constraint::VerticalDistance { a, b, .. }
            | Constraint::Angle { a, b, .. } => vec![*a, *b],
            Constraint::PointOnCurve { point, curve } => vec![*point, *curve],
            Constraint::Horizontal { line } | Constraint::Vertical { line } => vec![*line],
            Constraint::Symmetric { a, b, axis } => vec![*a, *b, *axis],
            Constraint::Midpoint { point, line } => vec![*point, *line],
            Constraint::Fix { point } => vec![*point],
            Constraint::Radius { curve, .. } | Constraint::Diameter { curve, .. } => vec![*curve],
        }
    }

    /// The dimension, for dimensional constraints.
    #[must_use]
    pub fn dimension(&self) -> Option<&Dimension> {
        match self {
            Constraint::Distance { dim, .. }
            | Constraint::HorizontalDistance { dim, .. }
            | Constraint::VerticalDistance { dim, .. }
            | Constraint::Angle { dim, .. }
            | Constraint::Radius { dim, .. }
            | Constraint::Diameter { dim, .. } => Some(dim),
            _ => None,
        }
    }

    pub(crate) fn dimension_mut(&mut self) -> Option<&mut Dimension> {
        match self {
            Constraint::Distance { dim, .. }
            | Constraint::HorizontalDistance { dim, .. }
            | Constraint::VerticalDistance { dim, .. }
            | Constraint::Angle { dim, .. }
            | Constraint::Radius { dim, .. }
            | Constraint::Diameter { dim, .. } => Some(dim),
            _ => None,
        }
    }

    /// Can this dimension measure to a stroke edge? Distances with a line,
    /// radii and diameters can; point-to-point, horizontal / vertical
    /// distances and angles cannot (a point has no edge).
    #[must_use]
    pub fn has_stroke_edge(&self, doc: &Document) -> bool {
        let is_line = |id: &EntityId| {
            doc.entity(*id)
                .is_some_and(|e| matches!(e.geom, Geom::Line { .. }))
        };
        match self {
            Constraint::Distance { a, b, .. } => is_line(a) || is_line(b),
            Constraint::Radius { .. } | Constraint::Diameter { .. } => true,
            _ => false,
        }
    }

    /// Check that every referenced entity exists in `doc`, has a compatible
    /// kind, that paired operands differ, and that dimension values are sane.
    ///
    /// # Errors
    /// The first problem found, as a [`DocError`].
    pub fn validate(&self, doc: &Document) -> Result<(), DocError> {
        let w = self.name();
        let unsolved = doc.unsolved_instances();
        if let Some(e) = self.references().iter().find(|e| unsolved.contains(e)) {
            return Err(DocError::Invalid(format!(
                "entity {} belongs to a live offset, which follows its source; explode the offset to constrain it",
                e.get()
            )));
        }
        let kind = |id: EntityId, ok: &[EntityKind], expected: &'static str| {
            let found = doc
                .entity(id)
                .ok_or(DocError::UnknownEntity(id))?
                .geom
                .kind();
            if ok.contains(&found) {
                Ok(found)
            } else {
                Err(DocError::WrongKind {
                    what: w,
                    entity: id,
                    expected,
                    found,
                })
            }
        };
        let distinct = |a: EntityId, b: EntityId| {
            if a == b {
                Err(DocError::SameEntity { what: w, entity: a })
            } else {
                Ok(())
            }
        };
        let curve = &[Line, Circle, Arc];
        let round = &[Circle, Arc];
        match self {
            Constraint::Coincident { a, b }
            | Constraint::HorizontalDistance { a, b, .. }
            | Constraint::VerticalDistance { a, b, .. } => {
                kind(*a, &[Point], "point")?;
                kind(*b, &[Point], "point")?;
                distinct(*a, *b)?;
            }
            Constraint::PointOnCurve { point, curve: c } => {
                kind(*point, &[Point], "point")?;
                kind(*c, curve, "line, circle or arc")?;
            }
            Constraint::Horizontal { line } | Constraint::Vertical { line } => {
                kind(*line, &[Line], "line")?;
            }
            Constraint::Parallel { a, b }
            | Constraint::Perpendicular { a, b }
            | Constraint::Collinear { a, b }
            | Constraint::Angle { a, b, .. } => {
                kind(*a, &[Line], "line")?;
                kind(*b, &[Line], "line")?;
                distinct(*a, *b)?;
            }
            Constraint::Tangent { a, b } => {
                let ka = kind(
                    *a,
                    &[Line, Circle, Arc, Bezier],
                    "line, circle, arc or Bézier",
                )?;
                let kb = kind(
                    *b,
                    &[Line, Circle, Arc, Bezier],
                    "line, circle, arc or Bézier",
                )?;
                distinct(*a, *b)?;
                if ka == Bezier || kb == Bezier {
                    // G1 at a shared end point, with a line, arc or Bézier.
                    let (other, o) = if ka == Bezier { (kb, *b) } else { (ka, *a) };
                    if other == Circle {
                        return Err(DocError::WrongKind {
                            what: w,
                            entity: o,
                            expected: "line, arc or Bézier (a Bézier cannot be tangent to a whole circle)",
                            found: other,
                        });
                    }
                    if g1_joint(doc, *a, *b).is_none() {
                        return Err(DocError::Invalid(
                            "a tangent Bézier must share an end point with the curve it is tangent to"
                                .to_owned(),
                        ));
                    }
                    return Ok(());
                }
                if ka == Line && kb == Line {
                    return Err(DocError::WrongKind {
                        what: w,
                        entity: *b,
                        expected: "circle or arc (a line cannot be tangent to a line)",
                        found: kb,
                    });
                }
            }
            Constraint::Concentric { a, b } => {
                kind(*a, round, "circle or arc")?;
                kind(*b, round, "circle or arc")?;
                distinct(*a, *b)?;
            }
            Constraint::Equal { a, b } => {
                let ka = kind(*a, &[Line, Circle, Arc], "line, circle or arc")?;
                let kb = kind(*b, &[Line, Circle, Arc], "line, circle or arc")?;
                distinct(*a, *b)?;
                if (ka == Line) != (kb == Line) {
                    return Err(DocError::WrongKind {
                        what: w,
                        entity: *b,
                        expected: "same family as the first operand (line, or circle/arc)",
                        found: kb,
                    });
                }
            }
            Constraint::Symmetric { a, b, axis } => {
                kind(*a, &[Point], "point")?;
                kind(*b, &[Point], "point")?;
                kind(*axis, &[Line], "line")?;
                distinct(*a, *b)?;
            }
            Constraint::Midpoint { point, line } => {
                kind(*point, &[Point], "point")?;
                kind(*line, &[Line], "line")?;
            }
            Constraint::Fix { point } => {
                kind(*point, &[Point], "point")?;
            }
            Constraint::Distance { a, b, .. } => {
                // Line–point in either order; line–line means parallel lines.
                kind(*a, &[Point, Line], "point or line")?;
                kind(*b, &[Point, Line], "point or line")?;
                distinct(*a, *b)?;
            }
            Constraint::Radius { curve: c, .. } | Constraint::Diameter { curve: c, .. } => {
                kind(*c, round, "circle or arc")?;
            }
        }
        if let Some(d) = self.dimension() {
            if !d.edge.is_centre() && !self.has_stroke_edge(doc) {
                return Err(DocError::Invalid(
                    "a stroke edge needs a line or a circle / arc to measure to".to_owned(),
                ));
            }
            let ok = d.value.is_finite()
                && match self {
                    Constraint::Radius { .. } | Constraint::Diameter { .. } => d.value > 0.0,
                    Constraint::Distance { .. } => d.value >= 0.0,
                    _ => true,
                };
            if !ok {
                return Err(DocError::BadValue {
                    what: w,
                    value: d.value,
                });
            }
        }
        Ok(())
    }
}

/// Where two curves meet for a tangent Bézier (G1 continuity, SPEC §4): the
/// shared end point, the Bézier's control point next to it (`handle`), and what
/// fixes the other curve's direction there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct G1Joint {
    pub joint: EntityId,
    pub handle: EntityId,
    pub other: G1Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum G1Other {
    /// A line: its far end point (handle, joint and far end are collinear).
    LineFar(EntityId),
    /// An arc: its centre (the handle direction is perpendicular to the radius).
    ArcCentre(EntityId),
    /// Another Bézier: its control point next to the joint.
    Handle(EntityId),
}

/// The G1 joint of a tangent between `a` and `b`, one of which is a Bézier;
/// `None` when they share no end point or the other one is a circle.
pub(crate) fn g1_joint(doc: &Document, a: EntityId, b: EntityId) -> Option<G1Joint> {
    let (ga, gb) = (&doc.entity(a)?.geom, &doc.entity(b)?.geom);
    let (bez, other) = if matches!(ga, Geom::Bezier { .. }) {
        (ga, gb)
    } else {
        (gb, ga)
    };
    let Geom::Bezier {
        start,
        start_ctrl,
        end_ctrl,
        end,
    } = *bez
    else {
        return None;
    };
    // Candidate joints in a fixed order, each with its handle.
    let ends = [(start, start_ctrl), (end, end_ctrl)];
    for (joint, handle) in ends {
        let found = match *other {
            Geom::Line { start: s, end: e } if s == joint || e == joint => {
                Some(G1Other::LineFar(if s == joint { e } else { s }))
            }
            Geom::Arc {
                center,
                start: s,
                end: e,
            } if s == joint || e == joint => Some(G1Other::ArcCentre(center)),
            Geom::Bezier {
                start: s,
                start_ctrl: sc,
                ..
            } if s == joint => Some(G1Other::Handle(sc)),
            Geom::Bezier {
                end_ctrl: ec,
                end: e,
                ..
            } if e == joint => Some(G1Other::Handle(ec)),
            _ => None,
        };
        if let Some(other) = found {
            return Some(G1Joint {
                joint,
                handle,
                other,
            });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{Fixture, fixture};

    /// One valid instance of every variant.
    fn all(f: &Fixture) -> Vec<Constraint> {
        let (p, l, c, a) = (f.p, f.l, f.c, f.a);
        let d = Dimension::driving(5.0);
        vec![
            Constraint::Coincident { a: p[0], b: p[1] },
            Constraint::PointOnCurve {
                point: p[2],
                curve: c[0],
            },
            Constraint::Horizontal { line: l[0] },
            Constraint::Vertical { line: l[0] },
            Constraint::Parallel { a: l[0], b: l[1] },
            Constraint::Perpendicular { a: l[0], b: l[1] },
            Constraint::Tangent { a: l[0], b: c[0] },
            Constraint::Collinear { a: l[0], b: l[1] },
            Constraint::Concentric { a: c[0], b: a[0] },
            Constraint::Equal { a: c[0], b: c[1] },
            Constraint::Symmetric {
                a: p[0],
                b: p[1],
                axis: l[1],
            },
            Constraint::Midpoint {
                point: p[2],
                line: l[0],
            },
            Constraint::Fix { point: p[3] },
            Constraint::Distance {
                a: p[0],
                b: p[1],
                dim: d,
            },
            Constraint::HorizontalDistance {
                a: p[0],
                b: p[1],
                dim: d,
            },
            Constraint::VerticalDistance {
                a: p[0],
                b: p[3],
                dim: Dimension::new(1.5, true, Edge::Centre),
            },
            Constraint::Angle {
                a: l[0],
                b: l[1],
                dim: Dimension::driving(core::f64::consts::FRAC_PI_2),
            },
            Constraint::Radius {
                curve: c[1],
                dim: d,
            },
            Constraint::Diameter {
                curve: a[0],
                dim: d,
            },
        ]
    }

    #[test]
    fn every_variant_validates_and_round_trips() {
        let f = fixture();
        let all = all(&f);
        assert_eq!(
            all.len(),
            19,
            "one entry per SPEC §5 v1 constraint (+ H/V distance)"
        );
        let mut names: Vec<_> = all.iter().map(Constraint::name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), all.len(), "each variant appears exactly once");
        for c in &all {
            c.validate(&f.doc)
                .unwrap_or_else(|e| panic!("{}: {e}", c.name()));
            let text = ron::to_string(c).unwrap();
            let back: Constraint = ron::from_str(&text).unwrap();
            assert_eq!(&back, c, "{}", c.name());
            assert!(!c.references().is_empty());
        }
    }

    #[test]
    fn dimension_accessors() {
        let f = fixture();
        let all = all(&f);
        let dims = all.iter().filter(|c| c.dimension().is_some()).count();
        assert_eq!(dims, 6);
    }

    #[test]
    fn rejects_unknown_wrong_kind_and_duplicates() {
        let f = fixture();
        let (p, l, c) = (f.p, f.l, f.c);
        let ghost = EntityId::new(999);
        let cases = [
            Constraint::Coincident { a: p[0], b: ghost },
            Constraint::Coincident { a: p[0], b: l[0] },
            Constraint::Coincident { a: p[0], b: p[0] },
            Constraint::Horizontal { line: c[0] },
            Constraint::Parallel { a: l[0], b: l[0] },
            Constraint::Tangent { a: l[0], b: l[1] },
            Constraint::Concentric { a: l[0], b: c[0] },
            Constraint::Equal { a: l[0], b: c[0] },
            Constraint::Midpoint {
                point: l[0],
                line: l[1],
            },
            Constraint::Radius {
                curve: l[0],
                dim: Dimension::driving(1.0),
            },
        ];
        for k in cases {
            assert!(k.validate(&f.doc).is_err(), "{k:?} should be rejected");
        }
    }

    #[test]
    fn rejects_bad_dimension_values() {
        let f = fixture();
        let bad = |v: f64| Constraint::Radius {
            curve: f.c[0],
            dim: Dimension::driving(v),
        };
        for v in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert!(bad(v).validate(&f.doc).is_err());
        }
        let dist = |v: f64| Constraint::Distance {
            a: f.p[0],
            b: f.p[1],
            dim: Dimension::driving(v),
        };
        assert!(dist(0.0).validate(&f.doc).is_ok());
        assert!(dist(-0.1).validate(&f.doc).is_err());
        // Distance accepts point–line in either order and line–line.
        let pl = Constraint::Distance {
            a: f.p[2],
            b: f.l[0],
            dim: Dimension::driving(1.0),
        };
        assert!(pl.validate(&f.doc).is_ok());
        let ll = Constraint::Distance {
            a: f.l[0],
            b: f.l[1],
            dim: Dimension::driving(1.0),
        };
        assert!(ll.validate(&f.doc).is_ok());
    }
}
