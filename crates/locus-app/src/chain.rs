// SPDX-License-Identifier: AGPL-3.0-or-later
//! Chain walking for chain select (design §3.1, §6.11; #220): the curves
//! joined end to end with a starting curve, stopping at branch points where
//! three or more curves meet, as Fusion does.
//!
//! Curves join where they share an endpoint (shared point identity, SPEC §4)
//! or where their endpoints are held together by a Coincident constraint.
//! A circle has no ends, so it is a chain on its own. Only curves of the
//! starting curve's linetype take part: a construction diagonal across a
//! rectangle doesn't break its outline into pieces, and an outline doesn't
//! leak into construction geometry. Read-only: selecting changes no document
//! state (SPEC §11).

use locus_doc::{Constraint, Document, EntityId, Geom};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// The two end points of an open curve.
fn ends(g: &Geom) -> Option<[EntityId; 2]> {
    match *g {
        Geom::Line { start, end }
        | Geom::Arc { start, end, .. }
        | Geom::Bezier { start, end, .. } => Some([start, end]),
        Geom::Point { .. } | Geom::Circle { .. } | Geom::Text { .. } => None,
    }
}

/// Union-find over point ids, joined by Coincident constraints.
struct Joins(BTreeMap<EntityId, EntityId>);

impl Joins {
    fn new(doc: &Document) -> Self {
        let mut j = Self(BTreeMap::new());
        let is_point = |id: &EntityId| {
            doc.entity(*id)
                .is_some_and(|e| matches!(e.geom, Geom::Point { .. }))
        };
        for c in doc.constraints().values() {
            if let Constraint::Coincident { a, b } = c
                && is_point(a)
                && is_point(b)
            {
                let (ra, rb) = (j.root(*a), j.root(*b));
                if ra != rb {
                    j.0.insert(ra, rb);
                }
            }
        }
        j
    }

    fn root(&self, mut id: EntityId) -> EntityId {
        // Bounded by the number of joins, so a hostile file can't loop.
        for _ in 0..=self.0.len() {
            match self.0.get(&id) {
                Some(next) if *next != id => id = *next,
                _ => break,
            }
        }
        id
    }
}

