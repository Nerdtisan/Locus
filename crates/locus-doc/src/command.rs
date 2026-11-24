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
use crate::style::{Style, StylePatch};
use locus_geom::Vec2;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A document mutation. Plain data: serialise it, log it, replay it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Command {
    /// Create an entity. The document assigns the ID (see [`Applied`]).
    AddEntity {
        layer: LayerId,
        construction: bool,
        geom: Geom,
        /// Absent (the default style) in commands written before M5.
        #[serde(default)]
        style: Style,
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
    /// Make every entity in `entities` construction (`true`) or normal, as
    /// one undo step (#289, #421). Absolute rather than a toggle, so a
    /// selection holding both a source and its copies can't flip a copy
    /// twice. Construction never moves geometry, so recipe outputs (offset,
    /// pattern and mirror copies) may be set too, and keep it when their
    /// recipe regenerates; copies of a source follow it (SPEC §6.2.1).
    SetConstruction {
        entities: Vec<EntityId>,
        construction: bool,
    },
    /// Change the style of every entity in `entities` (SPEC §4, #287), as one
    /// undo step. Only the fields set in `patch` change, so a mixed selection
    /// keeps what it doesn't share. Pattern and mirror copies follow their
    /// source, as they do for construction (SPEC §6.2.1). Style never moves
    /// geometry, so recipe instances and plugin output may be styled too.
    SetStyle {
        entities: Vec<EntityId>,
        patch: StylePatch,
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
    /// Measure a dimension to the centreline or a stroke edge (SPEC §5, #239).
    /// The value is kept; the geometry re-solves to honour it at the new edge.
    SetDimensionEdge {
        id: ConstraintId,
        edge: crate::constraint::Edge,
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
    /// Move a layer to position `to` (0 = first) in the export order and
    /// renumber every layer's `export_order` to its new position, as one
    /// step (#462, ADR 0019 decision 9). The canvas draw order and the
    /// export order follow it (#291). `to` past the end means last.
    ReorderLayer {
        layer: LayerId,
        to: u32,
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
    /// Select (or clear) the document's machine profile by id (SPEC §7).
    SetProfile {
        id: Option<String>,
    },
    /// Place the job on the machine bed (SPEC §7, #443): the offset, mm,
    /// added to the design on export. `None` clears it (export frames the
    /// design itself again).
    SetPlacement {
        offset: Option<Vec2>,
    },
    /// Fill regions (SPEC §6.3, #451): each listed region gets `fill`, or
    /// loses its fill with `None`. One undo step for the lot.
    SetRegionFill {
        regions: Vec<locus_region::RegionKey>,
        fill: Option<crate::region_fill::RegionFill>,
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
    /// Add a driving dimension whose value is an expression, as one step
    /// (`constraint`'s own value is replaced by the evaluated expression).
    AddDimensionExpr {
        constraint: Constraint,
        expr: String,
    },
    /// Add an associative recipe over `sources` (SPEC §6.2.1, ADR 0011): the
    /// document creates the instance entities (and, for a mirror, the
    /// constraints that tie them to the sources). IDs: see [`Applied`].
    AddRecipe {
        kind: RecipeKind,
        sources: Vec<EntityId>,
    },
    /// Insert a generator plugin's output (SPEC §15.3, §15.5; #183): an
    /// origin point at `origin`, a direction point 10 mm along +x from it,
    /// and a [`RecipeKind::Generated`] recipe drawing `output` in that frame,
    /// all on `layer`, as one undo step. IDs: the two points come first in
    /// [`Applied::entities`], then the instances.
    AddGenerated {
        layer: LayerId,
        origin: Vec2,
        plugin: crate::generated::PluginRef,
        output: crate::generated::GeneratedGeometry,
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
    /// Change a recipe's parameters (e.g. an offset's distance); the kind
    /// must stay the same and the sources are kept. The instances follow.
    /// Suppress or restore a whole feature from the timeline (#446, ADR
    /// 0019 decision 5): its copies go but its IDs stay, so restoring brings
    /// the same entities back. Patterns, offsets and plugin objects; a
    /// mirror, or a feature another feature copies from, is refused.
    SetRecipeSuppressed {
        id: RecipeId,
        suppressed: bool,
    },
    /// Suppress or restore one copy of a pattern (SPEC §6.2.1). `index` is
    /// the copy's position in generation order (row by row for a grid).
    SetInstanceSuppressed {
        id: RecipeId,
        index: usize,
        suppressed: bool,
    },
    EditRecipe {
        id: RecipeId,
        kind: RecipeKind,
    },
    /// Put a raster reference image on a Reference layer that has none
    /// (#390). Import adds the layer, the image and the lock as one
    /// compound, like a vector reference.
    AddImage {
        layer: LayerId,
        image: Box<crate::image::RefImage>,
    },
    /// Move, scale or fade a reference image (#390, Calibrate #391). It is
    /// how the reference is drawn, not geometry, so its layer may be locked.
    SetImagePlacement {
        layer: LayerId,
        placement: crate::image::ImagePlacement,
    },
    /// Calibrate a reference (#391, Fusion's Calibrate): scale everything
    /// on Reference layer `layer` by `factor` about `about`, mm: its points,
    /// circle radii and image. The scale is stored in the reference itself.
    /// Allowed while the layer is locked, since that only stops editing the
    /// reference by hand.
    ScaleReference {
        layer: LayerId,
        about: Vec2,
        factor: f64,
    },
    /// Move, turn or scale a whole reference (#463, ADR 0019 decision 9),
    /// as the shapes' Move / Rotate / Scale tools do: everything on
    /// Reference layer `layer`, its image included (its rotation is stored,
    /// format v22). Like Calibrate it is allowed while the layer is locked,
    /// since only these tools may move a reference.
    TransformReference {
        layer: LayerId,
        transform: crate::transform::Transform,
    },
    /// Expand to geometry (#428, ADR 0019 decision 4): replace the styled
    /// strokes of `entities` with the plain curves of their outlines, on the
    /// same layers, in one undo step. Curves are joined end to end as the
    /// canvas joins them; Béziers are lowered to arcs within `tol` mm (the
    /// Expand tolerance, default 0.01 mm). Recipe instances are refused:
    /// explode them first (ADR 0011).
    ExpandStyle {
        entities: Vec<EntityId>,
        tol: f64,
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
            Command::SetConstruction { construction, .. } => if *construction {
                "Make construction"
            } else {
                "Make normal"
            }
            .to_owned(),
            Command::SetStyle { .. } => "Set style".to_owned(),
            Command::AddConstraint { constraint } => {
                format!("Add {} constraint", constraint.name())
            }
            Command::RemoveConstraint { .. } => "Remove constraint".to_owned(),
            Command::SetDimension { .. } => "Set dimension".to_owned(),
            Command::SetDimensionEdge { .. } => "Set dimension edge".to_owned(),
            Command::SetDriven { .. } => "Set driven".to_owned(),
            Command::AddLayer { .. } => "Add layer".to_owned(),
            Command::SetLayerProps { .. } => "Change layer".to_owned(),
            Command::ReorderLayer { .. } => "Reorder layers".to_owned(),
            Command::MoveToLayer { .. } => "Move to layer".to_owned(),
            Command::DeleteLayer { .. } => "Delete layer".to_owned(),
            Command::SetLayerLocked { locked: true, .. } => "Lock layer".to_owned(),
            Command::SetLayerLocked { locked: false, .. } => "Unlock layer".to_owned(),
            Command::AddBoolean { .. } => "Add boolean".to_owned(),
            Command::EditBoolean { .. } => "Edit boolean".to_owned(),
            Command::RemoveBoolean { .. } => "Remove boolean".to_owned(),
            Command::SetParameter { name, .. } => format!("Set parameter {name}"),
            Command::SetProfile { .. } => "Set machine profile".to_owned(),
            Command::SetPlacement { .. } => "Place job on bed".to_owned(),
            Command::SetRegionFill { fill: Some(_), .. } => "Fill region".to_owned(),
            Command::SetRegionFill { fill: None, .. } => "Remove region fill".to_owned(),
            Command::RemoveParameter { name } => format!("Remove parameter {name}"),
            Command::RenameParameter { from, .. } => format!("Rename parameter {from}"),
            Command::SetDimensionExpr { .. } => "Set dimension expression".to_owned(),
            Command::AddDimensionExpr { constraint, .. } => {
                format!("Add {} constraint", constraint.name())
            }
            Command::AddRecipe { kind, .. } => format!("Add {}", kind.name().to_lowercase()),
            Command::AddGenerated { .. } => "Add plugin object".to_owned(),
            Command::ExplodeRecipe { .. } => "Explode".to_owned(),
            Command::RemoveRecipe { .. } => "Remove recipe".to_owned(),
            Command::SetRecipeSuppressed {
                suppressed: true, ..
            } => "Suppress feature".to_owned(),
            Command::SetRecipeSuppressed {
                suppressed: false, ..
            } => "Unsuppress feature".to_owned(),
            Command::SetInstanceSuppressed { suppressed, .. } => if *suppressed {
                "Suppress instance"
            } else {
                "Restore instance"
            }
            .to_owned(),
            Command::EditRecipe { kind, .. } => format!("Edit {}", kind.name().to_lowercase()),
            Command::AddImage { .. } => "Add image".to_owned(),
            Command::SetImagePlacement { .. } => "Change image".to_owned(),
            Command::ScaleReference { .. } => "Calibrate reference".to_owned(),
            Command::TransformReference { transform, .. } => match transform {
                crate::transform::Transform::Move(_) => "Move reference",
                crate::transform::Transform::Rotate { .. } => "Rotate reference",
                crate::transform::Transform::Scale { .. }
                | crate::transform::Transform::Stretch { .. } => "Scale reference",
            }
            .to_owned(),
            Command::ExpandStyle { .. } => "Expand to geometry".to_owned(),
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
    /// Centreline or Styled, on machine layers (#428).
    #[serde(default)]
    pub stroke_styles: Option<crate::style::StrokeStyles>,
}

/// One reversible state change: the state of an item before and after.
/// `None` means "absent".
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub(crate) enum Patch {
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
    Profile {
        before: Option<String>,
        after: Option<String>,
    },
    Placement {
        before: Option<Vec2>,
        after: Option<Vec2>,
    },
    RegionFill {
        key: locus_region::RegionKey,
        before: Option<crate::region_fill::RegionFill>,
        after: Option<crate::region_fill::RegionFill>,
    },
    Image {
        layer: LayerId,
        before: Option<crate::image::RefImage>,
        after: Option<crate::image::RefImage>,
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
            Patch::Profile { before, after } => {
                doc.profile = if forward { after } else { before }.clone();
            }
            Patch::Placement { before, after } => {
                doc.placement = if forward { *after } else { *before };
            }
            Patch::RegionFill { key, before, after } => {
                match if forward { after } else { before } {
                    Some(f) => doc.region_fills.insert(key.clone(), *f),
                    None => doc.region_fills.remove(key),
                };
            }
            Patch::Image {
                layer,
                before,
                after,
            } => {
                match if forward { after } else { before }.clone() {
                    Some(i) => doc.images.insert(*layer, i),
                    None => doc.images.remove(layer),
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

/// Which solve a callback is asked for (ADR 0015).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SolvePhase {
    /// Everything except constraints that touch pattern instances.
    Main,
    /// Those constraints, with the pattern and its sources held fixed.
    Late,
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
            style,
        } => {
            doc.check_unlocked(*layer)?;
            let e = Entity {
                layer: *layer,
                construction: *construction,
                geom: geom.clone(),
                style: *style,
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
            // Construction never moves geometry: a recipe's output may be
            // toggled too, and keeps it on regeneration (#421).
            let before = doc.entity(*id).ok_or(DocError::UnknownEntity(*id))?.clone();
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
        Command::SetConstruction {
            entities,
            construction,
        } => {
            if entities.is_empty() {
                return Err(DocError::NothingToDo("set construction"));
            }
            let mut targets: Vec<EntityId> = Vec::new();
            for id in entities {
                let e = doc.entity(*id).ok_or(DocError::UnknownEntity(*id))?;
                doc.check_unlocked(e.layer)?;
                targets.push(*id);
                // Instances follow their source (SPEC §6.2.1: associative).
                targets.extend(recipe::instances_of(doc, *id).into_iter().map(|(i, _)| i));
            }
            targets.sort_unstable();
            targets.dedup();
            for id in targets {
                let Some(before) = doc.entity(id).cloned() else {
                    continue;
                };
                if before.construction == *construction {
                    continue;
                }
                let after = Entity {
                    construction: *construction,
                    ..before.clone()
                };
                record(
                    doc,
                    patches,
                    Patch::Entity {
                        id,
                        before: Some(before),
                        after: Some(after),
                    },
                );
            }
        }
        Command::SetStyle { entities, patch } => {
            if entities.is_empty() || patch.is_empty() {
                return Err(DocError::NothingToDo("set style"));
            }
            let mut targets: Vec<EntityId> = Vec::new();
            for id in entities {
                let e = doc.entity(*id).ok_or(DocError::UnknownEntity(*id))?;
                doc.check_unlocked(e.layer)?;
                targets.push(*id);
                // Instances follow their source (SPEC §6.2.1: associative).
                targets.extend(recipe::instances_of(doc, *id).into_iter().map(|(i, _)| i));
            }
            targets.sort_unstable();
            targets.dedup();
            for id in targets {
                let Some(before) = doc.entity(id).cloned() else {
                    continue;
                };
                let after = Entity {
                    style: before.style.patched(patch),
                    ..before.clone()
                };
                after.style.validate()?;
                if after != before {
                    record(
                        doc,
                        patches,
                        Patch::Entity {
                            id,
                            before: Some(before),
                            after: Some(after),
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
        Command::SetDimensionEdge { id, edge } => {
            edit_dimension(doc, *id, patches, |d| d.edge = *edge)?;
        }
        Command::SetDriven { id, driven } => {
            edit_dimension(doc, *id, patches, |d| d.driven = *driven)?;
            if *driven {
                set_dim_expr(doc, *id, None, patches);
            }
        }
        Command::SetProfile { id } => {
            if let Some(id) = id {
                crate::document::check_profile_id(id)?;
            }
            let p = Patch::Profile {
                before: doc.profile.clone(),
                after: id.clone(),
            };
            record(doc, patches, p);
        }
        Command::SetRegionFill { regions, fill } => {
            if regions.len() > crate::region_fill::MAX_REGION_FILLS {
                return Err(DocError::Invalid("too many regions".to_owned()));
            }
            if let Some(f) = fill {
                f.validate()?;
            }
            let mut keys = regions.clone();
            keys.sort();
            keys.dedup();
            for key in keys {
                crate::region_fill::check_key(&key)?;
                let before = doc.region_fills.get(&key).copied();
                if before != *fill {
                    record(
                        doc,
                        patches,
                        Patch::RegionFill {
                            key,
                            before,
                            after: *fill,
                        },
                    );
                }
            }
        }
        Command::SetPlacement { offset } => {
            if let Some(p) = offset {
                crate::document::check_placement(*p)?;
            }
            if *offset != doc.placement {
                let p = Patch::Placement {
                    before: doc.placement,
                    after: *offset,
                };
                record(doc, patches, p);
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
                let rids: Vec<RecipeId> = doc.recipes.keys().copied().collect();
                for rid in rids {
                    let Some(b) = doc.recipe(rid).cloned() else {
                        continue;
                    };
                    let kind = b.kind.map_expressions(&|s| params::rename_in(s, from, to));
                    if kind != b.kind {
                        let mut after = b.clone();
                        after.kind = kind;
                        let p = Patch::Recipe {
                            id: rid,
                            before: Some(b),
                            after: Some(after),
                        };
                        record(doc, patches, p);
                    }
                }
                revalue(doc, patches)?;
            }
        }
        Command::AddDimensionExpr { constraint, expr } => {
            let v = crate::expr::evaluate(expr, doc.parameters())?;
            let mut c = constraint.clone();
            let value = params::dimension_value(&c, v);
            let d = c
                .dimension_mut()
                .ok_or_else(|| DocError::Invalid("not a dimensional constraint".to_owned()))?;
            d.value = value;
            d.driven = false;
            c.validate(doc)?;
            let id = ConstraintId::new(doc.alloc_id()?);
            let p = Patch::Constraint {
                id,
                before: None,
                after: Some(c),
            };
            record(doc, patches, p);
            set_dim_expr(doc, id, Some(expr.clone()), patches);
            out.constraints.push(id);
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
                stroke_styles: Default::default(),
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
            if let Some(v) = props.stroke_styles {
                after.stroke_styles = v;
            }
            Document::check_layer(&after)?;
            if doc.image(*id).is_some() && !matches!(after.operation, Operation::Reference) {
                return Err(DocError::Invalid(
                    "a layer with an image stays a Reference layer".to_owned(),
                ));
            }
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
        Command::ReorderLayer { layer, to } => {
            if doc.layer(*layer).is_none() {
                return Err(DocError::UnknownLayer(*layer));
            }
            let mut order: Vec<LayerId> = doc.layer_order();
            order.retain(|id| id != layer);
            let at = usize::try_from(*to).map_or(order.len(), |t| t.min(order.len()));
            order.insert(at, *layer);
            for (i, id) in order.iter().enumerate() {
                let want = u32::try_from(i)
                    .map_err(|_| DocError::Invalid("too many layers".to_owned()))?;
                let before = doc.layer(*id).ok_or(DocError::UnknownLayer(*id))?.clone();
                if before.export_order != want {
                    let after = Layer {
                        export_order: want,
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
            for rid in &plan.recipes {
                let before = doc.recipe(*rid).cloned();
                record(
                    doc,
                    patches,
                    Patch::Recipe {
                        id: *rid,
                        before,
                        after: None,
                    },
                );
            }
            if let Some(image) = doc.image(*id).cloned() {
                record(
                    doc,
                    patches,
                    Patch::Image {
                        layer: *id,
                        before: Some(image),
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
        Command::AddImage { layer, image } => {
            doc.check_unlocked(*layer)?;
            if doc.image(*layer).is_some() {
                return Err(DocError::Invalid(
                    "this layer already has an image".to_owned(),
                ));
            }
            doc.check_image(*layer, image)?;
            record(
                doc,
                patches,
                Patch::Image {
                    layer: *layer,
                    before: None,
                    after: Some((**image).clone()),
                },
            );
        }
        Command::SetImagePlacement { layer, placement } => {
            let before = doc
                .image(*layer)
                .cloned()
                .ok_or_else(|| DocError::Invalid("this layer has no image".to_owned()))?;
            let after = crate::image::RefImage {
                placement: *placement,
                ..before.clone()
            };
            after.validate()?;
            record(
                doc,
                patches,
                Patch::Image {
                    layer: *layer,
                    before: Some(before),
                    after: Some(after),
                },
            );
        }
        Command::ScaleReference {
            layer,
            about,
            factor,
        } => crate::reference::scale(doc, *layer, *about, *factor, patches)?,
        Command::TransformReference { layer, transform } => {
            crate::reference::transform(doc, *layer, transform, patches)?;
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
        Command::ExpandStyle { entities, tol } => {
            use crate::offset::Out;
            let (groups, sources) = crate::expand::plan(doc, entities, *tol)?;
            let delete = crate::delete::delete_entities(doc, &sources);
            for g in groups {
                let mut ids: Vec<EntityId> = Vec::with_capacity(g.out.len());
                for o in &g.out {
                    let at = |i: usize| {
                        ids.get(i)
                            .copied()
                            .ok_or_else(|| DocError::Invalid("expand: bad output".to_owned()))
                    };
                    let geom = match *o {
                        Out::Point(pos) => Geom::Point { pos },
                        Out::Line(a, b) => Geom::Line {
                            start: at(a)?,
                            end: at(b)?,
                        },
                        Out::Arc(c, a, b) => Geom::Arc {
                            center: at(c)?,
                            start: at(a)?,
                            end: at(b)?,
                        },
                        Out::Circle(c, radius) => Geom::Circle {
                            center: at(c)?,
                            radius,
                        },
                    };
                    let mut added = Applied::default();
                    exec(
                        doc,
                        &Command::AddEntity {
                            layer: g.layer,
                            construction: false,
                            geom,
                            style: g.style,
                        },
                        patches,
                        &mut added,
                    )?;
                    ids.extend(added.entities.iter().copied());
                    out.entities.extend(added.entities);
                }
            }
            if let Some(d) = delete {
                exec(doc, &d, patches, &mut Applied::default())?;
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
        Command::AddGenerated {
            layer,
            origin,
            plugin,
            output,
        } => {
            // Check the payload before allocating anything.
            plugin.validate()?;
            output.validate()?;
            doc.check_unlocked(*layer)?;
            if !(origin.x.is_finite() && origin.y.is_finite()) {
                return Err(DocError::Invalid(
                    "plugin object origin is not finite".into(),
                ));
            }
            let mut frame = Vec::with_capacity(2);
            for pos in [*origin, *origin + Vec2::new(10.0, 0.0)] {
                let id = EntityId::new(doc.alloc_id()?);
                let e = Entity {
                    layer: *layer,
                    construction: false,
                    geom: Geom::Point { pos },
                    style: Style::default(),
                };
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
                frame.push(id);
            }
            let kind = RecipeKind::Generated {
                plugin: plugin.clone(),
                output: output.clone(),
            };
            add_recipe(doc, &kind, &frame, patches, out)?;
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
        Command::EditRecipe { id, kind } => {
            let before = doc.recipe(*id).ok_or(DocError::UnknownRecipe(*id))?.clone();
            if core::mem::discriminant(&before.kind) != core::mem::discriminant(kind) {
                return Err(DocError::Invalid(
                    "a recipe's kind can't change; remove it and add another".to_owned(),
                ));
            }
            if !before.kind.solved() {
                recipe::validate(doc, kind, &before.sources, Some(*id))?;
            } else {
                return Err(DocError::Invalid(
                    "only an offset's parameters can be edited".to_owned(),
                ));
            }
            let mut after = before.clone();
            after.kind = kind.clone();
            record(
                doc,
                patches,
                Patch::Recipe {
                    id: *id,
                    before: Some(before),
                    after: Some(after),
                },
            );
            regenerate_one(doc, *id, patches, None)?;
        }
        Command::SetInstanceSuppressed {
            id,
            index,
            suppressed,
        } => {
            let before = doc.recipe(*id).ok_or(DocError::UnknownRecipe(*id))?.clone();
            if !before.kind.is_pattern() {
                return Err(DocError::Invalid(
                    "only pattern copies can be suppressed".to_owned(),
                ));
            }
            if *index >= before.active {
                return Err(DocError::Invalid(format!(
                    "the pattern has no copy {index}"
                )));
            }
            let mut after = before.clone();
            if *suppressed {
                after.suppressed.insert(*index);
            } else {
                after.suppressed.remove(index);
            }
            if after != before {
                record(
                    doc,
                    patches,
                    Patch::Recipe {
                        id: *id,
                        before: Some(before),
                        after: Some(after),
                    },
                );
                regenerate_one(doc, *id, patches, None)?;
            }
        }
        Command::SetRecipeSuppressed { id, suppressed } => {
            let before = doc.recipe(*id).ok_or(DocError::UnknownRecipe(*id))?.clone();
            // Something built on this feature's copies would lose its input.
            if *suppressed {
                let made: std::collections::BTreeSet<EntityId> = before.instances().collect();
                if let Some((other, r)) = doc
                    .recipes()
                    .iter()
                    .find(|(rid, r)| *rid != id && r.sources.iter().any(|s| made.contains(s)))
                {
                    return Err(DocError::Invalid(format!(
                        "{} {} uses this feature's copies; suppress or delete it first",
                        r.kind.name(),
                        other.get()
                    )));
                }
            }
            let after = crate::recipe::Recipe {
                feature_suppressed: *suppressed,
                ..before.clone()
            };
            if after != before {
                let mirror = matches!(before.kind, RecipeKind::Mirror { .. });
                if mirror && *suppressed {
                    suppress_mirror(doc, &before, patches)?;
                }
                record(
                    doc,
                    patches,
                    Patch::Recipe {
                        id: *id,
                        before: Some(before.clone()),
                        after: Some(after),
                    },
                );
                if mirror && !*suppressed {
                    restore_mirror(doc, &before, patches)?;
                } else {
                    regenerate_one(doc, *id, patches, None)?;
                }
            }
        }
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
    // A plugin object's sources are ordered (origin, direction); every other
    // kind keeps them sorted.
    if !matches!(kind, RecipeKind::Generated { .. }) {
        sources.sort_unstable();
    }
    recipe::validate(doc, kind, &sources, None)?;
    for id in &sources {
        if let Some(e) = doc.entity(*id) {
            doc.check_unlocked(e.layer)?;
        }
    }
    if kind.is_pattern() {
        let rid = RecipeId::new(doc.alloc_id()?);
        record(
            doc,
            patches,
            Patch::Recipe {
                id: rid,
                before: None,
                after: Some(crate::pattern::empty(kind.clone(), sources)),
            },
        );
        crate::pattern::regenerate(doc, rid, patches)?;
        out.entities.extend(
            doc.recipe(rid)
                .map(|r| r.instances().collect::<Vec<_>>())
                .unwrap_or_default(),
        );
        out.recipes.push(rid);
        return Ok(());
    }
    if !kind.solved() {
        // An offset: no copies of the sources; its instances are generated.
        let rid = RecipeId::new(doc.alloc_id()?);
        let recipe = Recipe {
            kind: kind.clone(),
            sources,
            pool: vec![Vec::new()],
            active: 0,
            suppressed: std::collections::BTreeSet::new(),
            feature_suppressed: false,
            constraints: Vec::new(),
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
        regenerate_one(doc, rid, patches, None)?;
        out.entities.extend(
            doc.recipe(rid)
                .map(|r| r.instances().collect::<Vec<_>>())
                .unwrap_or_default(),
        );
        out.recipes.push(rid);
        return Ok(());
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
            style: src.style,
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
    let RecipeKind::Mirror { axis } = kind else {
        return Err(DocError::Invalid("unsupported recipe kind".to_owned()));
    };
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
        suppressed: std::collections::BTreeSet::new(),
        feature_suppressed: false,
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

/// Suppress a mirror (#446): its copies and the constraints it owns go,
/// keeping every ID in the recipe so [`restore_mirror`] brings them back.
/// Refused while a constraint of the user's refers to a copy.
fn suppress_mirror(
    doc: &mut Document,
    r: &Recipe,
    patches: &mut Vec<Patch>,
) -> Result<(), DocError> {
    let copies: Vec<EntityId> = r.instances().collect();
    let owned: std::collections::BTreeSet<ConstraintId> = r.constraints.iter().copied().collect();
    if doc
        .constraints()
        .iter()
        .any(|(cid, c)| !owned.contains(cid) && c.references().iter().any(|e| copies.contains(e)))
    {
        return Err(DocError::Invalid(
            "a constraint holds this mirror's copies; delete it before suppressing".to_owned(),
        ));
    }
    for cid in &r.constraints {
        if let Some(c) = doc.constraint(*cid).cloned() {
            record(
                doc,
                patches,
                Patch::Constraint {
                    id: *cid,
                    before: Some(c),
                    after: None,
                },
            );
        }
    }
    // Curves before the points they are built on.
    let mut order = copies;
    order.sort_by_key(|id| {
        doc.entity(*id)
            .map_or(0, |e| u8::from(e.geom.kind() == EntityKind::Point))
    });
    for id in order {
        if let Some(e) = doc.entity(id).cloned() {
            record(
                doc,
                patches,
                Patch::Entity {
                    id,
                    before: Some(e),
                    after: None,
                },
            );
        }
    }
    Ok(())
}

/// Unsuppress a mirror (#446): the copies again under their kept IDs,
/// reflected from the sources as they are now, and the owned constraints
/// under theirs.
fn restore_mirror(
    doc: &mut Document,
    r: &Recipe,
    patches: &mut Vec<Patch>,
) -> Result<(), DocError> {
    let closure: Vec<EntityId> = recipe::closure(doc, &r.sources).into_iter().collect();
    let Some(set) = r.pool.first().filter(|s| s.len() == closure.len()) else {
        return Err(DocError::Invalid(
            "the mirror's sources changed; delete it and mirror again".to_owned(),
        ));
    };
    let xf = recipe::transform(doc, &r.kind, 0)?;
    let rename = |id: EntityId| {
        closure
            .iter()
            .position(|c| *c == id)
            .and_then(|i| set.get(i).copied())
            .unwrap_or(id)
    };
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
            style: src.style,
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
    let RecipeKind::Mirror { axis } = &r.kind else {
        return Ok(());
    };
    let mut ids = r.constraints.iter();
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
        let Some(cid) = ids.next() else { break };
        c.validate(doc)?;
        record(
            doc,
            patches,
            Patch::Constraint {
                id: *cid,
                before: None,
                after: Some(c),
            },
        );
    }
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
    // A pattern copying this recipe's output would lose its sources (#221).
    for inst in &instances {
        let users: Vec<RecipeId> = doc
            .recipes()
            .iter()
            .filter(|(rid, r)| **rid != id && r.inputs(doc).contains(inst))
            .map(|(rid, _)| *rid)
            .collect();
        if !users.is_empty() {
            return Err(DocError::RecipeInUse {
                entity: *inst,
                recipes: users,
            });
        }
    }
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

pub(crate) fn record(doc: &mut Document, patches: &mut Vec<Patch>, p: Patch) {
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

/// Most undo steps kept; the oldest is dropped (#446, ADR 0019 decision 5).
pub const MAX_HISTORY: usize = 1000;

/// The session's linear history (#446): every step's label, oldest first,
/// and how many of them are applied. Steps from `current` on are the redo
/// steps; doing something new there drops them (linear, ADR 0019).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct History {
    pub steps: Vec<String>,
    pub current: usize,
}

/// A document plus its undo/redo history: the only way to change a document.
#[derive(Debug, Clone)]
pub struct Editor {
    doc: Document,
    undo: Vec<Transaction>,
    redo: Vec<Transaction>,
    /// Re-runs modifier plugins when their sources change (#184); none in
    /// the web build or headless tools that don't load plugins.
    runner: Option<std::sync::Arc<dyn crate::generated::PluginRunner>>,
    /// Features whose last regeneration failed, with the reason (#446,
    /// ADR 0019 decision 5): they keep their last good copies and the
    /// command that changed their input still applies. Session state.
    failures: BTreeMap<RecipeId, String>,
}

impl Editor {
    #[must_use]
    pub fn new(doc: Document) -> Self {
        Self {
            doc,
            undo: Vec::new(),
            redo: Vec::new(),
            runner: None,
            failures: BTreeMap::new(),
        }
    }

    /// Set (or clear) what re-runs modifier plugins (SPEC §15.2).
    pub fn set_plugin_runner(
        &mut self,
        runner: Option<std::sync::Arc<dyn crate::generated::PluginRunner>>,
    ) {
        self.runner = runner;
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
        let mut failures: Option<BTreeMap<RecipeId, String>> = None;
        if let Err(e) = exec(&mut self.doc, cmd, &mut patches, &mut out).and_then(|()| {
            regenerate_all(&mut self.doc, &mut patches, self.runner.as_deref())
                .map(|f| failures = Some(f))
        }) {
            for p in patches.iter().rev() {
                p.set(&mut self.doc, false);
            }
            return Err(e);
        }
        if let Some(f) = failures {
            self.failures = f;
        }
        if !patches.is_empty() {
            self.undo.push(Transaction {
                label: cmd.label(),
                patches,
            });
            self.redo.clear();
            if self.undo.len() > MAX_HISTORY {
                let extra = self.undo.len() - MAX_HISTORY;
                self.undo.drain(..extra);
            }
        }
        Ok(out)
    }

    /// Features whose last regeneration failed, with the reason (#446).
    #[must_use]
    pub fn recipe_failures(&self) -> &BTreeMap<RecipeId, String> {
        &self.failures
    }

    /// The history panel's list (#446).
    #[must_use]
    pub fn history(&self) -> History {
        History {
            steps: self
                .undo
                .iter()
                .chain(self.redo.iter().rev())
                .map(|t| t.label.clone())
                .collect(),
            current: self.undo.len(),
        }
    }

    /// Undo or redo to just after step `step` of [`Self::history`] (0 =
    /// before the first), as one batch: the caller re-solves once, not
    /// once per step. Returns how many steps moved.
    ///
    /// # Errors
    /// [`DocError::NothingToDo`] when `step` is past the newest step.
    pub fn go_to(&mut self, step: usize) -> Result<usize, DocError> {
        if step > self.undo.len() + self.redo.len() {
            return Err(DocError::NothingToDo("history step"));
        }
        let mut moved = 0;
        while self.undo.len() > step {
            self.undo()?;
            moved += 1;
        }
        while self.undo.len() < step {
            self.redo()?;
            moved += 1;
        }
        Ok(moved)
    }

    /// Undo the last step, returning its label. Re-solve afterwards.
    ///
    /// # Errors
    /// [`DocError::NothingToDo`] when there is nothing to undo.
    pub fn undo(&mut self) -> Result<String, DocError> {
        let t = self.undo.pop().ok_or(DocError::NothingToDo("undo"))?;
        // The state undone to regenerated cleanly when it was made.
        self.failures.clear();
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
        self.failures.clear();
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
        mut solve: impl FnMut(&Document, SolvePhase) -> Result<Option<Solution>, DocError>,
    ) -> Result<Applied, DocError> {
        let mut patches = Vec::new();
        let mut out = Applied::default();
        let mut failures: Option<BTreeMap<RecipeId, String>> = None;
        let result = exec(&mut self.doc, cmd, &mut patches, &mut out)
            .and_then(|()| solve(&self.doc, SolvePhase::Main))
            .and_then(|sol| match sol {
                Some(sol) => solution_patches(&mut self.doc, &sol, &mut patches),
                None => Ok(()),
            })
            .and_then(|()| {
                regenerate_all(&mut self.doc, &mut patches, self.runner.as_deref())
                    .map(|f| failures = Some(f))
            })
            // Phase 2 (ADR 0015): constraints that touch pattern instances,
            // solved with the freshly regenerated pattern held fixed.
            .and_then(|()| solve(&self.doc, SolvePhase::Late))
            .and_then(|sol| match sol {
                Some(sol) => solution_patches(&mut self.doc, &sol, &mut patches),
                None => Ok(()),
            });
        if let Err(e) = result {
            for p in patches.iter().rev() {
                p.set(&mut self.doc, false);
            }
            return Err(e);
        }
        if let Some(f) = failures {
            self.failures = f;
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

    /// Run the second solve phase and add its changes to the last undo step
    /// (the one a drag just recorded; ADR 0015).
    pub(crate) fn late_into_last(
        &mut self,
        solve: impl FnOnce(&Document) -> Result<Option<Solution>, DocError>,
    ) -> Result<(), DocError> {
        let Some(sol) = solve(&self.doc)? else {
            return Ok(());
        };
        let mut patches = Vec::new();
        if let Err(e) = solution_patches(&mut self.doc, &sol, &mut patches) {
            for p in patches.iter().rev() {
                p.set(&mut self.doc, false);
            }
            return Err(e);
        }
        if let Some(t) = self.undo.last_mut() {
            t.patches.extend(patches);
        }
        Ok(())
    }

    /// Regenerate non-solved recipes (offsets) and add the changes to the
    /// last undo step, e.g. the one a drag just recorded (#164).
    pub(crate) fn regenerate_into_last(&mut self) -> Result<(), DocError> {
        let mut patches = Vec::new();
        match regenerate_all(&mut self.doc, &mut patches, self.runner.as_deref()) {
            Ok(f) => self.failures = f,
            Err(e) => {
                for p in patches.iter().rev() {
                    p.set(&mut self.doc, false);
                }
                return Err(e);
            }
        }
        if patches.is_empty() {
            return Ok(());
        }
        match self.undo.last_mut() {
            Some(t) => t.patches.extend(patches),
            None => self.undo.push(Transaction {
                label: "Regenerate".to_owned(),
                patches,
            }),
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

/// Rewrites every non-solved recipe's instances from the current geometry
/// (offsets, #164). Recorded in `patches`, so it is part of the same undo
/// step as whatever changed the sources.
pub(crate) fn regenerate_all(
    doc: &mut Document,
    patches: &mut Vec<Patch>,
    runner: Option<&dyn crate::generated::PluginRunner>,
) -> Result<BTreeMap<RecipeId, String>, DocError> {
    let ids: Vec<RecipeId> = doc
        .recipes()
        .iter()
        .filter(|(_, r)| !r.kind.solved())
        .map(|(id, _)| *id)
        .collect();
    // A feature that fails (an offset that collapses, a limit hit) keeps
    // its last good copies and reports why; it never stops the command or
    // the solve (#446, ADR 0019 decision 5).
    let mut failed = BTreeMap::new();
    for id in ids {
        let mark = patches.len();
        if let Err(e) = regenerate_one(doc, id, patches, runner) {
            // Guards that protect other geometry still refuse the command.
            if matches!(
                e,
                DocError::EntityInUse { .. } | DocError::RecipeInUse { .. }
            ) {
                return Err(e);
            }
            for p in patches[mark..].iter().rev() {
                p.set(doc, false);
            }
            patches.truncate(mark);
            failed.insert(id, e.to_string());
        }
    }
    Ok(failed)
}

/// Regenerate one non-solved recipe, whatever its kind.
fn regenerate_one(
    doc: &mut Document,
    id: RecipeId,
    patches: &mut Vec<Patch>,
    runner: Option<&dyn crate::generated::PluginRunner>,
) -> Result<(), DocError> {
    crate::offset::regenerate(doc, id, patches)?;
    crate::pattern::regenerate(doc, id, patches)?;
    crate::generated::regenerate(doc, id, patches, runner)
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
                style: Default::default(),
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
                style: Default::default(),
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
                    style: Default::default(),
                    geom: Geom::Point { pos: Vec2::ZERO },
                },
                Command::AddEntity {
                    layer: L,
                    construction: false,
                    style: Default::default(),
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
                    style: Default::default(),
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
                style: Default::default(),
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

    #[test]
    fn reorder_layer_is_one_step_and_renumbers_every_layer() {
        // #462: a drag in the Browser is one ReorderLayer step.
        let mut ed = Editor::new(Document::new());
        let add = |ed: &mut Editor, name: &str| {
            ed.apply(&Command::AddLayer {
                name: name.to_owned(),
                colour: [0, 0, 0],
                operation: Operation::Draw,
            })
            .unwrap()
            .layers[0]
        };
        let a = add(&mut ed, "A");
        let b = add(&mut ed, "B");
        assert_eq!(ed.document().layer_order(), [L, a, b]);
        let before = ed.document().clone();
        ed.apply(&Command::ReorderLayer { layer: b, to: 0 })
            .unwrap();
        assert_eq!(ed.document().layer_order(), [b, L, a]);
        let orders: Vec<u32> = [b, L, a]
            .iter()
            .map(|id| ed.document().layer(*id).unwrap().export_order)
            .collect();
        assert_eq!(orders, [0, 1, 2]);
        assert_eq!(ed.undo_label(), Some("Reorder layers"));
        ed.undo().unwrap();
        assert!(same_content(ed.document(), &before));
        // Past the end means last; an unknown layer is refused.
        ed.apply(&Command::ReorderLayer { layer: L, to: 99 })
            .unwrap();
        assert_eq!(ed.document().layer_order(), [a, b, L]);
        let ghost = LayerId::new(9_999);
        assert!(
            ed.apply(&Command::ReorderLayer {
                layer: ghost,
                to: 0
            })
            .is_err()
        );
    }

    #[test]
    fn history_jumps_as_one_batch_and_truncates_when_edited() {
        // #446: the History panel's list, jump and linear truncation.
        let mut ed = Editor::new(Document::new());
        for x in 0..4 {
            add_pt(&mut ed, f64::from(x), 0.0);
        }
        let h = ed.history();
        assert_eq!(h.steps.len(), 4);
        assert_eq!(h.current, 4);
        let all = ed.document().clone();
        assert_eq!(ed.go_to(1).unwrap(), 3);
        assert_eq!(ed.document().entities().len(), 1);
        let h = ed.history();
        assert_eq!(
            (h.steps.len(), h.current),
            (4, 1),
            "later steps stay as redo"
        );
        assert_eq!(ed.go_to(4).unwrap(), 3);
        assert!(same_content(ed.document(), &all));
        assert!(ed.go_to(5).is_err());
        assert_eq!(ed.go_to(4).unwrap(), 0);
        // Something new after jumping back drops the later steps.
        ed.go_to(2).unwrap();
        add_pt(&mut ed, 9.0, 9.0);
        let h = ed.history();
        assert_eq!((h.steps.len(), h.current), (3, 3));
        assert!(!ed.can_redo());
    }

    #[test]
    fn history_keeps_the_newest_max_steps() {
        let mut ed = Editor::new(Document::new());
        for x in 0..(MAX_HISTORY + 5) {
            add_pt(&mut ed, x as f64, 0.0);
        }
        let h = ed.history();
        assert_eq!(h.steps.len(), MAX_HISTORY);
        ed.go_to(0).unwrap();
        // The five oldest points can no longer be undone.
        assert_eq!(ed.document().entities().len(), 5);
    }

    #[test]
    fn a_suppressed_feature_drops_its_copies_and_keeps_their_ids() {
        // #446: Suppress from the timeline; restoring brings the same IDs.
        let mut ed = Editor::new(Document::new());
        let p = add_pt(&mut ed, 0.0, 0.0);
        let kind = RecipeKind::Linear {
            dir1: crate::recipe::LinearDir {
                axis: crate::recipe::Axis::X,
                count: "3".into(),
                spacing: "10".into(),
                symmetric: false,
            },
            dir2: None,
        };
        let r = ed
            .apply(&Command::AddRecipe {
                kind,
                sources: vec![p],
            })
            .unwrap()
            .recipes[0];
        let copies: Vec<EntityId> = ed.document().recipe(r).unwrap().instances().collect();
        assert_eq!(copies.len(), 2);
        ed.apply(&Command::SetRecipeSuppressed {
            id: r,
            suppressed: true,
        })
        .unwrap();
        assert_eq!(ed.undo_label(), Some("Suppress feature"));
        assert!(copies.iter().all(|c| ed.document().entity(*c).is_none()));
        assert_eq!(ed.document().recipe(r).unwrap().instances().count(), 0);
        ed.apply(&Command::SetRecipeSuppressed {
            id: r,
            suppressed: false,
        })
        .unwrap();
        let back: Vec<EntityId> = ed.document().recipe(r).unwrap().instances().collect();
        assert_eq!(back, copies, "the same IDs come back");
        assert!(copies.iter().all(|c| ed.document().entity(*c).is_some()));
    }

    #[test]
    fn a_suppressed_mirror_comes_back_with_its_ids() {
        // #446 part 3: mirrors suppress too; the copies and their owned
        // constraints return under the same IDs.
        let mut ed = Editor::new(Document::new());
        let (a, b) = (add_pt(&mut ed, 1.0, 1.0), add_pt(&mut ed, 1.0, 5.0));
        let (p, q) = (add_pt(&mut ed, 0.0, 0.0), add_pt(&mut ed, 0.0, 10.0));
        let line = |ed: &mut Editor, start, end, construction| {
            ed.apply(&Command::AddEntity {
                layer: L,
                construction,
                style: Default::default(),
                geom: Geom::Line { start, end },
            })
            .unwrap()
            .entities[0]
        };
        let axis = line(&mut ed, p, q, true);
        let seg = line(&mut ed, a, b, false);
        let r = ed
            .apply(&Command::AddRecipe {
                kind: RecipeKind::Mirror { axis },
                sources: vec![seg],
            })
            .unwrap()
            .recipes[0];
        let before = ed.document().clone();
        let copies: Vec<EntityId> = before.recipe(r).unwrap().instances().collect();
        let owned = before.recipe(r).unwrap().constraints.clone();
        assert!(!owned.is_empty());
        ed.apply(&Command::SetRecipeSuppressed {
            id: r,
            suppressed: true,
        })
        .unwrap();
        let doc = ed.document();
        assert!(copies.iter().all(|c| doc.entity(*c).is_none()));
        assert!(owned.iter().all(|c| doc.constraint(*c).is_none()));
        ed.apply(&Command::SetRecipeSuppressed {
            id: r,
            suppressed: false,
        })
        .unwrap();
        let doc = ed.document();
        assert_eq!(
            doc.entities(),
            before.entities(),
            "the same copies come back"
        );
        assert!(owned.iter().all(|c| doc.constraint(*c).is_some()));
    }

    /// Memory per undo step (#446, ADR 0019 decision 5), run with
    /// `cargo test -p locus-doc --release -- --ignored --nocapture`.
    /// A 500-entity sketch (250 points, 250 lines); the worst step moves
    /// every point (as Move on a select-all does), a typical one sets one
    /// style. Patches are counted by their inline size; points and lines own
    /// no heap memory, so this is their full cost.
    #[test]
    #[ignore = "measurement: prints memory per undo step"]
    fn memory_per_step() {
        let mut ed = Editor::new(Document::new());
        let pts: Vec<EntityId> = (0..250)
            .map(|i| add_pt(&mut ed, f64::from(i), 0.0))
            .collect();
        for w in pts.windows(2).take(250) {
            ed.apply(&Command::AddEntity {
                layer: L,
                construction: false,
                style: Default::default(),
                geom: Geom::Line {
                    start: w[0],
                    end: w[1],
                },
            })
            .unwrap();
        }
        let step_bytes = |ed: &Editor| {
            ed.undo.last().map_or(0, |t| {
                t.patches.len() * std::mem::size_of::<Patch>() + t.label.capacity()
            })
        };
        let moves: Vec<Command> = pts
            .iter()
            .map(|p| Command::ModifyEntity {
                id: *p,
                geom: Geom::Point {
                    pos: Vec2::new(1.0, 1.0),
                },
            })
            .collect();
        ed.apply(&Command::Compound {
            label: "Move".into(),
            commands: moves,
        })
        .unwrap();
        let worst = step_bytes(&ed);
        ed.apply(&Command::SetStyle {
            entities: vec![pts[0]],
            patch: StylePatch {
                width: Some(0.5),
                ..Default::default()
            },
        })
        .ok();
        let typical = step_bytes(&ed);
        println!(
            "memory per step: Patch = {} B; move 250 points = {} B; one edit = {} B; 1 000 worst steps = {:.1} MB",
            std::mem::size_of::<Patch>(),
            worst,
            typical,
            worst as f64 * 1000.0 / 1.0e6
        );
    }
}
