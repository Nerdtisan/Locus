// SPDX-License-Identifier: AGPL-3.0-or-later

//! The tools of SPEC §16.3, as plain functions over a [`Workspace`].
//!
//! Read tools only query the document. Write tools translate their
//! arguments into **one** [`Command::Compound`] labelled `AI: …` and apply
//! it, so one AI action is one undo step (§16.4, §11). Validation,
//! solving and refusals are the document's own; this module only maps
//! names to IDs and degrees to radians.

use std::collections::{BTreeMap, BTreeSet};

use locus_doc::{
    Command, Constraint, ConstraintId, Dimension, Document, Entity, EntityId, EntityKind, Geom,
    LayerId, LayerProps, Sketcher, Status,
};
use locus_geom::Vec2;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};

use crate::dto::{
    AddConstraintsArgs, AddEntitiesArgs, ApplyModifierArgs, ConstraintSpec, CreateLayerArgs,
    DeleteEntitiesArgs, DimSpec, ExportArgs, GeomSpec, IdArgs, IdsArgs, KindSpec,
    ListConstraintsArgs, ListEntitiesArgs, ModifyEntityArgs, MoveToLayerArgs, NoArgs, PreviewArgs,
    Ref, RunGeneratorArgs, SetDimensionArgs, SetLayerPropsArgs, SetParameterArgs, ValidateArgs,
};
use crate::error::ToolError;
use crate::host::Workspace;
use crate::policy::{MAX_ITEMS, Scope, cap};

/// Most items a list tool returns at once.
pub const MAX_PAGE: usize = 500;

/// What a tool returns.
#[derive(Debug, Clone, PartialEq)]
pub enum Output {
    /// Structured JSON (also sent as text for clients that only read text).
    Json(Value),
    /// A file: text formats are sent as text, others base64-encoded.
    File {
        mime: String,
        bytes: Vec<u8>,
        info: Value,
    },
    /// A PNG image (render_preview).
    Image { png: Vec<u8>, info: Value },
}

/// A parsed tool call.
#[derive(Debug, Clone)]
pub enum Call {
    GetDocumentSummary,
    ListEntities(ListEntitiesArgs),
    GetEntity(IdArgs),
    ListConstraints(ListConstraintsArgs),
    GetDofReport,
    GetConflicts,
    ValidateExport(ValidateArgs),
    ListPlugins,
    RenderPreview(PreviewArgs),
    AddEntities(AddEntitiesArgs),
    ModifyEntity(ModifyEntityArgs),
    DeleteEntities(DeleteEntitiesArgs),
    AddConstraints(AddConstraintsArgs),
    RemoveConstraints(IdsArgs),
    SetDimension(SetDimensionArgs),
    SetParameter(SetParameterArgs),
    CreateLayer(CreateLayerArgs),
    SetLayerProps(SetLayerPropsArgs),
    MoveToLayer(MoveToLayerArgs),
    RunGenerator(RunGeneratorArgs),
    ApplyModifier(ApplyModifierArgs),
    Save,
    Export(ExportArgs),
}

fn parse<T: DeserializeOwned>(args: &Map<String, Value>) -> Result<T, ToolError> {
    serde_json::from_value(Value::Object(args.clone()))
        .map_err(|e| ToolError::BadArguments(e.to_string()))
}

impl Call {
    /// Parse `name` with `args`; `None` for an unknown tool.
    ///
    /// # Errors
    /// [`ToolError::BadArguments`] when the arguments don't fit the schema.
    pub fn parse(name: &str, args: &Map<String, Value>) -> Option<Result<Call, ToolError>> {
        Some(match name {
            "get_document_summary" => parse::<NoArgs>(args).map(|_| Call::GetDocumentSummary),
            "list_entities" => parse(args).map(Call::ListEntities),
            "get_entity" => parse(args).map(Call::GetEntity),
            "list_constraints" => parse(args).map(Call::ListConstraints),
            "get_dof_report" => parse::<NoArgs>(args).map(|_| Call::GetDofReport),
            "get_conflicts" => parse::<NoArgs>(args).map(|_| Call::GetConflicts),
            "validate_export" => parse(args).map(Call::ValidateExport),
            "list_plugins" => parse::<NoArgs>(args).map(|_| Call::ListPlugins),
            "render_preview" => parse(args).map(Call::RenderPreview),
            "add_entities" => parse(args).map(Call::AddEntities),
            "modify_entity" => parse(args).map(Call::ModifyEntity),
            "delete_entities" => parse(args).map(Call::DeleteEntities),
            "add_constraints" => parse(args).map(Call::AddConstraints),
            "remove_constraints" => parse(args).map(Call::RemoveConstraints),
            "set_dimension" => parse(args).map(Call::SetDimension),
            "set_parameter" => parse(args).map(Call::SetParameter),
            "create_layer" => parse(args).map(Call::CreateLayer),
            "set_layer_props" => parse(args).map(Call::SetLayerProps),
            "move_to_layer" => parse(args).map(Call::MoveToLayer),
            "run_generator" => parse(args).map(Call::RunGenerator),
            "apply_modifier" => parse(args).map(Call::ApplyModifier),
            "save" => parse::<NoArgs>(args).map(|_| Call::Save),
            "export" => parse(args).map(Call::Export),
            _ => return None,
        })
    }

