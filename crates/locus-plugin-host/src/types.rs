// SPDX-License-Identifier: AGPL-3.0-or-later

//! Host-side mirror of the `locus:plugin@1.0.0` WIT types (ADR 0009).
//!
//! The runtime converts between these and the wasmtime bindings, so the rest
//! of the host (validation, document conversion, CLI) never depends on
//! generated code or on wasmtime, and builds on every target.

/// The plugin kinds of §15.2, one WIT world each (ADR 0009).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    Generator,
    Modifier,
    Exporter,
    Importer,
    Validator,
}

impl Kind {
    pub const ALL: [Kind; 5] = [
        Kind::Generator,
        Kind::Modifier,
        Kind::Exporter,
        Kind::Importer,
        Kind::Validator,
    ];

    /// The WIT interface name, also the `kinds` spelling in `plugin.toml`.
    pub const fn name(self) -> &'static str {
        match self {
            Kind::Generator => "generator",
            Kind::Modifier => "modifier",
            Kind::Exporter => "exporter",
            Kind::Importer => "importer",
            Kind::Validator => "validator",
        }
    }

    pub fn from_name(name: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.name() == name)
    }
}

/// A point or vector in millimetres.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vec2 {
    pub x: f64,
    pub y: f64,
}

/// One shape of a path. Arcs run counter-clockwise from `start` to `end`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Shape {
    Line {
        a: Vec2,
        b: Vec2,
    },
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

/// A shape placed in a path; `reversed` means traversed end to start.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Segment {
    pub shape: Shape,
    pub reversed: bool,
}

/// Segments joined end to end.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Path {
    pub segments: Vec<Segment>,
    pub closed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compensation {
    None,
    Inside,
    Outside,
}

/// A layer's machine operation and its settings (§7).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Operation {
    Cut {
        kerf_width: f64,
        compensation: Compensation,
        passes: u32,
    },
    Score,
    Engrave {
        hatch_spacing: f64,
        hatch_angle: f64,
    },
    Draw {
        pen_number: u32,
        pen_width: f64,
    },
    DragKnife {
        blade_offset: f64,
    },
    Reference,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Layer {
    pub name: String,
    /// 0xRRGGBB.
    pub colour: u32,
    pub operation: Operation,
    pub paths: Vec<Path>,
}

/// Validated export geometry, as the built-in writers receive it (§8).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ExportModel {
    pub layers: Vec<Layer>,
}

/// What a plugin says about itself (`info.describe`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginInfo {
    pub id: String,
    pub name: String,
    pub version: String,
}

/// An error the plugin itself reported (as opposed to a trap or a limit).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PluginFailure {
    #[error("bad parameter: {0}")]
    BadParameter(String),
    #[error("unsupported: {0}")]
    Unsupported(String),
    #[error("{0}")]
    Failed(String),
}

// ── Parameters (§15.3) ───────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct NumberSpec {
    pub unit: String,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub default: f64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntegerSpec {
    pub min: Option<i64>,
    pub max: Option<i64>,
    pub default: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChoiceSpec {
    pub options: Vec<String>,
    pub default: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextSpec {
    pub default: String,
    pub max_length: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ParamKind {
    Number(NumberSpec),
    Integer(IntegerSpec),
    Boolean(bool),
    Choice(ChoiceSpec),
    Text(TextSpec),
    DocParameter,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParamSpec {
    pub key: String,
    pub label: String,
    pub help: String,
    pub kind: ParamKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ParamValue {
    Number(f64),
    Integer(i64),
    Boolean(bool),
    Choice(u32),
    Text(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub key: String,
    pub value: ParamValue,
}

// ── Generators ───────────────────────────────────────────────────────────────

/// Generator geometry; curves refer to `Generated::points` by index.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Entity {
    Point(u32),
    Line(u32, u32),
    Circle { center: u32, radius: f64 },
    Arc { center: u32, start: u32, end: u32 },
}

/// Constraints a generator may emit; indices refer to `Generated::entities`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Constraint {
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

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Generated {
    pub points: Vec<Vec2>,
    pub entities: Vec<Entity>,
    pub constraints: Vec<Constraint>,
}

// ── Exporters ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatInfo {
    pub id: String,
    pub name: String,
    pub extension: String,
    pub mime: String,
}

// ── Validators (M4, #234) ────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Error,
    Warning,
}

/// One problem a validator plugin found. `layer` and `path` index the export
/// model the plugin received (checked by the host against that model).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub severity: Severity,
    pub message: String,
    pub layer: Option<u32>,
    pub path: Option<u32>,
}
