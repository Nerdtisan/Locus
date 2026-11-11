// SPDX-License-Identifier: AGPL-3.0-or-later

//! Plugin-made objects in the document (SPEC §15.3, §15.5; #183, #184).
//!
//! A generator plugin's output becomes a non-solved recipe
//! ([`RecipeKind::Generated`]): the document stores which plugin made it
//! (`id`, `version`), the parameters, and the **cached output** in the
//! object's local frame. The instances are ordinary entities regenerated from
//! that cache, so a document opens, renders and exports correctly without
//! the plugin installed (§15.5); the plugin only runs again when the user
//! changes a parameter or explicitly updates the plugin version.
//!
//! Placement is constrainable like the text entity's (§15.3): the recipe's
//! two sources are an *origin* point and a *direction* point, both ordinary
//! points. The cache is drawn at the origin, its +x axis along origin →
//! direction, so dragging or constraining either point moves or turns the
//! object without running the plugin.
//!
//! Plugin output is untrusted, and so is a `.locus` file: everything here is
//! validated with bounded sizes before it reaches the document.

use crate::document::Document;
use crate::error::DocError;
use crate::ids::{EntityId, RecipeId};
use crate::recipe::{MAX_INSTANCE_ENTITIES, RecipeKind};
use locus_geom::Vec2;
use serde::{Deserialize, Serialize};

/// Most parameters a plugin object stores.
pub const MAX_PLUGIN_PARAMS: usize = 256;
/// Longest stored plugin string (id, version, key, text value), in bytes.
pub const MAX_PLUGIN_TEXT: usize = 4096;
/// Largest |coordinate| in a cache: 1 km in millimetres.
pub const MAX_PLUGIN_COORD: f64 = 1.0e6;

/// A parameter value as stored in the document (mirrors the WIT
/// `param-value`, ADR 0009).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PluginValue {
    Number(f64),
    Integer(i64),
    Boolean(bool),
    Choice(u32),
    Text(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginParam {
    pub key: String,
    pub value: PluginValue,
}

/// Which plugin made an object, and with what (§15.5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginRef {
    /// Reverse-DNS plugin id.
    pub id: String,
    pub version: String,
    pub params: Vec<PluginParam>,
}

/// One piece of cached output; curves refer to [`GeneratedGeometry::points`]
/// by index, so shared corners stay shared.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum GenEntity {
    Point(u32),
    Line(u32, u32),
    Circle(u32, f64),
    /// Centre, start, end; counter-clockwise.
    Arc(u32, u32, u32),
}

/// A constraint a generator emitted (ADR 0009's subset of §5); indices refer
/// to [`GeneratedGeometry::entities`]. Kept with the cache; the object is
/// placed rigidly by its frame, so these are informational until exploded.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum GenConstraint {
    Coincident(u32, u32),
    Horizontal(u32),
    Vertical(u32),
    Parallel(u32, u32),
    Perpendicular(u32, u32),
    Tangent(u32, u32),
    Equal(u32, u32),
    Distance(u32, u32, f64),
    Radius(u32, f64),
}

/// A generator's cached output in the object's local frame (mm).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GeneratedGeometry {
    pub points: Vec<Vec2>,
    pub entities: Vec<GenEntity>,
    #[serde(default)]
    pub constraints: Vec<GenConstraint>,
}

fn bad(m: impl Into<String>) -> DocError {
    DocError::Invalid(format!("plugin object: {}", m.into()))
}

fn text_ok(s: &str) -> bool {
    !s.is_empty() && s.len() <= MAX_PLUGIN_TEXT && !s.chars().any(char::is_control)
}

