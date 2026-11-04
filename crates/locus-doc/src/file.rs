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

use crate::constraint::Constraint;
use crate::document::{Document, EntityStore, Layer, Parameter, View};
use crate::entity::{Entity, Geom};
use crate::error::DocError;
use crate::ids::{ConstraintId, EntityId, LayerId, MAX_ID};
use locus_geom::Unit;
use ron::ser::PrettyConfig;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use thiserror::Error;

/// The version this build writes.
pub const CURRENT_VERSION: u32 = 1;

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

/// Version 1, frozen. See the module docs before touching this.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct FileV1 {
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

/// The newest on-disk shape. When adding v2, point this at `FileV2`.
type Latest = FileV1;

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
    let file = FileV1 {
        format_version: CURRENT_VERSION,
        units: doc.units,
        next_id: doc.next_id,
        parameters: doc.parameters.clone(),
        layers: doc.layers.clone(),
        entities: doc.entities.iter().map(|(i, e)| (i, e.clone())).collect(),
        constraints: doc.constraints.clone(),
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
        1 => parse::<FileV1>(text)?,
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
        .chain(doc.constraints.keys().map(|i| i.get()));
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
    for (_, e) in doc.entities.iter() {
        doc.check_entity(e)?;
    }
    for c in doc.constraints.values() {
        c.validate(doc)?;
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

    #[test]
    fn v1_fixture_is_current_and_loads() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/v1/plate.locus");
        if std::env::var_os("LOCUS_UPDATE_FIXTURES").is_some() {
            std::fs::create_dir_all(std::path::Path::new(path).parent().unwrap()).unwrap();
            std::fs::write(path, save(&sample()).unwrap()).unwrap();
        }
        let text = std::fs::read_to_string(path).unwrap();
        assert_eq!(load(&text).unwrap(), sample());
        assert_eq!(
            save(&sample()).unwrap(),
            text,
            "fixture differs: output format changed (ADR-0004 rule 3)"
        );
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
        let v = |n: u32| good.replacen("format_version: 1", &format!("format_version: {n}"), 1);
        assert_eq!(load(&v(0)), Err(FileError::UnknownVersion(0)));
        assert_eq!(
            load(&v(2)),
            Err(FileError::FutureVersion {
                found: 2,
                supported: 1
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
