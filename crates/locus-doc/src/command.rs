// SPDX-License-Identifier: AGPL-3.0-or-later

//! The serialisable command API with undo/redo (SPEC §11).
//!
//! Every document mutation is a [`Command`] applied through an [`Editor`]. The
//! GUI, CLI, plugins and MCP server are all clients of this one path;
//! [`Document`] has no public mutators.
//!
//! One user action = one command = one undo step. Multi-step operations use
//! [`Command::Compound`], which is atomic: either every step applies, or the
//! document is left untouched.
//!
//! Undo/redo replays recorded state changes (including the exact IDs that were
//! allocated), so redo and "undo a delete" restore the same IDs (ADR-0003).
//! The ID counter itself never goes backwards. Solver output is not command
//! state: re-solve after every apply, undo and redo.

use crate::boolean::Boolean;
use crate::constraint::Constraint;
use crate::document::Parameter;
use crate::document::{Document, Layer};
use crate::entity::{Entity, EntityKind, Geom};
use crate::error::DocError;
use crate::ids::{BooleanId, ConstraintId, EntityId, LayerId, RecipeId};
use crate::operation::Operation;
use crate::params;
use crate::recipe::{self, Recipe, RecipeKind};
use locus_geom::Vec2;
use serde::{Deserialize, Serialize};

/// A document mutation. Plain data: serialise it, log it, replay it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Command {
    /// Create an entity. The document assigns the ID (see [`Applied`]).
    AddEntity {
        layer: LayerId,
        construction: bool,
        geom: Geom,
    },
    /// Replace an entity's geometry. The kind (point, line, …) must not change.
    ModifyEntity {
        id: EntityId,
        geom: Geom,
    },
    /// Delete an entity. Fails with [`DocError::EntityInUse`] while other
    /// entities or constraints still reference it: delete those first, in a
    /// [`Command::Compound`], so nothing is ever dropped silently (SPEC §4).
    DeleteEntity {
        id: EntityId,
    },
    ToggleConstruction {
        id: EntityId,
    },
    AddConstraint {
        constraint: Constraint,
    },
    RemoveConstraint {
        id: ConstraintId,
    },
    /// Change the value of a dimensional constraint.
    SetDimension {
        id: ConstraintId,
        value: f64,
    },
    /// Switch a dimensional constraint between driving and driven (reference).
    SetDriven {
        id: ConstraintId,
        driven: bool,
    },
    /// Create a layer (SPEC §4). The document assigns the ID (see
    /// [`Applied::layers`]) and puts it last in the export order.
    AddLayer {
        name: String,
        colour: [u8; 3],
        operation: Operation,
    },
    /// Change a layer's properties. Lock state has its own command; hiding
    /// has no solver effect (SPEC §4 rule 2).
    SetLayerProps {
        id: LayerId,
        props: LayerProps,
    },
    /// Lock or unlock a layer. A locked layer's geometry is fixed for the
    /// solver and refuses edits (SPEC §4 rule 3); [`Sketcher`](crate::Sketcher)
    /// refuses a lock that would conflict with other constraints.
    SetLayerLocked {
        id: LayerId,
        locked: bool,
    },
    /// Delete a layer together with its geometry and every constraint that
    /// touches that geometry, as one atomic step (SPEC §4 rule 4). Preview it
    /// with [`Document::layer_deletion`]. Undo restores layer, geometry and
    /// constraints with their original IDs. Refused for the last layer, a
    /// locked layer, or while entities on other layers are built on it.
    DeleteLayer {
        id: LayerId,
    },
    /// Move entities to another layer. Constraints reference entities, not
    /// layers, so every constraint is kept (SPEC §4 rule 1).
    MoveToLayer {
        entities: Vec<EntityId>,
        layer: LayerId,
    },
    /// Add a non-destructive boolean recipe (SPEC §6.3). The document assigns
    /// the ID (see [`Applied::booleans`]). Its result is recomputed after each
    /// solve, and is an error (not a failure) while a source region is gone.
    AddBoolean {
        boolean: Boolean,
    },
    /// Replace a recipe's operation and sources.
    EditBoolean {
        id: BooleanId,
        boolean: Boolean,
    },
    RemoveBoolean {
        id: BooleanId,
    },
    /// Add or change a named parameter (SPEC §5). The expression is checked
    /// (syntax, unknown names, cycles) and every dependent value is
    /// re-evaluated, all as one undo step.
    SetParameter {
        name: String,
        expr: String,
    },
    /// Delete a parameter. Refused with [`DocError::ParameterInUse`] (listing
    /// the users) while an expression refers to it.
    RemoveParameter {
        name: String,
    },
    /// Rename a parameter, rewriting every reference to it.
    RenameParameter {
        from: String,
        to: String,
    },
    /// Give a driving dimension an expression (`material * 2 + 0.2`), or
    /// `None` to keep its current value as a plain number. Angles are
    /// entered in degrees.
    SetDimensionExpr {
        id: ConstraintId,
        expr: Option<String>,
    },
    /// Add an associative recipe over `sources` (SPEC §6.2.1, ADR 0011): the
    /// document creates the instance entities (and, for a mirror, the
    /// constraints that tie them to the sources). IDs: see [`Applied`].
    AddRecipe {
        kind: RecipeKind,
        sources: Vec<EntityId>,
    },
    /// Turn a recipe's instances into ordinary, independent geometry and drop
    /// the recipe. Constraints it created stay, as ordinary constraints.
    ExplodeRecipe {
        id: RecipeId,
    },
    /// Delete a recipe together with its instances and owned constraints.
    /// Refused while anything else refers to the instances.
    RemoveRecipe {
        id: RecipeId,
    },
    /// Several commands as one labelled, atomic, single-step undo.
    Compound {
        label: String,
        commands: Vec<Command>,
    },
    /// Write solver output back into the stored positions (SPEC §10). Derived
    /// state: **not** recorded in the undo history and does not clear redo.
    ApplySolution {
        points: Vec<(EntityId, Vec2)>,
        radii: Vec<(EntityId, f64)>,
    },
}

