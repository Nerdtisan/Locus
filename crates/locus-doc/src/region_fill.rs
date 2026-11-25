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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RegionFill {
    pub fill: Fill,
    pub opacity: f64,
    /// A gradient instead of the solid colour (#436, format v27). `fill`
    /// stays the solid colour, used when the gradient is turned off, the
    /// same "remember the last choice" as #388. Omitted while `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gradient: Option<Box<crate::gradient::Gradient>>,
}

impl RegionFill {
    /// A solid, opaque fill.
    #[must_use]
    pub const fn colour(c: [u8; 3]) -> Self {
        Self {
            fill: Fill::Colour(c),
            opacity: 1.0,
            gradient: None,
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
        if let Some(g) = &self.gradient {
            g.validate()?;
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

/// A point strictly inside `r` (not on a curve, not in a hole): a little
/// inside the middle of one of its outer edges.
#[must_use]
pub fn interior_point(r: &locus_region::Region) -> Option<locus_geom::Vec2> {
    let size = r
        .outer
        .bbox()
        .map_or(1.0, |b| (b.max.x - b.min.x).max(b.max.y - b.min.y));
    for e in &r.outer.edges {
        let m = e.seg.at(0.5);
        // The outer loop runs counter-clockwise: the inside is on the left.
        let n = e.seg.tangent(0.5).perp();
        for k in [1e-4, 1e-3, 1e-2] {
            let p = m + n * (size * k);
            if r.contains(p) {
                return Some(p);
            }
        }
    }
    None
}

/// Fills to move after an edit changed curves they are keyed by (#515):
/// an offset, pattern or mirror that regenerates gives its region new
/// bounding curves, so its stored key stops matching and, unlike a split
/// (#457, see [`resolve`]), can't be rebuilt from its own curves either.
/// Each such fill moves to the region now at the same place: the innermost
/// region on its layer holding a point inside the region as it was in
/// `before`, unless that region has a stored fill already.
///
/// `changed` are the entities the edit touched; only fills keyed by one of
/// them are looked at, so most edits never build regions here.
pub(crate) fn carry(
    doc: &Document,
    before: &Document,
    changed: &std::collections::BTreeSet<EntityId>,
) -> Vec<(RegionKey, RegionKey)> {
    let touched: Vec<&RegionKey> = doc
        .region_fills
        .keys()
        .filter(|k| {
            k.bounds
                .iter()
                .any(|b| changed.contains(&EntityId::new(*b)))
        })
        .collect();
    if touched.is_empty() {
        return Vec::new();
    }
    let now = crate::regions::regions(doc);
    let live: std::collections::BTreeSet<&RegionKey> = now.iter().map(|r| &r.key).collect();
    let mut taken: std::collections::BTreeSet<RegionKey> =
        doc.region_fills.keys().cloned().collect();
    let mut out = Vec::new();
    for k in touched {
        // Still a region, or still rebuildable from its own curves (a split,
        // whose pieces all keep the fill): nothing to move.
        if live.contains(k) || crate::regions::rebuild(doc, k).is_some() {
            continue;
        }
        let Some(old) = crate::regions::rebuild(before, k) else {
            continue;
        };
        let (Some(p), layer) = (interior_point(&old), crate::regions::layer_of(before, &old))
        else {
            continue;
        };
        let target = now
            .iter()
            .filter(|r| crate::regions::layer_of(doc, r) == layer && r.contains(p))
            .min_by(|a, b| a.area().abs().total_cmp(&b.area().abs()));
        if let Some(t) = target
            && taken.insert(t.key.clone())
        {
            out.push((k.clone(), t.key.clone()));
        }
    }
    out
}

/// Each region's fill (#457): the one resolver the canvas, the Inspector
/// and export all use, so they can't disagree. A region takes
/// - the fill stored under its own key, if any (a stored "no fill" counts:
///   it hides an inherited fill), else
/// - the fill of a filled region that an edit on the same layer split: the
///   stored key no longer matches any region, so that region is rebuilt
///   from its own curves on the current geometry, and every piece inside it
///   keeps its fill. The innermost such region wins.
///
/// One entry per region, in the order of `regions`.
#[must_use]
pub fn resolve(doc: &Document, regions: &[locus_region::Region]) -> Vec<Option<RegionFill>> {
    if doc.region_fills.is_empty() {
        return vec![None; regions.len()];
    }
    let live: std::collections::BTreeSet<&RegionKey> = regions.iter().map(|r| &r.key).collect();
    // Filled regions that were split: their shape now, their layer, fill.
    let split: Vec<(
        locus_region::Region,
        Option<crate::LayerId>,
        RegionFill,
        f64,
    )> = doc
        .region_fills
        .iter()
        .filter(|(k, f)| !live.contains(k) && f.painted().is_some())
        .filter_map(|(k, f)| {
            let r = crate::regions::rebuild(doc, k)?;
            let layer = crate::regions::layer_of(doc, &r);
            let area = r.area().abs();
            Some((r, layer, f.clone(), area))
        })
        .collect();
    regions
        .iter()
        .map(|r| {
            if let Some(f) = doc.region_fills.get(&r.key) {
                return Some(f.clone());
            }
            if split.is_empty() {
                return None;
            }
            let p = interior_point(r)?;
            let layer = crate::regions::layer_of(doc, r);
            split
                .iter()
                .filter(|(old, l, ..)| *l == layer && old.contains(p))
                .min_by(|a, b| a.3.total_cmp(&b.3))
                .map(|(_, _, f, _)| f.clone())
        })
        .collect()
}

/// Format v20 and before built regions across layers, so a stored key could
/// name a face cut by another layer's shape (#457). Each such key (for
/// fills and boolean recipes) moves to the per-layer region holding the
/// same place: a point inside the old face, on the layer of its outline.
/// Keys that already match, or whose region is gone, stay as they are.
pub(crate) fn migrate_region_keys(doc: &mut Document) {
    let old = crate::regions::regions_across_layers(doc);
    let new = crate::regions::regions(doc);
    let rekey = |k: &RegionKey| -> RegionKey {
        if new.binary_search_by(|r| r.key.cmp(k)).is_ok() {
            return k.clone();
        }
        let Some(face) = old.iter().find(|r| r.key == *k) else {
            return k.clone();
        };
        let (Some(p), layer) = (interior_point(face), crate::regions::layer_of(doc, face)) else {
            return k.clone();
        };
        new.iter()
            .filter(|r| crate::regions::layer_of(doc, r) == layer && r.contains(p))
            .min_by(|a, b| a.area().abs().total_cmp(&b.area().abs()))
            .map_or_else(|| k.clone(), |r| r.key.clone())
    };
    let mut fills = std::collections::BTreeMap::new();
    for (k, f) in &doc.region_fills {
        fills.entry(rekey(k)).or_insert(f.clone());
    }
    let booleans: Vec<_> = doc
        .booleans
        .iter()
        .map(|(id, b)| {
            let mut b = b.clone();
            b.targets = b.targets.iter().map(&rekey).collect();
            b.tools = b.tools.iter().map(&rekey).collect();
            (*id, b)
        })
        .collect();
    doc.region_fills = fills;
    doc.booleans = booleans.into_iter().collect();
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
            let s = doc.entity(EntityId::new(e.source))?.style.clone();
            s.fill.painted().map(|c| RegionFill {
                fill: Fill::Colour(c),
                opacity: s.opacity.clamp(0.0, 1.0),
                gradient: None,
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
