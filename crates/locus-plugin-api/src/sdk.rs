// SPDX-License-Identifier: MIT OR Apache-2.0

//! Small helpers over the generated bindings: building parameter schemas,
//! reading parameter values and building generator output.

use crate::generator::exports::locus::plugin::generator::{Constraint, Entity, Generated};
use crate::shared::locus::plugin::params::{
    ChoiceSpec, IntegerSpec, NumberSpec, Param, ParamKind, ParamSpec, ParamValue,
};
use crate::shared::locus::plugin::types::{PluginError, Vec2};

/// A number parameter with a unit and an inclusive range (§15.3).
pub fn number(key: &str, label: &str, unit: &str, min: f64, max: f64, default: f64) -> ParamSpec {
    ParamSpec {
        key: key.into(),
        label: label.into(),
        help: String::new(),
        kind: ParamKind::Number(NumberSpec {
            unit: unit.into(),
            min: Some(min),
            max: Some(max),
            default,
        }),
    }
}

/// An integer parameter with an inclusive range.
pub fn integer(key: &str, label: &str, min: i64, max: i64, default: i64) -> ParamSpec {
    ParamSpec {
        key: key.into(),
        label: label.into(),
        help: String::new(),
        kind: ParamKind::Integer(IntegerSpec {
            min: Some(min),
            max: Some(max),
            default,
        }),
    }
}

/// A yes/no parameter.
pub fn flag(key: &str, label: &str, default: bool) -> ParamSpec {
    ParamSpec {
        key: key.into(),
        label: label.into(),
        help: String::new(),
        kind: ParamKind::Boolean(default),
    }
}

/// A choice between named options; `default` indexes `options`.
pub fn choice(key: &str, label: &str, options: &[&str], default: u32) -> ParamSpec {
    ParamSpec {
        key: key.into(),
        label: label.into(),
        help: String::new(),
        kind: ParamKind::Choice(ChoiceSpec {
            options: options.iter().map(|o| (*o).into()).collect(),
            default,
        }),
    }
}

/// Reads parameter values by key, with the schema's defaults filled in.
///
/// The host fills every key from the schema, so a missing key is an error;
/// a value outside the schema's range is reported as `bad-parameter`.
#[derive(Debug, Clone, Copy)]
pub struct Params<'a> {
    values: &'a [Param],
}

fn bad(msg: String) -> PluginError {
    PluginError::BadParameter(msg)
}

impl<'a> Params<'a> {
    pub fn new(values: &'a [Param]) -> Params<'a> {
        Params { values }
    }

    fn get(&self, key: &str) -> Result<&'a ParamValue, PluginError> {
        self.values
            .iter()
            .find(|p| p.key == key)
            .map(|p| &p.value)
            .ok_or_else(|| bad(format!("missing parameter `{key}`")))
    }

    /// A number; integers are accepted too.
    pub fn number(&self, key: &str) -> Result<f64, PluginError> {
        match self.get(key)? {
            ParamValue::Number(n) if n.is_finite() => Ok(*n),
            ParamValue::Integer(n) => Ok(*n as f64),
            _ => Err(bad(format!("`{key}` must be a number"))),
        }
    }

    /// A number checked against an inclusive range.
    pub fn number_in(&self, key: &str, min: f64, max: f64) -> Result<f64, PluginError> {
        let n = self.number(key)?;
        if n < min || n > max {
            return Err(bad(format!("`{key}` must be between {min} and {max}")));
        }
        Ok(n)
    }

    pub fn integer(&self, key: &str) -> Result<i64, PluginError> {
        match self.get(key)? {
            ParamValue::Integer(n) => Ok(*n),
            _ => Err(bad(format!("`{key}` must be an integer"))),
        }
    }

    pub fn boolean(&self, key: &str) -> Result<bool, PluginError> {
        match self.get(key)? {
            ParamValue::Boolean(b) => Ok(*b),
            _ => Err(bad(format!("`{key}` must be yes or no"))),
        }
    }

    /// The chosen option's index.
    pub fn choice(&self, key: &str) -> Result<u32, PluginError> {
        match self.get(key)? {
            ParamValue::Choice(c) => Ok(*c),
            _ => Err(bad(format!("`{key}` must be a choice"))),
        }
    }

    pub fn text(&self, key: &str) -> Result<&'a str, PluginError> {
        match self.get(key)? {
            ParamValue::Text(t) => Ok(t),
            _ => Err(bad(format!("`{key}` must be text"))),
        }
    }
}

/// Builds generator output. Identical points are shared, so corners stay
/// connected (and constrainable) in the document (§15.3).
#[derive(Debug, Default)]
pub struct Sketch {
    points: Vec<Vec2>,
    entities: Vec<Entity>,
    constraints: Vec<Constraint>,
}

impl Sketch {
    pub fn new() -> Sketch {
        Sketch::default()
    }

    /// The index of the point at (x, y), adding it if new. Exact comparison:
    /// compute shared corners once and reuse the value.
    pub fn point(&mut self, x: f64, y: f64) -> u32 {
        let found = self
            .points
            .iter()
            .position(|p| p.x.to_bits() == x.to_bits() && p.y.to_bits() == y.to_bits());
        let i = found.unwrap_or_else(|| {
            self.points.push(Vec2 { x, y });
            self.points.len() - 1
        });
        i as u32
    }

    fn entity(&mut self, e: Entity) -> u32 {
        self.entities.push(e);
        (self.entities.len() - 1) as u32
    }

    /// A line between two point indices; returns the entity index.
    pub fn line(&mut self, a: u32, b: u32) -> u32 {
        self.entity(Entity::Line((a, b)))
    }

    pub fn circle(&mut self, center: u32, radius: f64) -> u32 {
        self.entity(Entity::Circle((center, radius)))
    }

    /// A counter-clockwise arc from `start` to `end` around `center`.
    pub fn arc(&mut self, center: u32, start: u32, end: u32) -> u32 {
        self.entity(Entity::Arc((center, start, end)))
    }

    /// A closed polygon of lines; returns the line entity indices in order.
    /// Consecutive duplicate vertices are skipped.
    pub fn polygon(&mut self, vertices: &[(f64, f64)]) -> Vec<u32> {
        let mut idx: Vec<u32> = Vec::with_capacity(vertices.len());
        for &(x, y) in vertices {
            let p = self.point(x, y);
            if idx.last() != Some(&p) {
                idx.push(p);
            }
        }
        if idx.len() > 1 && idx.first() == idx.last() {
            idx.pop();
        }
        if idx.len() < 2 {
            return Vec::new();
        }
        (0..idx.len())
            .map(|i| self.line(idx[i], idx[(i + 1) % idx.len()]))
            .collect()
    }

    pub fn constrain(&mut self, c: Constraint) {
        self.constraints.push(c);
    }

    pub fn finish(self) -> Generated {
        Generated {
            points: self.points,
            entities: self.entities,
            constraints: self.constraints,
        }
    }
}
