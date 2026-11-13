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
//! A modifier plugin's output (#184) is the same kind of cache in a second
//! recipe kind, [`RecipeKind::Modified`], whose sources are the curves it
//! modifies. It is drawn as is (world coordinates), and run again through a
//! [`PluginRunner`] when its sources change (§15.2); without a runner (the
//! web build, or the plugin missing) the cache stays and the object is
//! reported stale.
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
    runner: Option<&dyn PluginRunner>,
) -> Result<(), DocError> {
    let Some(mut r) = doc.recipe(rid).cloned() else {
        return Ok(());
    };
    let (output, origin, angle) = match &r.kind {
        RecipeKind::Generated { output, .. } => {
            let (o, a) = frame(doc, &r.sources).unwrap_or((Vec2::ZERO, 0.0));
            (output.clone(), o, a)
        }
        RecipeKind::Modified {
            plugin,
            input_hash: h,
            output,
        } => {
            let mut output = output.clone();
            // Re-run only when the sources changed and a runner is here; a
            // failure keeps the cached output (the object reports stale).
            if let (Some(runner), Ok(input)) = (runner, modifier_input(doc, &r.sources)) {
                let now = input_hash(&input);
                if now != *h
                    && let Ok(fresh) = runner
                        .modify(plugin, &input)
                        .and_then(|g| g.validate().map(|()| g).map_err(|e| e.to_string()))
                {
                    let mut next = r.clone();
                    next.kind = RecipeKind::Modified {
                        plugin: plugin.clone(),
                        input_hash: now,
                        output: fresh.clone(),
                    };
                    crate::command::record(
                        doc,
                        patches,
                        crate::command::Patch::Recipe {
                            id: rid,
                            before: Some(r.clone()),
                            after: Some(next.clone()),
                        },
                    );
                    r = next;
                    output = fresh;
                }
            }
            (output, Vec2::ZERO, 0.0)
        }
        _ => return Ok(()),
    };
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
    crate::offset::write_flat(doc, rid, r, layer, crate::Style::default(), &out, patches)
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

// ── Modifiers (#184, §15.2) ─────────────────────────────────────────────────

/// One shape of a modifier's input path (mirrors the WIT `shape`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GenShape {
    Line {
        a: Vec2,
        b: Vec2,
    },
    /// Counter-clockwise from `start` to `end`.
    Arc {
        center: Vec2,
        start: Vec2,
        end: Vec2,
    },
    Circle {
        center: Vec2,
        radius: f64,
    },
}

/// A shape placed in a path; `reversed`: traversed end to start.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GenSegment {
    pub shape: GenShape,
    pub reversed: bool,
}

/// A modifier's input: the selected chain, as the plugin sees it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct GenPath {
    pub segments: Vec<GenSegment>,
    pub closed: bool,
}

/// Runs modifier plugins again when their source geometry changes (§15.2).
/// The document can't run plugins itself (core crates have no plugin host,
/// and the web build has none at all), so the app or CLI hands one to the
/// [`crate::Editor`]. Without one, modifier objects keep their cached output.
pub trait PluginRunner: Send + Sync + core::fmt::Debug {
    /// The plugin's output for `input`, or why there is none. An error keeps
    /// the cached output; it never fails the edit that changed the source.
    fn modify(&self, plugin: &PluginRef, input: &[GenPath]) -> Result<GeneratedGeometry, String>;
}

/// The modifier input for `sources`: one chained path (lines, arcs, or a
/// single circle). Béziers aren't part of the plugin interface (ADR 0009).
pub fn modifier_input(doc: &Document, sources: &[EntityId]) -> Result<Vec<GenPath>, DocError> {
    let (items, closed) = crate::offset::chain(doc, sources)?;
    let mut segments = Vec::with_capacity(items.len());
    for (id, reversed) in items {
        let shape = if let Some(l) = doc.line(id) {
            GenShape::Line { a: l.a, b: l.b }
        } else if let Some(a) = doc.arc(id) {
            GenShape::Arc {
                center: a.center,
                start: a.start,
                end: a.end,
            }
        } else if let Some(c) = doc.circle(id) {
            GenShape::Circle {
                center: c.center,
                radius: c.radius,
            }
        } else {
            return Err(bad("modifiers take lines, arcs and circles"));
        };
        segments.push(GenSegment { shape, reversed });
    }
    Ok(vec![GenPath { segments, closed }])
}

/// A stable fingerprint of a modifier's input (FNV-1a over the exact bits),
/// stored with the object so a changed source is noticed. Not `Hash`: this
/// is saved in files and must not change between Rust versions.
#[must_use]
pub fn input_hash(input: &[GenPath]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |v: u64| {
        for b in v.to_le_bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    };
    for p in input {
        eat(p.segments.len() as u64);
        eat(u64::from(p.closed));
        for s in &p.segments {
            eat(u64::from(s.reversed));
            match s.shape {
                GenShape::Line { a, b } => {
                    eat(1);
                    for v in [a.x, a.y, b.x, b.y] {
                        eat(v.to_bits());
                    }
                }
                GenShape::Arc { center, start, end } => {
                    eat(2);
                    for v in [center.x, center.y, start.x, start.y, end.x, end.y] {
                        eat(v.to_bits());
                    }
                }
                GenShape::Circle { center, radius } => {
                    eat(3);
                    for v in [center.x, center.y, radius] {
                        eat(v.to_bits());
                    }
                }
            }
        }
    }
    h
}

/// Whether a modifier object's cached output was made from different input
/// than its sources hold now (its plugin is missing, failed, or not run).
#[must_use]
pub fn is_stale(doc: &Document, rid: RecipeId) -> bool {
    match doc.recipe(rid) {
        Some(r) => match &r.kind {
            RecipeKind::Modified { input_hash: h, .. } => {
                modifier_input(doc, &r.sources).map_or(true, |i| input_hash(&i) != *h)
            }
            _ => false,
        },
        None => false,
    }
}

pub(crate) fn validate_modifier_sources(
    doc: &Document,
    sources: &[EntityId],
) -> Result<(), DocError> {
    modifier_input(doc, sources).map(|_| ())
}
