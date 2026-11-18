// SPDX-License-Identifier: AGPL-3.0-or-later

//! Lowering the document to region detection (SPEC §6.3, #121). Regions are
//! derived data: never stored, recomputed after each solve.
//!
//! Regions are **per layer** (#457): each layer's faces come from that
//! layer's curves alone, because layers are separate machine operations. A
//! shape on another layer never splits, fills or hides a region; overlaps
//! within one layer still make separate regions (#451). Region keys name
//! entities, which belong to one layer, so keys never collide.

use crate::document::Document;
use crate::entity::Geom;
use locus_geom::{EXPORT_TOL, Piece};
use locus_region::{Curve, Region, Shape};

/// The curves regions are built from: lines, circles, arcs and Béziers (as arcs) that are not
/// construction geometry and sit on a visible layer (hiding a layer hides
/// its regions too).
#[must_use]
pub fn region_curves(doc: &Document) -> Vec<Curve> {
    doc.entities()
        .iter()
        .filter(|(_, e)| !e.construction && doc.layer(e.layer).is_some_and(|l| l.visible))
        .flat_map(|(id, e)| {
            let key = id.get();
            let shapes: Vec<Shape> = match e.geom {
                // Text outlines are not regions (SPEC §9); they export as paths.
                Geom::Point { .. } | Geom::Text { .. } => Vec::new(),
                Geom::Line { .. } => doc.line(id).map(Shape::Line).into_iter().collect(),
                Geom::Circle { .. } => doc.circle(id).map(Shape::Circle).into_iter().collect(),
                Geom::Arc { .. } => doc.arc(id).map(Shape::Arc).into_iter().collect(),
                // Region detection is exact for lines and arcs only (ADR 0008),
                // so a Bézier enters as its arc chain within the export
                // tolerance; every piece carries the Bézier's key.
                Geom::Bezier { .. } => doc
                    .cubic(id)
                    .map(|c| {
                        c.to_pieces(EXPORT_TOL)
                            .into_iter()
                            .map(|p| match p {
                                Piece::Line(l) => Shape::Line(l),
                                Piece::Arc(a) => Shape::Arc(a),
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
            };
            shapes.into_iter().map(move |shape| Curve { key, shape })
        })
        .collect()
}

/// Every closed region of the document, layer by layer, sorted by key.
#[must_use]
pub fn regions(doc: &Document) -> Vec<Region> {
    let mut by_layer: std::collections::BTreeMap<crate::LayerId, Vec<Curve>> =
        std::collections::BTreeMap::new();
    for c in region_curves(doc) {
        if let Some(e) = doc.entity(crate::EntityId::new(c.key)) {
            by_layer.entry(e.layer).or_default().push(c);
        }
    }
    let mut out: Vec<Region> = by_layer
        .into_values()
        .flat_map(|curves| locus_region::detect(&curves))
        .collect();
    out.sort_by(|a, b| a.key.cmp(&b.key));
    out
}

/// The regions as they were built before #457: every layer's curves
/// together. Only the v21 migration uses it, to find what an older key
/// meant.
pub(crate) fn regions_across_layers(doc: &Document) -> Vec<Region> {
    locus_region::detect(&region_curves(doc))
}

/// The layer a region belongs to: that of its outline's curves.
#[must_use]
pub fn layer_of(doc: &Document, r: &Region) -> Option<crate::LayerId> {
    let e = r.outer.edges.first()?;
    doc.entity(crate::EntityId::new(e.source)).map(|e| e.layer)
}

/// The region `key` on the current geometry, built from its own curves
/// alone: what a filled region looks like now, even after other curves on
/// its layer split it (#457).
pub(crate) fn rebuild(doc: &Document, key: &locus_region::RegionKey) -> Option<Region> {
    let ids: std::collections::BTreeSet<u64> = key.bounds.iter().copied().collect();
    let curves: Vec<Curve> = region_curves(doc)
        .into_iter()
        .filter(|c| ids.contains(&c.key))
        .collect();
    if curves.is_empty() {
        return None;
    }
    locus_region::detect(&curves)
        .into_iter()
        .find(|r| r.key == *key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{line, pt};

    fn rect(doc: &mut Document) -> Vec<crate::EntityId> {
        let p = [
            pt(doc, 0.0, 0.0),
            pt(doc, 4.0, 0.0),
            pt(doc, 4.0, 2.0),
            pt(doc, 0.0, 2.0),
        ];
        (0..4).map(|i| line(doc, p[i], p[(i + 1) % 4])).collect()
    }

    #[test]
    fn closed_geometry_makes_a_region_keyed_by_its_entities() {
        let mut doc = Document::new();
        let ls = rect(&mut doc);
        let rs = regions(&doc);
        assert_eq!(rs.len(), 1);
        assert_eq!(
            rs[0].key.bounds,
            ls.iter().map(|l| l.get()).collect::<Vec<_>>()
        );
        assert!((rs[0].area() - 8.0).abs() < 1e-9);
    }

    #[test]
    fn construction_and_hidden_geometry_make_no_regions() {
        let mut doc = Document::new();
        let ls = rect(&mut doc);
        if let Some(e) = doc.entities.get_mut(ls[0]) {
            e.construction = true;
        }
        assert!(regions(&doc).is_empty());
        if let Some(e) = doc.entities.get_mut(ls[0]) {
            e.construction = false;
        }
        if let Some(l) = doc.layers.get_mut(&Document::default_layer()) {
            l.visible = false;
        }
        assert!(regions(&doc).is_empty());
    }

    /// A Bézier arch closed by a line is a region whose area matches the
    /// analytic value (3300 mm²) to within the arc approximation.
    #[test]
    fn a_bezier_and_a_line_enclose_a_region() {
        use crate::testutil::{add, pt};
        let mut doc = Document::new();
        let p = [
            pt(&mut doc, 0.0, 0.0),
            pt(&mut doc, 0.0, 55.0),
            pt(&mut doc, 100.0, 55.0),
            pt(&mut doc, 100.0, 0.0),
        ];
        let bez = add(
            &mut doc,
            Geom::Bezier {
                start: p[0],
                start_ctrl: p[1],
                end_ctrl: p[2],
                end: p[3],
            },
        );
        let base = line(&mut doc, p[3], p[0]);
        let rs = regions(&doc);
        assert_eq!(rs.len(), 1);
        assert!((rs[0].area() - 3300.0).abs() < 0.5, "{}", rs[0].area());
        assert_eq!(rs[0].key.bounds, vec![bez.get(), base.get()]);
    }
}
