// SPDX-License-Identifier: AGPL-3.0-or-later

//! The live offset (SPEC §6.2 offset, #164): a recipe (ADR 0011) whose
//! instances are the offset of a chain of curves, computed with the ADR 0010
//! engine and regenerated after every solve, so editing the chain updates the
//! offset.
//!
//! Unlike a mirror, an offset is **not solved**: how many lines and arcs it
//! has depends on the geometry (corners appear, notches close), so its
//! instances are written from the solved sources instead of taking part in
//! the solve. They stay out of the solver and can't carry constraints;
//! Explode turns them into ordinary geometry that can.
//!
//! The recipe's `pool[0]` is a flat list of every entity ID the offset ever
//! used, in output order; the first `active` exist. Regeneration reuses
//! them in order and allocates more only when the output grows (ADR 0003,
//! ADR 0011 ID policy).

use crate::command::{Patch, record};
use crate::document::Document;
use crate::entity::{Entity, Geom};
use crate::error::DocError;
use crate::ids::{EntityId, RecipeId};
use crate::recipe::{MAX_INSTANCE_ENTITIES, RecipeKind};
use core::f64::consts::TAU;
use locus_geom::{EXPORT_TOL, Piece, SOLVER_TOL, Vec2};
use locus_region::{Area, Edge, Join, Loop, Seg};
use std::collections::{BTreeMap, BTreeSet};

/// Miter limit for sharp offset corners: past it a corner is rounded.
pub const MITER_LIMIT: f64 = 4.0;

/// The distance an offset recipe uses now: its expression, evaluated with
/// the document's parameters, or the stored value.
#[must_use]
pub fn distance(doc: &Document, kind: &RecipeKind) -> Option<f64> {
    let RecipeKind::Offset { distance, expr, .. } = kind else {
        return None;
    };
    Some(
        expr.as_deref()
            .and_then(|e| crate::expr::evaluate(e, doc.parameters()).ok())
            .filter(|v| v.is_finite())
            .unwrap_or(*distance),
    )
}

/// A source curve with its end points (circles have none).
struct Item {
    id: EntityId,
    ends: Option<(EntityId, EntityId)>,
}

/// The sources in chain order, each with whether it is walked backwards,
/// and whether the chain is closed. One connected chain is required: every
/// shared end point joins exactly two sources.
pub(crate) fn chain(
    doc: &Document,
    sources: &[EntityId],
) -> Result<(Vec<(EntityId, bool)>, bool), DocError> {
    let bad = |m: &str| DocError::Invalid(format!("offset: {m}"));
    let mut items = Vec::new();
    for id in sources {
        let e = doc.entity(*id).ok_or(DocError::UnknownEntity(*id))?;
        let ends = match e.geom {
            Geom::Line { start, end }
            | Geom::Arc { start, end, .. }
            | Geom::Bezier { start, end, .. } => Some((start, end)),
            Geom::Circle { .. } => None,
            Geom::Point { .. } => return Err(bad("points can't be offset")),
        };
        items.push(Item { id: *id, ends });
    }
    if items.iter().any(|i| i.ends.is_none()) {
        return if items.len() == 1 {
            Ok((vec![(items[0].id, false)], true))
        } else {
            Err(bad("a circle can only be offset on its own"))
        };
    }
    let mut at: BTreeMap<EntityId, Vec<usize>> = BTreeMap::new();
    for (i, it) in items.iter().enumerate() {
        if let Some((a, b)) = it.ends {
            at.entry(a).or_default().push(i);
            at.entry(b).or_default().push(i);
        }
    }
    if at.values().any(|v| v.len() > 2) {
        return Err(bad("the curves branch; pick a single chain"));
    }
    // Open chains start at an end used once; closed ones anywhere.
    let start_pt = at.iter().find(|(_, v)| v.len() == 1).map(|(p, _)| *p);
    let closed = start_pt.is_none();
    let (mut cur, mut pt) = match start_pt {
        Some(p) => (at[&p][0], p),
        None => (0, items[0].ends.map_or(EntityId::new(0), |(a, _)| a)),
    };
    let mut used = BTreeSet::new();
    let mut out = Vec::new();
    loop {
        used.insert(cur);
        let Some((a, b)) = items[cur].ends else { break };
        let backwards = a != pt;
        out.push((items[cur].id, backwards));
        pt = if backwards { a } else { b };
        match at[&pt].iter().find(|i| !used.contains(*i)) {
            Some(&next) => cur = next,
            None => break,
        }
    }
    if out.len() != items.len() {
        return Err(bad("the curves are not one connected chain"));
    }
    Ok((out, closed))
}

