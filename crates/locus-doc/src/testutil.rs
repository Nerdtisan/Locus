// SPDX-License-Identifier: AGPL-3.0-or-later

//! Test-only helpers that build documents without going through commands.

use crate::document::Document;
use crate::entity::{Entity, Geom};
use crate::ids::EntityId;
use locus_geom::Vec2;

pub(crate) fn add(doc: &mut Document, geom: Geom) -> EntityId {
    let id = EntityId::new(doc.alloc_id().unwrap());
    let e = Entity {
        layer: Document::default_layer(),
        construction: false,
        style: Default::default(),
        geom,
    };
    doc.check_entity(&e).unwrap();
    doc.entities.insert(id, e);
    id
}

pub(crate) fn pt(doc: &mut Document, x: f64, y: f64) -> EntityId {
    add(
        doc,
        Geom::Point {
            pos: Vec2::new(x, y),
        },
    )
}

pub(crate) fn line(doc: &mut Document, a: EntityId, b: EntityId) -> EntityId {
    add(doc, Geom::Line { start: a, end: b })
}

/// A document with a few of each entity kind.
pub(crate) struct Fixture {
    pub doc: Document,
    pub p: [EntityId; 4],
    pub l: [EntityId; 2],
    pub c: [EntityId; 2],
    pub a: [EntityId; 2],
}

pub(crate) fn fixture() -> Fixture {
    let mut doc = Document::new();
    let p = [
        pt(&mut doc, 0.0, 0.0),
        pt(&mut doc, 10.0, 0.0),
        pt(&mut doc, 10.0, 10.0),
        pt(&mut doc, 0.0, 10.0),
    ];
    let l = [line(&mut doc, p[0], p[1]), line(&mut doc, p[3], p[2])];
    let cc = [pt(&mut doc, 5.0, 5.0), pt(&mut doc, 30.0, 5.0)];
    let c = [
        add(
            &mut doc,
            Geom::Circle {
                center: cc[0],
                radius: 2.0,
            },
        ),
        add(
            &mut doc,
            Geom::Circle {
                center: cc[1],
                radius: 3.0,
            },
        ),
    ];
    let s = [
        pt(&mut doc, 8.0, 5.0),
        pt(&mut doc, 5.0, 8.0),
        pt(&mut doc, 2.0, 5.0),
    ];
    let a = [
        add(
            &mut doc,
            Geom::Arc {
                center: cc[0],
                start: s[0],
                end: s[1],
            },
        ),
        add(
            &mut doc,
            Geom::Arc {
                center: cc[1],
                start: s[1],
                end: s[2],
            },
        ),
    ];
    Fixture { doc, p, l, c, a }
}