/// The chain `start` belongs to, `start` first, then in walk order. Empty
/// when `start` is not a curve.
#[must_use]
pub fn chain(doc: &Document, start: EntityId) -> Vec<EntityId> {
    let Some(first) = doc.entity(start) else {
        return Vec::new();
    };
    match &first.geom {
        Geom::Point { .. } => return Vec::new(),
        g if ends(g).is_none() => return vec![start],
        _ => {}
    }
    let joins = Joins::new(doc);
    let linetype = first.construction;
    // Which curves of that linetype end at each (joined) point.
    let mut at: BTreeMap<EntityId, Vec<EntityId>> = BTreeMap::new();
    for (id, e) in doc.entities().iter() {
        if e.construction != linetype {
            continue;
        }
        if let Some(ps) = ends(&e.geom) {
            for p in ps {
                at.entry(joins.root(p)).or_default().push(id);
            }
        }
    }
    let mut seen = BTreeSet::from([start]);
    let mut out = vec![start];
    let mut queue = VecDeque::from([start]);
    while let Some(c) = queue.pop_front() {
        let Some(ps) = doc.entity(c).and_then(|e| ends(&e.geom)) else {
            continue;
        };
        for p in ps {
            let curves = at.get(&joins.root(p)).map_or(&[][..], Vec::as_slice);
            // A branch point (three or more ends) stops the chain. A curve
            // whose two ends meet each other counts twice and also stops.
            if curves.len() != 2 {
                continue;
            }
            for next in curves {
                if seen.insert(*next) {
                    out.push(*next);
                    queue.push_back(*next);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use locus_doc::{Command, Editor, Style};
    use locus_geom::Vec2;

    struct B(Editor);

    impl B {
        fn add(&mut self, geom: Geom) -> EntityId {
            self.0
                .apply(&Command::AddEntity {
                    layer: Document::default_layer(),
                    construction: false,
                    geom,
                    style: Style::default(),
                })
                .map_or(EntityId::new(0), |a| a.entities[0])
        }

        fn pt(&mut self, x: f64, y: f64) -> EntityId {
            self.add(Geom::Point {
                pos: Vec2::new(x, y),
            })
        }

        fn line(&mut self, start: EntityId, end: EntityId) -> EntityId {
            self.add(Geom::Line { start, end })
        }
    }

    fn sorted(mut v: Vec<EntityId>) -> Vec<EntityId> {
        v.sort_unstable();
        v
    }

    /// A closed rectangle is one chain from any side.
    #[test]
    fn a_closed_loop_is_one_chain() {
        let mut b = B(Editor::new(Document::new()));
        let p: Vec<EntityId> = [(0.0, 0.0), (10.0, 0.0), (10.0, 5.0), (0.0, 5.0)]
            .iter()
            .map(|&(x, y)| b.pt(x, y))
            .collect();
        let sides: Vec<EntityId> = (0..4).map(|i| b.line(p[i], p[(i + 1) % 4])).collect();
        let d = b.0.document();
        for s in &sides {
            let c = chain(d, *s);
            assert_eq!(c[0], *s, "the clicked curve comes first");
            assert_eq!(sorted(c), sorted(sides.clone()));
        }
    }

    /// The walk stops where three curves meet, and takes a Coincident join.
    #[test]
    fn branches_stop_the_walk_and_coincidences_join() {
        let mut b = B(Editor::new(Document::new()));
        let (a, m, c) = (b.pt(0.0, 0.0), b.pt(10.0, 0.0), b.pt(20.0, 0.0));
        let l1 = b.line(a, m);
        let l2 = b.line(m, c);
        // A spur from the middle point: m is now a branch point.
        let up = b.pt(10.0, 10.0);
        let spur = b.line(m, up);
        let d = b.0.document();
        assert_eq!(chain(d, l1), vec![l1]);
        assert_eq!(chain(d, spur), vec![spur]);
        // A separate line whose start is held on c by a Coincident.
        let (c2, e) = (b.pt(20.0, 0.0), b.pt(30.0, 0.0));
        let l3 = b.line(c2, e);
        b.0.apply(&Command::AddConstraint {
            constraint: Constraint::Coincident { a: c, b: c2 },
        })
        .ok();
        let d = b.0.document();
        assert_eq!(sorted(chain(d, l3)), sorted(vec![l2, l3]));
    }

    /// Construction geometry neither joins nor breaks a normal chain.
    #[test]
    fn chains_keep_to_one_linetype() {
        let mut b = B(Editor::new(Document::new()));
        let p: Vec<EntityId> = [(0.0, 0.0), (10.0, 0.0), (10.0, 5.0), (0.0, 5.0)]
            .iter()
            .map(|&(x, y)| b.pt(x, y))
            .collect();
        let sides: Vec<EntityId> = (0..4).map(|i| b.line(p[i], p[(i + 1) % 4])).collect();
        let diagonal = b.line(p[0], p[2]);
        b.0.apply(&Command::ToggleConstruction { id: diagonal })
            .ok();
        let d = b.0.document();
        assert_eq!(sorted(chain(d, sides[0])), sorted(sides.clone()));
        assert_eq!(chain(d, diagonal), vec![diagonal]);
    }

    #[test]
    fn circles_and_points_chain_alone_or_not_at_all() {
        let mut b = B(Editor::new(Document::new()));
        let ctr = b.pt(0.0, 0.0);
        let circle = b.add(Geom::Circle {
            center: ctr,
            radius: 3.0,
        });
        let d = b.0.document();
        assert_eq!(chain(d, circle), vec![circle]);
        assert!(chain(d, ctr).is_empty());
        assert!(chain(d, EntityId::new(999)).is_empty());
    }
}