impl Command {
    /// Undo-menu label.
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Command::AddEntity { .. } => "Add entity".to_owned(),
            Command::ModifyEntity { .. } => "Modify entity".to_owned(),
            Command::DeleteEntity { .. } => "Delete entity".to_owned(),
            Command::ToggleConstruction { .. } => "Toggle construction".to_owned(),
            Command::AddConstraint { constraint } => {
                format!("Add {} constraint", constraint.name())
            }
            Command::RemoveConstraint { .. } => "Remove constraint".to_owned(),
            Command::SetDimension { .. } => "Set dimension".to_owned(),
            Command::SetDriven { .. } => "Set driven".to_owned(),
            Command::AddLayer { .. } => "Add layer".to_owned(),
            Command::SetLayerProps { .. } => "Change layer".to_owned(),
            Command::MoveToLayer { .. } => "Move to layer".to_owned(),
            Command::DeleteLayer { .. } => "Delete layer".to_owned(),
            Command::SetLayerLocked { locked: true, .. } => "Lock layer".to_owned(),
            Command::SetLayerLocked { locked: false, .. } => "Unlock layer".to_owned(),
            Command::AddBoolean { .. } => "Add boolean".to_owned(),
            Command::EditBoolean { .. } => "Edit boolean".to_owned(),
            Command::RemoveBoolean { .. } => "Remove boolean".to_owned(),
            Command::SetParameter { name, .. } => format!("Set parameter {name}"),
            Command::RemoveParameter { name } => format!("Remove parameter {name}"),
            Command::RenameParameter { from, .. } => format!("Rename parameter {from}"),
            Command::SetDimensionExpr { .. } => "Set dimension expression".to_owned(),
            Command::AddRecipe { kind, .. } => format!("Add {}", kind.name().to_lowercase()),
            Command::ExplodeRecipe { .. } => "Explode".to_owned(),
            Command::RemoveRecipe { .. } => "Remove recipe".to_owned(),
            Command::Compound { label, .. } => label.clone(),
            Command::ApplySolution { .. } => "Apply solution".to_owned(),
        }
    }
}

/// What a successful command created, in creation order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Applied {
    pub entities: Vec<EntityId>,
    pub constraints: Vec<ConstraintId>,
    pub layers: Vec<LayerId>,
    pub booleans: Vec<BooleanId>,
    pub recipes: Vec<RecipeId>,
}

/// New values for a layer's editable properties; `None` leaves a field alone.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LayerProps {
    pub name: Option<String>,
    pub colour: Option<[u8; 3]>,
    pub operation: Option<Operation>,
    pub export_order: Option<u32>,
    pub visible: Option<bool>,
    pub export_enabled: Option<bool>,
}

/// One reversible state change: the state of an item before and after.
/// `None` means "absent".
#[derive(Debug, Clone)]
enum Patch {
    Entity {
        id: EntityId,
        before: Option<Entity>,
        after: Option<Entity>,
    },
    Constraint {
        id: ConstraintId,
        before: Option<Constraint>,
        after: Option<Constraint>,
    },
    Layer {
        id: LayerId,
        before: Option<Layer>,
        after: Option<Layer>,
    },
    Boolean {
        id: BooleanId,
        before: Option<Boolean>,
        after: Option<Boolean>,
    },
    Recipe {
        id: RecipeId,
        before: Option<Recipe>,
        after: Option<Recipe>,
    },
    Parameter {
        name: String,
        before: Option<Parameter>,
        after: Option<Parameter>,
    },
    DimExpr {
        id: ConstraintId,
        before: Option<String>,
        after: Option<String>,
    },
}

impl Patch {
    fn set(&self, doc: &mut Document, forward: bool) {
        match self {
            Patch::Layer { id, before, after } => {
                match if forward { after } else { before }.clone() {
                    Some(l) => doc.layers.insert(*id, l),
                    None => doc.layers.remove(id),
                };
            }
            Patch::Entity { id, before, after } => {
                match if forward { after } else { before }.clone() {
                    Some(e) => doc.entities.insert(*id, e),
                    None => doc.entities.remove(*id),
                };
            }
            Patch::Constraint { id, before, after } => {
                match if forward { after } else { before }.clone() {
                    Some(c) => doc.constraints.insert(*id, c),
                    None => doc.constraints.remove(id),
                };
            }
            Patch::Boolean { id, before, after } => {
                match if forward { after } else { before }.clone() {
                    Some(b) => doc.booleans.insert(*id, b),
                    None => doc.booleans.remove(id),
                };
            }
            Patch::Parameter {
                name,
                before,
                after,
            } => {
                match if forward { after } else { before }.clone() {
                    Some(p) => doc.parameters.insert(name.clone(), p),
                    None => doc.parameters.remove(name),
                };
            }
            Patch::DimExpr { id, before, after } => {
                match if forward { after } else { before }.clone() {
                    Some(e) => doc.dim_exprs.insert(*id, e),
                    None => doc.dim_exprs.remove(id),
                };
            }
            Patch::Recipe { id, before, after } => {
                match if forward { after } else { before }.clone() {
                    Some(r) => doc.recipes.insert(*id, r),
                    None => doc.recipes.remove(id),
                };
            }
        }
    }
}

#[derive(Debug, Clone)]
struct Transaction {
    label: String,
    patches: Vec<Patch>,
}