    /// The scope the call needs (SPEC §16.3 table).
    #[must_use]
    pub fn scope(&self) -> Scope {
        match self {
            Call::GetDocumentSummary
            | Call::ListEntities(_)
            | Call::GetEntity(_)
            | Call::ListConstraints(_)
            | Call::GetDofReport
            | Call::GetConflicts
            | Call::ValidateExport(_)
            | Call::ListPlugins
            | Call::RenderPreview(_) => Scope::Read,
            Call::Export(_) => Scope::Export,
            _ => Scope::Write,
        }
    }

    /// Reject oversized write payloads before anything runs (§16.4).
    ///
    /// # Errors
    /// [`ToolError::TooLarge`].
    pub fn check_caps(&self) -> Result<(), ToolError> {
        match self {
            Call::AddEntities(a) => {
                cap("entities", a.entities.len(), MAX_ITEMS)?;
                cap("constraints", a.constraints.len(), MAX_ITEMS)
            }
            Call::AddConstraints(a) => cap("constraints", a.constraints.len(), MAX_ITEMS),
            Call::DeleteEntities(a) => cap("ids", a.ids.len(), MAX_ITEMS),
            Call::RemoveConstraints(a) => cap("ids", a.ids.len(), MAX_ITEMS),
            Call::MoveToLayer(a) => cap("entities", a.entities.len(), MAX_ITEMS),
            Call::ApplyModifier(a) => cap("sources", a.sources.len(), MAX_ITEMS),
            _ => Ok(()),
        }
    }

    /// One line for the approval prompt and the undo label, built from the
    /// arguments alone.
    #[must_use]
    pub fn describe(&self) -> String {
        let n =
            |k: usize, one: &str, many: &str| format!("{k} {}", if k == 1 { one } else { many });
        match self {
            Call::AddEntities(a) => {
                let mut s = format!("add {}", n(a.entities.len(), "entity", "entities"));
                if !a.constraints.is_empty() {
                    s += &format!(", {}", n(a.constraints.len(), "constraint", "constraints"));
                }
                s
            }
            Call::ModifyEntity(a) => format!("modify entity {}", a.id),
            Call::DeleteEntities(a) => format!(
                "delete {}{}",
                n(a.ids.len(), "entity", "entities"),
                if a.cascade {
                    " and their dependents"
                } else {
                    ""
                }
            ),
            Call::AddConstraints(a) => {
                format!(
                    "add {}",
                    n(a.constraints.len(), "constraint", "constraints")
                )
            }
            Call::RemoveConstraints(a) => {
                format!("remove {}", n(a.ids.len(), "constraint", "constraints"))
            }
            Call::SetDimension(a) => format!("set dimension {}", a.id),
            Call::SetParameter(a) => format!("set parameter {} = {}", a.name, a.expr),
            Call::CreateLayer(a) => format!("create layer \"{}\"", a.name),
            Call::SetLayerProps(a) => format!("change layer {}", a.id),
            Call::MoveToLayer(a) => format!(
                "move {} to layer {}",
                n(a.entities.len(), "entity", "entities"),
                a.layer
            ),
            Call::RunGenerator(a) => format!("run generator plugin {}", a.plugin),
            Call::ApplyModifier(a) => format!("apply modifier plugin {}", a.plugin),
            Call::Save => "save the document to its file".to_owned(),
            Call::Export(a) => format!("export the document as {}", a.format),
            Call::GetDocumentSummary => "read the document summary".to_owned(),
            Call::ListEntities(_) => "list entities".to_owned(),
            Call::GetEntity(a) => format!("read entity {}", a.id),
            Call::ListConstraints(_) => "list constraints".to_owned(),
            Call::GetDofReport => "read the DOF report".to_owned(),
            Call::GetConflicts => "read conflicts".to_owned(),
            Call::ValidateExport(_) => "validate for export".to_owned(),
            Call::ListPlugins => "list plugins".to_owned(),
            Call::RenderPreview(_) => "render a preview".to_owned(),
        }
    }

    /// Run the call against the document.
    ///
    /// # Errors
    /// Whatever the document, a plugin or an exporter rejected. A failed
    /// write leaves the document unchanged.
    pub fn run(&self, ws: &mut dyn Workspace) -> Result<Output, ToolError> {
        let label = format!("AI: {}", self.describe());
        match self {
            Call::GetDocumentSummary => Ok(Output::Json(summary(ws.sketcher()))),
            Call::ListEntities(a) => list_entities(ws.sketcher().document(), a).map(Output::Json),
            Call::GetEntity(a) => get_entity(ws.sketcher().document(), a.id).map(Output::Json),
            Call::ListConstraints(a) => {
                Ok(Output::Json(list_constraints(ws.sketcher().document(), a)))
            }
            Call::GetDofReport => Ok(Output::Json(dof_report(ws.sketcher()))),
            Call::GetConflicts => Ok(Output::Json(conflicts(ws.sketcher()))),
            Call::ValidateExport(a) => Ok(Output::Json(validate(ws.sketcher().document(), a.fix))),
            Call::ListPlugins => Ok(Output::Json(list_plugins(ws))),
            Call::RenderPreview(a) => crate::preview::render(ws.sketcher(), a)
                .map(|(png, info)| Output::Image { png, info }),
            Call::AddEntities(a) => add_entities(ws, a, &label).map(Output::Json),
            Call::ModifyEntity(a) => modify_entity(ws, a, &label).map(Output::Json),
            Call::DeleteEntities(a) => delete_entities(ws, a, &label).map(Output::Json),
            Call::AddConstraints(a) => add_constraints(ws, a, &label).map(Output::Json),
            Call::RemoveConstraints(a) => {
                let cmds = a
                    .ids
                    .iter()
                    .map(|&id| Command::RemoveConstraint {
                        id: ConstraintId::new(id),
                    })
                    .collect();
                commit(ws, &label, cmds, &[]).map(Output::Json)
            }
            Call::SetDimension(a) => set_dimension(ws, a, &label).map(Output::Json),
            Call::SetParameter(a) => commit(
                ws,
                &label,
                vec![Command::SetParameter {
                    name: a.name.clone(),
                    expr: a.expr.clone(),
                }],
                &[],
            )
            .map(Output::Json),
            Call::CreateLayer(a) => commit(
                ws,
                &label,
                vec![Command::AddLayer {
                    name: a.name.clone(),
                    colour: a.colour.unwrap_or([0, 0, 0]),
                    operation: a.operation.kind().default_operation(),
                }],
                &[],
            )
            .map(Output::Json),
            Call::SetLayerProps(a) => set_layer_props(ws, a, &label).map(Output::Json),
            Call::MoveToLayer(a) => commit(
                ws,
                &label,
                vec![Command::MoveToLayer {
                    entities: a.entities.iter().map(|&i| EntityId::new(i)).collect(),
                    layer: LayerId::new(a.layer),
                }],
                &[],
            )
            .map(Output::Json),
            Call::RunGenerator(a) => run_generator(ws, a, &label).map(Output::Json),
            Call::ApplyModifier(a) => apply_modifier(ws, a, &label).map(Output::Json),
            Call::Save => match ws.save() {
                Some(Ok(())) => Ok(Output::Json(json!({ "saved": true }))),
                Some(Err(e)) => Err(ToolError::Unavailable(format!("save failed: {e}"))),
                None => Err(ToolError::Unavailable(
                    "this host saves through its own UI".into(),
                )),
            },
            Call::Export(a) => export(ws, a),
        }
    }
}

