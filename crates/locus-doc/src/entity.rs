// SPDX-License-Identifier: AGPL-3.0-or-later

//! Sketch entities (SPEC §4, Entities v1).
//!
//! Points are entities in their own right; lines, circles and arcs reference
//! them by `EntityId`, so coincidence of endpoints is shared identity and the
//! solver sees one variable pair per point. Solved positions are stored here
//! (SPEC §10).

use crate::ids::{EntityId, LayerId};
use crate::text::TextData;
use locus_geom::Vec2;
use serde::{Deserialize, Serialize};

/// Which kind of geometry an entity is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EntityKind {
    Point,
    Line,
    Circle,
    Arc,
    /// Cubic Bézier (SPEC §4, limited constraints).
    Bezier,
    /// Text container (SPEC §9).
    Text,
}

/// Geometry of one entity. References are always to `Point` entities.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Geom {
    Point {
        pos: Vec2,
    },
    Line {
        start: EntityId,
        end: EntityId,
    },
    Circle {
        center: EntityId,
        radius: f64,
    },
    /// Counter-clockwise from `start` to `end` about `center`.
    Arc {
        center: EntityId,
        start: EntityId,
        end: EntityId,
    },
    /// Cubic Bézier from `start` to `end`; `start_ctrl` is the control point
    /// next to `start`, `end_ctrl` the one next to `end`. All four are point
    /// entities, so the solver treats control points like any other point.
    Bezier {
        start: EntityId,
        start_ctrl: EntityId,
        end_ctrl: EntityId,
        end: EntityId,
    },
    /// Text (SPEC §9; #179): a container that generates geometry. `anchor`
    /// is the baseline origin and `dir - anchor` the baseline direction, both
    /// ordinary points so they constrain like any sketch point. The laid-out
    /// geometry is cached in `text`, so no font file is needed to open it.
    Text {
        anchor: EntityId,
        dir: EntityId,
        text: Box<TextData>,
    },
}

impl Geom {
    #[must_use]
    pub fn kind(&self) -> EntityKind {
        match self {
            Geom::Point { .. } => EntityKind::Point,
            Geom::Line { .. } => EntityKind::Line,
            Geom::Circle { .. } => EntityKind::Circle,
            Geom::Arc { .. } => EntityKind::Arc,
            Geom::Bezier { .. } => EntityKind::Bezier,
            Geom::Text { .. } => EntityKind::Text,
        }
    }

    /// The point entities this geometry is built from, in a fixed order.
    #[must_use]
    pub fn point_refs(&self) -> Vec<EntityId> {
        match self {
            Geom::Point { .. } => Vec::new(),
            Geom::Line { start, end } => vec![*start, *end],
            Geom::Circle { center, .. } => vec![*center],
            Geom::Arc { center, start, end } => vec![*center, *start, *end],
            Geom::Bezier {
                start,
                start_ctrl,
                end_ctrl,
                end,
            } => vec![*start, *start_ctrl, *end_ctrl, *end],
            Geom::Text { anchor, dir, .. } => vec![*anchor, *dir],
        }
    }
}

/// An entity: geometry plus the per-entity flags every entity carries (SPEC §4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entity {
    pub layer: LayerId,
    /// Never exported, drawn dashed; independent of the layer.
    pub construction: bool,
    pub geom: Geom,
}