/// Apply `cmd`, recording reversible changes in `patches`. On error the caller
/// rolls `patches` back.
fn exec(
    doc: &mut Document,
    cmd: &Command,
    patches: &mut Vec<Patch>,
    out: &mut Applied,
) -> Result<(), DocError> {
    match cmd {
        Command::AddEntity {
            layer,
            construction,
            geom,
        } => {
            doc.check_unlocked(*layer)?;
            let e = Entity {
                layer: *layer,
                construction: *construction,
                geom: geom.clone(),
            };
            doc.check_entity(&e)?;
            let id = EntityId::new(doc.alloc_id()?);
            record(
                doc,
                patches,
                Patch::Entity {
                    id,
                    before: None,
                    after: Some(e),
                },
            );
            out.entities.push(id);
        }
        Command::ModifyEntity { id, geom } => {
            let before = doc.entity(*id).ok_or(DocError::UnknownEntity(*id))?.clone();
            doc.check_not_driven(*id)?;
            doc.check_unlocked(before.layer)?;
            if before.geom.kind() != geom.kind() {
                return Err(DocError::WrongKind {
                    what: "modify",
                    entity: *id,
                    expected: "same kind as the existing entity",
                    found: geom.kind(),
                });
            }
            let after = Entity {
                geom: geom.clone(),
                ..before.clone()
            };
            // A line may not be made to reference itself, etc.
            doc.check_entity(&after)?;
            if after.geom.point_refs().contains(id) {
                return Err(DocError::SameEntity {
                    what: "modify",
                    entity: *id,
                });
            }
            record(
                doc,
                patches,
                Patch::Entity {
                    id: *id,
                    before: Some(before),
                    after: Some(after),
                },
            );
        }
        Command::DeleteEntity { id } => {
            doc.check_removable(*id)?;
            let before = doc.entity(*id).cloned();
            if let Some(e) = &before {
                doc.check_unlocked(e.layer)?;
            }
            record(
                doc,
                patches,
                Patch::Entity {
                    id: *id,
                    before,
                    after: None,
                },
            );
        }
        Command::ToggleConstruction { id } => {
            let before = doc.entity(*id).ok_or(DocError::UnknownEntity(*id))?.clone();
            doc.check_not_driven(*id)?;
            doc.check_unlocked(before.layer)?;
            let construction = !before.construction;
            let after = Entity {
                construction,
                ..before.clone()
            };
            record(
                doc,
                patches,
                Patch::Entity {
                    id: *id,
                    before: Some(before),
                    after: Some(after),
                },
            );
            // Instances follow their source (SPEC §6.2.1: associative).
            for (inst, _) in recipe::instances_of(doc, *id) {
                if let Some(b) = doc.entity(inst).cloned() {
                    let a = Entity {
                        construction,
                        ..b.clone()
                    };
                    record(
                        doc,
                        patches,
                        Patch::Entity {
                            id: inst,
                            before: Some(b),
                            after: Some(a),
                        },
                    );
                }
            }
        }
        Command::AddConstraint { constraint } => {
            constraint.validate(doc)?;
            let id = ConstraintId::new(doc.alloc_id()?);
            record(
                doc,
                patches,
                Patch::Constraint {
                    id,
                    before: None,
                    after: Some(constraint.clone()),
                },
            );
            out.constraints.push(id);
        }
        Command::RemoveConstraint { id } => {
            if let Some(recipe) = doc.constraint_owner(*id) {
                return Err(DocError::ConstraintOwned {
                    constraint: *id,
                    recipe,
                });
            }
            let before = doc
                .constraint(*id)
                .ok_or(DocError::UnknownConstraint(*id))?
                .clone();
            record(
                doc,
                patches,
                Patch::Constraint {
                    id: *id,
                    before: Some(before),
                    after: None,
                },
            );
        }
        Command::SetDimension { id, value } => {
            edit_dimension(doc, *id, patches, |d| d.value = *value)?;
            set_dim_expr(doc, *id, None, patches);
        }
        Command::SetDriven { id, driven } => {
            edit_dimension(doc, *id, patches, |d| d.driven = *driven)?;
            if *driven {
                set_dim_expr(doc, *id, None, patches);
            }
        }
        Command::SetParameter { name, expr } => {
            params::check_name(name)?;
            if doc.parameters.len() >= params::MAX_PARAMETERS && !doc.parameters.contains_key(name)
            {
                return Err(DocError::Invalid("too many parameters".to_owned()));
            }
            let before = doc.parameters.get(name).cloned();
            let after = Parameter {
                expr: expr.clone(),
                value: before.as_ref().map_or(0.0, |p| p.value),
            };
            let p = Patch::Parameter {
                name: name.clone(),
                before,
                after: Some(after),
            };
            record(doc, patches, p);
            revalue(doc, patches)?;
        }
        Command::RemoveParameter { name } => {
            let before = doc
                .parameters
                .get(name)
                .cloned()
                .ok_or_else(|| DocError::UnknownParameter(name.clone()))?;
            let users = params::users_of(doc, name);
            if !users.is_empty() {
                return Err(DocError::ParameterInUse {
                    name: name.clone(),
                    users,
                });
            }
            let p = Patch::Parameter {
                name: name.clone(),
                before: Some(before),
                after: None,
            };
            record(doc, patches, p);
        }
        Command::RenameParameter { from, to } => {
            params::check_name(to)?;
            let old = doc
                .parameters
                .get(from)
                .cloned()
                .ok_or_else(|| DocError::UnknownParameter(from.clone()))?;
            if from != to {
                if doc.parameters.contains_key(to) {
                    return Err(DocError::Invalid(format!(
                        "parameter `{to}` already exists"
                    )));
                }
                let names: Vec<String> = doc.parameters.keys().cloned().collect();
                let p = Patch::Parameter {
                    name: from.clone(),
                    before: Some(old.clone()),
                    after: None,
                };
                record(doc, patches, p);
                let p = Patch::Parameter {
                    name: to.clone(),
                    before: None,
                    after: Some(old),
                };
                record(doc, patches, p);
                for n in names.iter().filter(|n| *n != from) {
                    let Some(b) = doc.parameters.get(n).cloned() else {
                        continue;
                    };
                    let e = params::rename_in(&b.expr, from, to);
                    if e != b.expr {
                        let p = Patch::Parameter {
                            name: n.clone(),
                            before: Some(b.clone()),
                            after: Some(Parameter { expr: e, ..b }),
                        };
                        record(doc, patches, p);
                    }
                }
                let ids: Vec<ConstraintId> = doc.dim_exprs.keys().copied().collect();
                for id in ids {
                    let Some(b) = doc.dim_exprs.get(&id).cloned() else {
                        continue;
                    };
                    let e = params::rename_in(&b, from, to);
                    if e != b {
                        set_dim_expr(doc, id, Some(e), patches);
                    }
                }
                revalue(doc, patches)?;
            }
        }
        Command::SetDimensionExpr { id, expr } => {
            let c = doc
                .constraint(*id)
                .ok_or(DocError::UnknownConstraint(*id))?
                .clone();
            let d = c.dimension().ok_or(DocError::NotADimension(*id))?;
            match expr {
                Some(src) => {
                    if d.driven {
                        return Err(DocError::Invalid(
                            "a driven dimension shows its measured value; make it driving first"
                                .to_owned(),
                        ));
                    }
                    // Parse and evaluate first so the error names the expression.
                    crate::expr::evaluate(src, doc.parameters())?;
                    set_dim_expr(doc, *id, Some(src.clone()), patches);
                    revalue(doc, patches)?;
                }
                None => set_dim_expr(doc, *id, None, patches),
            }
        }
        Command::AddLayer {
            name,
            colour,
            operation,
        } => {
            let layer = Layer {
                name: name.clone(),
                colour: *colour,
                operation: *operation,
                export_order: doc
                    .layers
                    .values()
                    .map(|l| l.export_order)
                    .max()
                    .map_or(0, |m| m.saturating_add(1)),
                visible: true,
                locked: false,
                export_enabled: true,
            };
            Document::check_layer(&layer)?;
            let id = LayerId::new(doc.alloc_id()?);
            record(
                doc,
                patches,
                Patch::Layer {
                    id,
                    before: None,
                    after: Some(layer),
                },
            );
            out.layers.push(id);
        }
        Command::SetLayerProps { id, props } => {
            let before = doc.layer(*id).ok_or(DocError::UnknownLayer(*id))?.clone();
            let mut after = before.clone();
            if let Some(v) = &props.name {
                after.name.clone_from(v);
            }
            if let Some(v) = props.colour {
                after.colour = v;
            }
            if let Some(v) = props.operation {
                after.operation = v;
            }
            if let Some(v) = props.export_order {
                after.export_order = v;
            }
            if let Some(v) = props.visible {
                after.visible = v;
            }
            if let Some(v) = props.export_enabled {
                after.export_enabled = v;
            }
            Document::check_layer(&after)?;
            if after != before {
                record(
                    doc,
                    patches,
                    Patch::Layer {
                        id: *id,
                        before: Some(before),
                        after: Some(after),
                    },
                );
            }
        }
        Command::SetLayerLocked { id, locked } => {
            let before = doc.layer(*id).ok_or(DocError::UnknownLayer(*id))?.clone();
            if before.locked != *locked {
                let after = Layer {
                    locked: *locked,
                    ..before.clone()
                };
                record(
                    doc,
                    patches,
                    Patch::Layer {
                        id: *id,
                        before: Some(before),
                        after: Some(after),
                    },
                );
            }
        }
        Command::DeleteLayer { id } => {
            let plan = doc.layer_deletion(*id)?;
            doc.check_unlocked(*id)?;
            if doc.layers.len() == 1 {
                return Err(DocError::Invalid(
                    "a document needs at least one layer".to_owned(),
                ));
            }
            for e in &plan.entities {
                let recipes = doc.recipes_involving(*e);
                if !recipes.is_empty() {
                    return Err(DocError::RecipeInUse {
                        entity: *e,
                        recipes,
                    });
                }
            }
            if !plan.blocked_by.is_empty() {
                return Err(DocError::LayerInUse {
                    layer: *id,
                    entities: plan.blocked_by,
                });
            }
            for cid in &plan.constraints {
                let before = doc.constraint(*cid).cloned();
                record(
                    doc,
                    patches,
                    Patch::Constraint {
                        id: *cid,
                        before,
                        after: None,
                    },
                );
            }
            for eid in &plan.entities {
                let before = doc.entity(*eid).cloned();
                record(
                    doc,
                    patches,
                    Patch::Entity {
                        id: *eid,
                        before,
                        after: None,
                    },
                );
            }
            let before = doc.layer(*id).cloned();
            record(
                doc,
                patches,
                Patch::Layer {
                    id: *id,
                    before,
                    after: None,
                },
            );
        }
        Command::MoveToLayer { entities, layer } => {
            doc.check_unlocked(*layer)?;
            if !doc.layers.contains_key(layer) {
                return Err(DocError::UnknownLayer(*layer));
            }
            for id in entities {
                let before = doc.entity(*id).ok_or(DocError::UnknownEntity(*id))?.clone();
                doc.check_unlocked(before.layer)?;
                if before.layer == *layer {
                    continue;
                }
                let recipes = doc.recipes_involving(*id);
                if !recipes.is_empty() {
                    return Err(DocError::RecipeInUse {
                        entity: *id,
                        recipes,
                    });
                }
                let after = Entity {
                    layer: *layer,
                    ..before.clone()
                };
                record(
                    doc,
                    patches,
                    Patch::Entity {
                        id: *id,
                        before: Some(before),
                        after: Some(after),
                    },
                );
            }
        }
        Command::Compound { commands, .. } => {
            for c in commands {
                exec(doc, c, patches, out)?;
            }
        }
        Command::AddBoolean { boolean } => {
            boolean.validate()?;
            let id = BooleanId::new(doc.alloc_id()?);
            let p = Patch::Boolean {
                id,
                before: None,
                after: Some(boolean.clone()),
            };
            record(doc, patches, p);
            out.booleans.push(id);
        }
        Command::EditBoolean { id, boolean } => {
            boolean.validate()?;
            let before = doc
                .boolean(*id)
                .ok_or(DocError::UnknownBoolean(*id))?
                .clone();
            if before != *boolean {
                let p = Patch::Boolean {
                    id: *id,
                    before: Some(before),
                    after: Some(boolean.clone()),
                };
                record(doc, patches, p);
            }
        }
        Command::RemoveBoolean { id } => {
            let before = doc
                .boolean(*id)
                .ok_or(DocError::UnknownBoolean(*id))?
                .clone();
            let p = Patch::Boolean {
                id: *id,
                before: Some(before),
                after: None,
            };
            record(doc, patches, p);
        }
        Command::AddRecipe { kind, sources } => {
            add_recipe(doc, kind, sources, patches, out)?;
        }
        Command::ExplodeRecipe { id } => {
            let before = doc.recipe(*id).ok_or(DocError::UnknownRecipe(*id))?.clone();
            let p = Patch::Recipe {
                id: *id,
                before: Some(before),
                after: None,
            };
            record(doc, patches, p);
        }
        Command::RemoveRecipe { id } => remove_recipe(doc, *id, patches)?,
        Command::ApplySolution { points, radii } => apply_solution(doc, points, radii)?,
    }
    Ok(())
}