// ── Read ────────────────────────────────────────────────────────────────────

fn status_name(s: Status) -> &'static str {
    match s {
        Status::UnderConstrained => "under_constrained",
        Status::FullyConstrained => "fully_constrained",
        Status::OverConstrained => "over_constrained",
    }
}

fn kind_name(k: EntityKind) -> &'static str {
    match k {
        EntityKind::Point => "point",
        EntityKind::Line => "line",
        EntityKind::Circle => "circle",
        EntityKind::Arc => "arc",
        EntityKind::Bezier => "bezier",
        EntityKind::Text => "text",
    }
}

fn spec_kind(k: KindSpec) -> EntityKind {
    match k {
        KindSpec::Point => EntityKind::Point,
        KindSpec::Line => EntityKind::Line,
        KindSpec::Circle => EntityKind::Circle,
        KindSpec::Arc => EntityKind::Arc,
        KindSpec::Bezier => EntityKind::Bezier,
        KindSpec::Text => EntityKind::Text,
    }
}

fn layers_json(doc: &Document) -> Value {
    let mut counts: BTreeMap<LayerId, usize> = BTreeMap::new();
    for (_, e) in doc.entities().iter() {
        *counts.entry(e.layer).or_default() += 1;
    }
    let mut layers: Vec<_> = doc.layers().iter().collect();
    layers.sort_by_key(|(id, l)| (l.export_order, **id));
    Value::Array(
        layers
            .into_iter()
            .map(|(id, l)| {
                json!({
                    "id": id,
                    "name": l.name,
                    "colour": l.colour,
                    "operation": l.operation.kind().name(),
                    "export_order": l.export_order,
                    "visible": l.visible,
                    "locked": l.locked,
                    "export_enabled": l.export_enabled,
                    "entities": counts.get(id).copied().unwrap_or(0),
                })
            })
            .collect(),
    )
}

/// The layer list resource and part of the summary.
#[must_use]
pub fn layers(doc: &Document) -> Value {
    layers_json(doc)
}

/// `get_document_summary`.
#[must_use]
pub fn summary(sk: &Sketcher) -> Value {
    let doc = sk.document();
    let st = sk.state();
    let mut by_kind: BTreeMap<&str, usize> = BTreeMap::new();
    for (_, e) in doc.entities().iter() {
        *by_kind.entry(kind_name(e.geom.kind())).or_default() += 1;
    }
    let params: Map<String, Value> = doc
        .parameters()
        .iter()
        .map(|(n, p)| (n.clone(), json!({ "expr": p.expr, "value": p.value })))
        .collect();
    json!({
        "units": "mm",
        "layers": layers_json(doc),
        "parameters": params,
        "entities": doc.entities().len(),
        "entities_by_kind": by_kind,
        "constraints": doc.constraints().len(),
        "recipes": doc.recipes().len(),
        "booleans": doc.booleans().len(),
        "solve": {
            "converged": st.converged,
            "dof": st.dof,
            "status": status_name(st.status),
            "conflicts": st.conflicts.len(),
            "redundant": st.redundant.len(),
        },
        "undo": sk.editor().undo_label(),
    })
}

fn pos(doc: &Document, id: EntityId) -> Value {
    doc.point_pos(id).map_or(Value::Null, |p| json!([p.x, p.y]))
}

