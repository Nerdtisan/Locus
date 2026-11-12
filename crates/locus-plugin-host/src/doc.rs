// SPDX-License-Identifier: AGPL-3.0-or-later

//! Between plugins and documents (#183, #184, §15.3, §15.5): parameter
//! values and defaults, generator and modifier output as document caches.
//!
//! Plugins never touch the document: these turn validated plugin output into
//! the data a `locus-doc` command carries, and the caller applies the command
//! (one undo step). A failed call returns an error and no command, so the
//! document is unchanged.

use locus_doc::generated::{
    GenConstraint, GenEntity, GenPath, GenSegment, GenShape, GeneratedGeometry, PluginParam,
    PluginRef, PluginValue,
};
use locus_geom::Vec2 as DocVec2;

use crate::types::{
    Constraint, Entity, Generated, Param, ParamKind, ParamSpec, ParamValue, Path, Segment, Shape,
};

/// Every parameter at its schema default. `doc-parameter` bindings default to 0.
#[must_use]
pub fn defaults(schema: &[ParamSpec]) -> Vec<Param> {
    schema
        .iter()
        .map(|s| Param {
            key: s.key.clone(),
            value: default_value(&s.kind),
        })
        .collect()
}

fn default_value(kind: &ParamKind) -> ParamValue {
    match kind {
        ParamKind::Number(n) => ParamValue::Number(n.default),
        ParamKind::Integer(n) => ParamValue::Integer(n.default),
        ParamKind::Boolean(b) => ParamValue::Boolean(*b),
        ParamKind::Choice(c) => ParamValue::Choice(c.default),
        ParamKind::Text(t) => ParamValue::Text(t.default.clone()),
        ParamKind::DocParameter => ParamValue::Number(0.0),
    }
}

/// Why a parameter value doesn't fit the schema.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("parameter `{key}`: {problem}")]
pub struct ParamError {
    pub key: String,
    pub problem: String,
}

fn perr(key: &str, problem: &str) -> ParamError {
    ParamError {
        key: key.chars().take(64).collect(),
        problem: problem.into(),
    }
}

/// Checks one value against its spec (range, type, length).
pub fn check_value(spec: &ParamSpec, value: &ParamValue) -> Result<(), ParamError> {
    let k = &spec.key;
    match (&spec.kind, value) {
        (ParamKind::Number(n), ParamValue::Number(v)) => {
            if !v.is_finite() || n.min.is_some_and(|m| *v < m) || n.max.is_some_and(|m| *v > m) {
                return Err(perr(k, "is out of range"));
            }
        }
        (ParamKind::DocParameter, ParamValue::Number(v)) if v.is_finite() => {}
        (ParamKind::Integer(n), ParamValue::Integer(v)) => {
            if n.min.is_some_and(|m| *v < m) || n.max.is_some_and(|m| *v > m) {
                return Err(perr(k, "is out of range"));
            }
        }
        (ParamKind::Boolean(_), ParamValue::Boolean(_)) => {}
        (ParamKind::Choice(c), ParamValue::Choice(i)) => {
            if *i as usize >= c.options.len() {
                return Err(perr(k, "is not one of the options"));
            }
        }
        (ParamKind::Text(t), ParamValue::Text(s)) => {
            if s.len() > t.max_length as usize {
                return Err(perr(k, "is too long"));
            }
        }
        _ => return Err(perr(k, "has the wrong type")),
    }
    Ok(())
}

/// The full parameter list for a call: every schema key in schema order,
/// taken from `given` where present (and checked), else the default. Keys
/// the schema doesn't know are refused.
pub fn resolve(schema: &[ParamSpec], given: &[Param]) -> Result<Vec<Param>, ParamError> {
    if let Some(p) = given
        .iter()
        .find(|p| !schema.iter().any(|s| s.key == p.key))
    {
        return Err(perr(&p.key, "isn't a parameter of this plugin"));
    }
    schema
        .iter()
        .map(|s| {
            let value = match given.iter().find(|p| p.key == s.key) {
                Some(p) => {
                    check_value(s, &p.value)?;
                    p.value.clone()
                }
                None => default_value(&s.kind),
            };
            Ok(Param {
                key: s.key.clone(),
                value,
            })
        })
        .collect()
}

