// SPDX-License-Identifier: AGPL-3.0-or-later

//! The `.locus` native file format (SPEC §10, ADR-0004: RON).
//!
//! * Deterministic: every map is a `BTreeMap`, the pretty-printer settings are
//!   fixed here, so an unchanged document saves to identical bytes.
//! * Stores solved positions as well as constraints (SPEC §10).
//! * Versioned: the `format_version` header is read first, then the file is
//!   parsed into *that version's* frozen structs and migrated step by step to
//!   the current one (`vN → vN+1`). Never edit a frozen `vN` struct; add `vN+1`
//!   and a migration, plus a fixture under `tests/fixtures/`.
//! * Untrusted input: loading validates everything and returns a structured
//!   [`FileError`]; it never panics.

use crate::boolean::Boolean;
use crate::constraint::Constraint;
use crate::document::{Document, EntityStore, Layer, Parameter, View};
use crate::entity::{Entity, Geom};
use crate::error::DocError;
use crate::ids::{BooleanId, ConstraintId, EntityId, LayerId, MAX_ID, RecipeId};
use crate::operation::{CutParams, DragKnifeParams, EngraveParams, Operation};
use crate::recipe::Recipe;
use locus_geom::Unit;
use ron::ser::PrettyConfig;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use thiserror::Error;

/// The version this build writes.
pub const CURRENT_VERSION: u32 = 30;

/// Upper bound on accepted input, to bound memory on hostile files.
pub const MAX_FILE_BYTES: usize = 256 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Error)]
pub enum FileError {
    #[error("file is too large ({0} bytes)")]
    TooLarge(usize),
    #[error("not a valid .locus file: {0}")]
    Parse(String),
    #[error("file format version {found} is newer than this build understands ({supported})")]
    FutureVersion { found: u32, supported: u32 },
    #[error("unsupported file format version {0}")]
    UnknownVersion(u32),
    #[error("document is invalid: {0}")]
    Invalid(String),
    #[error("cannot save: {0}")]
    Unsaveable(String),
}

impl From<DocError> for FileError {
    fn from(e: DocError) -> Self {
        FileError::Invalid(e.to_string())
    }
}

/// Just enough of any version to find out which one it is. Unknown fields are
/// ignored by serde.
#[derive(Deserialize)]
struct Header {
    format_version: u32,
}

/// v1 operation: no parameters. Frozen.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
enum OperationV1 {
    Cut,
    Score,
    Engrave,
    Draw,
}

/// v1 layer. Frozen.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct LayerV1 {
    name: String,
    colour: [u8; 3],
    operation: OperationV1,
    export_order: u32,
    visible: bool,
    locked: bool,
    export_enabled: bool,
}

/// v2–v12 pen-plotter parameters, on the layer. Frozen.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
struct DrawParamsV2 {
    pen_number: u32,
    pen_width: f64,
}

impl Default for DrawParamsV2 {
    fn default() -> Self {
        Self {
            pen_number: 1,
            pen_width: crate::style::DEFAULT_WIDTH,
        }
    }
}

/// v2–v12 operation: Draw carries the pen. Frozen.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
enum OperationV2 {
    Cut(CutParams),
    Score,
    Engrave(EngraveParams),
    Draw(DrawParamsV2),
    DragKnife(DragKnifeParams),
    Reference,
}

/// v2–v12 layer. Frozen.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct LayerV2 {
    name: String,
    colour: [u8; 3],
    operation: OperationV2,
    export_order: u32,
    visible: bool,
    locked: bool,
    export_enabled: bool,
}

impl From<LayerV2> for Layer {
    /// v12 → v13: the pen leaves the layer (SPEC §7, #287); the migration
    /// copies it onto the layer's entities first ([`FileV13`]).
    fn from(l: LayerV2) -> Self {
        Layer {
            name: l.name,
            colour: l.colour,
            operation: match l.operation {
                OperationV2::Cut(p) => Operation::Cut(p),
                OperationV2::Score => Operation::Score,
                OperationV2::Engrave(p) => Operation::Engrave(p),
                OperationV2::Draw(_) => Operation::Draw,
                OperationV2::DragKnife(p) => Operation::DragKnife(p),
                OperationV2::Reference => Operation::Reference,
            },
            export_order: l.export_order,
            visible: l.visible,
            locked: l.locked,
            export_enabled: l.export_enabled,
            stroke_styles: Default::default(),
        }
    }
}

impl From<LayerV1> for LayerV2 {
    /// v1 → v2: layers get the default parameters of their operation (SPEC §7).
    fn from(l: LayerV1) -> Self {
        LayerV2 {
            name: l.name,
            colour: l.colour,
            operation: match l.operation {
                OperationV1::Cut => OperationV2::Cut(CutParams::default()),
                OperationV1::Score => OperationV2::Score,
                OperationV1::Engrave => OperationV2::Engrave(EngraveParams::default()),
                OperationV1::Draw => OperationV2::Draw(DrawParamsV2::default()),
            },
            export_order: l.export_order,
            visible: l.visible,
            locked: l.locked,
            export_enabled: l.export_enabled,
        }
    }
}

/// Version 1, frozen. See the module docs before touching this.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct FileV1 {
    format_version: u32,
    units: Unit,
    next_id: u64,
    parameters: BTreeMap<String, Parameter>,
    layers: BTreeMap<LayerId, LayerV1>,
    entities: BTreeMap<EntityId, Entity>,
    constraints: BTreeMap<ConstraintId, Constraint>,
    #[serde(default)]
    view: Option<View>,
}

impl From<FileV1> for FileV2 {
    fn from(f: FileV1) -> Self {
        FileV2 {
            format_version: 2,
            units: f.units,
            next_id: f.next_id,
            parameters: f.parameters,
            layers: f.layers.into_iter().map(|(i, l)| (i, l.into())).collect(),
            entities: f.entities,
            constraints: f.constraints,
            view: f.view,
        }
    }
}

/// Version 2, frozen: layer operations carry parameters (SPEC §7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct FileV2 {
    format_version: u32,
    units: Unit,
    next_id: u64,
    parameters: BTreeMap<String, Parameter>,
    layers: BTreeMap<LayerId, LayerV2>,
    entities: BTreeMap<EntityId, Entity>,
    constraints: BTreeMap<ConstraintId, Constraint>,
    #[serde(default)]
    view: Option<View>,
}

impl From<FileV2> for FileV3 {
    /// v2 → v3: no boolean recipes yet.
    fn from(f: FileV2) -> Self {
        FileV3 {
            format_version: 3,
            units: f.units,
            next_id: f.next_id,
            parameters: f.parameters,
            layers: f.layers,
            entities: f.entities,
            constraints: f.constraints,
            booleans: BTreeMap::new(),
            view: f.view,
        }
    }
}

/// Version 3: boolean recipes (SPEC §6.3, #123).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct FileV3 {
    format_version: u32,
    units: Unit,
    next_id: u64,
    parameters: BTreeMap<String, Parameter>,
    layers: BTreeMap<LayerId, LayerV2>,
    entities: BTreeMap<EntityId, Entity>,
    constraints: BTreeMap<ConstraintId, Constraint>,
    booleans: BTreeMap<BooleanId, Boolean>,
    #[serde(default)]
    view: Option<View>,
}

impl From<FileV3> for FileV4 {
    /// v3 → v4: nothing to convert; v4 only adds the cubic Bézier entity
    /// (SPEC §4), which no v3 file can contain.
    fn from(f: FileV3) -> Self {
        FileV4 {
            format_version: 4,
            units: f.units,
            next_id: f.next_id,
            parameters: f.parameters,
            layers: f.layers,
            entities: f.entities,
            constraints: f.constraints,
            booleans: f.booleans,
            view: f.view,
        }
    }
}

/// Version 4: cubic Bézier entities and their G1 tangency (SPEC §4, #159).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct FileV4 {
    format_version: u32,
    units: Unit,
    next_id: u64,
    parameters: BTreeMap<String, Parameter>,
    layers: BTreeMap<LayerId, LayerV2>,
    entities: BTreeMap<EntityId, Entity>,
    constraints: BTreeMap<ConstraintId, Constraint>,
    booleans: BTreeMap<BooleanId, Boolean>,
    #[serde(default)]
    view: Option<View>,
}

impl From<FileV4> for FileV5 {
    /// v4 → v5: no recipes yet.
    fn from(f: FileV4) -> Self {
        FileV5 {
            format_version: 5,
            units: f.units,
            next_id: f.next_id,
            parameters: f.parameters,
            layers: f.layers,
            entities: f.entities,
            constraints: f.constraints,
            booleans: f.booleans,
            recipes: BTreeMap::new(),
            view: f.view,
        }
    }
}

/// Version 5: associative recipes — mirror and patterns (SPEC §6.2.1, #166).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct FileV5 {
    format_version: u32,
    units: Unit,
    next_id: u64,
    parameters: BTreeMap<String, Parameter>,
    layers: BTreeMap<LayerId, LayerV2>,
    entities: BTreeMap<EntityId, Entity>,
    constraints: BTreeMap<ConstraintId, Constraint>,
    booleans: BTreeMap<BooleanId, Boolean>,
    recipes: BTreeMap<RecipeId, Recipe>,
    #[serde(default)]
    view: Option<View>,
}

impl From<FileV5> for FileV6 {
    /// v5 → v6: no dimension expressions yet.
    fn from(f: FileV5) -> Self {
        FileV6 {
            format_version: 6,
            units: f.units,
            next_id: f.next_id,
            parameters: f.parameters,
            layers: f.layers,
            entities: f.entities,
            constraints: f.constraints,
            booleans: f.booleans,
            recipes: f.recipes,
            dim_exprs: BTreeMap::new(),
            view: f.view,
        }
    }
}

/// Version 6: expressions behind dimension values (SPEC §5, #172).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct FileV6 {
    format_version: u32,
    units: Unit,
    next_id: u64,
    parameters: BTreeMap<String, Parameter>,
    layers: BTreeMap<LayerId, LayerV2>,
    entities: BTreeMap<EntityId, Entity>,
    constraints: BTreeMap<ConstraintId, Constraint>,
    booleans: BTreeMap<BooleanId, Boolean>,
    recipes: BTreeMap<RecipeId, Recipe>,
    dim_exprs: BTreeMap<ConstraintId, String>,
    #[serde(default)]
    view: Option<View>,
}

impl From<FileV6> for FileV7 {
    /// v6 → v7: nothing to convert; v7 only adds the Engrave layer's
    /// `crosshatch` flag (SPEC §7, #175), absent (off) in every v6 file.
    fn from(f: FileV6) -> Self {
        FileV7 {
            format_version: 7,
            units: f.units,
            next_id: f.next_id,
            parameters: f.parameters,
            layers: f.layers,
            entities: f.entities,
            constraints: f.constraints,
            booleans: f.booleans,
            recipes: f.recipes,
            dim_exprs: f.dim_exprs,
            view: f.view,
        }
    }
}

/// Version 7: crosshatch on Engrave layers (SPEC §7, #175).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct FileV7 {
    format_version: u32,
    units: Unit,
    next_id: u64,
    parameters: BTreeMap<String, Parameter>,
    layers: BTreeMap<LayerId, LayerV2>,
    entities: BTreeMap<EntityId, Entity>,
    constraints: BTreeMap<ConstraintId, Constraint>,
    booleans: BTreeMap<BooleanId, Boolean>,
    recipes: BTreeMap<RecipeId, Recipe>,
    dim_exprs: BTreeMap<ConstraintId, String>,
    #[serde(default)]
    view: Option<View>,
}

impl From<FileV7> for FileV8 {
    /// v7 → v8: nothing to convert; v8 only adds the live offset recipe
    /// (SPEC §6.2, #164), which no v7 file can contain.
    fn from(f: FileV7) -> Self {
        FileV8 {
            format_version: 8,
            units: f.units,
            next_id: f.next_id,
            parameters: f.parameters,
            layers: f.layers,
            entities: f.entities,
            constraints: f.constraints,
            booleans: f.booleans,
            recipes: f.recipes,
            dim_exprs: f.dim_exprs,
            view: f.view,
        }
    }
}

/// Version 8: live offset recipes (SPEC §6.2, #164).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct FileV8 {
    format_version: u32,
    units: Unit,
    next_id: u64,
    parameters: BTreeMap<String, Parameter>,
    layers: BTreeMap<LayerId, LayerV2>,
    entities: BTreeMap<EntityId, Entity>,
    constraints: BTreeMap<ConstraintId, Constraint>,
    booleans: BTreeMap<BooleanId, Boolean>,
    recipes: BTreeMap<RecipeId, Recipe>,
    dim_exprs: BTreeMap<ConstraintId, String>,
    #[serde(default)]
    view: Option<View>,
}

/// Version 9: text entities (SPEC §9, #179). A v8 file holds no text, so the
/// migration is the identity on the data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct FileV9 {
    format_version: u32,
    units: Unit,
    next_id: u64,
    parameters: BTreeMap<String, Parameter>,
    layers: BTreeMap<LayerId, LayerV2>,
    entities: BTreeMap<EntityId, Entity>,
    constraints: BTreeMap<ConstraintId, Constraint>,
    booleans: BTreeMap<BooleanId, Boolean>,
    recipes: BTreeMap<RecipeId, Recipe>,
    dim_exprs: BTreeMap<ConstraintId, String>,
    #[serde(default)]
    view: Option<View>,
}

impl From<FileV8> for FileV9 {
    fn from(f: FileV8) -> Self {
        FileV9 {
            format_version: 9,
            units: f.units,
            next_id: f.next_id,
            parameters: f.parameters,
            layers: f.layers,
            entities: f.entities,
            constraints: f.constraints,
            booleans: f.booleans,
            recipes: f.recipes,
            dim_exprs: f.dim_exprs,
            view: f.view,
        }
    }
}

/// Version 10: plugin objects, the `Generated` recipe (SPEC §15.5, #183). A
/// v9 file holds none, so the migration is the identity on the data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct FileV10 {
    format_version: u32,
    units: Unit,
    next_id: u64,
    parameters: BTreeMap<String, Parameter>,
    layers: BTreeMap<LayerId, LayerV2>,
    entities: BTreeMap<EntityId, Entity>,
    constraints: BTreeMap<ConstraintId, Constraint>,
    booleans: BTreeMap<BooleanId, Boolean>,
    recipes: BTreeMap<RecipeId, Recipe>,
    dim_exprs: BTreeMap<ConstraintId, String>,
    #[serde(default)]
    view: Option<View>,
}

impl From<FileV9> for FileV10 {
    fn from(f: FileV9) -> Self {
        FileV10 {
            format_version: 10,
            units: f.units,
            next_id: f.next_id,
            parameters: f.parameters,
            layers: f.layers,
            entities: f.entities,
            constraints: f.constraints,
            booleans: f.booleans,
            recipes: f.recipes,
            dim_exprs: f.dim_exprs,
            view: f.view,
        }
    }
}

/// Version 11: the selected machine profile id (SPEC §7, #236). Absent in a
/// v10 file, so the migration adds `None`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct FileV11 {
    format_version: u32,
    units: Unit,
    next_id: u64,
    parameters: BTreeMap<String, Parameter>,
    layers: BTreeMap<LayerId, LayerV2>,
    entities: BTreeMap<EntityId, Entity>,
    constraints: BTreeMap<ConstraintId, Constraint>,
    booleans: BTreeMap<BooleanId, Boolean>,
    recipes: BTreeMap<RecipeId, Recipe>,
    dim_exprs: BTreeMap<ConstraintId, String>,
    #[serde(default)]
    view: Option<View>,
    #[serde(default)]
    profile: Option<String>,
}