fn geom_json(doc: &Document, g: &Geom) -> Value {
    match g {
        Geom::Point { pos } => json!({ "x": pos.x, "y": pos.y }),
        Geom::Line { start, end } => json!({
            "start": start, "end": end,
            "start_at": pos(doc, *start), "end_at": pos(doc, *end),
        }),
        Geom::Circle { center, radius } => json!({
            "center": center, "radius": radius, "center_at": pos(doc, *center),
        }),
        Geom::Arc { center, start, end } => json!({
            "center": center, "start": start, "end": end,
            "center_at": pos(doc, *center), "start_at": pos(doc, *start), "end_at": pos(doc, *end),
        }),
        Geom::Bezier {
            start,
            start_ctrl,
            end_ctrl,
            end,
        } => json!({
            "start": start, "start_ctrl": start_ctrl, "end_ctrl": end_ctrl, "end": end,
            "start_at": pos(doc, *start), "start_ctrl_at": pos(doc, *start_ctrl),
            "end_ctrl_at": pos(doc, *end_ctrl), "end_at": pos(doc, *end),
        }),
        Geom::Text { anchor, dir, text } => json!({
            "anchor": anchor, "dir": dir, "anchor_at": pos(doc, *anchor),
            "content": text.content, "size": text.size,
        }),
    }
}

fn entity_json(doc: &Document, id: EntityId, e: &Entity) -> Value {
    let mut geom = geom_json(doc, &e.geom);
    if let (Geom::Arc { .. }, Some(o), Some(a)) = (&e.geom, geom.as_object_mut(), doc.arc(id)) {
        o.insert("radius".into(), json!(a.radius()));
    }
    json!({
        "id": id,
        "kind": kind_name(e.geom.kind()),
        "layer": e.layer,
        "construction": e.construction,
        "driven_by_recipe": doc.driven_by(id),
        "geom": geom,
    })
}

/// Axis-aligned bounds `[min_x, min_y, max_x, max_y]` of an entity's
/// defining points (circles and arcs: the full circle).
fn bounds(doc: &Document, id: EntityId, e: &Entity) -> Option<[f64; 4]> {
    let mut pts: Vec<Vec2> = Vec::new();
    let mut grow = |c: Vec2, r: f64| {
        pts.push(Vec2::new(c.x - r, c.y - r));
        pts.push(Vec2::new(c.x + r, c.y + r));
    };
    match &e.geom {
        Geom::Point { pos } => grow(*pos, 0.0),
        Geom::Circle { center, radius } => grow(doc.point_pos(*center)?, *radius),
        Geom::Arc { .. } => {
            let a = doc.arc(id)?;
            grow(a.center, a.radius());
        }
        g => {
            for p in g.point_refs() {
                grow(doc.point_pos(p)?, 0.0);
            }
        }
    }
    let mut b = [
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    ];
    for p in pts {
        b = [b[0].min(p.x), b[1].min(p.y), b[2].max(p.x), b[3].max(p.y)];
    }
    b[0].is_finite().then_some(b)
}

fn page(offset: Option<usize>, limit: Option<usize>) -> (usize, usize) {
    (
        offset.unwrap_or(0),
        limit.unwrap_or(MAX_PAGE).clamp(1, MAX_PAGE),
    )
}

/// `list_entities`.
///
/// # Errors
/// An unknown layer.
pub fn list_entities(doc: &Document, a: &ListEntitiesArgs) -> Result<Value, ToolError> {
    let layer = a.layer.map(LayerId::new);
    if let Some(l) = layer
        && doc.layer(l).is_none()
    {
        return Err(locus_doc::DocError::UnknownLayer(l).into());
    }
    let kind = a.kind.map(spec_kind);
    let (offset, limit) = page(a.offset, a.limit);
    let matches: Vec<_> = doc
        .entities()
        .iter()
        .filter(|(_, e)| layer.is_none_or(|l| e.layer == l))
        .filter(|(_, e)| kind.is_none_or(|k| e.geom.kind() == k))
        .filter(|(id, e)| {
            a.within.is_none_or(|w| {
                bounds(doc, *id, e)
                    .is_some_and(|b| b[0] <= w[2] && b[2] >= w[0] && b[1] <= w[3] && b[3] >= w[1])
            })
        })
        .collect();
    let total = matches.len();
    let items: Vec<Value> = matches
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|(id, e)| entity_json(doc, id, e))
        .collect();
    Ok(json!({ "total": total, "offset": offset, "entities": items }))
}

/// `get_entity`: the entity, plus what references it.
///
/// # Errors
/// An unknown entity.
pub fn get_entity(doc: &Document, id: u64) -> Result<Value, ToolError> {
    let id = EntityId::new(id);
    let e = doc
        .entity(id)
        .ok_or(locus_doc::DocError::UnknownEntity(id))?;
    let mut v = entity_json(doc, id, e);
    let (ents, cons) = doc.dependents(id);
    if let Some(o) = v.as_object_mut() {
        o.insert("used_by_entities".into(), json!(ents));
        o.insert("used_by_constraints".into(), json!(cons));
        o.insert("recipes".into(), json!(doc.recipes_involving(id)));
    }
    Ok(v)
}

fn constraint_json(doc: &Document, id: ConstraintId, c: &Constraint) -> Value {
    let mut v = json!({
        "id": id,
        "type": constraint_type(c),
        "entities": c.references(),
        "owned_by_recipe": doc.constraint_owner(id),
    });
    if let (Some(d), Some(o)) = (c.dimension(), v.as_object_mut()) {
        let angle = matches!(c, Constraint::Angle { .. });
        o.insert(
            "value".into(),
            json!(if angle { d.value.to_degrees() } else { d.value }),
        );
        o.insert("unit".into(), json!(if angle { "deg" } else { "mm" }));
        o.insert("driven".into(), json!(d.driven));
        if let Some(e) = doc.dimension_expr(id) {
            o.insert("expr".into(), json!(e));
        }
    }
    v
}

