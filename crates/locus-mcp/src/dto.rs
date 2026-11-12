// SPDX-License-Identifier: AGPL-3.0-or-later

//! Tool arguments, with JSON Schemas for clients (SPEC §16.3).
//!
//! These mirror the §11 command types in a form an AI client can write
//! without knowing IDs in advance: lengths in mm, angles in **degrees**,
//! and references that may name items created earlier in the same call.
//! Converting them to commands is in [`crate::tools`]; nothing here decides
//! anything about the document.

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Map, Value};

/// A reference to a point or curve: an existing entity ID, the `name` of an
/// item created earlier in this call (`"corner"` or `"$corner"`), or, where a
/// point is expected, coordinates `[x, y]` in mm for a new point.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Ref {
    Id(u64),
    Name(String),
    At([f64; 2]),
}

/// Geometry of a new or changed entity.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GeomSpec {
    Point {
        x: f64,
        y: f64,
    },
    Line {
        start: Ref,
        end: Ref,
    },
    Circle {
        center: Ref,
        radius: f64,
    },
    /// Counter-clockwise from `start` to `end` about `center`.
    Arc {
        center: Ref,
        start: Ref,
        end: Ref,
    },
    /// Cubic Bézier; `start_ctrl` is the control point next to `start`.
    Bezier {
        start: Ref,
        start_ctrl: Ref,
        end_ctrl: Ref,
        end: Ref,
    },
}

/// A new entity.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
pub struct NewEntity {
    /// Optional name, so later entities and constraints in this call can refer
    /// to it; the result maps each name to its new ID.
    #[serde(default)]
    pub name: Option<String>,
    /// Construction geometry: drawn dashed, never exported.
    #[serde(default)]
    pub construction: bool,
    #[serde(flatten)]
    pub geom: GeomSpec,
}

/// A constraint (SPEC §5). Lengths in mm, angles in degrees. A dimensional
/// constraint takes a `value`, or an `expr` over document parameters
/// (`"width / 2"`); `driven: true` makes it a reference dimension (measured,
/// not enforced).
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ConstraintSpec {
    Coincident {
        a: Ref,
        b: Ref,
    },
    /// A point lies on a line, circle or arc.
    PointOnCurve {
        point: Ref,
        curve: Ref,
    },
    Horizontal {
        line: Ref,
    },
    Vertical {
        line: Ref,
    },
    Parallel {
        a: Ref,
        b: Ref,
    },
    Perpendicular {
        a: Ref,
        b: Ref,
    },
    Tangent {
        a: Ref,
        b: Ref,
    },
    Collinear {
        a: Ref,
        b: Ref,
    },
    Concentric {
        a: Ref,
        b: Ref,
    },
    /// Equal length (lines) or radius (circles, arcs).
    Equal {
        a: Ref,
        b: Ref,
    },
    /// Points `a` and `b` mirror each other about line `axis`.
    Symmetric {
        a: Ref,
        b: Ref,
        axis: Ref,
    },
    Midpoint {
        point: Ref,
        line: Ref,
    },
    /// Fix a point where it is.
    Fix {
        point: Ref,
    },
    /// Point–point, point–line or parallel line–line distance.
    Distance {
        a: Ref,
        b: Ref,
        #[serde(flatten)]
        dim: DimSpec,
    },
    HorizontalDistance {
        a: Ref,
        b: Ref,
        #[serde(flatten)]
        dim: DimSpec,
    },
    VerticalDistance {
        a: Ref,
        b: Ref,
        #[serde(flatten)]
        dim: DimSpec,
    },
    /// Angle between two lines, in degrees.
    Angle {
        a: Ref,
        b: Ref,
        #[serde(flatten)]
        dim: DimSpec,
    },
    Radius {
        curve: Ref,
        #[serde(flatten)]
        dim: DimSpec,
    },
    Diameter {
        curve: Ref,
        #[serde(flatten)]
        dim: DimSpec,
    },
}

/// The value of a dimensional constraint.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
pub struct DimSpec {
    /// mm, or degrees for an angle. Required unless `expr` is given.
    #[serde(default)]
    pub value: Option<f64>,
    /// An expression over document parameters; its value is used.
    #[serde(default)]
    pub expr: Option<String>,
    /// A reference (driven) dimension: measured, not enforced.
    #[serde(default)]
    pub driven: bool,
}

/// Layer operation (SPEC §4). New layers get the operation's default
/// settings; adjust them in the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OperationSpec {
    Cut,
    Score,
    Engrave,
    Draw,
    DragKnife,
    Reference,
}

impl OperationSpec {
    #[must_use]
    pub fn kind(self) -> locus_doc::OperationKind {
        use locus_doc::OperationKind as K;
        match self {
            OperationSpec::Cut => K::Cut,
            OperationSpec::Score => K::Score,
            OperationSpec::Engrave => K::Engrave,
            OperationSpec::Draw => K::Draw,
            OperationSpec::DragKnife => K::DragKnife,
            OperationSpec::Reference => K::Reference,
        }
    }
}