/// A source curve as region pieces in travel order.
fn pieces(doc: &Document, id: EntityId, backwards: bool) -> Vec<Seg> {
    let mut out: Vec<Seg> = match doc.entity(id).map(|e| &e.geom) {
        Some(Geom::Line { .. }) => doc
            .line(id)
            .map(|l| vec![Seg::Line { a: l.a, b: l.b }])
            .unwrap_or_default(),
        Some(Geom::Arc { .. }) => doc
            .arc(id)
            .map(|a| {
                vec![Seg::Arc {
                    center: a.center,
                    radius: a.radius(),
                    start: a.start_angle(),
                    sweep: a.sweep(),
                }]
            })
            .unwrap_or_default(),
        Some(Geom::Circle { .. }) => doc
            .circle(id)
            .map(|c| {
                vec![Seg::Arc {
                    center: c.center,
                    radius: c.radius,
                    start: 0.0,
                    sweep: TAU,
                }]
            })
            .unwrap_or_default(),
        Some(Geom::Bezier { .. }) => doc
            .cubic(id)
            .map(|c| {
                c.to_pieces(EXPORT_TOL)
                    .into_iter()
                    .map(|p| match p {
                        Piece::Line(l) => Seg::Line { a: l.a, b: l.b },
                        // Biarc pieces are stored CCW: orient them along the
                        // curve (from the piece nearer the curve's start).
                        Piece::Arc(a) => Seg::Arc {
                            center: a.center,
                            radius: a.radius(),
                            start: a.start_angle(),
                            sweep: a.sweep(),
                        },
                    })
                    .collect()
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    if matches!(doc.entity(id).map(|e| &e.geom), Some(Geom::Bezier { .. })) {
        orient(&mut out);
    }
    if backwards {
        out.reverse();
        for s in &mut out {
            *s = s.reversed();
        }
    }
    out
}

/// Turns pieces end to end: each is reversed if that makes it start where
/// the previous one ended (biarc pieces come CCW, not along the curve).
fn orient(segs: &mut [Seg]) {
    for i in 0..segs.len() {
        let prev_end = if i == 0 {
            None
        } else {
            Some(segs[i - 1].end())
        };
        let next = segs.get(i + 1).copied();
        let s = segs[i];
        let flip = match (prev_end, next) {
            (Some(p), _) => s.end().distance(p) < s.start().distance(p),
            (None, Some(n)) => {
                let d = |q: Vec2| n.start().distance(q).min(n.end().distance(q));
                d(s.start()) < d(s.end())
            }
            (None, None) => false,
        };
        if flip {
            segs[i] = s.reversed();
        }
    }
}

/// One output entity, referring to others by index.
#[derive(Debug, Clone, PartialEq)]
enum Out {
    Point(Vec2),
    Line(usize, usize),
    /// CCW: centre, start, end.
    Arc(usize, usize, usize),
    Circle(usize, f64),
}

/// The offset of `sources` by `d` as output entities (points first per run).
fn generate(
    doc: &Document,
    sources: &[EntityId],
    d: f64,
    sharp: bool,
) -> Result<Vec<Out>, DocError> {
    let (order, closed) = chain(doc, sources)?;
    let mut edges: Vec<Edge> = Vec::new();
    for (id, back) in &order {
        for seg in pieces(doc, *id, *back) {
            edges.push(Edge {
                seg,
                source: id.get(),
            });
        }
    }
    let join = if sharp {
        Join::Miter { limit: MITER_LIMIT }
    } else {
        Join::Round
    };
    let runs: Vec<(Vec<Edge>, bool)> = if closed {
        let mut l = Loop { edges };
        if l.area() < 0.0 {
            l = l.reversed();
        }
        locus_region::offset(&Area { loops: vec![l] }, d, join)
            .loops
            .into_iter()
            .map(|l| (l.edges, true))
            .collect()
    } else {
        locus_region::offset_open(&edges, d, join)
            .into_iter()
            .map(|r| (r, false))
            .collect()
    };
    let mut out = Vec::new();
    for (run, loop_) in runs {
        emit(&run, loop_, &mut out);
    }
    if out.len() > MAX_INSTANCE_ENTITIES {
        return Err(DocError::InstanceLimit {
            requested: out.len(),
            limit: MAX_INSTANCE_ENTITIES,
        });
    }
    Ok(out)
}

fn emit(run: &[Edge], closed: bool, out: &mut Vec<Out>) {
    if run.is_empty() {
        return;
    }
    // A whole circle on its own.
    if let [
        Edge {
            seg:
                Seg::Arc {
                    center,
                    radius,
                    sweep,
                    ..
                },
            ..
        },
    ] = run
        && sweep.abs() >= TAU - 1e-12
    {
        out.push(Out::Point(*center));
        out.push(Out::Circle(out.len() - 1, *radius));
        return;
    }
    let base = out.len();
    let n = run.len();
    for e in run {
        out.push(Out::Point(e.seg.start()));
    }
    if !closed {
        out.push(Out::Point(run[n - 1].seg.end()));
    }
    let npts = out.len() - base;
    for (i, e) in run.iter().enumerate() {
        let (a, b) = (base + i, base + (i + 1) % npts);
        match e.seg {
            Seg::Line { .. } => out.push(Out::Line(a, b)),
            Seg::Arc { center, sweep, .. } => {
                out.push(Out::Point(center));
                let c = out.len() - 1;
                out.push(if sweep >= 0.0 {
                    Out::Arc(c, a, b)
                } else {
                    Out::Arc(c, b, a)
                });
            }
        }
    }
}

/// Rewrites recipe `rid`'s instances from its sources' current geometry,
/// recording each change with `record`. Reuses the recipe's pooled IDs,
/// allocating more only when the output grows. A chain that no longer works
/// (e.g. a source was trimmed apart) leaves no instances.
pub(crate) fn regenerate(
    doc: &mut Document,
    rid: RecipeId,
    patches: &mut Vec<Patch>,
) -> Result<(), DocError> {
    let Some(r) = doc.recipe(rid).cloned() else {
        return Ok(());
    };
    let RecipeKind::Offset { sharp, .. } = r.kind else {
        return Ok(());
    };
    let d = distance(doc, &r.kind).unwrap_or(0.0);
    let out = if d.abs() > SOLVER_TOL {
        generate(doc, &r.sources, d, sharp).unwrap_or_default()
    } else {
        Vec::new()
    };
    let layer = r
        .sources
        .iter()
        .find_map(|s| doc.entity(*s).map(|e| e.layer))
        .unwrap_or_else(Document::default_layer);
    let mut pool = r.pool.first().cloned().unwrap_or_default();
    while pool.len() < out.len() {
        pool.push(EntityId::new(doc.alloc_id()?));
    }
    let id = |i: usize| pool[i];
    for (i, o) in out.iter().enumerate() {
        let geom = match *o {
            Out::Point(pos) => Geom::Point { pos },
            Out::Line(a, b) => Geom::Line {
                start: id(a),
                end: id(b),
            },
            Out::Arc(c, a, b) => Geom::Arc {
                center: id(c),
                start: id(a),
                end: id(b),
            },
            Out::Circle(c, radius) => Geom::Circle {
                center: id(c),
                radius,
            },
        };
        let e = Entity {
            layer,
            construction: false,
            geom,
        };
        if doc.entity(pool[i]) != Some(&e) {
            let before = doc.entity(pool[i]).cloned();
            record(
                doc,
                patches,
                Patch::Entity {
                    id: pool[i],
                    before,
                    after: Some(e),
                },
            );
        }
    }
    for &gone in pool.iter().take(r.active).skip(out.len()) {
        if let Some(before) = doc.entity(gone).cloned() {
            record(
                doc,
                patches,
                Patch::Entity {
                    id: gone,
                    before: Some(before),
                    after: None,
                },
            );
        }
    }
    let mut next = r.clone();
    next.pool = vec![pool];
    next.active = out.len();
    if next != r {
        record(
            doc,
            patches,
            Patch::Recipe {
                id: rid,
                before: Some(r),
                after: Some(next),
            },
        );
    }
    Ok(())
}

/// The connected chain of curves through `id`: curves on the same layer that
/// share end points with it, one after another, stopping at branches.
/// Construction curves and recipe instances are left out. The offset tool
/// uses this to turn one click into a chain.
#[must_use]
pub fn chain_through(doc: &Document, id: EntityId) -> Vec<EntityId> {
    let Some(e0) = doc.entity(id) else {
        return Vec::new();
    };
    let driven = doc.driven_entities();
    let ok = |e: &Entity, eid: EntityId| {
        e.layer == e0.layer && !e.construction && !driven.contains_key(&eid)
    };
    let ends = |g: &Geom| match *g {
        Geom::Line { start, end }
        | Geom::Arc { start, end, .. }
        | Geom::Bezier { start, end, .. } => Some((start, end)),
        _ => None,
    };
    if !ok(e0, id) {
        return Vec::new();
    }
    let Some(first) = ends(&e0.geom) else {
        return if matches!(e0.geom, Geom::Circle { .. }) {
            vec![id]
        } else {
            Vec::new()
        };
    };
    // Curves meeting at each point.
    let mut at: BTreeMap<EntityId, Vec<EntityId>> = BTreeMap::new();
    for (cid, e) in doc.entities().iter() {
        if ok(e, cid)
            && let Some((a, b)) = ends(&e.geom)
        {
            at.entry(a).or_default().push(cid);
            at.entry(b).or_default().push(cid);
        }
    }
    let mut chain = BTreeSet::from([id]);
    for start in [first.0, first.1] {
        let mut pt = start;
        let mut cur = id;
        loop {
            let users = at.get(&pt).map_or(&[][..], Vec::as_slice);
            if users.len() != 2 {
                break;
            }
            let Some(&next) = users.iter().find(|u| **u != cur) else {
                break;
            };
            if !chain.insert(next) {
                break;
            }
            let Some((a, b)) = doc.entity(next).and_then(|e| ends(&e.geom)) else {
                break;
            };
            pt = if a == pt { b } else { a };
            cur = next;
        }
    }
    chain.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::Command;
    use crate::constraint::Constraint;
    use crate::solve::Sketcher;
    use crate::testutil::{add, line, pt};

    fn v(x: f64, y: f64) -> Vec2 {
        Vec2::new(x, y)
    }

    /// A 20×10 rectangle of four lines sharing corner points.
    fn rect() -> (Document, Vec<EntityId>, Vec<EntityId>) {
        let mut d = Document::new();
        let p = [
            pt(&mut d, 0.0, 0.0),
            pt(&mut d, 20.0, 0.0),
            pt(&mut d, 20.0, 10.0),
            pt(&mut d, 0.0, 10.0),
        ];
        let l = (0..4).map(|i| line(&mut d, p[i], p[(i + 1) % 4])).collect();
        (d, l, p.to_vec())
    }

    fn offset_kind(d: f64, sharp: bool) -> RecipeKind {
        RecipeKind::Offset {
            distance: d,
            sharp,
            expr: None,
        }
    }

    fn lines_of(doc: &Document, ids: impl Iterator<Item = EntityId>) -> Vec<locus_geom::Line> {
        ids.filter_map(|i| doc.line(i)).collect()
    }

    fn the_recipe(doc: &Document) -> (RecipeId, crate::recipe::Recipe) {
        doc.recipes()
            .iter()
            .next()
            .map(|(i, r)| (*i, r.clone()))
            .unwrap()
    }

    #[test]
    fn sharp_offset_of_a_rectangle_is_a_rectangle() {
        let (d, l, _) = rect();
        let mut sk = Sketcher::new(d);
        sk.apply(&Command::AddRecipe {
            kind: offset_kind(2.0, true),
            sources: l,
        })
        .unwrap();
        let doc = sk.document();
        let (_, r) = the_recipe(doc);
        assert_eq!(r.active, 8, "4 points + 4 lines");
        let ls = lines_of(doc, r.instances());
        assert_eq!(ls.len(), 4);
        let xs: Vec<f64> = ls.iter().flat_map(|l| [l.a.x, l.b.x]).collect();
        assert!(
            xs.iter()
                .all(|x| (x + 2.0).abs() < 1e-9 || (x - 22.0).abs() < 1e-9)
        );
        // Instances stay out of the solver: the DOF count only covers the
        // rectangle (4 points × 2).
        assert_eq!(sk.state().dof, 8);
    }

    #[test]
    fn round_inward_offset_and_editing_the_distance() {
        let (d, l, _) = rect();
        let mut sk = Sketcher::new(d);
        sk.apply(&Command::AddRecipe {
            kind: offset_kind(2.0, false),
            sources: l,
        })
        .unwrap();
        // Outward round: 4 lines, 4 arcs (each with a centre point).
        let (rid, r) = the_recipe(sk.document());
        let doc = sk.document();
        assert_eq!(r.instances().filter(|i| doc.arc(*i).is_some()).count(), 4);
        // Inward: sharp corners come back by themselves.
        sk.apply(&Command::EditRecipe {
            id: rid,
            kind: offset_kind(-1.0, false),
        })
        .unwrap();
        let (_, r2) = the_recipe(sk.document());
        let doc = sk.document();
        assert_eq!(r2.instances().filter(|i| doc.arc(*i).is_some()).count(), 0);
        assert_eq!(lines_of(doc, r2.instances()).len(), 4);
        // IDs are the recipe's own: the smaller output reused the first ones.
        assert_eq!(r2.pool[0][..r2.active], r.pool[0][..r2.active]);
        // A kind change is refused.
        assert!(
            sk.apply(&Command::EditRecipe {
                id: rid,
                kind: RecipeKind::Mirror {
                    axis: EntityId::new(2)
                },
            })
            .is_err()
        );
    }

    #[test]
    fn editing_the_source_updates_the_offset_in_one_undo_step() {
        let (d, l, p) = rect();
        let mut sk = Sketcher::new(d);
        sk.apply(&Command::AddRecipe {
            kind: offset_kind(1.0, true),
            sources: l,
        })
        .unwrap();
        let before: Vec<EntityId> = the_recipe(sk.document()).1.instances().collect();
        sk.apply(&Command::ModifyEntity {
            id: p[2],
            geom: Geom::Point { pos: v(30.0, 10.0) },
        })
        .unwrap();
        let (_, r) = the_recipe(sk.document());
        let after: Vec<EntityId> = r.instances().collect();
        assert_eq!(before, after, "same entities, new geometry");
        let max_x = lines_of(sk.document(), r.instances())
            .iter()
            .flat_map(|l| [l.a.x, l.b.x])
            .fold(f64::MIN, f64::max);
        assert!(max_x > 30.0);
        // One undo puts source and offset back.
        sk.undo().unwrap();
        let max_x = lines_of(sk.document(), the_recipe(sk.document()).1.instances())
            .iter()
            .flat_map(|l| [l.a.x, l.b.x])
            .fold(f64::MIN, f64::max);
        assert!((max_x - 21.0).abs() < 1e-9);
        // Undo the recipe itself: its instances go; redo brings the same IDs.
        sk.undo().unwrap();
        assert!(sk.document().recipes().is_empty());
        assert!(before.iter().all(|i| sk.document().entity(*i).is_none()));
        sk.redo().unwrap();
        assert_eq!(
            the_recipe(sk.document()).1.instances().collect::<Vec<_>>(),
            before
        );
    }

    #[test]
    fn a_drag_updates_the_offset() {
        let (d, l, p) = rect();
        let mut sk = Sketcher::new(d);
        sk.apply(&Command::AddRecipe {
            kind: offset_kind(1.0, true),
            sources: l,
        })
        .unwrap();
        sk.begin_drag(p[2]).unwrap();
        sk.drag_to(v(25.0, 15.0)).unwrap();
        sk.end_drag().unwrap();
        let (_, r) = the_recipe(sk.document());
        let max_y = lines_of(sk.document(), r.instances())
            .iter()
            .flat_map(|l| [l.a.y, l.b.y])
            .fold(f64::MIN, f64::max);
        assert!(max_y > 15.0);
        sk.undo().unwrap();
        let (_, r) = the_recipe(sk.document());
        let max_y = lines_of(sk.document(), r.instances())
            .iter()
            .flat_map(|l| [l.a.y, l.b.y])
            .fold(f64::MIN, f64::max);
        assert!((max_y - 11.0).abs() < 1e-9);
    }

    #[test]
    fn instances_refuse_constraints_until_exploded() {
        let (d, l, _) = rect();
        let mut sk = Sketcher::new(d);
        sk.apply(&Command::AddRecipe {
            kind: offset_kind(1.0, true),
            sources: l,
        })
        .unwrap();
        let (rid, r) = the_recipe(sk.document());
        let inst = lines_of(sk.document(), r.instances()).len();
        assert_eq!(inst, 4);
        let some_line = r
            .instances()
            .find(|i| sk.document().line(*i).is_some())
            .unwrap();
        let hz = Command::AddConstraint {
            constraint: Constraint::Horizontal { line: some_line },
        };
        assert!(sk.apply(&hz).is_err());
        sk.apply(&Command::ExplodeRecipe { id: rid }).unwrap();
        assert!(sk.document().recipes().is_empty());
        assert!(sk.document().line(some_line).is_some());
        // Ordinary geometry now: the constraint is fine (it already holds).
        sk.apply(&hz).unwrap();
    }

    #[test]
    fn open_chains_circles_and_bad_chains() {
        let mut d = Document::new();
        let p = [
            pt(&mut d, 0.0, 0.0),
            pt(&mut d, 10.0, 0.0),
            pt(&mut d, 10.0, 10.0),
        ];
        let a = line(&mut d, p[0], p[1]);
        let b = line(&mut d, p[1], p[2]);
        let c0 = pt(&mut d, 50.0, 50.0);
        let circ = add(
            &mut d,
            Geom::Circle {
                center: c0,
                radius: 5.0,
            },
        );
        let mut sk = Sketcher::new(d.clone());
        // Left of travel: above the first line, left of the second.
        sk.apply(&Command::AddRecipe {
            kind: offset_kind(1.0, true),
            sources: vec![a, b],
        })
        .unwrap();
        let (_, r) = the_recipe(sk.document());
        let ls = lines_of(sk.document(), r.instances());
        assert_eq!(ls.len(), 2);
        assert!(
            ls.iter()
                .any(|l| (l.a.y - 1.0).abs() < 1e-9 && (l.b.y - 1.0).abs() < 1e-9)
        );
        assert!(
            ls.iter()
                .any(|l| (l.a.x - 9.0).abs() < 1e-9 && (l.b.x - 9.0).abs() < 1e-9)
        );
        // A circle grows into one circle.
        let mut sk = Sketcher::new(d.clone());
        sk.apply(&Command::AddRecipe {
            kind: offset_kind(2.0, false),
            sources: vec![circ],
        })
        .unwrap();
        let (_, r) = the_recipe(sk.document());
        let c = r.instances().find_map(|i| sk.document().circle(i)).unwrap();
        assert!((c.radius - 7.0).abs() < 1e-12);
        // Not a chain: refused.
        let mut sk = Sketcher::new(d);
        assert!(
            sk.apply(&Command::AddRecipe {
                kind: offset_kind(1.0, true),
                sources: vec![a, circ],
            })
            .is_err()
        );
        assert!(
            sk.apply(&Command::AddRecipe {
                kind: offset_kind(0.0, true),
                sources: vec![a],
            })
            .is_err()
        );
    }

    #[test]
    fn the_distance_can_be_an_expression() {
        let (d, l, _) = rect();
        let mut sk = Sketcher::new(d);
        sk.apply(&Command::SetParameter {
            name: "kerf".to_owned(),
            expr: "0.5".to_owned(),
        })
        .unwrap();
        sk.apply(&Command::AddRecipe {
            kind: RecipeKind::Offset {
                distance: 1.0,
                sharp: true,
                expr: Some("kerf * 4".to_owned()),
            },
            sources: l,
        })
        .unwrap();
        let min_x = |sk: &Sketcher| {
            lines_of(sk.document(), the_recipe(sk.document()).1.instances())
                .iter()
                .flat_map(|l| [l.a.x, l.b.x])
                .fold(f64::MAX, f64::min)
        };
        assert!((min_x(&sk) + 2.0).abs() < 1e-9);
        sk.apply(&Command::SetParameter {
            name: "kerf".to_owned(),
            expr: "1".to_owned(),
        })
        .unwrap();
        assert!((min_x(&sk) + 4.0).abs() < 1e-9);
    }

    #[test]
    fn chain_through_follows_shared_ends() {
        let (mut d, l, p) = rect();
        // A spur off one corner: the chain stops at the branch.
        let q = pt(&mut d, 30.0, 0.0);
        let spur = line(&mut d, p[1], q);
        let c = chain_through(&d, l[2]);
        assert!(c.contains(&l[2]) && c.contains(&l[3]) && c.contains(&l[0]));
        assert!(!c.contains(&spur));
        let (d2, l2, _) = rect();
        let mut all = chain_through(&d2, l2[0]);
        all.sort_unstable();
        let mut want = l2.clone();
        want.sort_unstable();
        assert_eq!(all, want);
    }
}
