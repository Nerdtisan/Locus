// SPDX-License-Identifier: AGPL-3.0-or-later
//! A flat, render-ready snapshot of a [`Document`] (#66).
//!
//! Drawing and hit-testing want resolved geometry (points looked up, bounding
//! boxes computed) in a cache-friendly `Vec`, not a `BTreeMap` walk with
//! point-entity indirection every frame. The scene is rebuilt after every
//! successful command; it is derived state and never edited (SPEC §11).

use locus_doc::{Constraint, ConstraintId, Document, Edge, EntityId, Geom};
use locus_geom::{Aabb, Arc, Circle, Cubic, Line, Vec2};
use std::collections::{BTreeSet, HashMap, HashSet};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Shape {
    Point(Vec2),
    Line(Line),
    Circle(Circle),
    Arc(Arc),
    Cubic(Cubic),
}

impl Shape {
    pub fn bbox(&self) -> Aabb {
        match self {
            Shape::Point(p) => Aabb::from_point(*p),
            Shape::Line(l) => Aabb::from_points(l.a, l.b),
            Shape::Circle(c) => c.bbox(),
            Shape::Arc(a) => a.bbox(),
            Shape::Cubic(c) => c.bbox(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Item {
    #[allow(dead_code)] // key for picking and selection (#67)
    pub id: EntityId,
    pub shape: Shape,
    pub construction: bool,
    /// Drawn in the fully-constrained colour (SPEC §5): see [`Scene::build`].
    pub constrained: bool,
    /// A point that belongs to a line, circle or arc (an endpoint or centre).
    pub owned_point: bool,
    /// The defining end points of a line or arc, for inference (#69).
    pub ends: Option<(EntityId, EntityId)>,
    pub bbox: Aabb,
}

/// What a dimension annotation measures; positions are already resolved.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DimKind {
    /// Aligned distance between two anchors.
    Distance {
        a: Vec2,
        b: Vec2,
    },
    Horizontal {
        a: Vec2,
        b: Vec2,
    },
    Vertical {
        a: Vec2,
        b: Vec2,
    },
    Radius {
        center: Vec2,
        radius: f64,
    },
    Diameter {
        center: Vec2,
        radius: f64,
    },
    Angle {
        at: Vec2,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Annot {
    /// Constraint glyph badge. `slot` spaces several glyphs on one anchor.
    Glyph {
        id: ConstraintId,
        at: Vec2,
        slot: u8,
        label: &'static str,
    },
    Dim {
        id: ConstraintId,
        kind: DimKind,
        value: f64,
        driven: bool,
        /// Which edge the value is measured to (#240).
        edge: Edge,
    },
}

#[derive(Clone, Debug, Default)]
pub struct Scene {
    pub items: Vec<Item>,
    pub annots: Vec<Annot>,
    pub bounds: Option<Aabb>,
}

fn glyph_label(c: &Constraint) -> Option<&'static str> {
    Some(match c {
        // Coincident points share identity in the model; no badge needed.
        Constraint::Coincident { .. } => return None,
        Constraint::PointOnCurve { .. } => "o",
        Constraint::Horizontal { .. } => "H",
        Constraint::Vertical { .. } => "V",
        Constraint::Parallel { .. } => "//",
        Constraint::Perpendicular { .. } => "L",
        Constraint::Tangent { .. } => "T",
        Constraint::Collinear { .. } => "C",
        Constraint::Concentric { .. } => "(o)",
        Constraint::Equal { .. } => "=",
        Constraint::Symmetric { .. } => "S",
        Constraint::Midpoint { .. } => "M",
        Constraint::Fix { .. } => "F",
        _ => return None,
    })
}

impl Scene {
    /// Where a constraint's glyph or dimension attaches to an entity.
    fn anchor(doc: &Document, id: EntityId) -> Option<Vec2> {
        match doc.entity(id)?.geom {
            Geom::Point { pos } => Some(pos),
            Geom::Line { .. } => doc.line(id).map(|l| l.eval(0.5)),
            Geom::Circle { .. } => doc.circle(id).map(|c| c.center),
            Geom::Arc { .. } => doc.arc(id).map(|a| a.eval(0.5)),
            Geom::Bezier { .. } => doc.cubic(id).map(|c| c.eval(0.5)),
            Geom::Text { anchor, .. } => doc.point_pos(anchor),
        }
    }

    /// Build the scene. Hidden layers are skipped. `fully_constrained` is the
    /// solver's set of entities that can no longer move (SPEC §5): they are
    /// drawn in the fully-constrained colour.
    ///
    /// A Bézier's control points are left out unless the curve is in
    /// `handles` (it, or one of its points, is selected): handles show only
    /// for the selected curve (#160).
    pub fn build(
        doc: &Document,
        fully_constrained: &BTreeSet<EntityId>,
        handles: &BTreeSet<EntityId>,
    ) -> Self {
        let hidden: HashSet<EntityId> = doc
            .entities()
            .iter()
            .filter(|(id, _)| !handles.contains(id))
            .filter_map(|(_, e)| match e.geom {
                Geom::Bezier {
                    start_ctrl,
                    end_ctrl,
                    ..
                } => Some([start_ctrl, end_ctrl]),
                _ => None,
            })
            .flatten()
            .collect();
        let owned: HashSet<EntityId> = doc
            .entities()
            .iter()
            .flat_map(|(_, e)| e.geom.point_refs())
            .collect();

        let mut items = Vec::with_capacity(doc.entities().len());
        let mut bounds: Option<Aabb> = None;
        for (id, e) in doc.entities().iter() {
            if doc.layer(e.layer).is_some_and(|l| !l.visible) || hidden.contains(&id) {
                continue;
            }
            let shape = match e.geom {
                // Text draws as its cached outline: one item per segment, all
                // carrying the text's id so any of them picks the text (#180).
                Geom::Text { .. } => {
                    let Some(g) = doc.text_geometry(id) else {
                        continue;
                    };
                    let constrained = fully_constrained.contains(&id);
                    for c in &g.contours {
                        let mut from = c.start;
                        for s in &c.segs {
                            let shape = match *s {
                                locus_text::Seg::Line { end } => Shape::Line(Line::new(from, end)),
                                locus_text::Seg::Cubic { c0, c1, end } => {
                                    Shape::Cubic(Cubic::new(from, c0, c1, end))
                                }
                            };
                            from = s.end();
                            let bbox = shape.bbox();
                            bounds = Some(bounds.map_or(bbox, |b| b.union(bbox)));
                            items.push(Item {
                                id,
                                shape,
                                construction: e.construction,
                                constrained,
                                owned_point: false,
                                ends: None,
                                bbox,
                            });
                        }
                    }
                    continue;
                }
                Geom::Point { pos } => Shape::Point(pos),
                Geom::Line { .. } => match doc.line(id) {
                    Some(l) => Shape::Line(l),
                    None => continue,
                },
                Geom::Circle { .. } => match doc.circle(id) {
                    Some(c) => Shape::Circle(c),
                    None => continue,
                },
                Geom::Arc { .. } => match doc.arc(id) {
                    Some(a) => Shape::Arc(a),
                    None => continue,
                },
                Geom::Bezier { .. } => match doc.cubic(id) {
                    Some(c) => Shape::Cubic(c),
                    None => continue,
                },
            };
            let constrained = fully_constrained.contains(&id);
            let bbox = shape.bbox();
            bounds = Some(bounds.map_or(bbox, |b| b.union(bbox)));
            items.push(Item {
                id,
                shape,
                construction: e.construction,
                constrained,
                owned_point: matches!(shape, Shape::Point(_)) && owned.contains(&id),
                ends: match e.geom {
                    Geom::Line { start, end }
                    | Geom::Arc { start, end, .. }
                    | Geom::Bezier { start, end, .. } => Some((start, end)),
                    _ => None,
                },
                bbox,
            });
        }

        let mut annots = Vec::new();
        let mut slots: HashMap<EntityId, u8> = HashMap::new();
        for (&cid, c) in doc.constraints() {
            if let Some(label) = glyph_label(c) {
                if let Some(&first) = c.references().first()
                    && let Some(at) = Self::anchor(doc, first)
                {
                    let slot = slots.entry(first).or_insert(0);
                    annots.push(Annot::Glyph {
                        id: cid,
                        at,
                        slot: *slot,
                        label,
                    });
                    *slot = slot.saturating_add(1);
                }
                continue;
            }
            let pos = |id| Self::anchor(doc, id);
            let dim = c.dimension();
            let kind = match c {
                Constraint::Distance { a, b, .. } => pos(*a)
                    .zip(pos(*b))
                    .map(|(a, b)| DimKind::Distance { a, b }),
                Constraint::HorizontalDistance { a, b, .. } => pos(*a)
                    .zip(pos(*b))
                    .map(|(a, b)| DimKind::Horizontal { a, b }),
                Constraint::VerticalDistance { a, b, .. } => pos(*a)
                    .zip(pos(*b))
                    .map(|(a, b)| DimKind::Vertical { a, b }),
                // Drawn on the edge being measured, not the centreline (#240).
                Constraint::Radius { curve, .. } => {
                    curve_centre(doc, *curve).map(|(center, radius)| DimKind::Radius {
                        center,
                        radius: radius - doc.edge_offset(c),
                    })
                }
                Constraint::Diameter { curve, .. } => {
                    curve_centre(doc, *curve).map(|(center, radius)| DimKind::Diameter {
                        center,
                        radius: radius - doc.edge_offset(c) / 2.0,
                    })
                }
                Constraint::Angle { a, .. } => pos(*a).map(|at| DimKind::Angle { at }),
                _ => None,
            };
            if let (Some(kind), Some(dim)) = (kind, dim) {
                annots.push(Annot::Dim {
                    id: cid,
                    kind,
                    value: dim.value,
                    driven: dim.driven,
                    edge: dim.edge,
                });
            }
        }
        Self {
            items,
            annots,
            bounds,
        }
    }
}

fn curve_centre(doc: &Document, id: EntityId) -> Option<(Vec2, f64)> {
    doc.circle(id)
        .map(|c| (c.center, c.radius))
        .or_else(|| doc.arc(id).map(|a| (a.center, a.radius())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo;

    fn solved(doc: &Document) -> BTreeSet<EntityId> {
        locus_doc::Sketcher::new(doc.clone())
            .state()
            .fully_constrained
            .clone()
    }

    #[test]
    fn demo_scene_resolves_geometry_and_flags() {
        let doc = demo::sketch();
        let scene = Scene::build(&doc, &solved(&doc), &BTreeSet::new());
        assert_eq!(scene.items.len(), doc.entities().len());
        assert!(scene.items.iter().any(|i| i.construction));
        // The fixed-and-sized hole is the one constrained circle.
        let circles: Vec<_> = scene
            .items
            .iter()
            .filter(|i| matches!(i.shape, Shape::Circle(_)))
            .collect();
        assert_eq!(circles.len(), 1);
        assert!(circles[0].constrained);
        // Glyphs and dimensions were produced from the constraints.
        assert!(
            scene
                .annots
                .iter()
                .any(|a| matches!(a, Annot::Glyph { .. }))
        );
        assert!(scene.annots.iter().any(|a| matches!(a, Annot::Dim { .. })));
        let b = scene.bounds.unwrap_or_else(|| Aabb::from_point(Vec2::ZERO));
        assert!(b.width() > 190.0 && b.height() > 110.0);
    }

    #[test]
    fn demo_sketch_has_no_redundant_or_conflicting_constraints() {
        let s = locus_doc::Sketcher::new(demo::sketch());
        assert!(s.state().converged);
        assert!(s.state().redundant.is_empty() && s.state().conflicts.is_empty());
    }

    #[test]
    fn owned_points_are_flagged_and_free_points_are_not() {
        let scene = Scene::build(&demo::sketch(), &BTreeSet::new(), &BTreeSet::new());
        let free = scene
            .items
            .iter()
            .filter(|i| matches!(i.shape, Shape::Point(_)) && !i.owned_point)
            .count();
        assert_eq!(free, 1);
    }

    #[test]
    fn empty_document_has_no_bounds() {
        let scene = Scene::build(&Document::new(), &BTreeSet::new(), &BTreeSet::new());
        assert!(scene.items.is_empty() && scene.bounds.is_none());
    }

    #[test]
    fn stress_scene_has_thousands_of_curves() {
        let scene = Scene::build(&demo::stress(500), &BTreeSet::new(), &BTreeSet::new());
        let curves = scene
            .items
            .iter()
            .filter(|i| !matches!(i.shape, Shape::Point(_)))
            .count();
        assert!(curves >= 495, "{curves}");
    }

    #[test]
    #[allow(clippy::unwrap_used)]
    fn bezier_handles_show_only_for_listed_curves() {
        use locus_doc::{Command, Editor};
        let mut ed = Editor::new(Document::new());
        let mut add = |geom| {
            ed.apply(&Command::AddEntity {
                layer: Document::default_layer(),
                construction: false,
                style: Default::default(),
                geom,
            })
            .unwrap()
            .entities[0]
        };
        let pt = |x| Geom::Point {
            pos: Vec2::new(x, 0.0),
        };
        let p = [add(pt(0.0)), add(pt(1.0)), add(pt(2.0)), add(pt(3.0))];
        let curve = add(Geom::Bezier {
            start: p[0],
            start_ctrl: p[1],
            end_ctrl: p[2],
            end: p[3],
        });
        let doc = ed.into_document();
        let points = |s: &Scene| {
            s.items
                .iter()
                .filter(|i| matches!(i.shape, Shape::Point(_)))
                .count()
        };
        let hidden = Scene::build(&doc, &BTreeSet::new(), &BTreeSet::new());
        assert_eq!(points(&hidden), 2);
        let shown = Scene::build(&doc, &BTreeSet::new(), &BTreeSet::from([curve]));
        assert_eq!(points(&shown), 4);
    }

    /// A stroke-edge radius is drawn on the edge it measures (#240).
    #[test]
    #[allow(clippy::unwrap_used)]
    fn edge_radius_dimension_is_drawn_on_the_edge() {
        use locus_doc::{Command, CutParams, Edge, LayerProps, Operation, Sketcher};
        let mut s = Sketcher::new(demo::sketch());
        let radius = s
            .document()
            .constraints()
            .iter()
            .find(|(_, c)| matches!(c, Constraint::Radius { .. }))
            .map(|(id, _)| *id)
            .unwrap();
        s.apply(&Command::SetLayerProps {
            id: Document::default_layer(),
            props: LayerProps {
                operation: Some(Operation::Cut(CutParams {
                    kerf_width: 1.0,
                    ..CutParams::default()
                })),
                ..LayerProps::default()
            },
        })
        .unwrap();
        s.apply(&Command::SetDimensionEdge {
            id: radius,
            edge: Edge::Outside,
        })
        .unwrap();
        let doc = s.document();
        let radius_id = radius;
        let scene = Scene::build(doc, &BTreeSet::new(), &BTreeSet::new());
        let Some(Constraint::Radius { curve, .. }) = doc.constraint(radius) else {
            unreachable!()
        };
        let centre_r = doc
            .circle(*curve)
            .map(|c| c.radius)
            .or_else(|| doc.arc(*curve).map(|a| a.radius()))
            .unwrap();
        let (shown, edge) = scene
            .annots
            .iter()
            .find_map(|a| match a {
                Annot::Dim {
                    id,
                    kind: DimKind::Radius { radius, .. },
                    edge,
                    ..
                } if *id == radius_id => Some((*radius, *edge)),
                _ => None,
            })
            .unwrap();
        assert_eq!(edge, Edge::Outside);
        assert!(
            (shown - (centre_r + 0.5)).abs() < 1e-9,
            "{shown} vs {centre_r}"
        );
    }
}