/// Create a recipe, its instance entities and its owned constraints.
fn add_recipe(
    doc: &mut Document,
    kind: &RecipeKind,
    sources: &[EntityId],
    patches: &mut Vec<Patch>,
    out: &mut Applied,
) -> Result<(), DocError> {
    let mut sources = sources.to_vec();
    sources.sort_unstable();
    recipe::validate(doc, kind, &sources, None)?;
    for id in &sources {
        if let Some(e) = doc.entity(*id) {
            doc.check_unlocked(e.layer)?;
        }
    }
    let closure: Vec<EntityId> = recipe::closure(doc, &sources).into_iter().collect();
    let rid = RecipeId::new(doc.alloc_id()?);
    // New IDs for copy 0, in closure order; the recipe keeps them for good.
    let mut set = Vec::with_capacity(closure.len());
    for _ in &closure {
        set.push(EntityId::new(doc.alloc_id()?));
    }
    let xf = recipe::transform(doc, kind, 0)?;
    let rename = |id: EntityId| {
        closure
            .iter()
            .position(|c| *c == id)
            .and_then(|i| set.get(i).copied())
            .unwrap_or(id)
    };
    // Points first so every reference resolves when the entity is checked.
    let mut order: Vec<usize> = (0..closure.len()).collect();
    order.sort_by_key(|i| {
        doc.entity(closure[*i])
            .map_or(1, |e| u8::from(e.geom.kind() != EntityKind::Point))
    });
    for i in order {
        let src = doc
            .entity(closure[i])
            .ok_or(DocError::UnknownEntity(closure[i]))?
            .clone();
        let e = Entity {
            layer: src.layer,
            construction: src.construction,
            geom: recipe::copy_geom(&src.geom, xf, &rename),
        };
        doc.check_entity(&e)?;
        record(
            doc,
            patches,
            Patch::Entity {
                id: set[i],
                before: None,
                after: Some(e),
            },
        );
    }
    let mut owned = Vec::new();
    let RecipeKind::Mirror { axis } = kind;
    for (i, src) in closure.iter().enumerate() {
        let c = match doc.entity(*src).map(|e| e.geom.kind()) {
            Some(EntityKind::Point) => Constraint::Symmetric {
                a: *src,
                b: set[i],
                axis: *axis,
            },
            Some(EntityKind::Circle) => Constraint::Equal { a: *src, b: set[i] },
            _ => continue,
        };
        c.validate(doc)?;
        let cid = ConstraintId::new(doc.alloc_id()?);
        record(
            doc,
            patches,
            Patch::Constraint {
                id: cid,
                before: None,
                after: Some(c),
            },
        );
        owned.push(cid);
        out.constraints.push(cid);
    }
    out.entities.extend(&set);
    out.recipes.push(rid);
    let recipe = Recipe {
        kind: kind.clone(),
        sources,
        pool: vec![set],
        active: 1,
        constraints: owned,
    };
    record(
        doc,
        patches,
        Patch::Recipe {
            id: rid,
            before: None,
            after: Some(recipe),
        },
    );
    Ok(())
}

