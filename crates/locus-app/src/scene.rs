// SPDX-License-Identifier: AGPL-3.0-or-later
//! A flat, render-ready snapshot of a [`Document`] (#66).
//!
//! Drawing and hit-testing want resolved geometry (points looked up, bounding
//! boxes computed) in a cache-friendly `Vec`, not a `BTreeMap` walk with
//! point-entity indirection every frame. The scene is rebuilt after every
//! successful command; it is derived state and never edited (SPEC §11).

use locus_doc::{Constraint, ConstraintId, Document, EntityId, Geom};
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
        }
    }

    /// Build the scene. Hidden layers are skipped. `fully_constrained` is the
    /// solver's set of entities that can no longer move (SPEC §5): they are
    /// drawn in the fully-constrained colour.
    pub fn build(doc: &Document, fully_constrained: &BTreeSet<EntityId>) -> Self {
        let owned: HashSet<EntityId> = doc
            .entities()
            .iter()
            .flat_map(|(_, e)| e.geom.point_refs())
            .collect();

        let mut items = Vec::with_capacity(doc.entities().len());
        let mut bounds: Option<Aabb> = None;
        for (id, e) in doc.entities().iter() {
            if doc.layer(e.layer).is_some_and(|l| !l.visible) {
                continue;
            }
            let shape = match e.geom {
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
                Constraint::Radius { curve, .. } => curve_centre(doc, *curve)
                    .map(|(center, radius)| DimKind::Radius { center, radius }),
                Constraint::Diameter { curve, .. } => curve_centre(doc, *curve)
                    .map(|(center, radius)| DimKind::Diameter { center, radius }),
                Constraint::Angle { a, .. } => pos(*a).map(|at| DimKind::Angle { at }),
                _ => None,
            };
            if let (Some(kind), Some(dim)) = (kind, dim) {
                annots.push(Annot::Dim {
                    id: cid,
                    kind,
                    value: dim.value,
                    driven: dim.driven,
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
        let scene = Scene::build(&doc, &solved(&doc));
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
        let scene = Scene::build(&demo::sketch(), &BTreeSet::new());
        let free = scene
            .items
            .iter()
            .filter(|i| matches!(i.shape, Shape::Point(_)) && !i.owned_point)
            .count();
        assert_eq!(free, 1);
    }

    #[test]
    fn empty_document_has_no_bounds() {
        let scene = Scene::build(&Document::new(), &BTreeSet::new());
        assert!(scene.items.is_empty() && scene.bounds.is_none());
    }

    #[test]
    fn stress_scene_has_thousands_of_curves() {
        let scene = Scene::build(&demo::stress(500), &BTreeSet::new());
        let curves = scene
            .items
            .iter()
            .filter(|i| !matches!(i.shape, Shape::Point(_)))
            .count();
        assert!(curves >= 495, "{curves}");
    }
}