fn constraint_type(c: &Constraint) -> &'static str {
    match c {
        Constraint::Coincident { .. } => "coincident",
        Constraint::PointOnCurve { .. } => "point_on_curve",
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
        Constraint::HorizontalDistance { .. } => "horizontal_distance",
        Constraint::VerticalDistance { .. } => "vertical_distance",
        Constraint::Angle { .. } => "angle",
        Constraint::Radius { .. } => "radius",
        Constraint::Diameter { .. } => "diameter",
    }
}

/// `list_constraints`.
#[must_use]
pub fn list_constraints(doc: &Document, a: &ListConstraintsArgs) -> Value {
    let entity = a.entity.map(EntityId::new);
    let (offset, limit) = page(a.offset, a.limit);
    let matches: Vec<_> = doc
        .constraints()
        .iter()
        .filter(|(_, c)| entity.is_none_or(|e| c.references().contains(&e)))
        .collect();
    let total = matches.len();
    let items: Vec<Value> = matches
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|(id, c)| constraint_json(doc, *id, c))
        .collect();
    json!({ "total": total, "offset": offset, "constraints": items })
}

fn groups(g: &[locus_doc::ConstraintGroup]) -> Value {
    Value::Array(
        g.iter()
            .map(|g| json!({ "constraints": g.constraints, "entities": g.entities }))
            .collect(),
    )
}

/// `get_dof_report`.
#[must_use]
pub fn dof_report(sk: &Sketcher) -> Value {
    let st = sk.state();
    let doc = sk.document();
    let free: Vec<EntityId> = doc
        .entities()
        .iter()
        .filter(|(id, e)| !e.construction && !st.fully_constrained.contains(id))
        .map(|(id, _)| id)
        .take(MAX_PAGE)
        .collect();
    json!({
        "dof": st.dof,
        "status": status_name(st.status),
        "converged": st.converged,
        "fully_constrained": st.fully_constrained.len(),
        "not_fully_constrained": free,
    })
}

/// `get_conflicts`.
#[must_use]
pub fn conflicts(sk: &Sketcher) -> Value {
    let st = sk.state();
    json!({
        "status": status_name(st.status),
        "conflicts": groups(&st.conflicts),
        "redundant": groups(&st.redundant),
    })
}

fn issue_kind(k: locus_export::IssueKind) -> &'static str {
    use locus_export::IssueKind as K;
    match k {
        K::OpenPath => "open_path",
        K::SelfIntersection => "self_intersection",
        K::DuplicateSegment => "duplicate_segment",
        K::Degenerate => "degenerate",
    }
}

fn report_json(p: &locus_export::Prepared) -> Value {
    let c = p.report.counts();
    let issues: Vec<Value> = p
        .report
        .issues
        .iter()
        .map(|i| {
            json!({
                "severity": match i.severity {
                    locus_export::Severity::Error => "error",
                    locus_export::Severity::Warning => "warning",
                },
                "kind": issue_kind(i.kind),
                "layer": i.layer,
                "entities": i.entities,
                "message": i.message,
            })
        })
        .collect();
    let fixes: Vec<Value> = p
        .fixes
        .iter()
        .map(|f| json!({ "layer": f.layer, "entities": f.entities, "message": f.message }))
        .collect();
    json!({
        "errors": c.errors,
        "warnings": c.warnings,
        "blocked": p.blocked(),
        "issues": issues,
        "fixes": fixes,
    })
}

/// `validate_export` (SPEC §8).
#[must_use]
pub fn validate(doc: &Document, fix: bool) -> Value {
    report_json(&locus_export::prepare(doc, fix))
}

/// `list_plugins` and the plugin catalogue resource.
pub fn list_plugins(ws: &mut dyn Workspace) -> Value {
    let Some(p) = ws.plugins() else {
        return json!({ "available": false, "plugins": [] });
    };
    let items: Vec<Value> = p
        .list()
        .into_iter()
        .map(|e| {
            json!({
                "id": e.id, "name": e.name, "version": e.version, "kinds": e.kinds,
                "generator_params": e.generator_params,
                "modifier_params": e.modifier_params,
            })
        })
        .collect();
    json!({ "available": true, "plugins": items })
}

// ── Write ───────────────────────────────────────────────────────────────────

/// Builds one compound, predicting the IDs it allocates (ADR-0003: one
/// counter, every add takes the next) so items can refer to earlier ones.
struct Plan {
    next: u64,
    layer: LayerId,
    cmds: Vec<Command>,
    names: BTreeMap<String, EntityId>,
    /// Which new entities have names, by position among created entities.
    named: Vec<(String, usize)>,
    entities: usize,
}

fn local(name: &str) -> &str {
    name.strip_prefix('$').unwrap_or(name)
}

impl Plan {
    fn new(doc: &Document, layer: LayerId) -> Self {
        Self {
            next: doc.next_id(),
            layer,
            cmds: Vec::new(),
            names: BTreeMap::new(),
            named: Vec::new(),
            entities: 0,
        }
    }

    fn alloc(&mut self) -> u64 {
        let id = self.next;
        self.next += 1;
        id
    }

    fn entity(&mut self, construction: bool, geom: Geom) -> EntityId {
        self.cmds.push(Command::AddEntity {
            layer: self.layer,
            construction,
            geom,
        });
        self.entities += 1;
        EntityId::new(self.alloc())
    }

    /// An existing or named entity.
    fn existing(&self, r: &Ref) -> Result<EntityId, ToolError> {
        match r {
            Ref::Id(id) => Ok(EntityId::new(*id)),
            Ref::Name(n) => self.names.get(local(n)).copied().ok_or_else(|| {
                ToolError::BadArguments(format!("`{n}` names nothing created earlier in this call"))
            }),
            Ref::At(_) => Err(ToolError::BadArguments(
                "coordinates can only stand for a point".into(),
            )),
        }
    }