impl PluginRef {
    /// Bounds and shape checks; the plugin's own schema is checked by the
    /// host before a value ever gets here.
    pub fn validate(&self) -> Result<(), DocError> {
        let dns = |s: &str| {
            s.len() <= 128
                && s.split('.').count() >= 2
                && s.split('.').all(|l| {
                    !l.is_empty()
                        && l.bytes()
                            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
                })
        };
        if !dns(&self.id) {
            return Err(bad("plugin id is not a reverse-DNS name"));
        }
        if !text_ok(&self.version) || self.version.len() > 64 {
            return Err(bad("bad plugin version"));
        }
        if self.params.len() > MAX_PLUGIN_PARAMS {
            return Err(bad("too many parameters"));
        }
        for (i, p) in self.params.iter().enumerate() {
            if !text_ok(&p.key) || p.key.len() > 64 {
                return Err(bad("bad parameter key"));
            }
            if self.params[..i].iter().any(|q| q.key == p.key) {
                return Err(bad("duplicate parameter key"));
            }
            match &p.value {
                PluginValue::Number(n) if !n.is_finite() => {
                    return Err(bad("parameter is not a finite number"));
                }
                PluginValue::Text(t) if t.len() > MAX_PLUGIN_TEXT => {
                    return Err(bad("text parameter is too long"));
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// A parameter's value by key.
    #[must_use]
    pub fn param(&self, key: &str) -> Option<&PluginValue> {
        self.params.iter().find(|p| p.key == key).map(|p| &p.value)
    }
}

impl GeneratedGeometry {
    /// Counts, finite coordinates and indices (§15.4: plugin output and
    /// files are untrusted).
    pub fn validate(&self) -> Result<(), DocError> {
        let n = self.points.len() + self.entities.len();
        if n > MAX_INSTANCE_ENTITIES || self.constraints.len() > MAX_INSTANCE_ENTITIES {
            return Err(DocError::InstanceLimit {
                requested: n,
                limit: MAX_INSTANCE_ENTITIES,
            });
        }
        let finite = |v: f64| v.is_finite() && v.abs() <= MAX_PLUGIN_COORD;
        if !self.points.iter().all(|p| finite(p.x) && finite(p.y)) {
            return Err(bad("a point is not finite or is out of range"));
        }
        let pt = |i: u32| (i as usize) < self.points.len();
        for e in &self.entities {
            let ok = match *e {
                GenEntity::Point(p) => pt(p),
                GenEntity::Line(a, b) => pt(a) && pt(b) && a != b,
                GenEntity::Circle(c, r) => pt(c) && finite(r) && r > 0.0,
                GenEntity::Arc(c, s, e) => pt(c) && pt(s) && pt(e) && c != s && c != e,
            };
            if !ok {
                return Err(bad("an entity refers to a missing point or is degenerate"));
            }
        }
        let en = |i: u32| (i as usize) < self.entities.len();
        for c in &self.constraints {
            let ok = match *c {
                GenConstraint::Horizontal(a) | GenConstraint::Vertical(a) => en(a),
                GenConstraint::Radius(a, r) => en(a) && finite(r) && r > 0.0,
                GenConstraint::Distance(a, b, d) => en(a) && en(b) && finite(d) && d >= 0.0,
                GenConstraint::Coincident(a, b)
                | GenConstraint::Parallel(a, b)
                | GenConstraint::Perpendicular(a, b)
                | GenConstraint::Tangent(a, b)
                | GenConstraint::Equal(a, b) => en(a) && en(b),
            };
            if !ok {
                return Err(bad("a constraint refers to a missing entity"));
            }
        }
        Ok(())
    }

    /// How many entities the object makes in the document.
    #[must_use]
    pub fn instance_count(&self) -> usize {
        self.points.len()
            + self
                .entities
                .iter()
                .filter(|e| !matches!(e, GenEntity::Point(_)))
                .count()
    }
}

/// Where an object's local frame sits: its origin point, and the angle of
/// origin → direction (0 if the two coincide). `None` if a frame point is
/// missing or isn't a point.
#[must_use]
pub fn frame(doc: &Document, sources: &[EntityId]) -> Option<(Vec2, f64)> {
    let [o, d] = sources else {
        return None;
    };
    let origin = doc.point_pos(*o)?;
    let dir = doc.point_pos(*d)?;
    let v = dir - origin;
    let angle = if v.length() > locus_geom::SOLVER_TOL {
        v.y.atan2(v.x)
    } else {
        0.0
    };
    Some((origin, angle))
}

/// Rewrites a generated object's instances from its cache and frame.
pub(crate) fn regenerate(
    doc: &mut Document,
    rid: RecipeId,
    patches: &mut Vec<crate::command::Patch>,
) -> Result<(), DocError> {
    let Some(r) = doc.recipe(rid).cloned() else {
        return Ok(());
    };
    let RecipeKind::Generated { output, .. } = &r.kind else {
        return Ok(());
    };
    let output = output.clone();
    let (origin, angle) = frame(doc, &r.sources).unwrap_or((Vec2::ZERO, 0.0));
    let (s, c) = angle.sin_cos();
    let place = |p: Vec2| Vec2::new(origin.x + p.x * c - p.y * s, origin.y + p.x * s + p.y * c);
    let mut out: Vec<crate::offset::Out> = output
        .points
        .iter()
        .map(|p| crate::offset::Out::Point(place(*p)))
        .collect();
    for e in &output.entities {
        out.push(match *e {
            GenEntity::Point(_) => continue,
            GenEntity::Line(a, b) => crate::offset::Out::Line(a as usize, b as usize),
            GenEntity::Circle(c, r) => crate::offset::Out::Circle(c as usize, r),
            GenEntity::Arc(c, s, e) => crate::offset::Out::Arc(c as usize, s as usize, e as usize),
        });
    }
    let layer = r
        .sources
        .first()
        .and_then(|s| doc.entity(*s).map(|e| e.layer))
        .unwrap_or_else(Document::default_layer);
    crate::offset::write_flat(doc, rid, r, layer, &out, patches)
}

/// The frame sources: exactly two distinct points (origin, direction).
pub(crate) fn validate(doc: &Document, sources: &[EntityId]) -> Result<(), DocError> {
    let [o, d] = sources else {
        return Err(bad("needs an origin point and a direction point"));
    };
    for id in [o, d] {
        let found = doc
            .entity(*id)
            .map_or(crate::EntityKind::Line, |e| e.geom.kind());
        if found != crate::EntityKind::Point {
            return Err(DocError::WrongKind {
                what: "plugin object frame",
                entity: *id,
                expected: "point",
                found,
            });
        }
    }
    Ok(())
}