impl From<FileV10> for FileV11 {
    fn from(f: FileV10) -> Self {
        FileV11 {
            format_version: 11,
            units: f.units,
            next_id: f.next_id,
            parameters: f.parameters,
            layers: f.layers,
            entities: f.entities,
            constraints: f.constraints,
            booleans: f.booleans,
            recipes: f.recipes,
            dim_exprs: f.dim_exprs,
            view: f.view,
            profile: None,
        }
    }
}

/// Version 12: stroke-edge dimensions (SPEC §5, #239, ADR 0014). `Dimension`
/// gains an `edge` that is omitted while it is the centreline, so a v11 file
/// loads unchanged and the migration is the identity on the data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct FileV12 {
    format_version: u32,
    units: Unit,
    next_id: u64,
    parameters: BTreeMap<String, Parameter>,
    layers: BTreeMap<LayerId, LayerV2>,
    entities: BTreeMap<EntityId, Entity>,
    constraints: BTreeMap<ConstraintId, Constraint>,
    booleans: BTreeMap<BooleanId, Boolean>,
    recipes: BTreeMap<RecipeId, Recipe>,
    dim_exprs: BTreeMap<ConstraintId, String>,
    #[serde(default)]
    view: Option<View>,
    #[serde(default)]
    profile: Option<String>,
}

impl From<FileV11> for FileV12 {
    fn from(f: FileV11) -> Self {
        FileV12 {
            format_version: 12,
            units: f.units,
            next_id: f.next_id,
            parameters: f.parameters,
            layers: f.layers,
            entities: f.entities,
            constraints: f.constraints,
            booleans: f.booleans,
            recipes: f.recipes,
            dim_exprs: f.dim_exprs,
            view: f.view,
            profile: f.profile,
        }
    }
}

/// Version 13: style belongs to each entity, not its layer (SPEC §4, §7,
/// #287, ADR 0017). `Operation::Draw` loses its pen; the migration copies each
/// Draw layer's pen number and width onto that layer's entities. Default
/// styles are not written, so a file whose pens were never changed migrates
/// to the same entities.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct FileV13 {
    format_version: u32,
    units: Unit,
    next_id: u64,
    parameters: BTreeMap<String, Parameter>,
    layers: BTreeMap<LayerId, Layer>,
    entities: BTreeMap<EntityId, Entity>,
    constraints: BTreeMap<ConstraintId, Constraint>,
    booleans: BTreeMap<BooleanId, Boolean>,
    recipes: BTreeMap<RecipeId, Recipe>,
    dim_exprs: BTreeMap<ConstraintId, String>,
    #[serde(default)]
    view: Option<View>,
    #[serde(default)]
    profile: Option<String>,
}

impl From<FileV12> for FileV13 {
    fn from(f: FileV12) -> Self {
        let pens: BTreeMap<LayerId, DrawParamsV2> = f
            .layers
            .iter()
            .filter_map(|(id, l)| match l.operation {
                OperationV2::Draw(p) => Some((*id, p)),
                _ => None,
            })
            .collect();
        let entities = f
            .entities
            .into_iter()
            .map(|(id, mut e)| {
                if let Some(p) = pens.get(&e.layer) {
                    e.style.width = p.pen_width;
                    e.style.pen = p.pen_number;
                }
                (id, e)
            })
            .collect();
        FileV13 {
            format_version: 13,
            units: f.units,
            next_id: f.next_id,
            parameters: f.parameters,
            layers: f.layers.into_iter().map(|(i, l)| (i, l.into())).collect(),
            entities,
            constraints: f.constraints,
            booleans: f.booleans,
            recipes: f.recipes,
            dim_exprs: f.dim_exprs,
            view: f.view,
            profile: f.profile,
        }
    }
}

/// Version 14: letter spacing on text (SPEC §9, #222). `TextData` gains a
/// `tracking` that is omitted while zero, so a v13 file loads unchanged and
/// the migration is the identity on the data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct FileV14 {
    format_version: u32,
    units: Unit,
    next_id: u64,
    parameters: BTreeMap<String, Parameter>,
    layers: BTreeMap<LayerId, Layer>,
    entities: BTreeMap<EntityId, Entity>,
    constraints: BTreeMap<ConstraintId, Constraint>,
    booleans: BTreeMap<BooleanId, Boolean>,
    recipes: BTreeMap<RecipeId, Recipe>,
    dim_exprs: BTreeMap<ConstraintId, String>,
    #[serde(default)]
    view: Option<View>,
    #[serde(default)]
    profile: Option<String>,
}

impl From<FileV13> for FileV14 {
    fn from(f: FileV13) -> Self {
        FileV14 {
            format_version: 14,
            units: f.units,
            next_id: f.next_id,
            parameters: f.parameters,
            layers: f.layers,
            entities: f.entities,
            constraints: f.constraints,
            booleans: f.booleans,
            recipes: f.recipes,
            dim_exprs: f.dim_exprs,
            view: f.view,
            profile: f.profile,
        }
    }
}

/// Version 15: a fill that is turned off keeps its colour (#388).
/// `Fill` gains `Off([r, g, b])`, so an older build refuses a v15 file
/// as too new instead of failing to parse it. Nothing else changes, and
/// the migration is the identity on the data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct FileV15 {
    format_version: u32,
    units: Unit,
    next_id: u64,
    parameters: BTreeMap<String, Parameter>,
    layers: BTreeMap<LayerId, Layer>,
    entities: BTreeMap<EntityId, Entity>,
    constraints: BTreeMap<ConstraintId, Constraint>,
    booleans: BTreeMap<BooleanId, Boolean>,
    recipes: BTreeMap<RecipeId, Recipe>,
    dim_exprs: BTreeMap<ConstraintId, String>,
    #[serde(default)]
    view: Option<View>,
    #[serde(default)]
    profile: Option<String>,
}

impl From<FileV14> for FileV15 {
    fn from(f: FileV14) -> Self {
        FileV15 {
            format_version: 15,
            units: f.units,
            next_id: f.next_id,
            parameters: f.parameters,
            layers: f.layers,
            entities: f.entities,
            constraints: f.constraints,
            booleans: f.booleans,
            recipes: f.recipes,
            dim_exprs: f.dim_exprs,
            view: f.view,
            profile: f.profile,
        }
    }
}

/// Version 16: raster reference images (#390). `images` maps a Reference
/// layer to its PNG or JPEG, kept as base64 of the original file, and is
/// omitted while empty, so the migration is the identity on the data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct FileV16 {
    format_version: u32,
    units: Unit,
    next_id: u64,
    parameters: BTreeMap<String, Parameter>,
    layers: BTreeMap<LayerId, Layer>,
    entities: BTreeMap<EntityId, Entity>,
    constraints: BTreeMap<ConstraintId, Constraint>,
    booleans: BTreeMap<BooleanId, Boolean>,
    recipes: BTreeMap<RecipeId, Recipe>,
    dim_exprs: BTreeMap<ConstraintId, String>,
    #[serde(default)]
    view: Option<View>,
    #[serde(default)]
    profile: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    images: BTreeMap<LayerId, crate::image::RefImage>,
}

impl From<FileV15> for FileV16 {
    fn from(f: FileV15) -> Self {
        FileV16 {
            format_version: 16,
            units: f.units,
            next_id: f.next_id,
            parameters: f.parameters,
            layers: f.layers,
            entities: f.entities,
            constraints: f.constraints,
            booleans: f.booleans,
            recipes: f.recipes,
            dim_exprs: f.dim_exprs,
            view: f.view,
            profile: f.profile,
            images: BTreeMap::new(),
        }
    }
}

/// Version 17: text on a path (#392). `TextData` gains an optional `path`,
/// omitted while unset, so an older build refuses a v17 file as too new
/// instead of silently dropping the path. Nothing else changes, and the
/// migration is the identity on the data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct FileV17 {
    format_version: u32,
    units: Unit,
    next_id: u64,
    parameters: BTreeMap<String, Parameter>,
    layers: BTreeMap<LayerId, Layer>,
    entities: BTreeMap<EntityId, Entity>,
    constraints: BTreeMap<ConstraintId, Constraint>,
    booleans: BTreeMap<BooleanId, Boolean>,
    recipes: BTreeMap<RecipeId, Recipe>,
    dim_exprs: BTreeMap<ConstraintId, String>,
    #[serde(default)]
    view: Option<View>,
    #[serde(default)]
    profile: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    images: BTreeMap<LayerId, crate::image::RefImage>,
}

impl From<FileV16> for FileV17 {
    fn from(f: FileV16) -> Self {
        FileV17 {
            format_version: 17,
            units: f.units,
            next_id: f.next_id,
            parameters: f.parameters,
            layers: f.layers,
            entities: f.entities,
            constraints: f.constraints,
            booleans: f.booleans,
            recipes: f.recipes,
            dim_exprs: f.dim_exprs,
            view: f.view,
            profile: f.profile,
            images: f.images,
        }
    }
}

/// Version 18: the job's placement on the machine bed (#443, SPEC §7). An
/// optional offset, omitted while unset, so the migration is the identity on
/// the data and a file without a placement differs from v17 only in its
/// version.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct FileV18 {
    format_version: u32,
    units: Unit,
    next_id: u64,
    parameters: BTreeMap<String, Parameter>,
    layers: BTreeMap<LayerId, Layer>,
    entities: BTreeMap<EntityId, Entity>,
    constraints: BTreeMap<ConstraintId, Constraint>,
    booleans: BTreeMap<BooleanId, Boolean>,
    recipes: BTreeMap<RecipeId, Recipe>,
    dim_exprs: BTreeMap<ConstraintId, String>,
    #[serde(default)]
    view: Option<View>,
    #[serde(default)]
    profile: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    images: BTreeMap<LayerId, crate::image::RefImage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    placement: Option<locus_geom::Vec2>,
}

impl From<FileV17> for FileV18 {
    fn from(f: FileV17) -> Self {
        FileV18 {
            format_version: 18,
            units: f.units,
            next_id: f.next_id,
            parameters: f.parameters,
            layers: f.layers,
            entities: f.entities,
            constraints: f.constraints,
            booleans: f.booleans,
            recipes: f.recipes,
            dim_exprs: f.dim_exprs,
            view: f.view,
            profile: f.profile,
            images: f.images,
            placement: None,
        }
    }
}

/// Version 19: fills belong to regions (#451, SPEC §4, §6.3). Each region's
/// fill colour and opacity, by region key, omitted while empty. Object
/// fills are moved onto the regions they enclosed when an older file loads
/// ([`crate::region_fill::migrate_object_fills`]), so the data changes:
/// object styles lose their fill.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct FileV19 {
    format_version: u32,
    units: Unit,
    next_id: u64,
    parameters: BTreeMap<String, Parameter>,
    layers: BTreeMap<LayerId, Layer>,
    entities: BTreeMap<EntityId, Entity>,
    constraints: BTreeMap<ConstraintId, Constraint>,
    booleans: BTreeMap<BooleanId, Boolean>,
    recipes: BTreeMap<RecipeId, Recipe>,
    dim_exprs: BTreeMap<ConstraintId, String>,
    #[serde(default)]
    view: Option<View>,
    #[serde(default)]
    profile: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    images: BTreeMap<LayerId, crate::image::RefImage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    placement: Option<locus_geom::Vec2>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    region_fills: Vec<(locus_region::RegionKey, crate::region_fill::RegionFill)>,
}

impl From<FileV18> for FileV19 {
    fn from(f: FileV18) -> Self {
        FileV19 {
            format_version: 19,
            units: f.units,
            next_id: f.next_id,
            parameters: f.parameters,
            layers: f.layers,
            entities: f.entities,
            constraints: f.constraints,
            booleans: f.booleans,
            recipes: f.recipes,
            dim_exprs: f.dim_exprs,
            view: f.view,
            profile: f.profile,
            images: f.images,
            placement: f.placement,
            // Filled from the object fills once the document is built.
            region_fills: Vec::new(),
        }
    }
}

/// Version 20: Engrave modes and the overlap rule (#455, SPEC §7).
/// `EngraveParams` gains `mode` and `overlap`; an older Engrave layer has
/// neither and reads as Line + Fill with even-odd overlaps, which is what
/// it exported, so the migration is the identity on the output. The shape
/// is v19's; the version stops an older build from dropping the new
/// fields.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct FileV20 {
    format_version: u32,
    units: Unit,
    next_id: u64,
    parameters: BTreeMap<String, Parameter>,
    layers: BTreeMap<LayerId, Layer>,
    entities: BTreeMap<EntityId, Entity>,
    constraints: BTreeMap<ConstraintId, Constraint>,
    booleans: BTreeMap<BooleanId, Boolean>,
    recipes: BTreeMap<RecipeId, Recipe>,
    dim_exprs: BTreeMap<ConstraintId, String>,
    #[serde(default)]
    view: Option<View>,
    #[serde(default)]
    profile: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    images: BTreeMap<LayerId, crate::image::RefImage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    placement: Option<locus_geom::Vec2>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    region_fills: Vec<(locus_region::RegionKey, crate::region_fill::RegionFill)>,
}

impl From<FileV19> for FileV20 {
    fn from(f: FileV19) -> Self {
        FileV20 {
            format_version: 20,
            units: f.units,
            next_id: f.next_id,
            parameters: f.parameters,
            layers: f.layers,
            entities: f.entities,
            constraints: f.constraints,
            booleans: f.booleans,
            recipes: f.recipes,
            dim_exprs: f.dim_exprs,
            view: f.view,
            profile: f.profile,
            images: f.images,
            placement: f.placement,
            region_fills: f.region_fills,
        }
    }
}

/// Version 21: regions are per layer (#457, SPEC §6.3). The shape is v20's,
/// but region keys (fills and boolean recipes) now name per-layer faces;
/// loading an older file moves each key that named a face cut by another
/// layer's shape onto the per-layer region in the same place
/// ([`crate::region_fill::migrate_region_keys`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct FileV21 {
    format_version: u32,
    units: Unit,
    next_id: u64,
    parameters: BTreeMap<String, Parameter>,
    layers: BTreeMap<LayerId, Layer>,
    entities: BTreeMap<EntityId, Entity>,
    constraints: BTreeMap<ConstraintId, Constraint>,
    booleans: BTreeMap<BooleanId, Boolean>,
    recipes: BTreeMap<RecipeId, Recipe>,
    dim_exprs: BTreeMap<ConstraintId, String>,
    #[serde(default)]
    view: Option<View>,
    #[serde(default)]
    profile: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    images: BTreeMap<LayerId, crate::image::RefImage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    placement: Option<locus_geom::Vec2>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    region_fills: Vec<(locus_region::RegionKey, crate::region_fill::RegionFill)>,
}

impl From<FileV20> for FileV21 {
    fn from(f: FileV20) -> Self {
        FileV21 {
            format_version: 21,
            units: f.units,
            next_id: f.next_id,
            parameters: f.parameters,
            layers: f.layers,
            entities: f.entities,
            constraints: f.constraints,
            booleans: f.booleans,
            recipes: f.recipes,
            dim_exprs: f.dim_exprs,
            view: f.view,
            profile: f.profile,
            images: f.images,
            placement: f.placement,
            region_fills: f.region_fills,
        }
    }
}

/// Version 22: reference images can be turned (#463, ADR 0019 decision 9).
/// `ImagePlacement` gains `rotation` (radians about the image's centre,
/// omitted while 0); an older image has none and reads as unturned. The
/// shape is v21's; the version stops an older build from dropping the
/// rotation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct FileV22 {
    format_version: u32,
    units: Unit,
    next_id: u64,
    parameters: BTreeMap<String, Parameter>,
    layers: BTreeMap<LayerId, Layer>,
    entities: BTreeMap<EntityId, Entity>,
    constraints: BTreeMap<ConstraintId, Constraint>,
    booleans: BTreeMap<BooleanId, Boolean>,
    recipes: BTreeMap<RecipeId, Recipe>,
    dim_exprs: BTreeMap<ConstraintId, String>,
    #[serde(default)]
    view: Option<View>,
    #[serde(default)]
    profile: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    images: BTreeMap<LayerId, crate::image::RefImage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    placement: Option<locus_geom::Vec2>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    region_fills: Vec<(locus_region::RegionKey, crate::region_fill::RegionFill)>,
}