/// Stored document values as plugin parameters, keeping only keys the
/// current schema has (a newer plugin may have dropped some) and filling
/// the rest with defaults.
#[must_use]
pub fn from_stored(schema: &[ParamSpec], stored: &[PluginParam]) -> Vec<Param> {
    schema
        .iter()
        .map(|s| {
            let value = stored
                .iter()
                .find(|p| p.key == s.key)
                .map(|p| to_value(&p.value))
                .filter(|v| check_value(s, v).is_ok())
                .unwrap_or_else(|| default_value(&s.kind));
            Param {
                key: s.key.clone(),
                value,
            }
        })
        .collect()
}

fn to_value(v: &PluginValue) -> ParamValue {
    match v {
        PluginValue::Number(n) => ParamValue::Number(*n),
        PluginValue::Integer(n) => ParamValue::Integer(*n),
        PluginValue::Boolean(b) => ParamValue::Boolean(*b),
        PluginValue::Choice(c) => ParamValue::Choice(*c),
        PluginValue::Text(t) => ParamValue::Text(t.clone()),
    }
}

fn to_stored(v: &ParamValue) -> PluginValue {
    match v {
        ParamValue::Number(n) => PluginValue::Number(*n),
        ParamValue::Integer(n) => PluginValue::Integer(*n),
        ParamValue::Boolean(b) => PluginValue::Boolean(*b),
        ParamValue::Choice(c) => PluginValue::Choice(*c),
        ParamValue::Text(t) => PluginValue::Text(t.clone()),
    }
}

/// What the document stores about a call: plugin id, version, parameters.
#[must_use]
pub fn plugin_ref(id: &str, version: &str, params: &[Param]) -> PluginRef {
    PluginRef {
        id: id.into(),
        version: version.into(),
        params: params
            .iter()
            .map(|p| PluginParam {
                key: p.key.clone(),
                value: to_stored(&p.value),
            })
            .collect(),
    }
}

/// Validated generator output as the document's cache.
#[must_use]
pub fn to_cache(g: &Generated) -> GeneratedGeometry {
    GeneratedGeometry {
        points: g.points.iter().map(|p| DocVec2::new(p.x, p.y)).collect(),
        entities: g
            .entities
            .iter()
            .map(|e| match *e {
                Entity::Point(p) => GenEntity::Point(p),
                Entity::Line(a, b) => GenEntity::Line(a, b),
                Entity::Circle { center, radius } => GenEntity::Circle(center, radius),
                Entity::Arc { center, start, end } => GenEntity::Arc(center, start, end),
            })
            .collect(),
        constraints: g
            .constraints
            .iter()
            .map(|c| match *c {
                Constraint::Coincident(a, b) => GenConstraint::Coincident(a, b),
                Constraint::Horizontal(a) => GenConstraint::Horizontal(a),
                Constraint::Vertical(a) => GenConstraint::Vertical(a),
                Constraint::Parallel(a, b) => GenConstraint::Parallel(a, b),
                Constraint::Perpendicular(a, b) => GenConstraint::Perpendicular(a, b),
                Constraint::Tangent(a, b) => GenConstraint::Tangent(a, b),
                Constraint::Equal(a, b) => GenConstraint::Equal(a, b),
                Constraint::Distance(a, b, d) => GenConstraint::Distance(a, b, d),
                Constraint::Radius(a, r) => GenConstraint::Radius(a, r),
            })
            .collect(),
    }
}

// ── Modifiers (#184) ────────────────────────────────────────────────────────

fn hv(p: DocVec2) -> crate::types::Vec2 {
    crate::types::Vec2 { x: p.x, y: p.y }
}

/// A modifier's document input in plugin terms.
#[must_use]
pub fn to_paths(input: &[GenPath]) -> Vec<Path> {
    input
        .iter()
        .map(|p| Path {
            segments: p
                .segments
                .iter()
                .map(|s| Segment {
                    shape: match s.shape {
                        GenShape::Line { a, b } => Shape::Line { a: hv(a), b: hv(b) },
                        GenShape::Arc { center, start, end } => Shape::Arc {
                            center: hv(center),
                            start: hv(start),
                            end: hv(end),
                        },
                        GenShape::Circle { center, radius } => Shape::Circle {
                            center: hv(center),
                            radius,
                        },
                    },
                    reversed: s.reversed,
                })
                .collect(),
            closed: p.closed,
        })
        .collect()
}

