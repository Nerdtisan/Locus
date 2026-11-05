// SPDX-License-Identifier: AGPL-3.0-or-later

//! Non-destructive boolean recipes (SPEC §6.3, #123): an operation over
//! source regions, referenced by their [`RegionKey`]s (entity IDs), stored in
//! the document and recomputed after each solve with the exact ADR 0008
//! engine. The result is derived data, never stored.

use crate::error::DocError;
use locus_region::{Area, BoolOp, Region, RegionKey, boolean};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Upper bound on the source regions of one recipe, against hostile files.
pub const MAX_SOURCES: usize = 10_000;

/// `union(targets) op union(tools)`. A union may leave `tools` empty.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Boolean {
    pub op: BoolOp,
    pub targets: Vec<RegionKey>,
    #[serde(default)]
    pub tools: Vec<RegionKey>,
}

/// Why a recipe has no result right now. The recipe itself is kept, so the
/// result comes back if an edit (or undo) restores the sources.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum BooleanError {
    #[error("{} source region(s) no longer exist", .0.len())]
    MissingRegions(Vec<RegionKey>),
}

impl Boolean {
    /// Structural checks, for commands and file loading.
    ///
    /// # Errors
    /// [`DocError::Invalid`] for no targets, a subtract or intersect without
    /// tools, or too many sources.
    pub fn validate(&self) -> Result<(), DocError> {
        if self.targets.is_empty() {
            return Err(DocError::Invalid(
                "a boolean needs at least one target region".to_owned(),
            ));
        }
        if self.op != BoolOp::Union && self.tools.is_empty() {
            return Err(DocError::Invalid(
                "subtract and intersect need at least one tool region".to_owned(),
            ));
        }
        if self.targets.len() + self.tools.len() > MAX_SOURCES {
            return Err(DocError::Invalid(format!(
                "a boolean may use at most {MAX_SOURCES} regions"
            )));
        }
        Ok(())
    }

    /// The result over the current regions (sorted by key, as
    /// [`locus_region::detect`] returns them).
    ///
    /// # Errors
    /// [`BooleanError::MissingRegions`] when a source region is gone.
    pub fn evaluate(&self, regions: &[Region]) -> Result<Area, BooleanError> {
        let mut missing = Vec::new();
        let mut gather = |keys: &[RegionKey]| {
            let mut area = Area::default();
            for k in keys {
                match regions.binary_search_by(|r| r.key.cmp(k)) {
                    Ok(i) => area.loops.extend(regions[i].to_area().loops),
                    Err(_) => missing.push(k.clone()),
                }
            }
            area
        };
        // Faces of one sketch never overlap, so each side is simply the set
        // of its faces' loops (shared edges cancel in the engine). A face in
        // both sides is fine: the non-zero rule counts it once per side.
        let (a, b) = (gather(&self.targets), gather(&self.tools));
        if !missing.is_empty() {
            missing.sort();
            missing.dedup();
            return Err(BooleanError::MissingRegions(missing));
        }
        Ok(boolean(&a, &b, self.op))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use locus_geom::{Circle, Line, Vec2};
    use locus_region::{Curve, Shape, detect};

    fn sketch(cx: f64) -> Vec<Region> {
        let v = Vec2::new;
        let p = [v(0.0, 0.0), v(4.0, 0.0), v(4.0, 4.0), v(0.0, 4.0)];
        let mut cs: Vec<Curve> = (0..4)
            .map(|i| Curve {
                key: i as u64 + 1,
                shape: Shape::Line(Line::new(p[i], p[(i + 1) % 4])),
            })
            .collect();
        cs.push(Curve {
            key: 9,
            shape: Shape::Circle(Circle::new(v(cx, 4.0), 2.0)),
        });
        detect(&cs)
    }

    #[test]
    fn union_of_all_faces_is_the_combined_outline() {
        let rs = sketch(4.0);
        let all: Vec<RegionKey> = rs.iter().map(|r| r.key.clone()).collect();
        let b = Boolean {
            op: BoolOp::Union,
            targets: all,
            tools: vec![],
        };
        b.validate().unwrap();
        let out = b.evaluate(&rs).unwrap();
        assert_eq!(out.loops.len(), 1);
        assert!((out.area() - (16.0 + 3.0 * core::f64::consts::PI)).abs() < 1e-9);
        assert_eq!(out.edges().filter(|e| e.seg.is_arc()).count(), 1);
    }

    #[test]
    fn missing_sources_are_reported_not_panicked() {
        let rs = sketch(4.0);
        let gone = RegionKey {
            bounds: vec![77],
            index: 0,
        };
        let b = Boolean {
            op: BoolOp::Subtract,
            targets: vec![rs[0].key.clone()],
            tools: vec![gone.clone()],
        };
        assert_eq!(
            b.evaluate(&rs),
            Err(BooleanError::MissingRegions(vec![gone]))
        );
    }

    #[test]
    fn validation() {
        let k = RegionKey {
            bounds: vec![1],
            index: 0,
        };
        let b = |op, targets: Vec<RegionKey>, tools| Boolean { op, targets, tools };
        assert!(b(BoolOp::Union, vec![], vec![]).validate().is_err());
        assert!(
            b(BoolOp::Subtract, vec![k.clone()], vec![])
                .validate()
                .is_err()
        );
        assert!(b(BoolOp::Union, vec![k], vec![]).validate().is_ok());
    }
}