impl From<FileV21> for FileV22 {
    fn from(f: FileV21) -> Self {
        FileV22 {
            format_version: 22,
            units: f.units,
            next_id: f.next_id,
            parameters: f.parameters,
            layers: f.layers,
            entities: f.entities,
            constraints: f.constraints,
            booleans: f.booleans,
            recipes: f.recipes,
            dim_exprs: f.dim_exprs,
            view: f.view,
            profile: f.profile,
            images: f.images,
            placement: f.placement,
            region_fills: f.region_fills,
        }
    }
}

/// Version 23: reference images can have non-square pixels (#463, a free
/// X/Y scale): `ImagePlacement` gains `mm_per_px_y`, omitted for square
/// ones, so older images read unchanged. The shape is v22's; the version
/// stops an older build from dropping the Y size.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct FileV23 {
    format_version: u32,
    units: Unit,
    next_id: u64,
    parameters: BTreeMap<String, Parameter>,
    layers: BTreeMap<LayerId, Layer>,
    entities: BTreeMap<EntityId, Entity>,
    constraints: BTreeMap<ConstraintId, Constraint>,
    booleans: BTreeMap<BooleanId, Boolean>,
    recipes: BTreeMap<RecipeId, Recipe>,
    dim_exprs: BTreeMap<ConstraintId, String>,
    #[serde(default)]
    view: Option<View>,
    #[serde(default)]
    profile: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    images: BTreeMap<LayerId, crate::image::RefImage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    placement: Option<locus_geom::Vec2>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    region_fills: Vec<(locus_region::RegionKey, crate::region_fill::RegionFill)>,
}

impl From<FileV22> for FileV23 {
    fn from(f: FileV22) -> Self {
        FileV23 {
            format_version: 23,
            units: f.units,
            next_id: f.next_id,
            parameters: f.parameters,
            layers: f.layers,
            entities: f.entities,
            constraints: f.constraints,
            booleans: f.booleans,
            recipes: f.recipes,
            dim_exprs: f.dim_exprs,
            view: f.view,
            profile: f.profile,
            images: f.images,
            placement: f.placement,
            region_fills: f.region_fills,
        }
    }
}

/// Version 24: a timeline feature can be suppressed (#446, ADR 0019
/// decision 5): `Recipe` gains `feature_suppressed`, omitted while false, so
/// older recipes read unchanged. The shape is v23's; the version stops an
/// older build from dropping the flag (and regenerating the copies).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct FileV24 {
    format_version: u32,
    units: Unit,
    next_id: u64,
    parameters: BTreeMap<String, Parameter>,
    layers: BTreeMap<LayerId, Layer>,
    entities: BTreeMap<EntityId, Entity>,
    constraints: BTreeMap<ConstraintId, Constraint>,
    booleans: BTreeMap<BooleanId, Boolean>,
    recipes: BTreeMap<RecipeId, Recipe>,
    dim_exprs: BTreeMap<ConstraintId, String>,
    #[serde(default)]
    view: Option<View>,
    #[serde(default)]
    profile: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    images: BTreeMap<LayerId, crate::image::RefImage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    placement: Option<locus_geom::Vec2>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    region_fills: Vec<(locus_region::RegionKey, crate::region_fill::RegionFill)>,
}

impl From<FileV23> for FileV24 {
    fn from(f: FileV23) -> Self {
        FileV24 {
            format_version: 24,
            units: f.units,
            next_id: f.next_id,
            parameters: f.parameters,
            layers: f.layers,
            entities: f.entities,
            constraints: f.constraints,
            booleans: f.booleans,
            recipes: f.recipes,
            dim_exprs: f.dim_exprs,
            view: f.view,
            profile: f.profile,
            images: f.images,
            placement: f.placement,
            region_fills: f.region_fills,
        }
    }
}

/// Version 25: line styles (#428, ADR 0019). `Style` gains `line` (caps,
/// joins, alignment) and `Layer` gains `stroke_styles` (Centreline or
/// Styled), both omitted at their defaults, so an older file reads with
/// round caps and joins and centreline cuts, as before. The shape is v24's;
/// the version stops an older build from dropping the styles.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct FileV25 {
    format_version: u32,
    units: Unit,
    next_id: u64,
    parameters: BTreeMap<String, Parameter>,
    layers: BTreeMap<LayerId, Layer>,
    entities: BTreeMap<EntityId, Entity>,
    constraints: BTreeMap<ConstraintId, Constraint>,
    booleans: BTreeMap<BooleanId, Boolean>,
    recipes: BTreeMap<RecipeId, Recipe>,
    dim_exprs: BTreeMap<ConstraintId, String>,
    #[serde(default)]
    view: Option<View>,
    #[serde(default)]
    profile: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    images: BTreeMap<LayerId, crate::image::RefImage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    placement: Option<locus_geom::Vec2>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    region_fills: Vec<(locus_region::RegionKey, crate::region_fill::RegionFill)>,
}

impl From<FileV24> for FileV25 {
    fn from(f: FileV24) -> Self {
        FileV25 {
            format_version: 25,
            units: f.units,
            next_id: f.next_id,
            parameters: f.parameters,
            layers: f.layers,
            entities: f.entities,
            constraints: f.constraints,
            booleans: f.booleans,
            recipes: f.recipes,
            dim_exprs: f.dim_exprs,
            view: f.view,
            profile: f.profile,
            images: f.images,
            placement: f.placement,
            region_fills: f.region_fills,
        }
    }
}

/// Version 26: the timeline's feature order (#446, ADR 0019 decision 5):
/// `recipe_order`, omitted while features are in creation order, so older
/// files read unchanged.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct FileV26 {
    format_version: u32,
    units: Unit,
    next_id: u64,
    parameters: BTreeMap<String, Parameter>,
    layers: BTreeMap<LayerId, Layer>,
    entities: BTreeMap<EntityId, Entity>,
    constraints: BTreeMap<ConstraintId, Constraint>,
    booleans: BTreeMap<BooleanId, Boolean>,
    recipes: BTreeMap<RecipeId, Recipe>,
    dim_exprs: BTreeMap<ConstraintId, String>,
    #[serde(default)]
    view: Option<View>,
    #[serde(default)]
    profile: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    images: BTreeMap<LayerId, crate::image::RefImage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    placement: Option<locus_geom::Vec2>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    region_fills: Vec<(locus_region::RegionKey, crate::region_fill::RegionFill)>,
    /// The timeline's feature order (#446), omitted in creation order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    recipe_order: Vec<RecipeId>,
}

impl From<FileV25> for FileV26 {
    fn from(f: FileV25) -> Self {
        FileV26 {
            format_version: 26,
            units: f.units,
            next_id: f.next_id,
            parameters: f.parameters,
            layers: f.layers,
            entities: f.entities,
            constraints: f.constraints,
            booleans: f.booleans,
            recipes: f.recipes,
            dim_exprs: f.dim_exprs,
            view: f.view,
            profile: f.profile,
            images: f.images,
            placement: f.placement,
            region_fills: f.region_fills,
            recipe_order: Vec::new(),
        }
    }
}

/// Version 27: dash patterns (#427, ADR 0019). `LineStyle` gains `dash`
/// (`pattern`, `phase`, `align`), omitted for a solid line, so an older
/// file reads solid. The shape is v26's; the version stops an older build
/// from dropping the dashes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct FileV27 {
    format_version: u32,
    units: Unit,
    next_id: u64,
    parameters: BTreeMap<String, Parameter>,
    layers: BTreeMap<LayerId, Layer>,
    entities: BTreeMap<EntityId, Entity>,
    constraints: BTreeMap<ConstraintId, Constraint>,
    booleans: BTreeMap<BooleanId, Boolean>,
    recipes: BTreeMap<RecipeId, Recipe>,
    dim_exprs: BTreeMap<ConstraintId, String>,
    #[serde(default)]
    view: Option<View>,
    #[serde(default)]
    profile: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    images: BTreeMap<LayerId, crate::image::RefImage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    placement: Option<locus_geom::Vec2>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    region_fills: Vec<(locus_region::RegionKey, crate::region_fill::RegionFill)>,
    /// The timeline's feature order (#446), omitted in creation order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    recipe_order: Vec<RecipeId>,
}

impl From<FileV26> for FileV27 {
    fn from(f: FileV26) -> Self {
        FileV27 {
            format_version: 27,
            units: f.units,
            next_id: f.next_id,
            parameters: f.parameters,
            layers: f.layers,
            entities: f.entities,
            constraints: f.constraints,
            booleans: f.booleans,
            recipes: f.recipes,
            dim_exprs: f.dim_exprs,
            view: f.view,
            profile: f.profile,
            images: f.images,
            placement: f.placement,
            region_fills: f.region_fills,
            recipe_order: f.recipe_order,
        }
    }
}

/// Version 28: gradient fills on regions (#436, ADR 0019 decision 1):
/// `RegionFill` gains `gradient` (kind, stops, frame angle and flip, Rotate
/// with shape, machine option), omitted while solid, so older fills read
/// unchanged. The shape is v27's; the version stops an older build from
/// dropping the gradient.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct FileV28 {
    format_version: u32,
    units: Unit,
    next_id: u64,
    parameters: BTreeMap<String, Parameter>,
    layers: BTreeMap<LayerId, Layer>,
    entities: BTreeMap<EntityId, Entity>,
    constraints: BTreeMap<ConstraintId, Constraint>,
    booleans: BTreeMap<BooleanId, Boolean>,
    recipes: BTreeMap<RecipeId, Recipe>,
    dim_exprs: BTreeMap<ConstraintId, String>,
    #[serde(default)]
    view: Option<View>,
    #[serde(default)]
    profile: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    images: BTreeMap<LayerId, crate::image::RefImage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    placement: Option<locus_geom::Vec2>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    region_fills: Vec<(locus_region::RegionKey, crate::region_fill::RegionFill)>,
    /// The timeline's feature order (#446), omitted in creation order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    recipe_order: Vec<RecipeId>,
}

impl From<FileV27> for FileV28 {
    fn from(f: FileV27) -> Self {
        FileV28 {
            format_version: 28,
            units: f.units,
            next_id: f.next_id,
            parameters: f.parameters,
            layers: f.layers,
            entities: f.entities,
            constraints: f.constraints,
            booleans: f.booleans,
            recipes: f.recipes,
            dim_exprs: f.dim_exprs,
            view: f.view,
            profile: f.profile,
            images: f.images,
            placement: f.placement,
            region_fills: f.region_fills,
            recipe_order: f.recipe_order,
        }
    }
}

/// Version 29: multi-strokes (#429, ADR 0019). `LineStyle` gains `extra`, a
/// list of extra strokes (offset, width, colour, pen, dash, cap), omitted
/// while empty, so an older file reads with one stroke. The shape is v28's;
/// the version stops an older build from dropping the strokes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct FileV29 {
    format_version: u32,
    units: Unit,
    next_id: u64,
    parameters: BTreeMap<String, Parameter>,
    layers: BTreeMap<LayerId, Layer>,
    entities: BTreeMap<EntityId, Entity>,
    constraints: BTreeMap<ConstraintId, Constraint>,
    booleans: BTreeMap<BooleanId, Boolean>,
    recipes: BTreeMap<RecipeId, Recipe>,
    dim_exprs: BTreeMap<ConstraintId, String>,
    #[serde(default)]
    view: Option<View>,
    #[serde(default)]
    profile: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    images: BTreeMap<LayerId, crate::image::RefImage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    placement: Option<locus_geom::Vec2>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    region_fills: Vec<(locus_region::RegionKey, crate::region_fill::RegionFill)>,
    /// The timeline's feature order (#446), omitted in creation order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    recipe_order: Vec<RecipeId>,
}

impl From<FileV28> for FileV29 {
    fn from(f: FileV28) -> Self {
        FileV29 {
            format_version: 29,
            units: f.units,
            next_id: f.next_id,
            parameters: f.parameters,
            layers: f.layers,
            entities: f.entities,
            constraints: f.constraints,
            booleans: f.booleans,
            recipes: f.recipes,
            dim_exprs: f.dim_exprs,
            view: f.view,
            profile: f.profile,
            images: f.images,
            placement: f.placement,
            region_fills: f.region_fills,
            recipe_order: f.recipe_order,
        }
    }
}

/// Version 30: width profiles (#426, ADR 0019). `LineStyle` gains `profile`,
/// width points along the path, omitted while unset, so an older file reads
/// with a uniform width. The shape is v29's; the version stops an older build
/// from dropping the profile.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct FileV30 {
    format_version: u32,
    units: Unit,
    next_id: u64,
    parameters: BTreeMap<String, Parameter>,
    layers: BTreeMap<LayerId, Layer>,
    entities: BTreeMap<EntityId, Entity>,
    constraints: BTreeMap<ConstraintId, Constraint>,
    booleans: BTreeMap<BooleanId, Boolean>,
    recipes: BTreeMap<RecipeId, Recipe>,
    dim_exprs: BTreeMap<ConstraintId, String>,
    #[serde(default)]
    view: Option<View>,
    #[serde(default)]
    profile: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    images: BTreeMap<LayerId, crate::image::RefImage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    placement: Option<locus_geom::Vec2>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    region_fills: Vec<(locus_region::RegionKey, crate::region_fill::RegionFill)>,
    /// The timeline's feature order (#446), omitted in creation order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    recipe_order: Vec<RecipeId>,
}

impl From<FileV29> for FileV30 {
    fn from(f: FileV29) -> Self {
        FileV30 {
            format_version: 30,
            units: f.units,
            next_id: f.next_id,
            parameters: f.parameters,
            layers: f.layers,
            entities: f.entities,
            constraints: f.constraints,
            booleans: f.booleans,
            recipes: f.recipes,
            dim_exprs: f.dim_exprs,
            view: f.view,
            profile: f.profile,
            images: f.images,
            placement: f.placement,
            region_fills: f.region_fills,
            recipe_order: f.recipe_order,
        }
    }
}

/// The newest on-disk shape. When adding v31, point this at `FileV31`.
type Latest = FileV30;

fn pretty() -> PrettyConfig {
    PrettyConfig::new()
        .indentor("  ".to_owned())
        .struct_names(false)
        .depth_limit(usize::MAX)
}

fn finite(v: f64, what: &str) -> Result<(), FileError> {
    if v.is_finite() {
        Ok(())
    } else {
        Err(FileError::Invalid(format!("{what} is not finite")))
    }
}

/// v9 → latest (and on to newer versions as they are added).
fn v9_to_latest(f: FileV9) -> Latest {
    v15_to_latest(FileV15::from(FileV14::from(FileV13::from(FileV12::from(
        FileV11::from(FileV10::from(f)),
    )))))
}

/// v15 → latest.
fn v15_to_latest(f: FileV15) -> Latest {
    FileV30::from(FileV29::from(FileV28::from(FileV27::from(FileV26::from(FileV25::from(
        FileV24::from(FileV23::from(FileV22::from(FileV21::from(FileV20::from(
            FileV19::from(FileV18::from(FileV17::from(FileV16::from(f)))),
        ))))),
    ))))))
}

