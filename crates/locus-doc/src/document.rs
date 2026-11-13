// SPDX-License-Identifier: AGPL-3.0-or-later

//! The document: layers, entity store, constraints (SPEC §4, §5, §10).
//!
//! `Document` exposes a read-only public API. All mutation goes through the
//! command API (SPEC §11, `crate::command`); the mutators here are
//! crate-private building blocks for it.

use crate::boolean::Boolean;
use crate::constraint::Constraint;
use crate::entity::{Entity, EntityKind, Geom};
use crate::error::DocError;
use crate::ids::{BooleanId, ConstraintId, EntityId, LayerId, MAX_ID, RecipeId};
use crate::operation::Operation;
use crate::recipe::Recipe;
use locus_geom::{Arc, Circle, Cubic, Line, Unit, Vec2};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Longest accepted layer name, in characters.
pub const MAX_LAYER_NAME: usize = 128;

/// A layer (SPEC §4). Its operation carries the SPEC §7 parameters.
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

/// What deleting a layer removes (SPEC §4 rule 4).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LayerDeletion {
    /// The layer's own geometry, which goes with it.
    pub entities: Vec<EntityId>,
    /// Every constraint that touches that geometry: all are dropped, and all
    /// come back on undo.
    pub constraints: Vec<ConstraintId>,
    /// Entities on **other** layers those constraints also touch.
    pub touched_elsewhere: Vec<EntityId>,
    /// Entities on other layers built on this layer's points (a line on
    /// another layer ending at one of them). Deletion is refused while any
    /// exist: move them first, so no geometry is lost silently.
    pub blocked_by: Vec<EntityId>,
    /// Patterns and plugin objects built on the layer's geometry. They go
    /// with it, instances included (even ones on other layers).
    pub recipes: Vec<RecipeId>,
}

/// Longest accepted machine-profile id.
pub const MAX_PROFILE_ID: usize = 64;