/// Entity kinds, for filters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum KindSpec {
    Point,
    Line,
    Circle,
    Arc,
    Bezier,
    Text,
}

// ── Read tools ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NoArgs {}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListEntitiesArgs {
    /// Only entities on this layer.
    #[serde(default)]
    pub layer: Option<u64>,
    /// Only entities of this kind.
    #[serde(default)]
    pub kind: Option<KindSpec>,
    /// Only entities whose bounding box overlaps `[min_x, min_y, max_x, max_y]` (mm).
    #[serde(default)]
    pub within: Option<[f64; 4]>,
    /// Skip this many matches (paging).
    #[serde(default)]
    pub offset: Option<usize>,
    /// Return at most this many (default and maximum 500).
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IdArgs {
    pub id: u64,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListConstraintsArgs {
    /// Only constraints that reference this entity.
    #[serde(default)]
    pub entity: Option<u64>,
    #[serde(default)]
    pub offset: Option<usize>,
    /// Default and maximum 500.
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ValidateArgs {
    /// Report what the safe auto-fixes would change (never applied to the
    /// document).
    #[serde(default)]
    pub fix: bool,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PreviewArgs {
    /// Image width in pixels (16–2048, default 800).
    #[serde(default)]
    pub width: Option<u32>,
    /// Image height in pixels (16–2048, default 600).
    #[serde(default)]
    pub height: Option<u32>,
    /// The area to show, `[min_x, min_y, max_x, max_y]` in mm (default:
    /// everything, with a margin).
    #[serde(default)]
    pub region: Option<[f64; 4]>,
}

// ── Write tools ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AddEntitiesArgs {
    /// Layer for every new entity (default: the current layer).
    #[serde(default)]
    pub layer: Option<u64>,
    pub entities: Vec<NewEntity>,
    /// Constraints to add in the same step; they may refer to the new
    /// entities by name.
    #[serde(default)]
    pub constraints: Vec<ConstraintSpec>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ModifyEntityArgs {
    pub id: u64,
    /// New geometry; the kind must stay the same. Moving a point is
    /// `{"type":"point","x":…,"y":…}`; the solver then re-applies constraints.
    #[serde(default)]
    pub geom: Option<GeomSpec>,
    #[serde(default)]
    pub construction: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeleteEntitiesArgs {
    pub ids: Vec<u64>,
    /// Also delete everything that depends on them (curves on deleted points,
    /// constraints). Without it, a still-used entity is refused and the error
    /// lists its users.
    #[serde(default)]
    pub cascade: bool,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AddConstraintsArgs {
    pub constraints: Vec<ConstraintSpec>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IdsArgs {
    pub ids: Vec<u64>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetDimensionArgs {
    /// A dimensional constraint.
    pub id: u64,
    /// mm, or degrees for an angle.
    #[serde(default)]
    pub value: Option<f64>,
    /// An expression over document parameters; `""` removes the expression
    /// and keeps the current value.
    #[serde(default)]
    pub expr: Option<String>,
    #[serde(default)]
    pub driven: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetParameterArgs {
    /// Parameter name (letters, digits, `_`).
    pub name: String,
    /// A number or an expression over other parameters, e.g. `"3"` or `"thickness * 2"`.
    pub expr: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateLayerArgs {
    pub name: String,
    /// `[r, g, b]`, 0–255 (default black).
    #[serde(default)]
    pub colour: Option<[u8; 3]>,
    pub operation: OperationSpec,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetLayerPropsArgs {
    pub id: u64,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub colour: Option<[u8; 3]>,
    /// Changing the operation resets its settings to that operation's defaults.
    #[serde(default)]
    pub operation: Option<OperationSpec>,
    #[serde(default)]
    pub export_order: Option<u32>,
    #[serde(default)]
    pub visible: Option<bool>,
    #[serde(default)]
    pub export_enabled: Option<bool>,
    /// A locked layer's geometry is fixed for the solver and refuses edits.
    #[serde(default)]
    pub locked: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MoveToLayerArgs {
    pub entities: Vec<u64>,
    pub layer: u64,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RunGeneratorArgs {
    /// Plugin ID from `list_plugins`.
    pub plugin: String,
    /// Parameter values by key; keys left out take the plugin's defaults.
    #[serde(default)]
    pub params: Map<String, Value>,
    /// Where to put the generated object's origin, `[x, y]` mm (default `[0, 0]`).
    #[serde(default)]
    pub origin: Option<[f64; 2]>,
    #[serde(default)]
    pub layer: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApplyModifierArgs {
    pub plugin: String,
    #[serde(default)]
    pub params: Map<String, Value>,
    /// The curves to modify (a chain).
    pub sources: Vec<u64>,
}

// ── Export ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExportArgs {
    /// Format ID: `svg`, `pdf`, `dxf`, or an exporter plugin's.
    pub format: String,
    /// Apply the safe auto-fixes to the exported geometry (never the document).
    #[serde(default)]
    pub fix: bool,
    /// Export even when validation finds errors.
    #[serde(default)]
    pub force: bool,
}