/// Delete a recipe with its instances and owned constraints, unless
/// something else still refers to them.
fn remove_recipe(
    doc: &mut Document,
    id: RecipeId,
    patches: &mut Vec<Patch>,
) -> Result<(), DocError> {
    let recipe = doc.recipe(id).ok_or(DocError::UnknownRecipe(id))?.clone();
    let instances: Vec<EntityId> = recipe.instances().collect();
    for inst in &instances {
        let (ents, cons) = doc.dependents(*inst);
        let ents: Vec<_> = ents
            .into_iter()
            .filter(|e| !instances.contains(e))
            .collect();
        let cons: Vec<_> = cons
            .into_iter()
            .filter(|c| !recipe.constraints.contains(c))
            .collect();
        if !ents.is_empty() || !cons.is_empty() {
            return Err(DocError::EntityInUse {
                id: *inst,
                entities: ents,
                constraints: cons,
            });
        }
    }
    for cid in &recipe.constraints {
        let before = doc.constraint(*cid).cloned();
        record(
            doc,
            patches,
            Patch::Constraint {
                id: *cid,
                before,
                after: None,
            },
        );
    }
    for inst in &instances {
        let before = doc.entity(*inst).cloned();
        record(
            doc,
            patches,
            Patch::Entity {
                id: *inst,
                before,
                after: None,
            },
        );
    }
    record(
        doc,
        patches,
        Patch::Recipe {
            id,
            before: Some(recipe),
            after: None,
        },
    );
    Ok(())
}

/// Set or clear the expression behind a dimension, as a patch.
fn set_dim_expr(
    doc: &mut Document,
    id: ConstraintId,
    expr: Option<String>,
    patches: &mut Vec<Patch>,
) {
    let before = doc.dim_exprs.get(&id).cloned();
    if before != expr {
        let p = Patch::DimExpr {
            id,
            before,
            after: expr,
        };
        record(doc, patches, p);
    }
}

/// Re-evaluate every parameter and expression-backed dimension, recording
/// what changed. Fails (the whole command is rolled back) when an expression
/// no longer evaluates or a dimension would become invalid.
fn revalue(doc: &mut Document, patches: &mut Vec<Patch>) -> Result<(), DocError> {
    let r = params::revalue(doc)?;
    for (name, v) in r.parameters {
        if let Some(b) = doc.parameters.get(&name).filter(|b| b.value != v).cloned() {
            let p = Patch::Parameter {
                name,
                before: Some(b.clone()),
                after: Some(Parameter { value: v, ..b }),
            };
            record(doc, patches, p);
        }
    }
    for (id, v) in r.dimensions {
        let before = doc
            .constraint(id)
            .ok_or(DocError::UnknownConstraint(id))?
            .clone();
        let mut after = before.clone();
        let d = after.dimension_mut().ok_or(DocError::NotADimension(id))?;
        if d.value == v {
            continue;
        }
        d.value = v;
        after.validate(doc)?;
        let p = Patch::Constraint {
            id,
            before: Some(before),
            after: Some(after),
        };
        record(doc, patches, p);
    }
    Ok(())
}

fn record(doc: &mut Document, patches: &mut Vec<Patch>, p: Patch) {
    p.set(doc, true);
    patches.push(p);
}

fn edit_dimension(
    doc: &mut Document,
    id: ConstraintId,
    patches: &mut Vec<Patch>,
    f: impl FnOnce(&mut crate::constraint::Dimension),
) -> Result<(), DocError> {
    let before = doc
        .constraint(id)
        .ok_or(DocError::UnknownConstraint(id))?
        .clone();
    let mut after = before.clone();
    f(after.dimension_mut().ok_or(DocError::NotADimension(id))?);
    after.validate(doc)?;
    let p = Patch::Constraint {
        id,
        before: Some(before),
        after: Some(after),
    };
    record(doc, patches, p);
    Ok(())
}

/// Validate everything first, then write: a bad solution changes nothing.
fn apply_solution(
    doc: &mut Document,
    points: &[(EntityId, Vec2)],
    radii: &[(EntityId, f64)],
) -> Result<(), DocError> {
    for (id, p) in points {
        if doc.point_pos(*id).is_none() {
            return Err(DocError::UnknownEntity(*id));
        }
        if !p.is_finite() {
            return Err(DocError::BadValue {
                what: "solved position",
                value: if p.x.is_finite() { p.y } else { p.x },
            });
        }
    }
    for (id, r) in radii {
        match doc.entity(*id).map(|e| e.geom.kind()) {
            None => return Err(DocError::UnknownEntity(*id)),
            Some(EntityKind::Circle) => {}
            Some(found) => {
                return Err(DocError::WrongKind {
                    what: "solved radius",
                    entity: *id,
                    expected: "circle",
                    found,
                });
            }
        }
        if !(r.is_finite() && *r > 0.0) {
            return Err(DocError::BadValue {
                what: "solved radius",
                value: *r,
            });
        }
    }
    for (id, p) in points {
        if let Some(Entity {
            geom: Geom::Point { pos },
            ..
        }) = doc.entities.get_mut(*id)
        {
            *pos = *p;
        }
    }
    for (id, r) in radii {
        if let Some(Entity {
            geom: Geom::Circle { radius, .. },
            ..
        }) = doc.entities.get_mut(*id)
        {
            *radius = *r;
        }
    }
    Ok(())
}