    /// A point: existing, named, or new at the given coordinates.
    fn point(&mut self, r: &Ref, construction: bool) -> Result<EntityId, ToolError> {
        match r {
            Ref::At([x, y]) => Ok(self.entity(
                construction,
                Geom::Point {
                    pos: Vec2::new(*x, *y),
                },
            )),
            r => self.existing(r),
        }
    }

    fn geom(&mut self, g: &GeomSpec, construction: bool) -> Result<Geom, ToolError> {
        Ok(match g {
            GeomSpec::Point { x, y } => Geom::Point {
                pos: Vec2::new(*x, *y),
            },
            GeomSpec::Line { start, end } => Geom::Line {
                start: self.point(start, construction)?,
                end: self.point(end, construction)?,
            },
            GeomSpec::Circle { center, radius } => Geom::Circle {
                center: self.point(center, construction)?,
                radius: *radius,
            },
            GeomSpec::Arc { center, start, end } => Geom::Arc {
                center: self.point(center, construction)?,
                start: self.point(start, construction)?,
                end: self.point(end, construction)?,
            },
            GeomSpec::Bezier {
                start,
                start_ctrl,
                end_ctrl,
                end,
            } => Geom::Bezier {
                start: self.point(start, construction)?,
                start_ctrl: self.point(start_ctrl, construction)?,
                end_ctrl: self.point(end_ctrl, construction)?,
                end: self.point(end, construction)?,
            },
        })
    }

    fn name(&mut self, name: &str, id: EntityId) -> Result<(), ToolError> {
        let n = local(name).to_owned();
        if n.is_empty() || self.names.insert(n.clone(), id).is_some() {
            return Err(ToolError::BadArguments(format!(
                "name `{name}` is empty or used twice"
            )));
        }
        self.named.push((n, self.entities - 1));
        Ok(())
    }

    fn constraint(&mut self, c: &ConstraintSpec) -> Result<(), ToolError> {
        use ConstraintSpec as S;
        let e = |p: &Self, r: &Ref| p.existing(r);
        let dim = |d: &DimSpec, angle: bool| -> Result<(Dimension, Option<String>), ToolError> {
            if d.expr.is_some() && d.driven {
                return Err(ToolError::BadArguments(
                    "a driven dimension can't have an expression".into(),
                ));
            }
            let value = match (d.value, &d.expr) {
                (Some(v), _) => v,
                (None, Some(_)) => 1.0,
                // Driven: the solver writes the measured value back.
                (None, None) if d.driven => 1.0,
                (None, None) => {
                    return Err(ToolError::BadArguments(
                        "a driving dimension needs a `value` or an `expr`".into(),
                    ));
                }
            };
            Ok((
                Dimension {
                    value: if angle { value.to_radians() } else { value },
                    driven: d.driven,
                },
                d.expr.clone(),
            ))
        };
        let (constraint, expr) = match c {
            S::Coincident { a, b } => (
                Constraint::Coincident {
                    a: e(self, a)?,
                    b: e(self, b)?,
                },
                None,
            ),
            S::PointOnCurve { point, curve } => (
                Constraint::PointOnCurve {
                    point: e(self, point)?,
                    curve: e(self, curve)?,
                },
                None,
            ),
            S::Horizontal { line } => (
                Constraint::Horizontal {
                    line: e(self, line)?,
                },
                None,
            ),
            S::Vertical { line } => (
                Constraint::Vertical {
                    line: e(self, line)?,
                },
                None,
            ),
            S::Parallel { a, b } => (
                Constraint::Parallel {
                    a: e(self, a)?,
                    b: e(self, b)?,
                },
                None,
            ),
            S::Perpendicular { a, b } => (
                Constraint::Perpendicular {
                    a: e(self, a)?,
                    b: e(self, b)?,
                },
                None,
            ),
            S::Tangent { a, b } => (
                Constraint::Tangent {
                    a: e(self, a)?,
                    b: e(self, b)?,
                },
                None,
            ),
            S::Collinear { a, b } => (
                Constraint::Collinear {
                    a: e(self, a)?,
                    b: e(self, b)?,
                },
                None,
            ),
            S::Concentric { a, b } => (
                Constraint::Concentric {
                    a: e(self, a)?,
                    b: e(self, b)?,
                },
                None,
            ),
            S::Equal { a, b } => (
                Constraint::Equal {
                    a: e(self, a)?,
                    b: e(self, b)?,
                },
                None,
            ),
            S::Symmetric { a, b, axis } => (
                Constraint::Symmetric {
                    a: e(self, a)?,
                    b: e(self, b)?,
                    axis: e(self, axis)?,
                },
                None,
            ),
            S::Midpoint { point, line } => (
                Constraint::Midpoint {
                    point: e(self, point)?,
                    line: e(self, line)?,
                },
                None,
            ),
            S::Fix { point } => (
                Constraint::Fix {
                    point: e(self, point)?,
                },
                None,
            ),
            S::Distance { a, b, dim: d } => {
                let (dim, x) = dim(d, false)?;
                (
                    Constraint::Distance {
                        a: e(self, a)?,
                        b: e(self, b)?,
                        dim,
                    },
                    x,
                )
            }
            S::HorizontalDistance { a, b, dim: d } => {
                let (dim, x) = dim(d, false)?;
                (
                    Constraint::HorizontalDistance {
                        a: e(self, a)?,
                        b: e(self, b)?,
                        dim,
                    },
                    x,
                )
            }
            S::VerticalDistance { a, b, dim: d } => {
                let (dim, x) = dim(d, false)?;
                (
                    Constraint::VerticalDistance {
                        a: e(self, a)?,
                        b: e(self, b)?,
                        dim,
                    },
                    x,
                )
            }
            S::Angle { a, b, dim: d } => {
                let (dim, x) = dim(d, true)?;
                (
                    Constraint::Angle {
                        a: e(self, a)?,
                        b: e(self, b)?,
                        dim,
                    },
                    x,
                )
            }
            S::Radius { curve, dim: d } => {
                let (dim, x) = dim(d, false)?;
                (
                    Constraint::Radius {
                        curve: e(self, curve)?,
                        dim,
                    },
                    x,
                )
            }
            S::Diameter { curve, dim: d } => {
                let (dim, x) = dim(d, false)?;
                (
                    Constraint::Diameter {
                        curve: e(self, curve)?,
                        dim,
                    },
                    x,
                )
            }
        };
        self.cmds.push(match expr {
            Some(expr) => Command::AddDimensionExpr { constraint, expr },
            None => Command::AddConstraint { constraint },
        });
        self.alloc();
        Ok(())
    }
}

