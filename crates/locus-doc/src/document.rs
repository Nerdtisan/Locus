// SPDX-License-Identifier: AGPL-3.0-or-later

//! The document: layers, entity store, constraints (SPEC §4, §5, §10).
//!
//! `Document` exposes a read-only public API. All mutation goes through the
//! command API (SPEC §11, `crate::command`); the mutators here are
//! crate-private building blocks for it.

use crate::constraint::Constraint;
use crate::entity::{Entity, EntityKind, Geom};
use crate::error::DocError;
use crate::ids::{ConstraintId, EntityId, LayerId, MAX_ID};
use locus_geom::{Arc, Circle, Line, Unit, Vec2};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// What a layer does on the machine (SPEC §7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Operation {
    Cut,
    Score,
    Engrave,
    Draw,
}

/// A layer (SPEC §4). Operation parameters (SPEC §7) arrive with M2.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Layer {
    pub name: String,
    pub colour: [u8; 3],
    pub operation: Operation,
    pub export_order: u32,
    pub visible: bool,
    pub locked: bool,
    pub export_enabled: bool,
}

/// A named document parameter (SPEC §5). Stored with its last evaluated value;
/// the expression evaluator is not part of M1's core model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Parameter {
    pub expr: String,
    pub value: f64,
}

/// Optional saved viewport; ignorable by diff tools (SPEC §10).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct View {
    pub center: Vec2,
    pub scale: f64,
}

/// Entities keyed by ID. `BTreeMap` gives the deterministic iteration order
/// SPEC §10 needs.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EntityStore {
    map: BTreeMap<EntityId, Entity>,
}

impl EntityStore {
    #[must_use]
    pub fn get(&self, id: EntityId) -> Option<&Entity> {
        self.map.get(&id)
    }

