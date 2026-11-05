// SPDX-License-Identifier: AGPL-3.0-or-later

//! Lowering the document to region detection (SPEC §6.3, #121). Regions are
//! derived data: never stored, recomputed after each solve.

use crate::document::Document;
use crate::entity::Geom;
use locus_region::{Curve, Region, Shape};

/// The curves regions are built from: lines, circles and arcs that are not
/// construction geometry and sit on a visible layer (hiding a layer hides
/// its regions too).
#[must_use]
pub fn region_curves(doc: &Document) -> Vec<Curve> {
    doc.entities()
        .iter()
        .filter(|(_, e)| !e.construction && doc.layer(e.layer).is_some_and(|l| l.visible))
        .filter_map(|(id, e)| {
            let shape = match e.geom {
                Geom::Point { .. } => return None,
                Geom::Line { .. } => Shape::Line(doc.line(id)?),
                Geom::Circle { .. } => Shape::Circle(doc.circle(id)?),
                Geom::Arc { .. } => Shape::Arc(doc.arc(id)?),
            };
            Some(Curve {
                key: id.get(),
                shape,
            })
        })
        .collect()
}

/// Every closed region of the document, sorted by key.
#[must_use]
pub fn regions(doc: &Document) -> Vec<Region> {
    locus_region::detect(&region_curves(doc))
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
}
