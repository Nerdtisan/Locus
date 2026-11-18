// SPDX-License-Identifier: AGPL-3.0-or-later

//! Fills belong to regions (SPEC §4, §6.3, #451): each closed region (a face
//! from `locus-region`, keyed by its [`RegionKey`]) has its own fill colour
//! and opacity. Strokes always draw above every fill, so a fill can never
//! hide a line. Objects have no fill of their own any more.
//!
//! Files before v19 kept the fill on objects. [`migrate_object_fills`] moves
//! each object fill onto the regions it bounded, giving each region the
//! colour the canvas showed it in (#338).

use crate::document::Document;
use crate::error::DocError;
use crate::ids::EntityId;
use crate::style::Fill;
use locus_region::RegionKey;
use serde::{Deserialize, Serialize};

/// Most regions one [`Command::SetRegionFill`](crate::Command::SetRegionFill)
/// or one file may fill, and most curves a region key may name.
pub const MAX_REGION_FILLS: usize = 100_000;

/// A region's fill: a colour (or none, remembering the colour last used, as
/// object fills did, #388) and its opacity, `0..=1`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RegionFill {
    pub fill: Fill,
    pub opacity: f64,
}

impl RegionFill {
    /// A solid, opaque fill.
    #[must_use]
    pub const fn colour(c: [u8; 3]) -> Self {
        Self {
            fill: Fill::Colour(c),
            opacity: 1.0,
        }
    }

    /// The colour painted, if any.
    #[must_use]
    pub fn painted(&self) -> Option<[u8; 3]> {
        self.fill.painted()
    }

    /// # Errors
    /// [`DocError::BadValue`] for an opacity outside `0..=1`.
    pub fn validate(&self) -> Result<(), DocError> {
        if !(self.opacity.is_finite() && (0.0..=1.0).contains(&self.opacity)) {
            return Err(DocError::BadValue {
                what: "fill opacity",
                value: self.opacity,
            });
        }
        Ok(())
    }
}

/// A region key must name at least one curve, and not absurdly many
/// (untrusted files and commands).
pub(crate) fn check_key(k: &RegionKey) -> Result<(), DocError> {
    if k.bounds.is_empty() || k.bounds.len() > MAX_REGION_FILLS {
        return Err(DocError::Invalid("bad region key".to_owned()));
    }
    Ok(())
}

/// Move every object fill (format v18 and before) onto the regions it
/// bounded, then clear the object fills. A region gets the fill of the first
/// filled curve on its outer boundary, at that object's opacity: exactly
/// what the canvas showed since #338, so holes stay open and an inner shape
/// keeps its own fill.
pub(crate) fn migrate_object_fills(doc: &mut Document) {
    let mut fills = std::collections::BTreeMap::new();
    for r in crate::regions::regions(doc) {
        let fill = r.outer.edges.iter().find_map(|e| {
            let s = doc.entity(EntityId::new(e.source))?.style;
            s.fill.painted().map(|c| RegionFill {
                fill: Fill::Colour(c),
                opacity: s.opacity.clamp(0.0, 1.0),
            })
        });
        if let Some(f) = fill {
            fills.insert(r.key, f);
        }
    }
    let ids: Vec<EntityId> = doc
        .entities
        .iter()
        .filter(|(_, e)| e.style.fill != Fill::None)
        .map(|(id, _)| id)
        .collect();
    for id in ids {
        if let Some(e) = doc.entities.get_mut(id) {
            e.style.fill = Fill::None;
        }
    }
    doc.region_fills = fills;
}