/// Plugin paths in document terms (the inverse of [`to_paths`]).
#[must_use]
pub fn from_paths(paths: &[Path]) -> Vec<GenPath> {
    let dv = |p: crate::types::Vec2| DocVec2::new(p.x, p.y);
    paths
        .iter()
        .map(|p| GenPath {
            segments: p
                .segments
                .iter()
                .map(|s| GenSegment {
                    shape: match s.shape {
                        Shape::Line { a, b } => GenShape::Line { a: dv(a), b: dv(b) },
                        Shape::Arc { center, start, end } => GenShape::Arc {
                            center: dv(center),
                            start: dv(start),
                            end: dv(end),
                        },
                        Shape::Circle { center, radius } => GenShape::Circle {
                            center: dv(center),
                            radius,
                        },
                    },
                    reversed: s.reversed,
                })
                .collect(),
            closed: p.closed,
        })
        .collect()
}

/// Validated importer output (#233) as the command that adds it on a new
/// locked Reference layer named `name`: one undo step (§15.3).
pub fn import_command(
    doc: &locus_doc::Document,
    name: &str,
    paths: &[Path],
) -> Result<locus_doc::Command, locus_doc::DocError> {
    locus_doc::reference::import_command(doc, name, &from_paths(paths))
}

/// Validated modifier output as a document cache: shared end points become
/// shared points, so paths stay connected in the document.
#[must_use]
pub fn paths_to_cache(paths: &[Path]) -> GeneratedGeometry {
    let mut g = GeneratedGeometry::default();
    let point = |p: crate::types::Vec2, g: &mut GeneratedGeometry| -> u32 {
        let found = g
            .points
            .iter()
            .position(|q| q.x.to_bits() == p.x.to_bits() && q.y.to_bits() == p.y.to_bits());
        found.unwrap_or_else(|| {
            g.points.push(DocVec2::new(p.x, p.y));
            g.points.len() - 1
        }) as u32
    };
    for path in paths {
        for seg in &path.segments {
            let e = match seg.shape {
                Shape::Line { a, b } => {
                    let (i, j) = (point(a, &mut g), point(b, &mut g));
                    if i == j {
                        continue;
                    }
                    GenEntity::Line(i, j)
                }
                Shape::Arc { center, start, end } => {
                    let c = point(center, &mut g);
                    GenEntity::Arc(c, point(start, &mut g), point(end, &mut g))
                }
                Shape::Circle { center, radius } => {
                    GenEntity::Circle(point(center, &mut g), radius)
                }
            };
            g.entities.push(e);
        }
    }
    g
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ChoiceSpec, NumberSpec};

    fn schema() -> Vec<ParamSpec> {
        vec![
            ParamSpec {
                key: "width".into(),
                label: "Width".into(),
                help: String::new(),
                kind: ParamKind::Number(NumberSpec {
                    unit: "mm".into(),
                    min: Some(1.0),
                    max: Some(100.0),
                    default: 10.0,
                }),
            },
            ParamSpec {
                key: "style".into(),
                label: "Style".into(),
                help: String::new(),
                kind: ParamKind::Choice(ChoiceSpec {
                    options: vec!["a".into(), "b".into()],
                    default: 1,
                }),
            },
        ]
    }

    fn p(key: &str, value: ParamValue) -> Param {
        Param {
            key: key.into(),
            value,
        }
    }

    #[test]
    fn resolve_fills_defaults_and_checks_values() {
        let s = schema();
        let got = resolve(&s, &[p("width", ParamValue::Number(20.0))]).unwrap();
        assert_eq!(
            got,
            vec![
                p("width", ParamValue::Number(20.0)),
                p("style", ParamValue::Choice(1))
            ]
        );
        assert!(resolve(&s, &[p("width", ParamValue::Number(500.0))]).is_err());
        assert!(resolve(&s, &[p("width", ParamValue::Boolean(true))]).is_err());
        assert!(resolve(&s, &[p("style", ParamValue::Choice(5))]).is_err());
        assert!(resolve(&s, &[p("colour", ParamValue::Number(1.0))]).is_err());
    }

    #[test]
    fn stored_values_survive_a_schema_change() {
        let s = schema();
        let stored = plugin_ref(
            "com.example.x",
            "1.0.0",
            &[
                p("width", ParamValue::Number(42.0)),
                p("gone", ParamValue::Number(1.0)),
                p("style", ParamValue::Choice(9)),
            ],
        );
        let back = from_stored(&s, &stored.params);
        assert_eq!(
            back,
            vec![
                p("width", ParamValue::Number(42.0)),
                p("style", ParamValue::Choice(1))
            ]
        );
    }
}