/// A document plus its undo/redo history: the only way to change a document.
#[derive(Debug, Clone)]
pub struct Editor {
    doc: Document,
    undo: Vec<Transaction>,
    redo: Vec<Transaction>,
}

impl Editor {
    #[must_use]
    pub fn new(doc: Document) -> Self {
        Self {
            doc,
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }

    #[must_use]
    pub fn document(&self) -> &Document {
        &self.doc
    }

    /// Give up the history and take the document.
    #[must_use]
    pub fn into_document(self) -> Document {
        self.doc
    }

    /// Apply a command as one undo step. Atomic: on error the document is
    /// exactly as it was and the history is unchanged.
    ///
    /// # Errors
    /// Whatever validation rejected the command.
    pub fn apply(&mut self, cmd: &Command) -> Result<Applied, DocError> {
        let mut patches = Vec::new();
        let mut out = Applied::default();
        if let Err(e) = exec(&mut self.doc, cmd, &mut patches, &mut out) {
            for p in patches.iter().rev() {
                p.set(&mut self.doc, false);
            }
            return Err(e);
        }
        if !patches.is_empty() {
            self.undo.push(Transaction {
                label: cmd.label(),
                patches,
            });
            self.redo.clear();
        }
        Ok(out)
    }

    /// Undo the last step, returning its label. Re-solve afterwards.
    ///
    /// # Errors
    /// [`DocError::NothingToDo`] when there is nothing to undo.
    pub fn undo(&mut self) -> Result<String, DocError> {
        let t = self.undo.pop().ok_or(DocError::NothingToDo("undo"))?;
        for p in t.patches.iter().rev() {
            p.set(&mut self.doc, false);
        }
        let label = t.label.clone();
        self.redo.push(t);
        Ok(label)
    }

    /// Redo the last undone step, returning its label. Re-solve afterwards.
    ///
    /// # Errors
    /// [`DocError::NothingToDo`] when there is nothing to redo.
    pub fn redo(&mut self) -> Result<String, DocError> {
        let t = self.redo.pop().ok_or(DocError::NothingToDo("redo"))?;
        for p in &t.patches {
            p.set(&mut self.doc, true);
        }
        let label = t.label.clone();
        self.undo.push(t);
        Ok(label)
    }

    #[must_use]
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    #[must_use]
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    #[must_use]
    pub fn undo_label(&self) -> Option<&str> {
        self.undo.last().map(|t| t.label.as_str())
    }

    #[must_use]
    pub fn redo_label(&self) -> Option<&str> {
        self.redo.last().map(|t| t.label.as_str())
    }

    /// Apply `cmd`, then `solve` the result, as **one** undo step: the solver's
    /// moves are recorded in the same transaction, so undo puts the geometry
    /// back exactly (SPEC §11). If the command or the solve fails, the
    /// document and the history are exactly as they were.
    pub(crate) fn apply_solved(
        &mut self,
        cmd: &Command,
        solve: impl FnOnce(&Document) -> Result<Solution, DocError>,
    ) -> Result<Applied, DocError> {
        let mut patches = Vec::new();
        let mut out = Applied::default();
        let result = exec(&mut self.doc, cmd, &mut patches, &mut out)
            .and_then(|()| solve(&self.doc))
            .and_then(|sol| solution_patches(&mut self.doc, &sol, &mut patches));
        if let Err(e) = result {
            for p in patches.iter().rev() {
                p.set(&mut self.doc, false);
            }
            return Err(e);
        }
        if !patches.is_empty() {
            self.undo.push(Transaction {
                label: cmd.label(),
                patches,
            });
            self.redo.clear();
        }
        Ok(out)
    }

    /// Record everything that changed since `start` as **one** undo step.
    ///
    /// For an interaction (a drag, #71) whose frames were written with
    /// [`Self::apply_solution_unrecorded`]: the document is rewound to
    /// `start` and the current state replayed as a recorded step, so undo
    /// restores `start` exactly and redo the final state. Nothing is recorded
    /// when nothing changed.
    pub(crate) fn commit_from(&mut self, label: &str, start: &Solution) -> Result<(), DocError> {
        let end = Solution::capture(&self.doc);
        solution_patches(&mut self.doc, start, &mut Vec::new())?;
        let mut patches = Vec::new();
        solution_patches(&mut self.doc, &end, &mut patches)?;
        if !patches.is_empty() {
            self.undo.push(Transaction {
                label: label.to_owned(),
                patches,
            });
            self.redo.clear();
        }
        Ok(())
    }

    /// Write solver output without an undo step, e.g. the first solve after
    /// opening a file (SPEC §10: stored positions are derived state).
    pub(crate) fn apply_solution_unrecorded(&mut self, sol: &Solution) -> Result<(), DocError> {
        let mut patches = Vec::new();
        solution_patches(&mut self.doc, sol, &mut patches)
    }
}

/// Solver output in document terms (`crate::solve`).
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Solution {
    pub points: Vec<(EntityId, Vec2)>,
    pub radii: Vec<(EntityId, f64)>,
    /// Measured values of driven (reference) dimensions.
    pub driven: Vec<(ConstraintId, f64)>,
}

impl Solution {
    /// Every solver-written value in the document as it stands: all point
    /// positions, circle radii and measured (driven) dimension values.
    pub(crate) fn capture(doc: &Document) -> Self {
        let mut s = Self::default();
        for (id, e) in doc.entities().iter() {
            match e.geom {
                Geom::Point { pos } => s.points.push((id, pos)),
                Geom::Circle { radius, .. } => s.radii.push((id, radius)),
                _ => {}
            }
        }
        for (id, c) in doc.constraints() {
            if let Some(d) = c.dimension().filter(|d| d.driven) {
                s.driven.push((*id, d.value));
            }
        }
        s
    }
}

/// Turn a [`Solution`] into recorded patches. Everything is validated before
/// anything is written; unchanged values produce no patch.
fn solution_patches(
    doc: &mut Document,
    sol: &Solution,
    patches: &mut Vec<Patch>,
) -> Result<(), DocError> {
    let mut new = Vec::new();
    for (id, p) in &sol.points {
        let before = doc.entity(*id).ok_or(DocError::UnknownEntity(*id))?.clone();
        if !p.is_finite() {
            return Err(DocError::BadValue {
                what: "solved position",
                value: if p.x.is_finite() { p.y } else { p.x },
            });
        }
        let mut after = before.clone();
        match &mut after.geom {
            Geom::Point { pos } if *pos == *p => continue,
            Geom::Point { pos } => *pos = *p,
            other => {
                return Err(DocError::WrongKind {
                    what: "solved position",
                    entity: *id,
                    expected: "point",
                    found: other.kind(),
                });
            }
        }
        new.push(Patch::Entity {
            id: *id,
            before: Some(before),
            after: Some(after),
        });
    }
    for (id, r) in &sol.radii {
        let before = doc.entity(*id).ok_or(DocError::UnknownEntity(*id))?.clone();
        if !(r.is_finite() && *r > 0.0) {
            return Err(DocError::BadValue {
                what: "solved radius",
                value: *r,
            });
        }
        let mut after = before.clone();
        match &mut after.geom {
            Geom::Circle { radius, .. } if *radius == *r => continue,
            Geom::Circle { radius, .. } => *radius = *r,
            other => {
                return Err(DocError::WrongKind {
                    what: "solved radius",
                    entity: *id,
                    expected: "circle",
                    found: other.kind(),
                });
            }
        }
        new.push(Patch::Entity {
            id: *id,
            before: Some(before),
            after: Some(after),
        });
    }
    for (id, v) in &sol.driven {
        let before = doc
            .constraint(*id)
            .ok_or(DocError::UnknownConstraint(*id))?
            .clone();
        if !v.is_finite() {
            return Err(DocError::BadValue {
                what: "measured dimension",
                value: *v,
            });
        }
        let mut after = before.clone();
        let dim = after.dimension_mut().ok_or(DocError::NotADimension(*id))?;
        if dim.value == *v {
            continue;
        }
        dim.value = *v;
        new.push(Patch::Constraint {
            id: *id,
            before: Some(before),
            after: Some(after),
        });
    }
    for p in new {
        record(doc, patches, p);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constraint::Dimension;

    const L: LayerId = Document::default_layer();

    fn add_pt(ed: &mut Editor, x: f64, y: f64) -> EntityId {
        let r = ed
            .apply(&Command::AddEntity {
                layer: L,
                construction: false,
                geom: Geom::Point {
                    pos: Vec2::new(x, y),
                },
            })
            .unwrap();
        r.entities[0]
    }

    /// Content equality; the ID counter legitimately only moves forward.
    fn same_content(a: &Document, b: &Document) -> bool {
        let mut a = a.clone();
        a.next_id = b.next_id;
        a == *b
    }

    fn square(ed: &mut Editor) -> (Vec<EntityId>, EntityId) {
        let p: Vec<_> = [(0.0, 0.0), (4.0, 0.0), (4.0, 3.0)]
            .iter()
            .map(|(x, y)| add_pt(ed, *x, *y))
            .collect();
        let l = ed
            .apply(&Command::AddEntity {
                layer: L,
                construction: false,
                geom: Geom::Line {
                    start: p[0],
                    end: p[1],
                },
            })
            .unwrap()
            .entities[0];
        (p, l)
    }

    #[test]
    fn apply_undo_redo_restores_document_and_ids() {
        let mut ed = Editor::new(Document::new());
        let original = ed.document().clone();
        let a = add_pt(&mut ed, 1.0, 2.0);
        let after = ed.document().clone();
        assert_eq!(ed.undo().unwrap(), "Add entity");
        assert!(same_content(ed.document(), &original));
        assert!(
            ed.document().next_id() > original.next_id(),
            "counter never rewinds"
        );
        assert_eq!(ed.redo().unwrap(), "Add entity");
        assert_eq!(ed.document(), &after);
        assert!(
            ed.document().entity(a).is_some(),
            "redo restores the same ID"
        );
        // A fresh add after undo gets a new, higher ID.
        ed.undo().unwrap();
        let b = add_pt(&mut ed, 1.0, 2.0);
        assert!(b > a);
    }

    #[test]
    fn every_command_round_trips_through_undo() {
        let mut ed = Editor::new(Document::new());
        let (p, l) = square(&mut ed);
        let mut steps: Vec<Command> = vec![
            Command::ModifyEntity {
                id: p[2],
                geom: Geom::Point {
                    pos: Vec2::new(9.0, 9.0),
                },
            },
            Command::ToggleConstruction { id: l },
            Command::AddConstraint {
                constraint: Constraint::Horizontal { line: l },
            },
            Command::AddConstraint {
                constraint: Constraint::Distance {
                    a: p[0],
                    b: p[1],
                    dim: Dimension::driving(4.0),
                },
            },
        ];
        for s in &steps {
            let before = ed.document().clone();
            ed.apply(s).unwrap();
            assert!(!same_content(ed.document(), &before));
            ed.undo().unwrap();
            assert!(same_content(ed.document(), &before), "{s:?}");
            ed.redo().unwrap();
        }
        // Dimension edits and removals need the constraint IDs.
        let cids: Vec<_> = ed.document().constraints().keys().copied().collect();
        steps = vec![
            Command::SetDimension {
                id: cids[1],
                value: 7.0,
            },
            Command::SetDriven {
                id: cids[1],
                driven: true,
            },
            Command::RemoveConstraint { id: cids[0] },
        ];
        for s in &steps {
            let before = ed.document().clone();
            ed.apply(s).unwrap();
            ed.undo().unwrap();
            assert!(same_content(ed.document(), &before), "{s:?}");
            ed.redo().unwrap();
        }
        // Delete is only possible once nothing references the entity.
        let lone = add_pt(&mut ed, 50.0, 50.0);
        let before = ed.document().clone();
        ed.apply(&Command::DeleteEntity { id: lone }).unwrap();
        assert!(ed.document().entity(lone).is_none());
        ed.undo().unwrap();
        assert!(same_content(ed.document(), &before));
        assert!(
            ed.document().entity(lone).is_some(),
            "undo of delete restores the same ID"
        );
    }

    #[test]
    fn compound_is_one_labelled_atomic_step() {
        let mut ed = Editor::new(Document::new());
        let original = ed.document().clone();
        let cmd = Command::Compound {
            label: "Draw segment".to_owned(),
            commands: vec![
                Command::AddEntity {
                    layer: L,
                    construction: false,
                    geom: Geom::Point { pos: Vec2::ZERO },
                },
                Command::AddEntity {
                    layer: L,
                    construction: false,
                    geom: Geom::Point {
                        pos: Vec2::new(1.0, 0.0),
                    },
                },
            ],
        };
        let out = ed.apply(&cmd).unwrap();
        assert_eq!(out.entities.len(), 2);
        assert_eq!(ed.document().entities().len(), 2);
        assert_eq!(ed.undo_label(), Some("Draw segment"));
        ed.undo().unwrap();
        assert!(
            same_content(ed.document(), &original),
            "one undo removes both"
        );
        assert!(!ed.can_undo());
        ed.redo().unwrap();
        assert_eq!(ed.document().entities().len(), 2);
    }

    #[test]
    fn failing_compound_leaves_document_untouched() {
        let mut ed = Editor::new(Document::new());
        let a = add_pt(&mut ed, 0.0, 0.0);
        let before = ed.document().clone();
        let depth = ed.undo.len();
        let cmd = Command::Compound {
            label: "Doomed".to_owned(),
            commands: vec![
                Command::ToggleConstruction { id: a },
                Command::AddEntity {
                    layer: L,
                    construction: false,
                    geom: Geom::Point {
                        pos: Vec2::new(5.0, 5.0),
                    },
                },
                Command::DeleteEntity {
                    id: EntityId::new(9999),
                },
            ],
        };
        assert_eq!(
            ed.apply(&cmd),
            Err(DocError::UnknownEntity(EntityId::new(9999)))
        );
        assert!(same_content(ed.document(), &before));
        assert_eq!(ed.undo.len(), depth, "history unchanged");
    }

    #[test]
    fn delete_refuses_while_referenced_then_compound_deletes_all() {
        let mut ed = Editor::new(Document::new());
        let (p, l) = square(&mut ed);
        ed.apply(&Command::AddConstraint {
            constraint: Constraint::Horizontal { line: l },
        })
        .unwrap();
        let cid = *ed.document().constraints().keys().next().unwrap();
        let err = ed.apply(&Command::DeleteEntity { id: l }).unwrap_err();
        assert!(
            matches!(err, DocError::EntityInUse { ref constraints, .. } if constraints == &[cid])
        );
        let before = ed.document().clone();
        ed.apply(&Command::Compound {
            label: "Delete line".to_owned(),
            commands: vec![
                Command::RemoveConstraint { id: cid },
                Command::DeleteEntity { id: l },
                Command::DeleteEntity { id: p[0] },
                Command::DeleteEntity { id: p[1] },
            ],
        })
        .unwrap();
        assert!(ed.document().entity(l).is_none());
        ed.undo().unwrap();
        assert!(
            same_content(ed.document(), &before),
            "atomic restore incl. constraint"
        );
    }

    #[test]
    fn new_command_clears_redo() {
        let mut ed = Editor::new(Document::new());
        add_pt(&mut ed, 0.0, 0.0);
        ed.undo().unwrap();
        assert!(ed.can_redo());
        add_pt(&mut ed, 1.0, 1.0);
        assert!(!ed.can_redo());
        assert_eq!(ed.redo(), Err(DocError::NothingToDo("redo")));
    }

    #[test]
    fn rejected_commands() {
        let mut ed = Editor::new(Document::new());
        let (p, l) = square(&mut ed);
        let bad = [
            Command::ModifyEntity {
                id: l,
                geom: Geom::Point { pos: Vec2::ZERO },
            },
            Command::ModifyEntity {
                id: l,
                geom: Geom::Line {
                    start: l,
                    end: p[0],
                },
            },
            Command::ModifyEntity {
                id: p[0],
                geom: Geom::Point {
                    pos: Vec2::new(f64::NAN, 0.0),
                },
            },
            Command::AddEntity {
                layer: LayerId::new(42),
                construction: false,
                geom: Geom::Point { pos: Vec2::ZERO },
            },
            Command::SetDimension {
                id: ConstraintId::new(5),
                value: 1.0,
            },
            Command::AddConstraint {
                constraint: Constraint::Horizontal { line: p[0] },
            },
            Command::ToggleConstruction {
                id: EntityId::new(1234),
            },
        ];
        for c in bad {
            let before = ed.document().clone();
            assert!(ed.apply(&c).is_err(), "{c:?}");
            assert_eq!(ed.document(), &before);
        }
        // SetDimension on a non-dimensional constraint.
        ed.apply(&Command::AddConstraint {
            constraint: Constraint::Horizontal { line: l },
        })
        .unwrap();
        let cid = *ed.document().constraints().keys().next().unwrap();
        assert_eq!(
            ed.apply(&Command::SetDimension {
                id: cid,
                value: 1.0
            }),
            Err(DocError::NotADimension(cid))
        );
        // Nothing to undo on a fresh editor.
        assert_eq!(
            Editor::new(Document::new()).undo(),
            Err(DocError::NothingToDo("undo"))
        );
    }

    #[test]
    fn apply_solution_is_not_an_undo_step() {
        let mut ed = Editor::new(Document::new());
        let a = add_pt(&mut ed, 0.0, 0.0);
        let depth = ed.undo.len();
        ed.apply(&Command::ApplySolution {
            points: vec![(a, Vec2::new(3.0, 4.0))],
            radii: vec![],
        })
        .unwrap();
        assert_eq!(ed.document().point_pos(a), Some(Vec2::new(3.0, 4.0)));
        assert_eq!(ed.undo.len(), depth);
        let bad = Command::ApplySolution {
            points: vec![(a, Vec2::new(1.0, 1.0)), (a, Vec2::new(f64::INFINITY, 0.0))],
            radii: vec![],
        };
        assert!(ed.apply(&bad).is_err());
        assert_eq!(
            ed.document().point_pos(a),
            Some(Vec2::new(3.0, 4.0)),
            "all-or-nothing"
        );
    }

    #[test]
    fn commands_serialise() {
        let c = Command::Compound {
            label: "x".to_owned(),
            commands: vec![
                Command::AddConstraint {
                    constraint: Constraint::Fix {
                        point: EntityId::new(2),
                    },
                },
                Command::SetDimension {
                    id: ConstraintId::new(3),
                    value: 1.5,
                },
            ],
        };
        let text = ron::to_string(&c).unwrap();
        assert_eq!(ron::from_str::<Command>(&text).unwrap(), c);
    }
}