/// A profile id is a short slug: `a-z 0-9 . _ -`, 1..=64 chars. Documents are
/// untrusted input (SPEC §10), so the id is checked on load as well.
///
/// # Errors
/// [`DocError::Invalid`] when the id is empty, too long or has other characters.
pub fn check_profile_id(id: &str) -> Result<(), DocError> {
    let ok = !id.is_empty()
        && id.len() <= MAX_PROFILE_ID
        && id.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-')
        });
    if ok {
        Ok(())
    } else {
        Err(DocError::Invalid("invalid machine profile id".to_owned()))
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
    /// Boolean recipes (SPEC §6.3); their results are derived, not stored.
    pub(crate) booleans: BTreeMap<BooleanId, Boolean>,
    /// Associative recipes (SPEC §6.2.1, ADR 0011).
    pub(crate) recipes: BTreeMap<RecipeId, Recipe>,
    /// Expressions behind dimension values (SPEC §5): the dimension's value
    /// is re-evaluated from these when a parameter changes.
    pub(crate) dim_exprs: BTreeMap<ConstraintId, String>,
    pub(crate) view: Option<View>,
    /// Selected machine profile id (SPEC §7, #236). Only the id is stored;
    /// the profile itself lives in the app (built-in or user settings).
    pub(crate) profile: Option<String>,
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
            operation: Operation::Draw,
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
            booleans: BTreeMap::new(),
            recipes: BTreeMap::new(),
            dim_exprs: BTreeMap::new(),
            view: None,
            profile: None,
        }
    }

    /// The layer a new document starts with.
    #[must_use]
    pub const fn default_layer() -> LayerId {
        LayerId::new(1)
    }

    // ── Read API ─────────────────────────────────────────────────────────

    /// The stroke width an entity has on the machine, mm: its own pen width
    /// on a Draw layer (SPEC §4, #287), the kerf on a Cut layer, otherwise 0
    /// (SPEC §5, §7).
    #[must_use]
    pub fn stroke_width(&self, entity: EntityId) -> f64 {
        let Some(e) = self.entity(entity) else {
            return 0.0;
        };
        match self.layers.get(&e.layer).map(|l| &l.operation) {
            Some(Operation::Cut(p)) => p.kerf_width,
            Some(Operation::Draw) => e.style.width,
            _ => 0.0,
        }
    }

    /// What to add to a dimension's value to get the centreline value the
    /// solver is given (SPEC §5 Stroke-edge reference, ADR 0014): `-half` for
    /// the outside edge, `+half` for the inside edge, where `half` sums the
    /// half stroke widths of the measured operands (a circle's diameter counts
    /// the stroke twice). Zero for centreline dimensions.
    #[must_use]
    pub fn edge_offset(&self, c: &crate::constraint::Constraint) -> f64 {
        use crate::constraint::{Constraint, Edge};
        let Some(dim) = c.dimension() else {
            return 0.0;
        };
        let half = |id: &EntityId| self.stroke_width(*id) / 2.0;
        let sum = match c {
            Constraint::Distance { a, b, .. } => {
                // A point has no stroke; only lines contribute.
                let is_line = |id: &EntityId| {
                    self.entity(*id)
                        .is_some_and(|e| matches!(e.geom, crate::entity::Geom::Line { .. }))
                };
                [a, b].into_iter().filter(|i| is_line(i)).map(half).sum()
            }
            Constraint::Radius { curve, .. } => half(curve),
            Constraint::Diameter { curve, .. } => 2.0 * half(curve),
            _ => 0.0,
        };
        match dim.edge {
            Edge::Centre => 0.0,
            Edge::Inside => sum,
            Edge::Outside => -sum,
        }
    }

    /// The selected machine profile id, if any (SPEC §7).
    #[must_use]
    pub fn profile(&self) -> Option<&str> {
        self.profile.as_deref()
    }

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
    pub fn booleans(&self) -> &BTreeMap<BooleanId, Boolean> {
        &self.booleans
    }

    #[must_use]
    pub fn boolean(&self, id: BooleanId) -> Option<&Boolean> {
        self.booleans.get(&id)
    }

    /// The expression a dimension's value comes from, if it was entered as one.
    #[must_use]
    pub fn dimension_expr(&self, id: ConstraintId) -> Option<&str> {
        self.dim_exprs.get(&id).map(String::as_str)
    }

    #[must_use]
    pub fn recipes(&self) -> &BTreeMap<RecipeId, Recipe> {
        &self.recipes
    }

    #[must_use]
    pub fn recipe(&self, id: RecipeId) -> Option<&Recipe> {
        self.recipes.get(&id)
    }

    /// Every entity a recipe generated (SPEC §6.2.1: instances are driven),
    /// with the recipe that did.
    #[must_use]
    pub fn driven_entities(&self) -> BTreeMap<EntityId, RecipeId> {
        self.recipes
            .iter()
            .flat_map(|(rid, r)| r.instances().map(move |e| (e, *rid)))
            .collect()
    }

    /// Instances of recipes that are not solved (offsets): written after
    /// each solve, kept out of the solver, never constrained.
    #[must_use]
    pub fn unsolved_instances(&self) -> BTreeSet<EntityId> {
        self.recipes
            .values()
            .filter(|r| !r.kind.solved())
            .flat_map(Recipe::instances)
            .collect()
    }

    /// Instances of pattern recipes (linear, circular): written after each
    /// solve like any unsolved instance, but they take constraints through
    /// the second solve phase (SPEC §6.2.1, ADR 0015).
    #[must_use]
    pub fn pattern_instances(&self) -> BTreeSet<EntityId> {
        self.recipes
            .values()
            .filter(|r| r.kind.is_pattern())
            .flat_map(Recipe::instances)
            .collect()
    }

    /// Unsolved instances that still take **no** constraint: offsets and
    /// plugin objects.
    #[must_use]
    pub fn constraint_free_instances(&self) -> BTreeSet<EntityId> {
        self.recipes
            .values()
            .filter(|r| !r.kind.solved() && !r.kind.is_pattern())
            .flat_map(Recipe::instances)
            .collect()
    }

    /// Constraints that touch a pattern instance: solved in the second phase,
    /// with the pattern held fixed (ADR 0015).
    #[must_use]
    pub fn late_constraints(&self) -> Vec<ConstraintId> {
        let inst = self.pattern_instances();
        if inst.is_empty() {
            return Vec::new();
        }
        self.constraints
            .iter()
            .filter(|(_, c)| c.references().iter().any(|e| inst.contains(e)))
            .map(|(id, _)| *id)
            .collect()
    }

    /// What the second phase holds fixed: every pattern instance, its
    /// sources and whatever else the pattern reads (centre, direction lines),
    /// with the points they are built on.
    pub(crate) fn late_pins(&self) -> BTreeSet<EntityId> {
        let mut pins = BTreeSet::new();
        for r in self.recipes.values().filter(|r| r.kind.is_pattern()) {
            pins.extend(r.instances());
            pins.extend(crate::recipe::closure(self, &r.sources));
            pins.extend(crate::recipe::closure(self, &r.kind.references()));
        }
        pins
    }

    /// The recipe that generated `id`, if any.
    #[must_use]
    pub fn driven_by(&self, id: EntityId) -> Option<RecipeId> {
        self.recipes
            .iter()
            .find(|(_, r)| r.instances().any(|e| e == id))
            .map(|(rid, _)| *rid)
    }

    /// Recipes that use `id` as a source, a source's point, an extra such as
    /// the mirror axis, or as an instance.
    #[must_use]
    pub fn recipes_involving(&self, id: EntityId) -> Vec<RecipeId> {
        self.recipes
            .iter()
            .filter(|(_, r)| r.inputs(self).contains(&id) || r.instances().any(|e| e == id))
            .map(|(rid, _)| *rid)
            .collect()
    }

    /// The recipe that owns constraint `id`, if any.
    #[must_use]
    pub fn constraint_owner(&self, id: ConstraintId) -> Option<RecipeId> {
        self.recipes
            .iter()
            .find(|(_, r)| r.constraints.contains(&id))
            .map(|(rid, _)| *rid)
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

    /// A Bézier entity as `locus_geom` geometry.
    #[must_use]
    pub fn cubic(&self, id: EntityId) -> Option<Cubic> {
        match self.entity(id)?.geom {
            Geom::Bezier {
                start,
                start_ctrl,
                end_ctrl,
                end,
            } => Some(Cubic::new(
                self.point_pos(start)?,
                self.point_pos(start_ctrl)?,
                self.point_pos(end_ctrl)?,
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

    /// What deleting `layer` would remove (SPEC §4 rule 4). Read-only; the
    /// UI shows it as the warning, and [`Command::DeleteLayer`] removes
    /// exactly this.
    ///
    /// [`Command::DeleteLayer`]: crate::Command::DeleteLayer
    ///
    /// # Errors
    /// [`DocError::UnknownLayer`].
    pub fn layer_deletion(&self, layer: LayerId) -> Result<LayerDeletion, DocError> {
        if !self.layers.contains_key(&layer) {
            return Err(DocError::UnknownLayer(layer));
        }
        let mut doomed: BTreeSet<EntityId> = self
            .entities
            .iter()
            .filter(|(_, e)| e.layer == layer)
            .map(|(i, _)| i)
            .collect();
        // A recipe on doomed geometry is removed whole, so its instances
        // (and anything chained on them) are doomed too.
        let mut recipes: BTreeSet<RecipeId> = BTreeSet::new();
        loop {
            let found: Vec<RecipeId> = self
                .recipes
                .iter()
                .filter(|(rid, r)| {
                    !recipes.contains(*rid)
                        && (r.inputs(self).iter().any(|e| doomed.contains(e))
                            || r.instances().any(|e| doomed.contains(&e)))
                })
                .map(|(rid, _)| *rid)
                .collect();
            if found.is_empty() {
                break;
            }
            for rid in found {
                if let Some(r) = self.recipes.get(&rid) {
                    doomed.extend(r.instances());
                }
                recipes.insert(rid);
            }
        }
        let mut out = LayerDeletion {
            entities: doomed.iter().copied().collect(),
            recipes: recipes.iter().copied().collect(),
            ..LayerDeletion::default()
        };
        let owned: BTreeSet<ConstraintId> = recipes
            .iter()
            .filter_map(|rid| self.recipes.get(rid))
            .flat_map(|r| r.constraints.iter().copied())
            .collect();
        for (id, c) in &self.constraints {
            let refs = c.references();
            if owned.contains(id) || refs.iter().any(|r| doomed.contains(r)) {
                out.constraints.push(*id);
                out.touched_elsewhere.extend(
                    refs.into_iter()
                        .filter(|r| !doomed.contains(r) && self.entities.contains(*r)),
                );
            }
        }
        out.touched_elsewhere.sort_unstable();
        out.touched_elsewhere.dedup();
        out.blocked_by = self
            .entities
            .iter()
            .filter(|(i, e)| {
                !doomed.contains(i) && e.geom.point_refs().iter().any(|r| doomed.contains(r))
            })
            .map(|(i, _)| i)
            .collect();
        Ok(out)
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
        e.style.validate()?;
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
            Geom::Text { text, .. } => text.validate()?,
            Geom::Line { .. } | Geom::Arc { .. } | Geom::Bezier { .. } => {}
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

    /// Refuse to edit an entity a recipe generated (SPEC §6.2.1).
    pub(crate) fn check_not_driven(&self, id: EntityId) -> Result<(), DocError> {
        match self.driven_by(id) {
            Some(recipe) => Err(DocError::Driven { entity: id, recipe }),
            None => Ok(()),
        }
    }

    /// Refuse to edit geometry on a locked layer (SPEC §4 rule 3).
    pub(crate) fn check_unlocked(&self, layer: LayerId) -> Result<(), DocError> {
        if self.layer(layer).is_some_and(|l| l.locked) {
            Err(DocError::LayerLocked(layer))
        } else {
            Ok(())
        }
    }

    /// A layer's own invariants: a usable name and a valid operation (SPEC §7).
    pub(crate) fn check_layer(l: &Layer) -> Result<(), DocError> {
        let name = l.name.trim();
        if name.is_empty() || l.name.chars().count() > MAX_LAYER_NAME {
            return Err(DocError::Invalid(
                "a layer name must be 1 to 128 characters".to_owned(),
            ));
        }
        l.operation.validate()
    }

    /// Refuse to remove an entity something else still depends on (SPEC §4
    /// Layer rule 5: never silently drop constraints).
    pub(crate) fn check_removable(&self, id: EntityId) -> Result<(), DocError> {
        if !self.entities.contains(id) {
            return Err(DocError::UnknownEntity(id));
        }
        let recipes = self.recipes_involving(id);
        if !recipes.is_empty() {
            return Err(DocError::RecipeInUse {
                entity: id,
                recipes,
            });
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
            style: Default::default(),
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

    /// #288: a new document's first layer is Draw. Files written earlier
    /// keep their stored operation (see the file-format migration tests).
    #[test]
    fn new_document_starts_with_a_draw_layer() {
        let d = Document::new();
        let l = d.layer(Document::default_layer()).unwrap();
        assert!(matches!(l.operation, Operation::Draw));
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
            style: Default::default(),
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
            style: Default::default(),
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