fn target_layer(ws: &dyn Workspace, layer: Option<u64>) -> Result<LayerId, ToolError> {
    let doc = ws.sketcher().document();
    if let Some(l) = layer {
        let l = LayerId::new(l);
        return if doc.layer(l).is_some() {
            Ok(l)
        } else {
            Err(locus_doc::DocError::UnknownLayer(l).into())
        };
    }
    ws.default_layer()
        .filter(|l| doc.layer(*l).is_some())
        .or_else(|| {
            doc.layers()
                .iter()
                .min_by_key(|(id, l)| (l.export_order, **id))
                .map(|(id, _)| *id)
        })
        .ok_or_else(|| ToolError::BadArguments("the document has no layer".into()))
}

/// Apply `cmds` as one labelled undo step; report what was created and the
/// new solve state.
fn commit(
    ws: &mut dyn Workspace,
    label: &str,
    cmds: Vec<Command>,
    named: &[(String, usize)],
) -> Result<Value, ToolError> {
    if cmds.is_empty() {
        return Err(ToolError::BadArguments("nothing to do".into()));
    }
    let applied = ws.apply(&Command::Compound {
        label: label.to_owned(),
        commands: cmds,
    })?;
    let names: Map<String, Value> = named
        .iter()
        .filter_map(|(n, i)| applied.entities.get(*i).map(|id| (n.clone(), json!(id))))
        .collect();
    let st = ws.sketcher().state();
    Ok(json!({
        "undo_label": label,
        "created": {
            "entities": applied.entities,
            "constraints": applied.constraints,
            "layers": applied.layers,
            "recipes": applied.recipes,
        },
        "names": names,
        "solve": {
            "converged": st.converged,
            "dof": st.dof,
            "status": status_name(st.status),
        },
    }))
}

fn add_entities(
    ws: &mut dyn Workspace,
    a: &AddEntitiesArgs,
    label: &str,
) -> Result<Value, ToolError> {
    let layer = target_layer(ws, a.layer)?;
    let mut plan = Plan::new(ws.sketcher().document(), layer);
    for e in &a.entities {
        let geom = plan.geom(&e.geom, e.construction)?;
        let id = plan.entity(e.construction, geom);
        if let Some(n) = &e.name {
            plan.name(n, id)?;
        }
    }
    for c in &a.constraints {
        plan.constraint(c)?;
    }
    let Plan { cmds, named, .. } = plan;
    commit(ws, label, cmds, &named)
}

fn add_constraints(
    ws: &mut dyn Workspace,
    a: &AddConstraintsArgs,
    label: &str,
) -> Result<Value, ToolError> {
    let layer = target_layer(ws, None)?;
    let mut plan = Plan::new(ws.sketcher().document(), layer);
    for c in &a.constraints {
        plan.constraint(c)?;
    }
    let cmds = plan.cmds;
    commit(ws, label, cmds, &[])
}

fn modify_entity(
    ws: &mut dyn Workspace,
    a: &ModifyEntityArgs,
    label: &str,
) -> Result<Value, ToolError> {
    let id = EntityId::new(a.id);
    let doc = ws.sketcher().document();
    let e = doc
        .entity(id)
        .ok_or(locus_doc::DocError::UnknownEntity(id))?;
    let mut plan = Plan::new(doc, e.layer);
    let construction = e.construction;
    if let Some(g) = &a.geom {
        let geom = plan.geom(g, construction)?;
        plan.cmds.push(Command::ModifyEntity { id, geom });
    }
    if a.construction.is_some_and(|c| c != construction) {
        plan.cmds.push(Command::ToggleConstruction { id });
    }
    let cmds = plan.cmds;
    commit(ws, label, cmds, &[])
}