    #[must_use]
    pub fn contains(&self, id: EntityId) -> bool {
        self.map.contains_key(&id)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.map.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Entities in ascending ID order.
    pub fn iter(&self) -> impl Iterator<Item = (EntityId, &Entity)> {
        self.map.iter().map(|(id, e)| (*id, e))
    }

    pub(crate) fn insert(&mut self, id: EntityId, e: Entity) -> Option<Entity> {
        self.map.insert(id, e)
    }

    pub(crate) fn remove(&mut self, id: EntityId) -> Option<Entity> {
        self.map.remove(&id)
    }

    pub(crate) fn get_mut(&mut self, id: EntityId) -> Option<&mut Entity> {
        self.map.get_mut(&id)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Document {
    pub(crate) units: Unit,
    pub(crate) next_id: u64,
    pub(crate) parameters: BTreeMap<String, Parameter>,
    pub(crate) layers: BTreeMap<LayerId, Layer>,
    pub(crate) entities: EntityStore,
    pub(crate) constraints: BTreeMap<ConstraintId, Constraint>,
    pub(crate) view: Option<View>,
}

impl Default for Document {
    fn default() -> Self {
        Self::new()
    }
}

impl Document {
    /// An empty document with one default layer (SPEC §4: a single default
    /// layer until layers get their own UI). The layer takes ID 1; the shared
    /// counter (ADR-0003) then continues from 2.
    #[must_use]
    pub fn new() -> Self {
        let layer = Layer {
            name: "Layer 1".to_owned(),
            colour: [0, 0, 0],
            operation: Operation::Cut,
            export_order: 0,
            visible: true,
            locked: false,
            export_enabled: true,
        };
        Self {
            units: Unit::Mm,
            next_id: 2,
            parameters: BTreeMap::new(),
            layers: BTreeMap::from([(LayerId::new(1), layer)]),
            entities: EntityStore::default(),
            constraints: BTreeMap::new(),
            view: None,
        }
    }

    /// The layer a new document starts with.
    #[must_use]
    pub const fn default_layer() -> LayerId {
        LayerId::new(1)
    }

    // ── Read API ─────────────────────────────────────────────────────────

    #[must_use]
    pub fn units(&self) -> Unit {
        self.units
    }

    /// The next ID that will be handed out (ADR-0003). Only ever increases.
    #[must_use]
    pub fn next_id(&self) -> u64 {
        self.next_id
    }

    #[must_use]
    pub fn parameters(&self) -> &BTreeMap<String, Parameter> {
        &self.parameters
    }

    #[must_use]
    pub fn layers(&self) -> &BTreeMap<LayerId, Layer> {
        &self.layers
    }

    #[must_use]
    pub fn layer(&self, id: LayerId) -> Option<&Layer> {
        self.layers.get(&id)
    }

    #[must_use]
    pub fn entities(&self) -> &EntityStore {
        &self.entities
    }

    #[must_use]
    pub fn entity(&self, id: EntityId) -> Option<&Entity> {
        self.entities.get(id)
    }

    #[must_use]
    pub fn constraints(&self) -> &BTreeMap<ConstraintId, Constraint> {
        &self.constraints
    }

    #[must_use]
    pub fn constraint(&self, id: ConstraintId) -> Option<&Constraint> {
        self.constraints.get(&id)
    }

    #[must_use]
    pub fn view(&self) -> Option<&View> {
        self.view.as_ref()
    }

    /// Position of a point entity.
    #[must_use]
    pub fn point_pos(&self, id: EntityId) -> Option<Vec2> {
        match self.entity(id)?.geom {
            Geom::Point { pos } => Some(pos),
            _ => None,
        }
    }

    /// A line entity as `locus_geom` geometry.
    #[must_use]
    pub fn line(&self, id: EntityId) -> Option<Line> {
        match self.entity(id)?.geom {
            Geom::Line { start, end } => {
                Some(Line::new(self.point_pos(start)?, self.point_pos(end)?))
            }
            _ => None,
        }
    }

    /// A circle entity as `locus_geom` geometry.
    #[must_use]
    pub fn circle(&self, id: EntityId) -> Option<Circle> {
        match self.entity(id)?.geom {
            Geom::Circle { center, radius } => Some(Circle::new(self.point_pos(center)?, radius)),
            _ => None,
        }
    }

    /// An arc entity as `locus_geom` geometry.
    #[must_use]
    pub fn arc(&self, id: EntityId) -> Option<Arc> {
        match self.entity(id)?.geom {
            Geom::Arc { center, start, end } => Some(Arc::new(
                self.point_pos(center)?,
                self.point_pos(start)?,
                self.point_pos(end)?,
            )),
            _ => None,
        }
    }

    /// Entities and constraints that mention `id`.
    #[must_use]
    pub fn dependents(&self, id: EntityId) -> (Vec<EntityId>, Vec<ConstraintId>) {
        let ents = self
            .entities
            .iter()
            .filter(|(_, e)| e.geom.point_refs().contains(&id))
            .map(|(i, _)| i)
            .collect();
        let cons = self
            .constraints
            .iter()
            .filter(|(_, c)| c.references().contains(&id))
            .map(|(i, _)| *i)
            .collect();
        (ents, cons)
    }

    // ── Crate-private building blocks ────────────────────────────────────

    /// Hand out the next ID. Never reuses (ADR-0003 rule 2).
    pub(crate) fn alloc_id(&mut self) -> Result<u64, DocError> {
        if self.next_id >= MAX_ID {
            return Err(DocError::IdSpaceExhausted);
        }
        let id = self.next_id;
        self.next_id += 1;
        Ok(id)
    }

    /// Validate an entity against the current document: layer exists, finite
    /// numbers, referenced points exist and really are points.
    pub(crate) fn check_entity(&self, e: &Entity) -> Result<(), DocError> {
        if !self.layers.contains_key(&e.layer) {
            return Err(DocError::UnknownLayer(e.layer));
        }
        match &e.geom {
            Geom::Point { pos } => {
                if !pos.is_finite() {
                    return Err(DocError::BadValue {
                        what: "point position",
                        value: if pos.x.is_finite() { pos.y } else { pos.x },
                    });
                }
            }
            Geom::Circle { radius, .. } => {
                if !(radius.is_finite() && *radius > 0.0) {
                    return Err(DocError::BadValue {
                        what: "circle radius",
                        value: *radius,
                    });
                }
            }
            Geom::Line { .. } | Geom::Arc { .. } => {}
        }
        let refs = e.geom.point_refs();
        for (i, id) in refs.iter().enumerate() {
            let found = self
                .entity(*id)
                .ok_or(DocError::UnknownEntity(*id))?
                .geom
                .kind();
            if found != EntityKind::Point {
                return Err(DocError::WrongKind {
                    what: "entity reference",
                    entity: *id,
                    expected: "point",
                    found,
                });
            }
            if refs[..i].contains(id) {
                return Err(DocError::SameEntity {
                    what: "entity",
                    entity: *id,
                });
            }
        }
        Ok(())
    }

    /// Refuse to remove an entity something else still depends on (SPEC §4
    /// Layer rule 5: never silently drop constraints).
    pub(crate) fn check_removable(&self, id: EntityId) -> Result<(), DocError> {
        if !self.entities.contains(id) {
            return Err(DocError::UnknownEntity(id));
        }
        let (entities, constraints) = self.dependents(id);
        if entities.is_empty() && constraints.is_empty() {
            Ok(())
        } else {
            Err(DocError::EntityInUse {
                id,
                entities,
                constraints,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(doc: &mut Document, x: f64, y: f64) -> EntityId {
        let id = EntityId::new(doc.alloc_id().unwrap());
        let e = Entity {
            layer: Document::default_layer(),
            construction: false,
            geom: Geom::Point {
                pos: Vec2::new(x, y),
            },
        };
        doc.check_entity(&e).unwrap();
        doc.entities.insert(id, e);
        id
    }

    #[test]
    fn new_document_has_default_layer_and_counter() {
        let d = Document::new();
        assert_eq!(d.layers().len(), 1);
        assert!(d.layer(Document::default_layer()).is_some());
        assert_eq!(d.next_id(), 2);
        assert!(d.entities().is_empty());
    }

    #[test]
    fn insert_lookup_remove_and_ids_never_reused() {
        let mut d = Document::new();
        let a = point(&mut d, 0.0, 0.0);
        let b = point(&mut d, 1.0, 0.0);
        assert!(a < b);
        assert_eq!(d.point_pos(b), Some(Vec2::new(1.0, 0.0)));
        d.check_removable(b).unwrap();
        d.entities.remove(b);
        assert!(d.entity(b).is_none());
        let c = point(&mut d, 2.0, 0.0);
        assert!(c > b, "removed ID {b} must not be handed out again");
        // Iteration is ascending by ID.
        let ids: Vec<_> = d.entities().iter().map(|(i, _)| i).collect();
        assert_eq!(ids, vec![a, c]);
    }

    #[test]
    fn line_must_reference_existing_distinct_points() {
        let mut d = Document::new();
        let a = point(&mut d, 0.0, 0.0);
        let mk = |start, end| Entity {
            layer: Document::default_layer(),
            construction: false,
            geom: Geom::Line { start, end },
        };
        assert_eq!(
            d.check_entity(&mk(a, EntityId::new(99))),
            Err(DocError::UnknownEntity(EntityId::new(99)))
        );
        assert!(matches!(
            d.check_entity(&mk(a, a)),
            Err(DocError::SameEntity { .. })
        ));
        let b = point(&mut d, 1.0, 1.0);
        d.check_entity(&mk(a, b)).unwrap();
        let lid = EntityId::new(d.alloc_id().unwrap());
        d.entities.insert(lid, mk(a, b));
        // A line is not a point.
        assert!(matches!(
            d.check_entity(&mk(a, lid)),
            Err(DocError::WrongKind { .. })
        ));
        // Points under a line cannot be removed from under it.
        assert!(matches!(
            d.check_removable(a),
            Err(DocError::EntityInUse { .. })
        ));
    }

    #[test]
    fn bad_numbers_and_layers_are_rejected() {
        let mut d = Document::new();
        let c = point(&mut d, 0.0, 0.0);
        let circle = |radius| Entity {
            layer: Document::default_layer(),
            construction: false,
            geom: Geom::Circle { center: c, radius },
        };
        assert!(d.check_entity(&circle(5.0)).is_ok());
        for r in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert!(d.check_entity(&circle(r)).is_err());
        }
        let mut e = circle(1.0);
        e.layer = LayerId::new(77);
        assert_eq!(
            d.check_entity(&e),
            Err(DocError::UnknownLayer(LayerId::new(77)))
        );
    }
}