/// Serialise to `.locus` text.
///
/// # Errors
/// [`FileError::Unsaveable`] if the document holds NaN/inf (ADR-0004 rule 2) or
/// serialisation fails.
pub fn save(doc: &Document) -> Result<String, FileError> {
    check_finite(doc).map_err(|e| FileError::Unsaveable(e.to_string()))?;
    let file = Latest {
        format_version: CURRENT_VERSION,
        units: doc.units,
        next_id: doc.next_id,
        parameters: doc.parameters.clone(),
        layers: doc.layers.clone(),
        entities: doc.entities.iter().map(|(i, e)| (i, e.clone())).collect(),
        constraints: doc.constraints.clone(),
        booleans: doc.booleans.clone(),
        recipes: doc.recipes.clone(),
        dim_exprs: doc.dim_exprs.clone(),
        view: doc.view,
        profile: doc.profile.clone(),
        images: doc.images.clone(),
        placement: doc.placement,
        region_fills: doc
            .region_fills
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
        recipe_order: doc.recipe_order.clone(),
    };
    let mut text = ron::ser::to_string_pretty(&file, pretty())
        .map_err(|e| FileError::Unsaveable(e.to_string()))?;
    text.push('\n');
    Ok(text)
}

/// Parse `.locus` text, migrating older versions, and validate the result.
///
/// # Errors
/// A [`FileError`] for malformed, too-new, or internally inconsistent files.
pub fn load(text: &str) -> Result<Document, FileError> {
    if text.len() > MAX_FILE_BYTES {
        return Err(FileError::TooLarge(text.len()));
    }
    let header: Header = ron::from_str(text).map_err(|e| FileError::Parse(e.to_string()))?;
    let latest = match header.format_version {
        0 => return Err(FileError::UnknownVersion(0)),
        1 => v9_to_latest(FileV9::from(FileV8::from(FileV7::from(FileV6::from(
            FileV5::from(FileV4::from(FileV3::from(FileV2::from(parse::<FileV1>(
                text,
            )?)))),
        ))))),
        2 => v9_to_latest(FileV9::from(FileV8::from(FileV7::from(FileV6::from(
            FileV5::from(FileV4::from(FileV3::from(parse::<FileV2>(text)?))),
        ))))),
        3 => v9_to_latest(FileV9::from(FileV8::from(FileV7::from(FileV6::from(
            FileV5::from(FileV4::from(parse::<FileV3>(text)?)),
        ))))),
        4 => v9_to_latest(FileV9::from(FileV8::from(FileV7::from(FileV6::from(
            FileV5::from(parse::<FileV4>(text)?),
        ))))),
        5 => v9_to_latest(FileV9::from(FileV8::from(FileV7::from(FileV6::from(
            parse::<FileV5>(text)?,
        ))))),
        6 => v9_to_latest(FileV9::from(FileV8::from(FileV7::from(parse::<FileV6>(
            text,
        )?)))),
        7 => v9_to_latest(FileV9::from(FileV8::from(parse::<FileV7>(text)?))),
        8 => v9_to_latest(FileV9::from(parse::<FileV8>(text)?)),
        9 => v9_to_latest(parse::<FileV9>(text)?),
        10 => v15_to_latest(FileV15::from(FileV14::from(FileV13::from(FileV12::from(
            FileV11::from(parse::<FileV10>(text)?),
        ))))),
        11 => v15_to_latest(FileV15::from(FileV14::from(FileV13::from(FileV12::from(
            parse::<FileV11>(text)?,
        ))))),
        12 => v15_to_latest(FileV15::from(FileV14::from(FileV13::from(
            parse::<FileV12>(text)?,
        )))),
        13 => v15_to_latest(FileV15::from(FileV14::from(parse::<FileV13>(text)?))),
        14 => v15_to_latest(FileV15::from(parse::<FileV14>(text)?)),
        15 => v15_to_latest(parse::<FileV15>(text)?),
        16 => FileV30::from(FileV29::from(FileV28::from(FileV27::from(FileV26::from(FileV25::from(
            FileV24::from(FileV23::from(FileV22::from(FileV21::from(FileV20::from(
                FileV19::from(FileV18::from(FileV17::from(parse::<FileV16>(text)?))),
            ))))),
        )))))),
        17 => FileV30::from(FileV29::from(FileV28::from(FileV27::from(FileV26::from(FileV25::from(
            FileV24::from(FileV23::from(FileV22::from(FileV21::from(FileV20::from(
                FileV19::from(FileV18::from(parse::<FileV17>(text)?)),
            ))))),
        )))))),
        18 => FileV30::from(FileV29::from(FileV28::from(FileV27::from(FileV26::from(FileV25::from(
            FileV24::from(FileV23::from(FileV22::from(FileV21::from(FileV20::from(
                FileV19::from(parse::<FileV18>(text)?),
            ))))),
        )))))),
        19 => FileV30::from(FileV29::from(FileV28::from(FileV27::from(FileV26::from(FileV25::from(
            FileV24::from(FileV23::from(FileV22::from(FileV21::from(FileV20::from(
                parse::<FileV19>(text)?,
            ))))),
        )))))),
        20 => FileV30::from(FileV29::from(FileV28::from(FileV27::from(FileV26::from(FileV25::from(
            FileV24::from(FileV23::from(FileV22::from(FileV21::from(
                parse::<FileV20>(text)?,
            )))),
        )))))),
        21 => FileV30::from(FileV29::from(FileV28::from(FileV27::from(FileV26::from(FileV25::from(
            FileV24::from(FileV23::from(FileV22::from(parse::<FileV21>(text)?))),
        )))))),
        22 => FileV30::from(FileV29::from(FileV28::from(FileV27::from(FileV26::from(FileV25::from(
            FileV24::from(FileV23::from(parse::<FileV22>(text)?)),
        )))))),
        23 => FileV30::from(FileV29::from(FileV28::from(FileV27::from(FileV26::from(FileV25::from(
            FileV24::from(parse::<FileV23>(text)?),
        )))))),
        24 => FileV30::from(FileV29::from(FileV28::from(FileV27::from(FileV26::from(FileV25::from(
            parse::<FileV24>(text)?,
        )))))),
        25 => FileV30::from(FileV29::from(FileV28::from(FileV27::from(FileV26::from(
            parse::<FileV25>(text)?,
        ))))),
        26 => FileV30::from(FileV29::from(FileV28::from(FileV27::from(parse::<FileV26>(text)?)))),
        27 => FileV30::from(FileV29::from(FileV28::from(parse::<FileV27>(text)?))),
        28 => FileV30::from(FileV29::from(parse::<FileV28>(text)?)),
        29 => FileV30::from(parse::<FileV29>(text)?),
        30 => parse::<FileV30>(text)?,
        v if v > CURRENT_VERSION => {
            return Err(FileError::FutureVersion {
                found: v,
                supported: CURRENT_VERSION,
            });
        }
        v => return Err(FileError::UnknownVersion(v)),
    };
    // Before v19 fills were on objects: move them onto their regions (#451).
    // Before v21 region keys were built across layers: move them (#457).
    into_document(
        latest,
        header.format_version < 19,
        (19..21).contains(&header.format_version),
    )
}

fn parse<T: serde::de::DeserializeOwned>(text: &str) -> Result<T, FileError> {
    ron::from_str(text).map_err(|e| FileError::Parse(e.to_string()))
}

fn into_document(
    f: Latest,
    migrate_fills: bool,
    migrate_keys: bool,
) -> Result<Document, FileError> {
    if f.region_fills.len() > crate::region_fill::MAX_REGION_FILLS {
        return Err(FileError::Invalid("too many region fills".to_owned()));
    }
    let mut doc = Document {
        units: f.units,
        next_id: f.next_id,
        parameters: f.parameters,
        layers: f.layers,
        entities: {
            let mut s = EntityStore::default();
            for (id, e) in f.entities {
                s.insert(id, e);
            }
            s
        },
        constraints: f.constraints,
        booleans: f.booleans,
        recipes: f.recipes,
        dim_exprs: f.dim_exprs,
        view: f.view,
        profile: f.profile,
        images: f.images,
        placement: f.placement,
        region_fills: f.region_fills.into_iter().collect(),
        recipe_order: f.recipe_order,
    };
    if migrate_fills {
        crate::region_fill::migrate_object_fills(&mut doc);
    }
    if migrate_keys {
        crate::region_fill::migrate_region_keys(&mut doc);
    }
    validate(&doc)?;
    Ok(doc)
}

fn check_finite(doc: &Document) -> Result<(), FileError> {
    for (name, p) in &doc.parameters {
        finite(p.value, &format!("parameter {name}"))?;
    }
    if let Some(v) = &doc.view {
        finite(v.center.x, "view")?;
        finite(v.center.y, "view")?;
        finite(v.scale, "view")?;
    }
    for (id, e) in doc.entities.iter() {
        match &e.geom {
            Geom::Point { pos } => {
                finite(pos.x, &format!("entity {id}"))?;
                finite(pos.y, &format!("entity {id}"))?;
            }
            Geom::Circle { radius, .. } => finite(*radius, &format!("entity {id}"))?,
            _ => {}
        }
    }
    for (id, c) in &doc.constraints {
        if let Some(d) = c.dimension() {
            finite(d.value, &format!("constraint {id}"))?;
        }
    }
    Ok(())
}