fn delete_entities(
    ws: &mut dyn Workspace,
    a: &DeleteEntitiesArgs,
    label: &str,
) -> Result<Value, ToolError> {
    let doc = ws.sketcher().document();
    let mut ents: BTreeSet<EntityId> = a.ids.iter().map(|&i| EntityId::new(i)).collect();
    let mut cons: BTreeSet<ConstraintId> = BTreeSet::new();
    if a.cascade {
        let mut todo: Vec<EntityId> = ents.iter().copied().collect();
        while let Some(id) = todo.pop() {
            let (e, c) = doc.dependents(id);
            cons.extend(c);
            for d in e {
                if ents.insert(d) {
                    todo.push(d);
                }
            }
            cap("entities to delete", ents.len(), MAX_ITEMS * 4)?;
        }
    }
    // Constraints first, then curves, then the points they were built on.
    let mut cmds: Vec<Command> = cons
        .into_iter()
        .map(|id| Command::RemoveConstraint { id })
        .collect();
    let is_point = |id: &EntityId| {
        doc.entity(*id)
            .is_some_and(|e| e.geom.kind() == EntityKind::Point)
    };
    let (points, curves): (Vec<EntityId>, Vec<EntityId>) = ents.into_iter().partition(is_point);
    cmds.extend(
        curves
            .into_iter()
            .chain(points)
            .map(|id| Command::DeleteEntity { id }),
    );
    commit(ws, label, cmds, &[])
}

fn set_dimension(
    ws: &mut dyn Workspace,
    a: &SetDimensionArgs,
    label: &str,
) -> Result<Value, ToolError> {
    let id = ConstraintId::new(a.id);
    let c = ws
        .sketcher()
        .document()
        .constraint(id)
        .ok_or(locus_doc::DocError::UnknownConstraint(id))?;
    let angle = matches!(c, Constraint::Angle { .. });
    let mut cmds = Vec::new();
    if a.driven == Some(false) {
        cmds.push(Command::SetDriven { id, driven: false });
    }
    if let Some(v) = a.value {
        cmds.push(Command::SetDimension {
            id,
            value: if angle { v.to_radians() } else { v },
        });
    }
    if let Some(x) = &a.expr {
        cmds.push(Command::SetDimensionExpr {
            id,
            expr: (!x.trim().is_empty()).then(|| x.clone()),
        });
    }
    if a.driven == Some(true) {
        cmds.push(Command::SetDriven { id, driven: true });
    }
    commit(ws, label, cmds, &[])
}

fn set_layer_props(
    ws: &mut dyn Workspace,
    a: &SetLayerPropsArgs,
    label: &str,
) -> Result<Value, ToolError> {
    let id = LayerId::new(a.id);
    let props = LayerProps {
        name: a.name.clone(),
        colour: a.colour,
        operation: a.operation.map(|o| o.kind().default_operation()),
        export_order: a.export_order,
        visible: a.visible,
        export_enabled: a.export_enabled,
    };
    let mut cmds = Vec::new();
    if a.locked == Some(false) {
        cmds.push(Command::SetLayerLocked { id, locked: false });
    }
    if props != LayerProps::default() {
        cmds.push(Command::SetLayerProps { id, props });
    }
    if a.locked == Some(true) {
        cmds.push(Command::SetLayerLocked { id, locked: true });
    }
    commit(ws, label, cmds, &[])
}

fn run_generator(
    ws: &mut dyn Workspace,
    a: &RunGeneratorArgs,
    label: &str,
) -> Result<Value, ToolError> {
    let layer = target_layer(ws, a.layer)?;
    let plugins = ws
        .plugins()
        .ok_or_else(|| ToolError::Plugin("no plugins are available in this host".into()))?;
    let (plugin, output) = plugins
        .generate(&a.plugin, &a.params)
        .map_err(ToolError::Plugin)?;
    let [x, y] = a.origin.unwrap_or([0.0, 0.0]);
    commit(
        ws,
        label,
        vec![Command::AddGenerated {
            layer,
            origin: Vec2::new(x, y),
            plugin,
            output,
        }],
        &[],
    )
}

fn apply_modifier(
    ws: &mut dyn Workspace,
    a: &ApplyModifierArgs,
    label: &str,
) -> Result<Value, ToolError> {
    use locus_doc::generated::{input_hash, modifier_input};
    let sources: Vec<EntityId> = a.sources.iter().map(|&i| EntityId::new(i)).collect();
    let input = modifier_input(ws.sketcher().document(), &sources)?;
    let plugins = ws
        .plugins()
        .ok_or_else(|| ToolError::Plugin("no plugins are available in this host".into()))?;
    let (plugin, output) = plugins
        .modify(&a.plugin, &a.params, &input)
        .map_err(ToolError::Plugin)?;
    let kind = locus_doc::RecipeKind::Modified {
        plugin,
        input_hash: input_hash(&input),
        output,
    };
    commit(ws, label, vec![Command::AddRecipe { kind, sources }], &[])
}

// ── Export ──────────────────────────────────────────────────────────────────

/// `export`: the bytes, never a file (§16.4: no arbitrary path writes).
fn export(ws: &mut dyn Workspace, a: &ExportArgs) -> Result<Output, ToolError> {
    let exporter = ws.exporters().get(&a.format).ok_or_else(|| {
        ToolError::BadArguments(format!(
            "unknown format `{}` (one of: {})",
            a.format,
            ws.exporters().ids().join(", ")
        ))
    })?;
    let prepared = locus_export::prepare(ws.sketcher().document(), a.fix);
    let report = report_json(&prepared);
    if prepared.blocked() && !a.force {
        return Err(ToolError::Export(format!(
            "blocked by {} validation error(s); fix them, pass fix:true, or force:true (report: {report})",
            prepared.report.counts().errors
        )));
    }
    let out = exporter
        .export(&prepared.model)
        .map_err(|e| ToolError::Export(e.to_string()))?;
    Ok(Output::File {
        info: json!({
            "format": a.format,
            "mime": out.mime,
            "extension": out.extension,
            "bytes": out.bytes.len(),
            "validation": report,
        }),
        mime: out.mime,
        bytes: out.bytes,
    })
}
