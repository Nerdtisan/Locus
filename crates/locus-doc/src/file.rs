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
use crate::operation::{CutParams, DrawParams, EngraveParams, Operation};
use crate::recipe::Recipe;
use locus_geom::Unit;
use ron::ser::PrettyConfig;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use thiserror::Error;

/// The version this build writes.
pub const CURRENT_VERSION: u32 = 8;

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

impl From<LayerV1> for Layer {
    /// v1 → v2: layers get the default parameters of their operation (SPEC §7).
    fn from(l: LayerV1) -> Self {
        Layer {
            name: l.name,
            colour: l.colour,
            operation: match l.operation {
                OperationV1::Cut => Operation::Cut(CutParams::default()),
                OperationV1::Score => Operation::Score,
                OperationV1::Engrave => Operation::Engrave(EngraveParams::default()),
                OperationV1::Draw => Operation::Draw(DrawParams::default()),
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
    layers: BTreeMap<LayerId, Layer>,
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
    layers: BTreeMap<LayerId, Layer>,
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
    layers: BTreeMap<LayerId, Layer>,
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
    layers: BTreeMap<LayerId, Layer>,
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
    layers: BTreeMap<LayerId, Layer>,
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
    layers: BTreeMap<LayerId, Layer>,
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
    layers: BTreeMap<LayerId, Layer>,
    entities: BTreeMap<EntityId, Entity>,
    constraints: BTreeMap<ConstraintId, Constraint>,
    booleans: BTreeMap<BooleanId, Boolean>,
    recipes: BTreeMap<RecipeId, Recipe>,
    dim_exprs: BTreeMap<ConstraintId, String>,
    #[serde(default)]
    view: Option<View>,
}

/// The newest on-disk shape. When adding v9, point this at `FileV9`.
type Latest = FileV8;

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
        1 => FileV8::from(FileV7::from(FileV6::from(FileV5::from(FileV4::from(
            FileV3::from(FileV2::from(parse::<FileV1>(text)?)),
        ))))),
        2 => FileV8::from(FileV7::from(FileV6::from(FileV5::from(FileV4::from(
            FileV3::from(parse::<FileV2>(text)?),
        ))))),
        3 => FileV8::from(FileV7::from(FileV6::from(FileV5::from(FileV4::from(
            parse::<FileV3>(text)?,
        ))))),
        4 => FileV8::from(FileV7::from(FileV6::from(FileV5::from(parse::<FileV4>(
            text,
        )?)))),
        5 => FileV8::from(FileV7::from(FileV6::from(parse::<FileV5>(text)?))),
        6 => FileV8::from(FileV7::from(parse::<FileV6>(text)?)),
        7 => FileV8::from(parse::<FileV7>(text)?),
        8 => parse::<FileV8>(text)?,
        v if v > CURRENT_VERSION => {
            return Err(FileError::FutureVersion {
                found: v,
                supported: CURRENT_VERSION,
            });
        }
        v => return Err(FileError::UnknownVersion(v)),
    };
    into_document(latest)
}

fn parse<T: serde::de::DeserializeOwned>(text: &str) -> Result<T, FileError> {
    ron::from_str(text).map_err(|e| FileError::Parse(e.to_string()))
}

fn into_document(f: Latest) -> Result<Document, FileError> {
    let doc = Document {
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
    };
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

    const L: LayerId = Document::default_layer();

    /// 60 × 40 plate with a Ø10 hole, a construction point, a parameter and a
    /// saved view: the v1 fixture.
    pub(crate) fn sample() -> Document {
        let mut ed = Editor::new(Document::new());
        let mut add = |construction, geom| {
            ed.apply(&Command::AddEntity {
                layer: L,
                construction,
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
            }),
            Operation::Draw(DrawParams {
                pen_number: 3,
                pen_width: 0.7,
            }),
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

    #[test]
    fn v8_fixture_is_current_and_loads() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/v8/plate.locus");
        if std::env::var_os("LOCUS_UPDATE_FIXTURES").is_some() {
            std::fs::create_dir_all(std::path::Path::new(path).parent().unwrap()).unwrap();
            std::fs::write(path, save(&sample_v8()).unwrap()).unwrap();
        }
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.contains("format_version: 8"));
        assert!(text.contains("Offset("));
        assert_eq!(load(&text).unwrap(), sample_v8());
        assert_eq!(
            save(&sample_v8()).unwrap(),
            text,
            "fixture differs: output format changed (ADR-0004 rule 3)"
        );
    }

    /// The v7 fixture is frozen; it migrates to v8 with no offsets.
    #[test]
    fn v7_fixture_migrates_to_v8() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/v7/plate.locus");
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.contains("format_version: 7"));
        let doc = load(&text).unwrap();
        assert_eq!(doc, sample_v7());
        assert!(save(&doc).unwrap().contains("format_version: 8"));
    }

    /// The v6 fixture is frozen; it migrates to v7 with crosshatch off.
    #[test]
    fn v6_fixture_migrates_to_v8() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/v6/plate.locus");
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.contains("format_version: 6"));
        let doc = load(&text).unwrap();
        assert_eq!(doc, sample_v6());
        assert!(save(&doc).unwrap().contains("format_version: 8"));
    }

    /// The v5 fixture is frozen; it migrates to v6 with no expressions.
    #[test]
    fn v5_fixture_migrates_to_v8() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/v5/plate.locus");
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.contains("format_version: 5"));
        let doc = load(&text).unwrap();
        assert_eq!(doc, sample_v5());
        assert!(save(&doc).unwrap().contains("format_version: 8"));
    }

    /// The v4 fixture is frozen; it migrates to v5 with no recipes.
    #[test]
    fn v4_fixture_migrates_to_v8() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/v4/plate.locus");
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.contains("format_version: 4"));
        let doc = load(&text).unwrap();
        assert_eq!(doc, sample_v4());
        assert!(doc.recipes().is_empty());
        assert!(save(&doc).unwrap().contains("format_version: 8"));
    }

    /// The v3 fixture is frozen; it migrates to v4 unchanged.
    #[test]
    fn v3_fixture_migrates_to_v8() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/v3/plate.locus");
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.contains("format_version: 3"));
        let doc = load(&text).unwrap();
        assert_eq!(doc, sample_v3());
        assert!(save(&doc).unwrap().contains("format_version: 8"));
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
    fn v2_fixture_migrates_to_v8() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/v2/plate.locus");
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.contains("format_version: 2"));
        let doc = load(&text).unwrap();
        assert_eq!(doc, sample_v2());
        assert!(doc.booleans().is_empty());
        assert!(save(&doc).unwrap().contains("format_version: 8"));
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
    fn v1_fixture_migrates_to_v8() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/v1/plate.locus");
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.contains("format_version: 1"));
        let doc = load(&text).unwrap();
        assert_eq!(doc, sample());
        assert_eq!(
            doc.layer(L).unwrap().operation,
            Operation::Cut(CutParams::default())
        );
        assert!(save(&doc).unwrap().contains("format_version: 8"));
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
        let v = |n: u32| good.replacen("format_version: 8", &format!("format_version: {n}"), 1);
        assert_eq!(load(&v(0)), Err(FileError::UnknownVersion(0)));
        assert_eq!(
            load(&v(9)),
            Err(FileError::FutureVersion {
                found: 9,
                supported: 8
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