/// Whole-document consistency (ADR-0003 rule 6): IDs below the counter and
/// the 2^53 cap, no ID shared between kinds, every reference resolves, finite
/// numbers, constraints compatible with what they reference.
pub(crate) fn validate(doc: &Document) -> Result<(), FileError> {
    if let Some(id) = &doc.profile {
        crate::document::check_profile_id(id)?;
    }
    if let Some(p) = doc.placement {
        crate::document::check_placement(p)?;
    }
    // The timeline order (#446) names existing features, each once.
    let mut seen = std::collections::BTreeSet::new();
    for id in &doc.recipe_order {
        if !doc.recipes.contains_key(id) || !seen.insert(*id) {
            return Err(FileError::Invalid("bad feature order".to_owned()));
        }
    }
    for (k, fill) in &doc.region_fills {
        crate::region_fill::check_key(k)?;
        fill.validate()?;
    }
    // Objects have no fill since v19 (#451).
    if doc
        .entities
        .iter()
        .any(|(_, e)| e.style.fill != crate::style::Fill::None)
    {
        return Err(FileError::Invalid(
            "an object has a fill; fills belong to regions".to_owned(),
        ));
    }
    if doc.next_id > MAX_ID {
        return Err(FileError::Invalid(format!(
            "next_id {} exceeds the ID limit",
            doc.next_id
        )));
    }
    check_finite(doc)?;
    let mut seen = std::collections::BTreeSet::new();
    let ids = doc
        .layers
        .keys()
        .map(|i| i.get())
        .chain(doc.entities.iter().map(|(i, _)| i.get()))
        .chain(doc.constraints.keys().map(|i| i.get()))
        .chain(doc.booleans.keys().map(|i| i.get()))
        .chain(doc.recipes.keys().map(|i| i.get()));
    for id in ids {
        if id >= doc.next_id {
            return Err(FileError::Invalid(format!(
                "ID {id} is not below next_id {}",
                doc.next_id
            )));
        }
        if !seen.insert(id) {
            return Err(FileError::Invalid(format!("ID {id} is used twice")));
        }
    }
    if doc.layers.is_empty() {
        return Err(FileError::Invalid("document has no layers".to_owned()));
    }
    for l in doc.layers.values() {
        l.operation.validate()?;
    }
    for (_, e) in doc.entities.iter() {
        doc.check_entity(e)?;
    }
    for c in doc.constraints.values() {
        c.validate(doc)?;
    }
    for b in doc.booleans.values() {
        b.validate()?;
    }
    crate::recipe::validate_all(doc)?;
    if doc.images.len() > crate::image::MAX_IMAGES {
        return Err(FileError::Invalid("too many images".to_owned()));
    }
    for (layer, image) in &doc.images {
        doc.check_image(*layer, image)?;
    }
    for (name, p) in &doc.parameters {
        crate::params::check_name(name)?;
        if crate::expr::evaluate(&p.expr, &doc.parameters).is_err() {
            return Err(FileError::Invalid(format!(
                "parameter {name} has an invalid expression"
            )));
        }
    }
    for (id, src) in &doc.dim_exprs {
        let driving = doc
            .constraint(*id)
            .and_then(Constraint::dimension)
            .is_some_and(|d| !d.driven);
        if !driving || crate::expr::evaluate(src, &doc.parameters).is_err() {
            return Err(FileError::Invalid(format!(
                "constraint {id} has an invalid dimension expression"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::{Command, Editor};
    use crate::constraint::Dimension;
    use locus_geom::Vec2;
    use locus_region::RegionKey;

    const L: LayerId = Document::default_layer();

    /// The version line this build writes.
    fn current() -> String {
        format!("format_version: {CURRENT_VERSION}")
    }

    /// 60 × 40 plate with a Ø10 hole, a construction point, a parameter and a
    /// saved view: the v1 fixture.
    pub(crate) fn sample() -> Document {
        // Cut, like the frozen v1 fixture: new documents default to Draw
        // (#288), but files written earlier keep what they say.
        let mut doc = Document::new();
        if let Some(l) = doc.layers.get_mut(&L) {
            l.operation = Operation::Cut(CutParams::default());
        }
        let mut ed = Editor::new(doc);
        let mut add = |construction, geom| {
            ed.apply(&Command::AddEntity {
                layer: L,
                construction,
                style: Default::default(),
                geom,
            })
            .unwrap()
            .entities[0]
        };
        let p = [
            add(
                false,
                Geom::Point {
                    pos: Vec2::new(0.0, 0.0),
                },
            ),
            add(
                false,
                Geom::Point {
                    pos: Vec2::new(60.0, 0.0),
                },
            ),
            add(
                false,
                Geom::Point {
                    pos: Vec2::new(60.0, 40.0),
                },
            ),
            add(
                false,
                Geom::Point {
                    pos: Vec2::new(0.0, 40.0),
                },
            ),
        ];
        let l: Vec<_> = (0..4)
            .map(|i| {
                add(
                    false,
                    Geom::Line {
                        start: p[i],
                        end: p[(i + 1) % 4],
                    },
                )
            })
            .collect();
        let c = add(
            false,
            Geom::Point {
                pos: Vec2::new(30.0, 20.0),
            },
        );
        let hole = add(
            false,
            Geom::Circle {
                center: c,
                radius: 5.0,
            },
        );
        // Not exactly representable, to exercise f64 output.
        add(
            true,
            Geom::Point {
                pos: Vec2::new(0.1 + 0.2, 40.0 / 3.0),
            },
        );
        let k = |constraint| Command::AddConstraint { constraint };
        for cmd in [
            k(Constraint::Fix { point: p[0] }),
            k(Constraint::Horizontal { line: l[0] }),
            k(Constraint::Vertical { line: l[1] }),
            k(Constraint::Horizontal { line: l[2] }),
            k(Constraint::Vertical { line: l[3] }),
            k(Constraint::Distance {
                a: p[0],
                b: p[1],
                dim: Dimension::driving(60.0),
            }),
            k(Constraint::Radius {
                curve: hole,
                dim: Dimension::driving(5.0),
            }),
            k(Constraint::Midpoint {
                point: c,
                line: l[0],
            }),
        ] {
            ed.apply(&cmd).unwrap();
        }
        let mut doc = ed.into_document();
        doc.parameters.insert(
            "width".to_owned(),
            Parameter {
                expr: "60".to_owned(),
                value: 60.0,
            },
        );
        doc.view = Some(View {
            center: Vec2::new(30.0, 20.0),
            scale: 4.0,
        });
        doc
    }

    #[test]
    fn save_load_save_is_byte_identical() {
        let doc = sample();
        let a = save(&doc).unwrap();
        let loaded = load(&a).unwrap();
        assert_eq!(loaded, doc);
        let b = save(&loaded).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn output_is_deterministic_across_insertion_order() {
        // Same content built two ways gives the same bytes.
        assert_eq!(save(&sample()).unwrap(), save(&sample()).unwrap());
    }

    #[test]
    fn empty_document_round_trips() {
        let a = save(&Document::new()).unwrap();
        assert_eq!(save(&load(&a).unwrap()).unwrap(), a);
    }

    #[test]
    fn f64_values_round_trip_exactly() {
        let doc = sample();
        let loaded = load(&save(&doc).unwrap()).unwrap();
        let odd = loaded
            .entities()
            .iter()
            .find_map(|(_, e)| match e.geom {
                Geom::Point { pos } if e.construction => Some(pos),
                _ => None,
            })
            .unwrap();
        assert_eq!(odd.x.to_bits(), (0.1f64 + 0.2).to_bits());
    }

    /// The v2 sample: `sample()` plus one layer per remaining operation, each
    /// with non-default parameters.
    pub(crate) fn sample_v2() -> Document {
        use crate::operation::{Compensation, DragKnifeParams};
        let mut doc = sample();
        doc.layers.get_mut(&L).unwrap().operation = Operation::Cut(CutParams {
            kerf_width: 0.15,
            compensation: Compensation::Outside,
            passes: 2,
        });
        let ops = [
            Operation::Score,
            Operation::Engrave(EngraveParams {
                hatch_spacing: 0.25,
                hatch_angle: 45.0,
                crosshatch: false,
                // What every older file loads as (#455): Line + Fill.
                mode: crate::operation::EngraveMode::LineFill,
                overlap: crate::operation::Overlap::EvenOdd,
            }),
            Operation::Draw,
            Operation::DragKnife(DragKnifeParams { blade_offset: 0.5 }),
            Operation::Reference,
        ];
        for (n, operation) in ops.into_iter().enumerate() {
            let id = LayerId::new(doc.alloc_id().unwrap());
            doc.layers.insert(
                id,
                Layer {
                    name: format!("{} layer", operation.kind().name()),
                    colour: [10 * n as u8, 20, 30],
                    operation,
                    export_order: n as u32 + 1,
                    visible: true,
                    locked: false,
                    export_enabled: true,
                    stroke_styles: Default::default(),
                },
            );
        }
        doc
    }

    #[test]
    fn every_operation_round_trips_byte_identically() {
        let doc = sample_v2();
        let a = save(&doc).unwrap();
        let loaded = load(&a).unwrap();
        assert_eq!(loaded, doc);
        assert_eq!(save(&loaded).unwrap(), a);
    }

    /// The v3 sample: `sample_v2()` plus a boolean recipe: the plate minus
    /// its hole, as a subtract of the two regions.
    pub(crate) fn sample_v3() -> Document {
        use locus_region::BoolOp;
        let mut doc = sample_v2();
        let regions = crate::regions::regions(&doc);
        assert_eq!(regions.len(), 2, "plate and hole");
        let by_area = |big: bool| {
            let r = regions.iter().max_by(|a, b| {
                let (x, y) = (a.area(), b.area());
                if big {
                    x.total_cmp(&y)
                } else {
                    y.total_cmp(&x)
                }
            });
            r.unwrap().key.clone()
        };
        let id = BooleanId::new(doc.alloc_id().unwrap());
        doc.booleans.insert(
            id,
            Boolean {
                op: BoolOp::Subtract,
                targets: vec![by_area(true), by_area(false)],
                tools: vec![by_area(false)],
            },
        );
        doc
    }

    /// The v4 sample: `sample_v3()` plus a Bézier chain: a line, then a
    /// Bézier tangent to it, then a second Bézier tangent to the first.
    pub(crate) fn sample_v4() -> Document {
        let mut ed = Editor::new(sample_v3());
        let mut add = |geom| {
            ed.apply(&Command::AddEntity {
                layer: L,
                construction: false,
                style: Default::default(),
                geom,
            })
            .unwrap()
            .entities[0]
        };
        let pt = |x, y| Geom::Point {
            pos: Vec2::new(x, y),
        };
        let p = [
            add(pt(70.0, 0.0)),
            add(pt(80.0, 0.0)),
            add(pt(90.0, 0.0)),
            add(pt(100.0, 10.0)),
            add(pt(105.0, 20.0)),
            add(pt(110.0, 30.0)),
            add(pt(120.0, 30.0)),
            add(pt(125.0, 35.0)),
        ];
        let line = add(Geom::Line {
            start: p[0],
            end: p[1],
        });
        let b1 = add(Geom::Bezier {
            start: p[1],
            start_ctrl: p[2],
            end_ctrl: p[3],
            end: p[4],
        });
        let b2 = add(Geom::Bezier {
            start: p[4],
            start_ctrl: p[5],
            end_ctrl: p[6],
            end: p[7],
        });
        for constraint in [
            Constraint::Tangent { a: line, b: b1 },
            Constraint::Tangent { a: b1, b: b2 },
            Constraint::Fix { point: p[0] },
        ] {
            ed.apply(&Command::AddConstraint { constraint }).unwrap();
        }
        ed.into_document()
    }

    /// The v5 sample: `sample_v4()` plus a mirror of the hole about the
    /// plate's left edge (SPEC §6.2.1, #166).
    pub(crate) fn sample_v5() -> Document {
        let mut ed = Editor::new(sample_v4());
        ed.apply(&Command::AddRecipe {
            kind: crate::recipe::RecipeKind::Mirror {
                axis: EntityId::new(9),
            },
            sources: vec![EntityId::new(11)],
        })
        .unwrap();
        ed.into_document()
    }

    /// The v6 sample: `sample_v5()` plus a parameter and a dimension that
    /// uses it (SPEC §5).
    pub(crate) fn sample_v6() -> Document {
        let mut ed = Editor::new(sample_v5());
        ed.apply(&Command::SetParameter {
            name: "plate".to_owned(),
            expr: "width / 2".to_owned(),
        })
        .unwrap();
        ed.apply(&Command::SetDimensionExpr {
            id: ConstraintId::new(18),
            expr: Some("plate * 2".to_owned()),
        })
        .unwrap();
        ed.into_document()
    }

    /// The v7 sample: `sample_v6()` with crosshatch on its Engrave layer
    /// (SPEC §7, #175).
    pub(crate) fn sample_v7() -> Document {
        let mut doc = sample_v6();
        for l in doc.layers.values_mut() {
            if let Operation::Engrave(p) = &mut l.operation {
                p.crosshatch = true;
            }
        }
        doc
    }

    /// The v8 sample: `sample_v7()` plus a live offset of the hole
    /// (SPEC §6.2, #164).
    pub(crate) fn sample_v8() -> Document {
        let mut ed = Editor::new(sample_v7());
        ed.apply(&Command::AddRecipe {
            kind: crate::recipe::RecipeKind::Offset {
                distance: 1.5,
                sharp: false,
                expr: None,
            },
            sources: vec![EntityId::new(11)],
        })
        .unwrap();
        ed.into_document()
    }

    /// The v9 sample: `sample_v8()` plus a text entity (SPEC §9, #179), laid
    /// out with a bundled single-line font so the fixture stays small.
    pub(crate) fn sample_v9() -> Document {
        use locus_text::{Align, BundledFont, BundledProvider, FontRef};
        let mut ed = Editor::new(sample_v8());
        let mut pt = |x: f64, y: f64| {
            ed.apply(&Command::AddEntity {
                layer: L,
                construction: false,
                style: Default::default(),
                geom: Geom::Point {
                    pos: Vec2::new(x, y),
                },
            })
            .unwrap()
            .entities[0]
        };
        let (anchor, dir) = (pt(5.0, 50.0), pt(15.0, 50.0));
        let text = crate::text::TextData::new(
            "Locus",
            FontRef::bundled(BundledFont::HersheySans),
            6.0,
            Align::Left,
            1.0,
            &BundledProvider,
        )
        .unwrap();
        ed.apply(&Command::AddEntity {
            layer: L,
            construction: false,
            style: Default::default(),
            geom: Geom::Text {
                anchor,
                dir,
                text: Box::new(text),
            },
        })
        .unwrap();
        ed.into_document()
    }

    /// The v10 sample: `sample_v9()` plus a plugin object (SPEC §15.5,
    /// #183): a 10 mm square with one constraint, as a generator returns it.
    /// The v12 sample: `sample_v11()` with a 0.2 mm kerf and its radius
    /// dimension measured to the circle's outside edge (SPEC §5, #239).
    pub(crate) fn sample_v12() -> Document {
        let mut ed = Editor::new(sample_v11());
        ed.apply(&Command::SetLayerProps {
            id: Document::default_layer(),
            props: crate::command::LayerProps {
                operation: Some(Operation::Cut(CutParams {
                    kerf_width: 0.2,
                    ..CutParams::default()
                })),
                ..Default::default()
            },
        })
        .unwrap();
        ed.apply(&Command::SetDimensionEdge {
            id: ConstraintId::new(19),
            edge: crate::constraint::Edge::Outside,
        })
        .unwrap();
        ed.into_document()
    }

    pub(crate) fn sample_v11() -> Document {
        let mut d = sample_v10();
        d.profile = Some("lightburn".to_owned());
        d
    }

    pub(crate) fn sample_v10() -> Document {
        use crate::generated::{
            GenConstraint, GenEntity, GeneratedGeometry, PluginParam, PluginRef, PluginValue,
        };
        let mut ed = Editor::new(sample_v9());
        ed.apply(&Command::AddGenerated {
            layer: L,
            origin: Vec2::new(60.0, 10.0),
            plugin: PluginRef {
                id: "com.example.square".into(),
                version: "1.0.0".into(),
                params: vec![PluginParam {
                    key: "size".into(),
                    value: PluginValue::Number(10.0),
                }],
            },
            output: GeneratedGeometry {
                points: vec![
                    Vec2::new(0.0, 0.0),
                    Vec2::new(10.0, 0.0),
                    Vec2::new(10.0, 10.0),
                    Vec2::new(0.0, 10.0),
                ],
                entities: vec![
                    GenEntity::Line(0, 1),
                    GenEntity::Line(1, 2),
                    GenEntity::Line(2, 3),
                    GenEntity::Line(3, 0),
                ],
                constraints: vec![GenConstraint::Horizontal(0)],
            },
        })
        .unwrap();
        ed.into_document()
    }

    /// The v9 fixture is frozen; it migrates to v10 with no plugin objects.
    #[test]
    fn v9_fixture_migrates_to_v10() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/v9/plate.locus");
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.contains("format_version: 9"));
        assert!(text.contains("Text("));
        let doc = load(&text).unwrap();
        assert_eq!(doc, sample_v9());
        assert!(save(&doc).unwrap().contains(&current()));
    }

    /// The v10 fixture is frozen; it migrates to v11 with no profile.
    #[test]
    fn v10_fixture_migrates_to_v11() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/v10/plate.locus"
        );
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.contains("format_version: 10"));
        assert!(text.contains("Generated("));
        let doc = load(&text).unwrap();
        assert_eq!(doc, sample_v10());
        assert_eq!(doc.profile(), None);
        assert!(save(&doc).unwrap().contains(&current()));
    }

    /// The v11 fixture is frozen; it migrates to v12 (no stroke edges).
    #[test]
    fn v11_fixture_migrates_to_v12() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/v11/plate.locus"
        );
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.contains("format_version: 11"));
        assert!(text.contains("profile: Some(\"lightburn\")"));
        let doc = load(&text).unwrap();
        assert_eq!(doc, sample_v11());
        let out = save(&doc).unwrap();
        assert!(out.contains(&current()));
        assert!(!out.contains("edge:"), "centreline edges are not written");
    }

    /// The v12 fixture is frozen; it migrates to v13 (its Draw layer holds
    /// no geometry, so only the layer changes).
    #[test]
    fn v12_fixture_migrates_to_v13() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/v12/plate.locus"
        );
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.contains("format_version: 12"));
        assert!(text.contains("pen_width: 0.7"));
        let doc = load(&text).unwrap();
        assert_eq!(doc, sample_v12());
        let out = save(&doc).unwrap();
        assert!(out.contains(&current()));
        assert!(!out.contains("pen_width"), "the pen left the layer");
    }

    /// v12 → v13 copies each Draw layer's pen onto its entities (#287). The
    /// default pen migrates to the default style, which is not written.
    #[test]
    fn v12_pens_move_onto_their_entities() {
        use crate::style::Style;
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/v12/pens.locus");
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.contains("format_version: 12"));
        let doc = load(&text).unwrap();
        let style = |id: u64| doc.entity(EntityId::new(id)).unwrap().style.clone();
        for id in [4, 5, 6] {
            assert_eq!((style(id).width, style(id).pen), (0.7, 3), "entity {id}");
        }
        for id in [7, 8, 9, 10, 11, 12] {
            assert_eq!(style(id), Style::default(), "entity {id}");
        }
        for l in doc.layers().values() {
            l.operation.validate().unwrap();
        }
        assert_eq!(doc.stroke_width(EntityId::new(6)), 0.7);
        let out = save(&doc).unwrap();
        assert!(out.contains(&current()));
        assert_eq!(
            out.matches("width: 0.7").count(),
            3,
            "three styled entities"
        );
        // Save → load → save is byte-identical (SPEC §14).
        assert_eq!(save(&load(&out).unwrap()).unwrap(), out);
    }

    /// What an older sample loads as since v19 (#451): its object fills
    /// moved onto the regions they enclosed.
    pub(crate) fn migrated(mut d: Document) -> Document {
        crate::region_fill::migrate_object_fills(&mut d);
        d
    }

    /// The v13 sample: `sample_v12()` with a styled line (SPEC §4, #287).
    /// Its blue fill is set on the object directly, as a v13 file stored it:
    /// commands refuse object fills since v19 (#451).
    pub(crate) fn sample_v13() -> Document {
        use crate::style::{Fill, Stroke, StylePatch};
        let mut ed = Editor::new(sample_v12());
        ed.apply(&Command::SetStyle {
            entities: vec![EntityId::new(6)],
            patch: StylePatch {
                stroke: Some(Stroke::Colour([200, 10, 10])),
                width: Some(0.5),
                pen: Some(2),
                fill: None,
                opacity: Some(0.5),
                fill_on: None,
                align: Default::default(),
                cap: Default::default(),
                join: Default::default(),
                dash: Default::default(),
                extra: Default::default(),
                profile: Default::default(),
            },
        })
        .unwrap();
        let mut d = ed.into_document();
        d.entities.get_mut(EntityId::new(6)).unwrap().style.fill = Fill::Colour([0, 0, 255]);
        d
    }

    /// The v13 fixture is frozen; it migrates to v14 unchanged (no text
    /// has letter spacing, which is all v14 adds).
    #[test]
    fn v13_fixture_migrates_to_v14() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/v13/plate.locus"
        );
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.contains("format_version: 13"));
        assert!(text.contains("edge: Outside"));
        assert!(text.contains("style: ("));
        let doc = load(&text).unwrap();
        assert_eq!(doc, migrated(sample_v13()));
        // The object fill moved onto its region (#451), so the file changes;
        // the document survives save then load.
        let out = save(&doc).unwrap();
        assert!(out.contains(&current()));
        assert_eq!(load(&out).unwrap(), doc);
    }

    /// The v14 sample: `sample_v13()` with letter-spaced text (#222).
    pub(crate) fn sample_v14() -> Document {
        use locus_text::{Align, BundledFont, BundledProvider, FontRef};
        let mut ed = Editor::new(sample_v13());
        let mut point = |x: f64| {
            ed.apply(&Command::AddEntity {
                layer: L,
                construction: false,
                geom: Geom::Point {
                    pos: Vec2::new(x, 50.0),
                },
                style: Default::default(),
            })
            .unwrap()
            .entities[0]
        };
        let (anchor, dir) = (point(0.0), point(10.0));
        let text = crate::text::TextData::new(
            "LO",
            FontRef::bundled(BundledFont::HersheySans),
            8.0,
            Align::Left,
            1.0,
            &BundledProvider,
        )
        .unwrap()
        .edited(&BundledProvider, |t| t.tracking = 1.5)
        .unwrap();
        ed.apply(&Command::AddEntity {
            layer: L,
            construction: false,
            geom: Geom::Text {
                anchor,
                dir,
                text: Box::new(text),
            },
            style: Default::default(),
        })
        .unwrap();
        ed.into_document()
    }

    /// The v14 fixture is frozen; it migrates to v15 unchanged (no fill is
    /// turned off, which is all v15 adds).
    #[test]
    fn v14_fixture_migrates_to_v15() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/v14/plate.locus"
        );
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.contains("format_version: 14"));
        assert!(text.contains("tracking: 1.5"));
        let doc = load(&text).unwrap();
        assert_eq!(doc, migrated(sample_v14()));
        // The object fill moved onto its region (#451), so the file changes;
        // the document survives save then load.
        let out = save(&doc).unwrap();
        assert!(out.contains(&current()));
        assert_eq!(load(&out).unwrap(), doc);
    }

    /// The v15 sample: `sample_v14()` with the blue fill turned off, its
    /// colour kept (#388).
    pub(crate) fn sample_v15() -> Document {
        use crate::style::Fill;
        let doc = sample_v14();
        let filled: Vec<EntityId> = doc
            .entities()
            .iter()
            .filter(|(_, e)| e.style.fill == Fill::Colour([0, 0, 255]))
            .map(|(id, _)| id)
            .collect();
        assert!(!filled.is_empty(), "sample_v13 has a blue fill");
        // Set directly, as a v15 file stored it (#451: no object fills now).
        let mut doc = doc;
        for id in filled {
            let e = doc.entities.get_mut(id).unwrap();
            e.style.fill = e.style.fill.switched(false, None);
        }
        doc
    }

    /// The v15 fixture is frozen; it migrates to the current version
    /// unchanged (it has no image, which is all v16 adds).
    #[test]
    fn v15_fixture_migrates_to_current() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/v15/plate.locus"
        );
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.contains("format_version: 15"));
        assert!(text.contains("fill: Off((0, 0, 255))"));
        let doc = load(&text).unwrap();
        assert_eq!(doc, migrated(sample_v15()));
        // The object fill moved onto its region (#451), so the file changes;
        // the document survives save then load.
        let out = save(&doc).unwrap();
        assert!(out.contains(&current()));
        assert_eq!(load(&out).unwrap(), doc);
    }

    /// The v16 sample: `sample_v15()` with a reference image (#390).
    pub(crate) fn sample_v16() -> Document {
        let mut ed = Editor::new(sample_v15());
        let layer = LayerId::new(ed.document().next_id());
        let image = crate::image::tiny_png();
        ed.apply(&Command::Compound {
            label: "Import".to_owned(),
            commands: vec![
                Command::AddLayer {
                    name: "photo.png".to_owned(),
                    colour: crate::reference::REFERENCE_COLOUR,
                    operation: crate::operation::Operation::Reference,
                },
                Command::AddImage {
                    layer,
                    image: Box::new(image),
                },
                Command::SetLayerLocked {
                    id: layer,
                    locked: true,
                },
            ],
        })
        .unwrap();
        ed.into_document()
    }

    /// The v16 fixture is frozen; it migrates to the current version
    /// unchanged (no text on a path, which is all v17 adds).
    #[test]
    fn v16_fixture_migrates_to_current() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/v16/plate.locus"
        );
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.contains("format_version: 16"));
        assert!(text.contains("images: {"));
        let doc = load(&text).unwrap();
        assert_eq!(doc, migrated(sample_v16()));
        // The object fill moved onto its region (#451), so the file changes;
        // the document survives save then load.
        let out = save(&doc).unwrap();
        assert!(out.contains(&current()));
        assert_eq!(load(&out).unwrap(), doc);
    }

    /// The v17 sample: `sample_v16()` with its letter-spaced text bound to
    /// the plate's first edge, below it and slid along (#392).
    pub(crate) fn sample_v17() -> Document {
        use crate::path_text::{PathSide, TextPath};
        use locus_text::BundledProvider;
        let mut ed = Editor::new(sample_v16());
        let doc = ed.document();
        let edge = doc
            .entities()
            .iter()
            .find(|(_, e)| e.layer == L && matches!(e.geom, Geom::Line { .. }))
            .map(|(id, _)| id)
            .unwrap();
        let (id, (data, anchor, dir)) = doc
            .entities()
            .iter()
            .find_map(|(id, _)| doc.text(id).map(|t| (id, t)))
            .unwrap();
        let bound = data
            .edited(&BundledProvider, |t| {
                let mut p = TextPath::new(vec![edge]);
                p.side = PathSide::Below;
                p.offset = 2.0;
                t.path = Some(Box::new(p));
            })
            .unwrap();
        ed.apply(&Command::ModifyEntity {
            id,
            geom: Geom::Text {
                anchor,
                dir,
                text: Box::new(bound),
            },
        })
        .unwrap();
        ed.into_document()
    }

    /// The v17 fixture is frozen; it migrates to the current version
    /// unchanged (no placement, which is all v18 adds).
    #[test]
    fn v17_fixture_migrates_to_current() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/v17/plate.locus"
        );
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.contains("format_version: 17"));
        assert!(text.contains("path: Some(("), "{text}");
        let doc = load(&text).unwrap();
        assert_eq!(doc, migrated(sample_v17()));
        assert_eq!(doc.placement(), None);
        // The object fill moved onto its region (#451), so the file changes;
        // the document survives save then load.
        let out = save(&doc).unwrap();
        assert!(out.contains(&current()));
        assert_eq!(load(&out).unwrap(), doc);
    }

    /// The v18 sample: `sample_v17()` placed on the bed (#443).
    pub(crate) fn sample_v18() -> Document {
        let mut ed = Editor::new(sample_v17());
        ed.apply(&Command::SetPlacement {
            offset: Some(locus_geom::Vec2::new(12.5, 30.0)),
        })
        .unwrap();
        ed.into_document()
    }

    /// The v18 fixture is frozen; it migrates to the current version with
    /// its object fill moved onto the region it enclosed (#451).
    #[test]
    fn v18_fixture_migrates_to_current() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/v18/plate.locus"
        );
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.contains("format_version: 18"));
        let doc = load(&text).unwrap();
        assert_eq!(doc, migrated(sample_v18()));
        let out = save(&doc).unwrap();
        assert!(out.contains(&current()));
        assert_eq!(load(&out).unwrap(), doc);
    }

    /// #451: a v13-era blue fill on one edge of the plate fills the plate's
    /// region (at the object's 50 % opacity), as the canvas showed it; the
    /// hole and the regions inside it stay open, and no object keeps a fill.
    #[test]
    fn object_fills_move_onto_the_regions_they_enclose() {
        use crate::style::Fill;
        let doc = migrated(sample_v13());
        assert!(
            doc.entities()
                .iter()
                .all(|(_, e)| e.style.fill == Fill::None)
        );
        let regions = crate::regions::regions(&doc);
        let filled: Vec<_> = regions
            .iter()
            .filter_map(|r| doc.region_fill(&r.key).map(|f| (r, f)))
            .collect();
        assert_eq!(filled.len(), 1, "{filled:?}");
        let (r, f) = filled[0];
        assert_eq!(f.fill, Fill::Colour([0, 0, 255]));
        assert!((f.opacity - 0.5).abs() < 1e-12);
        assert!(r.key.bounds.contains(&6), "the plate, around edge 6");
        assert!(!r.holes.is_empty(), "the hole is not filled");
    }

    /// The v19 sample: `sample_v18()` migrated, with a second region filled
    /// red at 40 % (#451).
    pub(crate) fn sample_v19() -> Document {
        let doc = migrated(sample_v18());
        let regions = crate::regions::regions(&doc);
        let free = regions
            .iter()
            .find(|r| doc.region_fill(&r.key).is_none())
            .expect("an unfilled region")
            .key
            .clone();
        let mut ed = Editor::new(doc);
        ed.apply(&Command::SetRegionFill {
            regions: vec![free],
            fill: Some(crate::region_fill::RegionFill {
                fill: crate::style::Fill::Colour([255, 0, 0]),
                opacity: 0.4,
                gradient: None,
            }),
        })
        .unwrap();
        ed.into_document()
    }

    /// The v19 fixture is frozen; it migrates to the current version
    /// unchanged: its Engrave layer reads as Line + Fill, even-odd, which is
    /// what it exported (#455).
    #[test]
    fn v19_fixture_migrates_to_current() {
        use crate::operation::{EngraveMode, Overlap};
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/v19/plate.locus"
        );
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.contains("format_version: 19"));
        assert!(!text.contains("mode:"), "v19 had no engrave mode");
        let doc = load(&text).unwrap();
        assert_eq!(doc, sample_v19());
        let engrave: Vec<_> = doc
            .layers()
            .values()
            .filter_map(|l| match l.operation {
                Operation::Engrave(p) => Some(p),
                _ => None,
            })
            .collect();
        assert!(!engrave.is_empty(), "the sample has an Engrave layer");
        assert!(
            engrave
                .iter()
                .all(|p| p.mode == EngraveMode::LineFill && p.overlap == Overlap::EvenOdd)
        );
        let out = save(&doc).unwrap();
        assert!(out.contains(&current()));
        assert_eq!(load(&out).unwrap(), doc);
    }

    /// The v20 sample: `sample_v19()` with its Engrave layer set to Offset
    /// fill with overlaps unioned (#455).
    pub(crate) fn sample_v20() -> Document {
        use crate::operation::{EngraveMode, Overlap};
        let doc = sample_v19();
        let (id, p) = doc
            .layers()
            .iter()
            .find_map(|(id, l)| match l.operation {
                Operation::Engrave(p) => Some((*id, p)),
                _ => None,
            })
            .expect("an Engrave layer");
        let mut ed = Editor::new(doc);
        ed.apply(&Command::SetLayerProps {
            id,
            props: crate::command::LayerProps {
                operation: Some(Operation::Engrave(EngraveParams {
                    mode: EngraveMode::OffsetFill,
                    overlap: Overlap::Union,
                    ..p
                })),
                ..Default::default()
            },
        })
        .unwrap();
        ed.into_document()
    }

    /// The v20 fixture is frozen; it migrates to the current version
    /// unchanged: none of its region keys named a face another layer cut.
    #[test]
    fn v20_fixture_migrates_to_current() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/v20/plate.locus"
        );
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.contains("format_version: 20"));
        let doc = load(&text).unwrap();
        assert_eq!(doc, sample_v20());
        let out = save(&doc).unwrap();
        assert!(out.contains(&current()));
        assert_eq!(load(&out).unwrap(), doc);
    }

    /// The v21 sample is `sample_v20()`: v21 changes what region keys mean,
    /// not the shape of the file.
    pub(crate) fn sample_v21() -> Document {
        sample_v20()
    }

    /// The v21 fixture is frozen; it loads unchanged (no image rotation,
    /// which is all v22 adds) and saves as the current version.
    #[test]
    fn v21_fixture_migrates_to_current() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/v21/plate.locus"
        );
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.contains("format_version: 21"));
        let doc = load(&text).unwrap();
        assert_eq!(doc, sample_v21());
        let out = save(&doc).unwrap();
        assert!(out.contains(&current()));
        assert_eq!(load(&out).unwrap(), doc);
    }

    /// The v22 sample: `sample_v21()` with its reference image moved, turned
    /// a quarter and scaled by one `TransformReference` each (#463).
    pub(crate) fn sample_v22() -> Document {
        use crate::transform::Transform;
        let doc = sample_v21();
        let layer = *doc.images().keys().next().expect("an image");
        let mut ed = Editor::new(doc);
        for transform in [
            Transform::Move(locus_geom::Vec2::new(5.0, -2.0)),
            Transform::Rotate {
                center: locus_geom::Vec2::new(1.0, 1.0),
                angle: std::f64::consts::FRAC_PI_2,
            },
            Transform::Scale {
                center: locus_geom::Vec2::new(0.0, 0.0),
                factor: 2.0,
            },
        ] {
            ed.apply(&Command::TransformReference { layer, transform })
                .unwrap();
        }
        ed.into_document()
    }

    /// The v22 fixture is frozen; it loads unchanged (square pixels, which
    /// is all v23 adds) and saves as the current version.
    #[test]
    fn v22_fixture_migrates_to_current() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/v22/plate.locus"
        );
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.contains("format_version: 22"));
        let doc = load(&text).unwrap();
        assert_eq!(doc, sample_v22());
        let out = save(&doc).unwrap();
        assert!(out.contains(&current()));
        assert_eq!(load(&out).unwrap(), doc);
    }

    /// The v23 sample: `sample_v22()` with its turned image scaled freely
    /// along its own edges (#463).
    pub(crate) fn sample_v23() -> Document {
        use crate::transform::Transform;
        let doc = sample_v22();
        let layer = *doc.images().keys().next().expect("an image");
        let img = doc.image(layer).cloned().expect("an image");
        let mut ed = Editor::new(doc);
        ed.apply(&Command::TransformReference {
            layer,
            transform: Transform::Stretch {
                center: img.centre(),
                sx: 2.0,
                sy: 0.5,
                angle: img.placement.rotation,
            },
        })
        .unwrap();
        ed.into_document()
    }

    /// The v23 fixture is frozen; it loads unchanged (no suppressed
    /// feature, which is all v24 adds) and saves as the current version.
    #[test]
    fn v23_fixture_migrates_to_current() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/v23/plate.locus"
        );
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.contains("format_version: 23"));
        let doc = load(&text).unwrap();
        assert_eq!(doc, sample_v23());
        let out = save(&doc).unwrap();
        assert!(out.contains(&current()));
        assert_eq!(load(&out).unwrap(), doc);
    }

    /// The v24 sample: `sample_v23()` with its first pattern-or-offset
    /// feature suppressed (#446).
    pub(crate) fn sample_v24() -> Document {
        let doc = sample_v23();
        let id = doc
            .recipes()
            .iter()
            .find(|(_, r)| !matches!(r.kind, crate::recipe::RecipeKind::Mirror { .. }))
            .map(|(id, _)| *id)
            .expect("a pattern, offset or plugin object");
        let mut ed = Editor::new(doc);
        ed.apply(&Command::SetRecipeSuppressed {
            id,
            suppressed: true,
        })
        .unwrap();
        ed.into_document()
    }

    /// The v24 fixture is frozen; it loads unchanged (no line styles, which
    /// is all v25 adds) and saves as the current version.
    #[test]
    fn v24_fixture_migrates_to_current() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/v24/plate.locus"
        );
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.contains("format_version: 24"));
        let doc = load(&text).unwrap();
        assert_eq!(doc, sample_v24());
        let out = save(&doc).unwrap();
        assert!(out.contains(&current()));
        assert!(!out.contains("cap:") && !out.contains("join:") && !out.contains("stroke_styles"));
        assert_eq!(load(&out).unwrap(), doc);
    }

    /// The v25 sample: `sample_v24()` with one curve given square caps and
    /// miter joins, and its layer set to cut the styled outline (#428).
    pub(crate) fn sample_v25() -> Document {
        let doc = sample_v24();
        let (id, layer) = doc
            .entities()
            .iter()
            .find(|(_, e)| matches!(e.geom, Geom::Line { .. }) && !e.construction)
            .map(|(i, e)| (i, e.layer))
            .expect("a line");
        let mut ed = Editor::new(doc);
        ed.apply(&Command::SetStyle {
            entities: vec![id],
            patch: crate::style::StylePatch {
                cap: Some(locus_stroke::Cap::Square),
                join: Some(locus_stroke::Join::Miter { limit: 6.0 }),
                ..crate::style::StylePatch::default()
            },
        })
        .unwrap();
        ed.apply(&Command::SetLayerProps {
            id: layer,
            props: crate::command::LayerProps {
                stroke_styles: Some(crate::style::StrokeStyles::Styled),
                ..crate::command::LayerProps::default()
            },
        })
        .unwrap();
        ed.into_document()
    }

    /// The v25 fixture is frozen; it loads unchanged (creation order,
    /// which is all v26 adds) and saves as the current version.
    #[test]
    fn v25_fixture_migrates_to_current() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/v25/plate.locus"
        );
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.contains("format_version: 25"));
        let doc = load(&text).unwrap();
        assert_eq!(doc, sample_v25());
        let out = save(&doc).unwrap();
        assert!(out.contains(&current()));
        assert_eq!(load(&out).unwrap(), doc);
    }

    /// The v26 sample: `sample_v25()` with its newest feature moved first
    /// in the timeline (#446).
    pub(crate) fn sample_v26() -> Document {
        let doc = sample_v25();
        let last = *doc.recipe_order().last().expect("a feature");
        let mut ed = Editor::new(doc);
        ed.apply(&Command::ReorderRecipe { id: last, to: 0 })
            .unwrap();
        ed.into_document()
    }

    /// The v26 fixture is frozen; it loads unchanged and saves as the
    /// current version.
    #[test]
    fn v26_fixture_migrates_to_current() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/v26/plate.locus"
        );
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.contains("format_version: 26"));
        let doc = load(&text).unwrap();
        assert_eq!(doc, sample_v26());
        let out = save(&doc).unwrap();
        assert!(out.contains(&current()));
        assert!(!out.contains("pattern:"));
        assert_eq!(load(&out).unwrap(), doc);
    }

    pub(crate) fn sample_v27() -> Document {
        // `sample_v26()` with its styled line dashed 3/2, aligned to corners.
        let doc = sample_v26();
        let id = doc
            .entities()
            .iter()
            .find(|(_, e)| e.style.line.cap == locus_stroke::Cap::Square)
            .map(|(i, _)| i)
            .expect("the styled line");
        let mut ed = Editor::new(doc);
        ed.apply(&Command::SetStyle {
            entities: vec![id],
            patch: crate::style::StylePatch {
                dash: Some(Some(
                    locus_stroke::Dash::new(&[3.0, 2.0], 0.5, locus_stroke::DashAlign::Corners)
                        .unwrap(),
                )),
                ..crate::style::StylePatch::default()
            },
        })
        .unwrap();
        ed.into_document()
    }

    /// The v27 fixture is frozen; it loads unchanged (no gradients, which
    /// is all v28 adds) and saves as the current version.
    #[test]
    fn v27_fixture_migrates_to_current() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/v27/plate.locus"
        );
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.contains("format_version: 27"));
        let doc = load(&text).unwrap();
        assert_eq!(doc, sample_v27());
        let out = save(&doc).unwrap();
        assert!(out.contains(&current()));
        assert!(!out.contains("gradient:"));
        assert_eq!(load(&out).unwrap(), doc);
    }

    /// The v28 sample: `sample_v27()` with its first filled region given an
    /// elliptical gradient, turned and not following the shape (#436).
    pub(crate) fn sample_v28() -> Document {
        use crate::gradient::{Gradient, GradientKind};
        let doc = sample_v27();
        let (key, fill) = doc
            .region_fills()
            .iter()
            .next()
            .map(|(k, f)| (k.clone(), f.clone()))
            .expect("a region fill");
        let mut g = Gradient::linear_from([30, 90, 200]);
        g.kind = GradientKind::Elliptical { ratio: 0.5 };
        g.from = locus_geom::Vec2::new(0.5, 0.5);
        g.frame_angle = 0.25;
        g.rotate_with_shape = false;
        let mut ed = Editor::new(doc);
        ed.apply(&Command::SetRegionFill {
            regions: vec![key],
            fill: Some(crate::region_fill::RegionFill {
                gradient: Some(Box::new(g)),
                ..fill
            }),
        })
        .unwrap();
        ed.into_document()
    }

    /// The v28 fixture is frozen; it loads unchanged and saves as the
    /// current version.
    #[test]
    fn v28_fixture_migrates_to_current() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/v28/plate.locus"
        );
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.contains("format_version: 28"));
        let doc = load(&text).unwrap();
        assert_eq!(doc, sample_v28());
        let out = save(&doc).unwrap();
        assert!(out.contains(&current()));
        assert!(!out.contains("extra:"));
        assert_eq!(load(&out).unwrap(), doc);
    }

    pub(crate) fn sample_v29() -> Document {
        // `sample_v28()` with two extra strokes on its styled line: one
        // red, 1.5 mm to the left, one dashed, 1.5 mm to the right.
        let doc = sample_v28();
        let id = doc
            .entities()
            .iter()
            .find(|(_, e)| e.style.line.cap == locus_stroke::Cap::Square)
            .map(|(i, _)| i)
            .expect("the styled line");
        let mut ed = Editor::new(doc);
        ed.apply(&Command::SetStyle {
            entities: vec![id],
            patch: crate::style::StylePatch {
                extra: Some(vec![
                    locus_stroke::StrokeLayer {
                        offset: 1.5,
                        width: 0.3,
                        colour: Some([200, 0, 0]),
                        ..locus_stroke::StrokeLayer::default()
                    },
                    locus_stroke::StrokeLayer {
                        offset: -1.5,
                        width: 0.2,
                        pen: Some(2),
                        dash: Some(
                            locus_stroke::Dash::new(
                                &[1.0, 1.0],
                                0.0,
                                locus_stroke::DashAlign::Exact,
                            )
                            .unwrap(),
                        ),
                        ..locus_stroke::StrokeLayer::default()
                    },
                ]),
                ..crate::style::StylePatch::default()
            },
        })
        .unwrap();
        ed.into_document()
    }

    #[test]
    fn v29_fixture_migrates() {
        let text = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/v29/plate.locus"
        ))
        .unwrap();
        assert!(text.contains("format_version: 29"));
        let doc = load(&text).unwrap();
        assert_eq!(doc, sample_v29());
        let out = save(&doc).unwrap();
        assert!(out.contains(&current()));
        assert!(!out.contains("profile: Some(("));
        assert_eq!(load(&out).unwrap(), doc);
    }

    pub(crate) fn sample_v30() -> Document {
        // `sample_v29()` with an asymmetric width profile on its styled line.
        let doc = sample_v29();
        let id = doc
            .entities()
            .iter()
            .find(|(_, e)| e.style.line.cap == locus_stroke::Cap::Square)
            .map(|(i, _)| i)
            .expect("the styled line");
        let mut ed = Editor::new(doc);
        let p = locus_stroke::WidthProfile {
            points: vec![
                locus_stroke::WidthPoint::new(0.0, 0.0, 0.0),
                locus_stroke::WidthPoint::new(0.4, 2.0, 1.0),
                locus_stroke::WidthPoint::new(1.0, 0.5, 0.5),
            ],
            seed: None,
        };
        ed.apply(&Command::SetStyle {
            entities: vec![id],
            patch: crate::style::StylePatch {
                profile: Some(Some(p)),
                ..crate::style::StylePatch::default()
            },
        })
        .unwrap();
        ed.into_document()
    }

    #[test]
    fn v30_fixture_is_current_and_loads() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/v30/plate.locus"
        );
        if std::env::var_os("LOCUS_UPDATE_FIXTURES").is_some() {
            std::fs::create_dir_all(std::path::Path::new(path).parent().unwrap()).unwrap();
            std::fs::write(path, save(&sample_v30()).unwrap()).unwrap();
        }
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.contains("format_version: 30"));
        assert!(text.contains("profile: Some(("));
        assert_eq!(load(&text).unwrap(), sample_v30());
        assert_eq!(
            save(&sample_v30()).unwrap(),
            text,
            "fixture differs: output format changed (ADR-0004 rule 3)"
        );
    }

    #[test]
    fn a_hostile_width_profile_in_a_file_is_refused() {
        let good = save(&sample_v30()).unwrap();
        for bad in [
            good.replace("left: 2.0", "left: 99.0"),
            good.replace("t: 0.4", "t: 1.4"),
        ] {
            assert_ne!(bad, good);
            assert!(load(&bad).is_err());
        }
    }

    #[test]
    fn a_bad_feature_order_is_refused() {
        let text = save(&sample_v26())
            .unwrap()
            .replace("recipe_order: [", "recipe_order: [999999, ");
        assert!(load(&text).is_err());
    }

    #[test]
    fn a_hostile_dash_pattern_in_a_file_is_refused() {
        let good = save(&sample_v27()).unwrap();
        let at = good.find("pattern: [").unwrap();
        let end = at + good[at..].find(']').unwrap() + 1;
        let long = format!("pattern: [{}]", vec!["1.0"; 18].join(", "));
        for bad in [
            "pattern: [3.0]",
            "pattern: [3.0, -2.0]",
            "pattern: [0.0, 0.0]",
            long.as_str(),
        ] {
            let text = format!("{}{bad}{}", &good[..at], &good[end..]);
            assert!(load(&text).is_err(), "{bad}");
        }
        let same = format!("{}pattern: [3.0, 2.0]{}", &good[..at], &good[end..]);
        assert_eq!(load(&same).unwrap(), sample_v27());
    }

    #[test]
    fn a_bad_miter_limit_in_a_file_is_refused() {
        let text = save(&sample_v25())
            .unwrap()
            .replace("limit: 6.0", "limit: 1e9");
        assert!(load(&text).is_err());
    }

    #[test]
    fn an_unturned_image_saves_without_a_rotation() {
        // Older apps' files stay byte-identical until something is turned.
        let out = save(&sample_v21()).unwrap();
        assert!(out.contains("images: {"));
        assert!(!out.contains("rotation"));
    }

    /// The owner's file (#457): a filled circle on Layer 1, then an
    /// overlapping circle on Layer 2.
    fn two_layer_circles(fill_before_overlap: bool) -> (Document, RegionKey) {
        let mut ed = Editor::new(Document::new());
        let l1 = Document::default_layer();
        let l2 = ed
            .apply(&Command::AddLayer {
                name: "Layer 2".to_owned(),
                colour: [0, 0, 255],
                operation: Operation::Draw,
            })
            .unwrap()
            .layers[0];
        let circle = |ed: &mut Editor, layer, x: f64| {
            let c = ed
                .apply(&Command::AddEntity {
                    layer,
                    construction: false,
                    geom: Geom::Point {
                        pos: Vec2::new(x, 0.0),
                    },
                    style: Default::default(),
                })
                .unwrap()
                .entities[0];
            ed.apply(&Command::AddEntity {
                layer,
                construction: false,
                geom: Geom::Circle {
                    center: c,
                    radius: 10.0,
                },
                style: Default::default(),
            })
            .unwrap()
            .entities[0]
        };
        let c1 = circle(&mut ed, l1, 0.0);
        let key = RegionKey {
            bounds: vec![c1.get()],
            index: 0,
        };
        let red = crate::region_fill::RegionFill {
            fill: crate::style::Fill::Colour([243, 94, 94]),
            opacity: 1.0,
            gradient: None,
        };
        let fill = |ed: &mut Editor| {
            ed.apply(&Command::SetRegionFill {
                regions: vec![key.clone()],
                fill: Some(red.clone()),
            })
            .unwrap();
        };
        if fill_before_overlap {
            fill(&mut ed);
        }
        circle(&mut ed, l2, 12.0);
        if !fill_before_overlap {
            fill(&mut ed);
        }
        (ed.into_document(), key)
    }

    /// #457: another layer's shape never splits, hides or recolours a
    /// layer's region; the canvas and export resolve the same red fill.
    #[test]
    fn another_layers_overlap_keeps_the_fill() {
        let (doc, key) = two_layer_circles(true);
        let regions = crate::regions::regions(&doc);
        // One region per circle, each from its own layer.
        assert_eq!(regions.len(), 2, "{regions:?}");
        let fills = crate::region_fill::resolve(&doc, &regions);
        let i = regions
            .iter()
            .position(|r| r.key == key)
            .expect("still there");
        assert_eq!(
            fills[i].as_ref().and_then(|f| f.painted()),
            Some([243, 94, 94]),
            "the red fill survives"
        );
        assert!(
            fills.iter().filter(|f| f.is_some()).count() == 1,
            "Layer 2 stays unfilled"
        );
    }

    /// #457: a v20 file whose fill was keyed to a face cut by another
    /// layer's circle migrates to the per-layer region in the same place.
    #[test]
    fn cross_layer_keys_migrate_to_the_per_layer_region() {
        let (mut doc, key) = two_layer_circles(true);
        // The key a v20 build made: Layer 1's circle minus Layer 2's.
        let old = crate::regions::regions_across_layers(&doc);
        let crescent = old
            .iter()
            .find(|r| r.key.bounds.len() == 2 && r.contains(Vec2::new(-5.0, 0.0)))
            .expect("the crescent")
            .key
            .clone();
        let red = doc.region_fills.remove(&key).unwrap();
        doc.region_fills.insert(crescent, red.clone());
        let mut text = save(&doc).unwrap();
        text = text.replace(&current(), "format_version: 20");
        let back = load(&text).unwrap();
        assert_eq!(
            back.region_fill(&key),
            Some(&red),
            "moved to Layer 1's circle"
        );
        assert_eq!(back.region_fills().len(), 1);
    }

    /// #457: a split within one layer keeps the fill on every piece; a new
    /// region outside the old one doesn't take it.
    #[test]
    fn a_split_on_the_same_layer_keeps_the_fill_inside_only() {
        let (doc, key) = two_layer_circles(true);
        let mut ed = Editor::new(doc);
        let l1 = Document::default_layer();
        let point = |ed: &mut Editor, x: f64, y: f64| {
            ed.apply(&Command::AddEntity {
                layer: l1,
                construction: false,
                geom: Geom::Point {
                    pos: Vec2::new(x, y),
                },
                style: Default::default(),
            })
            .unwrap()
            .entities[0]
        };
        // A line across the circle, and a square beside it touching nothing.
        let (a, b) = (point(&mut ed, 0.0, -20.0), point(&mut ed, 0.0, 20.0));
        ed.apply(&Command::AddEntity {
            layer: l1,
            construction: false,
            geom: Geom::Line { start: a, end: b },
            style: Default::default(),
        })
        .unwrap();
        let sq: Vec<_> = [(-40.0, -5.0), (-30.0, -5.0), (-30.0, 5.0), (-40.0, 5.0)]
            .iter()
            .map(|&(x, y)| point(&mut ed, x, y))
            .collect();
        for i in 0..4 {
            ed.apply(&Command::AddEntity {
                layer: l1,
                construction: false,
                geom: Geom::Line {
                    start: sq[i],
                    end: sq[(i + 1) % 4],
                },
                style: Default::default(),
            })
            .unwrap();
        }
        let doc = ed.into_document();
        let regions = crate::regions::regions(&doc);
        assert!(regions.iter().all(|r| r.key != key), "the circle was split");
        let fills = crate::region_fill::resolve(&doc, &regions);
        let at = |x: f64| {
            let i = regions
                .iter()
                .position(|r| {
                    crate::regions::layer_of(&doc, r) == Some(l1) && r.contains(Vec2::new(x, 0.0))
                })
                .unwrap();
            fills[i].as_ref().and_then(|f| f.painted())
        };
        assert_eq!(at(-5.0), Some([243, 94, 94]), "left half");
        assert_eq!(at(5.0), Some([243, 94, 94]), "right half");
        assert_eq!(at(-35.0), None, "the square outside stays open");
    }

    /// Region fills: one undo step for many regions, bad opacity refused,
    /// object fills refused (#451).
    #[test]
    fn region_fills_are_undoable_and_checked() {
        use crate::region_fill::RegionFill;
        use crate::style::{Fill, StylePatch};
        let doc = sample_v19();
        let keys: Vec<_> = crate::regions::regions(&doc)
            .into_iter()
            .map(|r| r.key)
            .collect();
        let mut ed = Editor::new(doc.clone());
        ed.apply(&Command::SetRegionFill {
            regions: keys.clone(),
            fill: Some(RegionFill::colour([0, 200, 0])),
        })
        .unwrap();
        assert!(
            keys.iter()
                .all(|k| ed.document().region_fill(k) == Some(&RegionFill::colour([0, 200, 0])))
        );
        ed.undo().unwrap();
        assert_eq!(ed.document(), &doc);
        assert!(
            ed.apply(&Command::SetRegionFill {
                regions: keys.clone(),
                fill: Some(RegionFill {
                    fill: Fill::Colour([1, 2, 3]),
                    opacity: 1.5,
                    gradient: None,
                }),
            })
            .is_err()
        );
        assert!(
            ed.apply(&Command::SetStyle {
                entities: vec![EntityId::new(6)],
                patch: StylePatch {
                    fill: Some(Fill::Colour([0, 0, 255])),
                    ..StylePatch::default()
                },
            })
            .is_err(),
            "objects have no fill"
        );
        let good = save(&sample_v19()).unwrap();
        assert!(load(&good).is_ok());
        let bad = good.replacen("opacity: 0.4", "opacity: 7.0", 1);
        assert_ne!(bad, good);
        assert!(load(&bad).is_err());
    }

    /// Placing the job is one undoable step; hostile offsets are refused,
    /// from a command and from a file (SPEC §7, #443).
    #[test]
    fn placement_is_undoable_and_checked() {
        let mut ed = Editor::new(Document::new());
        let at = locus_geom::Vec2::new(5.0, -2.0);
        ed.apply(&Command::SetPlacement { offset: Some(at) })
            .unwrap();
        assert_eq!(ed.document().placement(), Some(at));
        for bad in [f64::NAN, 1e9] {
            assert!(
                ed.apply(&Command::SetPlacement {
                    offset: Some(locus_geom::Vec2::new(bad, 0.0))
                })
                .is_err()
            );
        }
        assert_eq!(ed.document().placement(), Some(at));
        ed.undo().unwrap();
        assert_eq!(ed.document().placement(), None);
        ed.redo().unwrap();
        assert_eq!(ed.document().placement(), Some(at));
        let good = save(&sample_v19()).unwrap();
        assert!(load(&good).is_ok());
        let bad = good.replacen(
            "placement: Some((
    x: 12.5,",
            "placement: Some((
    x: 1e300,",
            1,
        );
        assert_ne!(bad, good);
        assert!(load(&bad).is_err());
    }

    /// A hostile text path is refused on load (CLAUDE.md: untrusted input).
    #[test]
    fn bad_text_paths_are_refused_on_load() {
        let good = save(&migrated(sample_v17())).unwrap();
        assert!(load(&good).is_ok());
        for (from, to) in [
            ("offset: 2.0", "offset: 1e300"),
            ("curves: [", "curves: [], x: ["),
        ] {
            assert!(good.contains(from), "{from}");
            let bad = good.replacen(from, to, 1);
            assert!(load(&bad).is_err(), "{to}");
        }
    }

    /// A hostile image is refused on load (CLAUDE.md: untrusted input).
    #[test]
    fn bad_images_are_refused_on_load() {
        let good = save(&migrated(sample_v16())).unwrap();
        assert!(load(&good).is_ok());
        for (from, to) in [
            ("width_px: 4", "width_px: 0"),
            ("mm_per_px: 0.5", "mm_per_px: -1.0"),
            (
                "opacity: 0.5,
",
                "opacity: 9.0,
",
            ),
            ("format: Png", "format: Jpeg"),
            ("data: \"iVBORw0KGg", "data: \"!!!!0KGg"),
        ] {
            assert!(good.contains(from), "{from}");
            let bad = good.replacen(from, to, 1);
            assert!(load(&bad).is_err(), "{to}");
        }
    }

    /// Hostile letter spacing is refused on load (CLAUDE.md: untrusted input).
    #[test]
    fn bad_tracking_is_refused_on_load() {
        let good = save(&migrated(sample_v15())).unwrap();
        assert!(load(&good).is_ok());
        let bad = good.replacen("tracking: 1.5", "tracking: 1e300", 1);
        assert!(matches!(load(&bad), Err(FileError::Invalid(_))));
    }

    /// A hostile style is refused on load, not stored (CLAUDE.md: untrusted input).
    #[test]
    fn bad_styles_are_refused_on_load() {
        let good = save(&migrated(sample_v13())).unwrap();
        assert!(load(&good).is_ok());
        for (from, to) in [
            ("width: 0.5", "width: -1.0"),
            ("pen: 2", "pen: 0"),
            ("opacity: 0.5", "opacity: 2.0"),
        ] {
            assert!(good.contains(from), "{from}");
            let bad = good.replacen(from, to, 1);
            assert!(matches!(load(&bad), Err(FileError::Invalid(_))), "{to}");
        }
    }

    /// Selecting a profile is one undoable step; bad ids are refused (SPEC §7, #236).
    #[test]
    fn set_profile_is_undoable_and_checked() {
        let mut ed = Editor::new(Document::new());
        ed.apply(&Command::SetProfile {
            id: Some("cricut".into()),
        })
        .unwrap();
        assert_eq!(ed.document().profile(), Some("cricut"));
        assert!(
            ed.apply(&Command::SetProfile {
                id: Some("Bad Id".into())
            })
            .is_err()
        );
        assert_eq!(ed.document().profile(), Some("cricut"));
        ed.undo().unwrap();
        assert_eq!(ed.document().profile(), None);
        ed.redo().unwrap();
        assert_eq!(ed.document().profile(), Some("cricut"));
    }

    #[test]
    fn hostile_profile_id_is_rejected() {
        let good = save(&sample_v11()).unwrap();
        for bad_id in ["../x", "", "UPPER", &"a".repeat(65)] {
            let bad = good.replacen("Some(\"lightburn\")", &format!("Some({bad_id:?})"), 1);
            assert_ne!(bad, good);
            assert!(load(&bad).is_err(), "{bad_id:?}");
        }
    }

    /// A plugin object opens from its cache alone: no plugin is involved in
    /// loading (§15.5), and a hostile cache is refused.
    #[test]
    fn plugin_object_round_trips_and_hostile_caches_are_rejected() {
        let good = save(&sample_v10()).unwrap();
        let back = load(&good).unwrap();
        assert_eq!(save(&back).unwrap(), good, "save → load → save is stable");
        for (from, to) in [
            ("Line(3, 0)", "Line(3, 99)"),
            ("Horizontal(0)", "Horizontal(77)"),
            ("id: \"com.example.square\"", "id: \"../../evil\""),
            ("value: Number(10.0)", "value: Number(NaN)"),
        ] {
            let bad = good.replacen(from, to, 1);
            assert_ne!(bad, good, "{from}");
            assert!(load(&bad).is_err(), "{from} -> {to}");
        }
        // The cache's last point (the plugin object is last in the file),
        // moved beyond the 1 km bound.
        let at = good.rfind("x: 0.0,").unwrap();
        let far = format!(
            "{}x: 5000000.0,{}",
            &good[..at],
            &good[at + "x: 0.0,".len()..]
        );
        assert!(load(&far).is_err());
    }

    /// Opening a document whose font is not here is not an error: the cached
    /// geometry is all a file needs (#179).
    #[test]
    fn text_with_a_missing_font_round_trips_unchanged() {
        let mut doc = sample_v9();
        let id = doc
            .entities()
            .iter()
            .find(|(_, e)| matches!(e.geom, Geom::Text { .. }))
            .map(|(i, _)| i)
            .unwrap();
        if let Some(Entity {
            geom: Geom::Text { text, .. },
            ..
        }) = doc.entities.get_mut(id)
        {
            text.font = locus_text::FontRef::File("NotInstalled.ttf".into());
        }
        let saved = save(&doc).unwrap();
        let back = load(&saved).unwrap();
        assert_eq!(back, doc);
        assert_eq!(save(&back).unwrap(), saved, "save → load → save is stable");
        let (t, ..) = back.text(id).unwrap();
        assert!(t.font_missing(&locus_text::BundledProvider));
        assert!(!t.layout.contours.is_empty());
        assert!(back.text_geometry(id).is_some());
    }

    #[test]
    fn hostile_text_is_rejected_on_load() {
        let good = save(&sample_v9()).unwrap();
        for (from, to) in [
            ("size: 6.0", "size: -1.0"),
            ("size: 6.0", "size: NaN"),
            ("line_spacing: 1.0", "line_spacing: 0.0"),
            ("Bundled(\"hershey-sans\")", "Bundled(\"\")"),
        ] {
            let bad = good.replacen(from, to, 1);
            assert_ne!(bad, good, "{from}");
            assert!(load(&bad).is_err(), "{from} -> {to}");
        }
        // A text whose anchor is a missing point.
        let dangling = good.replacen("anchor: ", "anchor: 9", 1);
        assert!(load(&dangling).is_err());
    }

    /// The v8 fixture is frozen; it migrates to v9 with no text.
    #[test]
    fn v8_fixture_migrates_to_v10() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/v8/plate.locus");
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.contains("format_version: 8"));
        let doc = load(&text).unwrap();
        assert_eq!(doc, sample_v8());
        assert!(save(&doc).unwrap().contains(&current()));
    }

    /// The v7 fixture is frozen; it migrates to v9.
    #[test]
    fn v7_fixture_migrates_to_v10() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/v7/plate.locus");
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.contains("format_version: 7"));
        let doc = load(&text).unwrap();
        assert_eq!(doc, sample_v7());
        assert!(save(&doc).unwrap().contains(&current()));
    }

    /// The v6 fixture is frozen; it migrates to v7 with crosshatch off.
    #[test]
    fn v6_fixture_migrates_to_v10() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/v6/plate.locus");
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.contains("format_version: 6"));
        let doc = load(&text).unwrap();
        assert_eq!(doc, sample_v6());
        assert!(save(&doc).unwrap().contains(&current()));
    }

    /// The v5 fixture is frozen; it migrates to v6 with no expressions.
    #[test]
    fn v5_fixture_migrates_to_v10() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/v5/plate.locus");
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.contains("format_version: 5"));
        let doc = load(&text).unwrap();
        assert_eq!(doc, sample_v5());
        assert!(save(&doc).unwrap().contains(&current()));
    }

    /// The v4 fixture is frozen; it migrates to v5 with no recipes.
    #[test]
    fn v4_fixture_migrates_to_v10() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/v4/plate.locus");
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.contains("format_version: 4"));
        let doc = load(&text).unwrap();
        assert_eq!(doc, sample_v4());
        assert!(doc.recipes().is_empty());
        assert!(save(&doc).unwrap().contains(&current()));
    }

    /// The v3 fixture is frozen; it migrates to v4 unchanged.
    #[test]
    fn v3_fixture_migrates_to_v10() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/v3/plate.locus");
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.contains("format_version: 3"));
        let doc = load(&text).unwrap();
        assert_eq!(doc, sample_v3());
        assert!(save(&doc).unwrap().contains(&current()));
    }

    #[test]
    fn bezier_tangent_must_share_an_end_point() {
        let mut doc = sample_v4();
        // Cut the shared point out of the first tangent by pointing the
        // constraint at the unrelated plate line.
        let plate_line = doc
            .entities()
            .iter()
            .find_map(|(id, e)| matches!(e.geom, Geom::Line { .. }).then_some(id))
            .unwrap();
        let bez = doc
            .entities()
            .iter()
            .find_map(|(id, e)| matches!(e.geom, Geom::Bezier { .. }).then_some(id))
            .unwrap();
        let cid = *doc.constraints.keys().next().unwrap();
        doc.constraints.insert(
            cid,
            Constraint::Tangent {
                a: plate_line,
                b: bez,
            },
        );
        assert!(matches!(validate(&doc), Err(FileError::Invalid(_))));
    }

    /// The v2 fixture is frozen; it migrates to v4 with no recipes.
    #[test]
    fn v2_fixture_migrates_to_v10() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/v2/plate.locus");
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.contains("format_version: 2"));
        let doc = load(&text).unwrap();
        assert_eq!(doc, sample_v2());
        assert!(doc.booleans().is_empty());
        assert!(save(&doc).unwrap().contains(&current()));
    }

    #[test]
    fn rejects_invalid_boolean_recipes() {
        // A subtract whose tools were removed (e.g. by hand-editing).
        let mut doc = sample_v3();
        doc.booleans.values_mut().for_each(|b| b.tools.clear());
        assert!(matches!(validate(&doc), Err(FileError::Invalid(_))));
        let text = save(&doc).unwrap();
        assert!(matches!(load(&text), Err(FileError::Invalid(_))));
    }

    /// The v1 fixture is frozen; it must keep loading, with default
    /// parameters on its layers (SPEC §10 migration).
    #[test]
    fn v1_fixture_migrates_to_v10() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/v1/plate.locus");
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.contains("format_version: 1"));
        let doc = load(&text).unwrap();
        assert_eq!(doc, sample());
        assert_eq!(
            doc.layer(L).unwrap().operation,
            Operation::Cut(CutParams::default())
        );
        assert!(save(&doc).unwrap().contains(&current()));
    }

    #[test]
    fn rejects_out_of_range_operation_params() {
        let good = save(&sample()).unwrap();
        let bad = good.replacen("passes: 1", "passes: 0", 1);
        assert_ne!(bad, good);
        assert!(matches!(load(&bad), Err(FileError::Invalid(_))));
    }

    #[test]
    fn rejects_bad_input_without_panicking() {
        let good = save(&sample()).unwrap();
        // Garbage and truncation.
        assert!(matches!(load(""), Err(FileError::Parse(_))));
        assert!(matches!(load("not ron at all"), Err(FileError::Parse(_))));
        assert!(matches!(
            load(&good[..good.len() / 2]),
            Err(FileError::Parse(_))
        ));
        // Versions.
        let v = |n: u32| good.replacen(&current(), &format!("format_version: {n}"), 1);
        assert_eq!(load(&v(0)), Err(FileError::UnknownVersion(0)));
        assert_eq!(
            load(&v(CURRENT_VERSION + 1)),
            Err(FileError::FutureVersion {
                found: CURRENT_VERSION + 1,
                supported: CURRENT_VERSION
            })
        );
        // Dangling reference: a line pointing at a missing point.
        let dangling = good.replacen("end: 3,", "end: 4000,", 1);
        assert_ne!(dangling, good);
        assert!(matches!(load(&dangling), Err(FileError::Invalid(_))));
        // next_id lower than an existing ID.
        let next = format!("next_id: {}", sample().next_id());
        let low = good.replacen(&next, "next_id: 3", 1);
        assert!(matches!(load(&low), Err(FileError::Invalid(_))));
        // Over the 2^53 cap.
        let big = good.replacen(&next, "next_id: 9007199254740993", 1);
        assert!(matches!(load(&big), Err(FileError::Invalid(_))));
    }

    #[test]
    fn rejects_non_finite_on_load_and_save() {
        let good = save(&sample()).unwrap();
        let nan = good.replacen("radius: 5.0", "radius: NaN", 1);
        assert_ne!(nan, good);
        assert!(matches!(load(&nan), Err(FileError::Invalid(_))));
        let mut doc = sample();
        doc.view = Some(View {
            center: Vec2::ZERO,
            scale: f64::INFINITY,
        });
        assert!(matches!(save(&doc), Err(FileError::Unsaveable(_))));
    }

    #[test]
    fn rejects_duplicate_id_across_kinds() {
        let mut doc = sample();
        let some_entity = doc.entities().iter().next().unwrap().0;
        let c = doc
            .constraints
            .remove(&ConstraintId::new(
                doc.constraints.keys().next().unwrap().get(),
            ))
            .unwrap();
        doc.constraints
            .insert(ConstraintId::new(some_entity.get()), c);
        assert!(matches!(validate(&doc), Err(FileError::Invalid(_))));
    }
}
