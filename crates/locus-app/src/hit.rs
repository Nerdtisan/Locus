// SPDX-License-Identifier: AGPL-3.0-or-later
//! Hit-testing and selection logic (#67, SPEC §6), independent of the GUI.
//!
//! Everything here works in world millimetres on [`Scene`] items, so it is
//! unit-testable without a window and reusable by tools later. Selection is
//! UI state: it is never written to the document (SPEC §11).

use crate::scene::{Item, Shape};
use locus_doc::EntityId;
use locus_geom::{Aabb, Line, Vec2};
use std::collections::BTreeSet;

/// Distance from `p` to the drawn shape.
pub fn distance(shape: &Shape, p: Vec2) -> f64 {
    match shape {
        Shape::Point(q) => p.distance(*q),
        Shape::Line(l) => l.distance_to(p),
        Shape::Circle(c) => (p.distance(c.center) - c.radius).abs(),
        Shape::Arc(a) => p.distance(a.closest_point(p)),
        Shape::Cubic(c) => c.distance_to(p),
    }
}

/// The item under `p`, if any, within `tol` mm.
///
/// Point-priority picking: a point (endpoint, centre or free point) within
/// tolerance always beats a curve, even when the curve is nearer, so
/// endpoints stay selectable where curves meet. Ties go to the nearest, then
/// the lowest id (deterministic).
pub fn pick(items: &[Item], p: Vec2, tol: f64) -> Option<EntityId> {
    let mut best_point: Option<(f64, EntityId)> = None;
    let mut best_curve: Option<(f64, EntityId)> = None;
    let probe = Aabb::from_point(p);
    for item in items {
        // Cheap reject before the exact distance.
        if !bbox_near(&item.bbox, &probe, tol) {
            continue;
        }
        let d = distance(&item.shape, p);
        if d > tol {
            continue;
        }
        let slot = if matches!(item.shape, Shape::Point(_)) {
            &mut best_point
        } else {
            &mut best_curve
        };
        if slot.is_none_or(|(bd, bid)| d < bd || (d == bd && item.id < bid)) {
            *slot = Some((d, item.id));
        }
    }
    best_point.or(best_curve).map(|(_, id)| id)
}

/// The nearest curve (not a point) within `tol` mm of `p`: the target of the
/// modify tools (SPEC §6.2).
pub fn pick_curve(items: &[Item], p: Vec2, tol: f64) -> Option<EntityId> {
    let probe = Aabb::from_point(p);
    items
        .iter()
        .filter(|item| !matches!(item.shape, Shape::Point(_)) && bbox_near(&item.bbox, &probe, tol))
        .map(|item| (distance(&item.shape, p), item.id))
        .filter(|(d, _)| *d <= tol)
        .min_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)))
        .map(|(_, id)| id)
}

/// The nearest point entity within `tol` mm of `p`, with its position: the
/// snap target for drawing tools (#68).
pub fn pick_point(items: &[Item], p: Vec2, tol: f64) -> Option<(EntityId, Vec2)> {
    let mut best: Option<(f64, EntityId, Vec2)> = None;
    for item in items {
        let Shape::Point(q) = item.shape else {
            continue;
        };
        let d = p.distance(q);
        if d <= tol && best.is_none_or(|(bd, bid, _)| d < bd || (d == bd && item.id < bid)) {
            best = Some((d, item.id, q));
        }
    }
    best.map(|(_, id, q)| (id, q))
}

fn bbox_near(b: &Aabb, probe: &Aabb, tol: f64) -> bool {
    b.max.x + tol >= probe.min.x
        && b.min.x - tol <= probe.max.x
        && b.max.y + tol >= probe.min.y
        && b.min.y - tol <= probe.max.y
}

/// How a rubber-band box selects (Fusion/AutoCAD convention).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoxMode {
    /// Drag left to right: only items entirely inside.
    Window,
    /// Drag right to left: anything the box touches.
    Crossing,
}

impl BoxMode {
    /// Direction of the drag in screen space decides the mode.
    pub fn from_drag(start_x: f32, end_x: f32) -> Self {
        if end_x >= start_x {
            BoxMode::Window
        } else {
            BoxMode::Crossing
        }
    }
}

fn inside(rect: &Aabb, b: &Aabb) -> bool {
    b.min.x >= rect.min.x && b.max.x <= rect.max.x && b.min.y >= rect.min.y && b.max.y <= rect.max.y
}

fn overlaps(rect: &Aabb, b: &Aabb) -> bool {
    b.max.x >= rect.min.x && b.min.x <= rect.max.x && b.max.y >= rect.min.y && b.min.y <= rect.max.y
}

/// Does the shape's outline touch `rect`? (Entirely-inside counts.)
fn touches(shape: &Shape, bbox: &Aabb, rect: &Aabb) -> bool {
    if !overlaps(rect, bbox) {
        return false;
    }
    if inside(rect, bbox) {
        return true;
    }
    let corners = [
        Vec2::new(rect.min.x, rect.min.y),
        Vec2::new(rect.max.x, rect.min.y),
        Vec2::new(rect.max.x, rect.max.y),
        Vec2::new(rect.min.x, rect.max.y),
    ];
    let edges = [
        Line::new(corners[0], corners[1]),
        Line::new(corners[1], corners[2]),
        Line::new(corners[2], corners[3]),
        Line::new(corners[3], corners[0]),
    ];
    match shape {
        Shape::Point(p) => rect.contains(*p, 0.0),
        Shape::Line(l) => {
            rect.contains(l.a, 0.0)
                || rect.contains(l.b, 0.0)
                || edges.iter().any(|e| e.intersect_line(l).is_some())
        }
        Shape::Circle(c) => edges.iter().any(|e| !e.intersect_circle(c).is_empty()),
        Shape::Arc(a) => {
            rect.contains(a.start, 0.0)
                || rect.contains(a.end, 0.0)
                || edges.iter().any(|e| !e.intersect_arc(a).is_empty())
        }
        Shape::Cubic(c) => {
            let pl = c.flatten(0.05);
            pl.iter().any(|q| rect.contains(*q, 0.0))
                || pl.windows(2).any(|w| {
                    let seg = Line::new(w[0], w[1]);
                    edges.iter().any(|e| e.intersect_line(&seg).is_some())
                })
        }
    }
}

/// Items selected by a box in world space.
pub fn box_select(items: &[Item], rect: Aabb, mode: BoxMode) -> Vec<EntityId> {
    items
        .iter()
        .filter(|i| match mode {
            BoxMode::Window => inside(&rect, &i.bbox),
            BoxMode::Crossing => touches(&i.shape, &i.bbox, &rect),
        })
        .map(|i| i.id)
        .collect()
}

/// Modifier keys, as far as selection cares.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    /// Shift: add to the selection.
    pub add: bool,
    /// Ctrl / Cmd: toggle membership.
    pub toggle: bool,
}

/// The set of selected entities. UI state only.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Selection(BTreeSet<EntityId>);

impl Selection {
    pub fn contains(&self, id: EntityId) -> bool {
        self.0.contains(&id)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = EntityId> + '_ {
        self.0.iter().copied()
    }

    pub fn clear(&mut self) {
        self.0.clear();
    }

    pub fn set_all(&mut self, ids: impl IntoIterator<Item = EntityId>) {
        self.0 = ids.into_iter().collect();
    }

    /// Drop ids that no longer exist (after a command removed entities).
    pub fn retain(&mut self, keep: impl Fn(EntityId) -> bool) {
        self.0.retain(|id| keep(*id));
    }

    /// A click: plain replaces, shift adds, ctrl toggles. Clicking empty
    /// space clears unless a modifier is held.
    pub fn click(&mut self, hit: Option<EntityId>, mods: Mods) {
        match (hit, mods) {
            (None, m) if m.add || m.toggle => {}
            (None, _) => self.0.clear(),
            (Some(id), m) if m.toggle => {
                if !self.0.remove(&id) {
                    self.0.insert(id);
                }
            }
            (Some(id), m) if m.add => {
                self.0.insert(id);
            }
            (Some(id), _) => {
                self.0.clear();
                self.0.insert(id);
            }
        }
    }

    /// A box result: plain replaces, shift adds, ctrl toggles each hit.
    pub fn apply_box(&mut self, hits: &[EntityId], mods: Mods) {
        if mods.toggle {
            for id in hits {
                if !self.0.remove(id) {
                    self.0.insert(*id);
                }
            }
        } else {
            if !mods.add {
                self.0.clear();
            }
            self.0.extend(hits.iter().copied());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use locus_geom::{Arc, Circle};

    fn item(id: u64, shape: Shape) -> Item {
        Item {
            id: EntityId::new(id),
            bbox: shape.bbox(),
            shape,
            construction: false,
            constrained: false,
            owned_point: false,
            ends: None,
        }
    }

    fn v(x: f64, y: f64) -> Vec2 {
        Vec2::new(x, y)
    }

    fn line(id: u64, a: Vec2, b: Vec2) -> Item {
        item(id, Shape::Line(Line::new(a, b)))
    }

    #[test]
    fn pick_point_ignores_curves_and_takes_the_nearest_point() {
        let items = [
            line(1, v(0.0, 0.0), v(10.0, 0.0)),
            item(2, Shape::Point(v(5.0, 0.4))),
            item(3, Shape::Point(v(5.0, 0.2))),
        ];
        let hit = pick_point(&items, v(5.0, 0.0), 1.0);
        assert_eq!(hit, Some((EntityId::new(3), v(5.0, 0.2))));
        assert_eq!(pick_point(&items, v(5.0, 5.0), 1.0), None);
    }

    fn quarter_arc(id: u64) -> Item {
        // CCW from (10,0) to (0,10) about the origin.
        item(
            id,
            Shape::Arc(Arc::new(v(0.0, 0.0), v(10.0, 0.0), v(0.0, 10.0))),
        )
    }

    #[test]
    fn distances_to_each_shape() {
        assert!((distance(&Shape::Point(v(3.0, 4.0)), v(0.0, 0.0)) - 5.0).abs() < 1e-12);
        let l = Shape::Line(Line::new(v(0.0, 0.0), v(10.0, 0.0)));
        assert!((distance(&l, v(5.0, 3.0)) - 3.0).abs() < 1e-12);
        // Beyond the end it is the distance to the endpoint, not the line.
        assert!((distance(&l, v(13.0, 4.0)) - 5.0).abs() < 1e-12);
        let c = Shape::Circle(Circle::new(v(0.0, 0.0), 10.0));
        assert!((distance(&c, v(13.0, 0.0)) - 3.0).abs() < 1e-12);
        assert!((distance(&c, v(4.0, 0.0)) - 6.0).abs() < 1e-12);
    }

    #[test]
    fn arc_hit_respects_its_sweep() {
        let items = [quarter_arc(1)];
        let on_arc = v(10.0 * 0.5f64.sqrt(), 10.0 * 0.5f64.sqrt());
        assert_eq!(pick(&items, on_arc, 0.5), Some(EntityId::new(1)));
        // Same circle, but in the part of the circle the arc does not cover.
        assert_eq!(pick(&items, v(-10.0, 0.0), 0.5), None);
    }

    #[test]
    fn tolerance_is_inclusive_and_misses_beyond_it() {
        let items = [line(1, v(0.0, 0.0), v(10.0, 0.0))];
        assert!(pick(&items, v(5.0, 0.4), 0.5).is_some());
        assert!(pick(&items, v(5.0, 0.6), 0.5).is_none());
    }

    #[test]
    fn point_beats_a_nearer_curve() {
        // An endpoint 0.4 mm away vs the line itself 0.1 mm away.
        let items = [
            line(1, v(0.0, 0.0), v(10.0, 0.0)),
            item(2, Shape::Point(v(10.0, 0.0))),
        ];
        let at = v(9.6, 0.1);
        assert_eq!(pick(&items, at, 0.5), Some(EntityId::new(2)));
        // Out of the point's tolerance, the curve is picked.
        assert_eq!(pick(&items, v(5.0, 0.1), 0.5), Some(EntityId::new(1)));
    }

    #[test]
    fn nearest_wins_and_ties_are_deterministic() {
        let items = [
            line(5, v(0.0, 0.0), v(10.0, 0.0)),
            line(3, v(0.0, 0.2), v(10.0, 0.2)),
        ];
        assert_eq!(pick(&items, v(5.0, 0.15), 1.0), Some(EntityId::new(3)));
        let tie = [
            line(9, v(0.0, 1.0), v(10.0, 1.0)),
            line(4, v(0.0, -1.0), v(10.0, -1.0)),
        ];
        assert_eq!(pick(&tie, v(5.0, 0.0), 2.0), Some(EntityId::new(4)));
    }

    #[test]
    fn nothing_under_the_cursor() {
        assert_eq!(pick(&[], v(0.0, 0.0), 1.0), None);
        let items = [line(1, v(0.0, 0.0), v(1.0, 0.0))];
        assert_eq!(pick(&items, v(50.0, 50.0), 1.0), None);
    }

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Aabb {
        Aabb::from_points(v(x0, y0), v(x1, y1))
    }

    #[test]
    fn window_needs_full_containment_crossing_needs_a_touch() {
        let items = [
            line(1, v(1.0, 1.0), v(4.0, 4.0)),                      // inside
            line(2, v(3.0, 3.0), v(20.0, 3.0)),                     // crosses the edge
            line(3, v(30.0, 30.0), v(40.0, 40.0)),                  // far away
            item(4, Shape::Circle(Circle::new(v(5.0, 5.0), 2.0))),  // inside
            item(5, Shape::Circle(Circle::new(v(10.0, 5.0), 3.0))), // straddles
            item(6, Shape::Point(v(6.0, 6.0))),
        ];
        let r = rect(0.0, 0.0, 9.0, 9.0);
        let ids = |m| {
            let mut s: Vec<u64> = box_select(&items, r, m).iter().map(|i| i.get()).collect();
            s.sort_unstable();
            s
        };
        assert_eq!(ids(BoxMode::Window), vec![1, 4, 6]);
        assert_eq!(ids(BoxMode::Crossing), vec![1, 2, 4, 5, 6]);
    }

    #[test]
    fn crossing_ignores_curves_that_only_share_a_bbox() {
        // The circle's bbox overlaps the box, but the box sits inside the
        // circle without touching the outline.
        let items = [item(1, Shape::Circle(Circle::new(v(0.0, 0.0), 50.0)))];
        let r = rect(-1.0, -1.0, 1.0, 1.0);
        assert!(box_select(&items, r, BoxMode::Crossing).is_empty());
        // A diagonal line whose bbox covers the box but whose body misses it.
        let miss = [line(2, v(-10.0, 10.0), v(10.0, 30.0))];
        assert!(box_select(&miss, rect(-9.0, 14.0, -8.0, 30.0), BoxMode::Crossing).is_empty());
    }

    #[test]
    fn crossing_picks_up_arcs_by_outline() {
        let items = [quarter_arc(1)];
        // Box across the arc's middle.
        let r = rect(6.0, 6.0, 9.0, 9.0);
        assert_eq!(box_select(&items, r, BoxMode::Crossing).len(), 1);
        // Box on the uncovered side of the same circle.
        let r = rect(-11.0, -1.0, -9.0, 1.0);
        assert!(box_select(&items, r, BoxMode::Crossing).is_empty());
    }

    #[test]
    fn drag_direction_chooses_the_mode() {
        assert_eq!(BoxMode::from_drag(10.0, 50.0), BoxMode::Window);
        assert_eq!(BoxMode::from_drag(50.0, 10.0), BoxMode::Crossing);
    }

    fn id(n: u64) -> EntityId {
        EntityId::new(n)
    }

    const PLAIN: Mods = Mods {
        add: false,
        toggle: false,
    };
    const SHIFT: Mods = Mods {
        add: true,
        toggle: false,
    };
    const CTRL: Mods = Mods {
        add: false,
        toggle: true,
    };

    #[test]
    fn click_semantics() {
        let mut s = Selection::default();
        s.click(Some(id(1)), PLAIN);
        s.click(Some(id(2)), SHIFT);
        assert_eq!(s.iter().collect::<Vec<_>>(), vec![id(1), id(2)]);
        s.click(Some(id(1)), CTRL);
        assert_eq!(s.iter().collect::<Vec<_>>(), vec![id(2)]);
        s.click(Some(id(1)), CTRL);
        assert!(s.contains(id(1)) && s.contains(id(2)));
        // Empty space with a modifier keeps it; plain clears.
        s.click(None, SHIFT);
        s.click(None, CTRL);
        assert_eq!(s.len(), 2);
        s.click(None, PLAIN);
        assert!(s.is_empty());
        // Plain click on a member replaces the whole selection with it.
        s.set_all([id(1), id(2), id(3)]);
        s.click(Some(id(2)), PLAIN);
        assert_eq!(s.iter().collect::<Vec<_>>(), vec![id(2)]);
    }

    #[test]
    fn box_application_modes() {
        let mut s = Selection::default();
        s.set_all([id(1)]);
        s.apply_box(&[id(2), id(3)], SHIFT);
        assert_eq!(s.len(), 3);
        s.apply_box(&[id(3), id(4)], CTRL);
        assert_eq!(s.iter().collect::<Vec<_>>(), vec![id(1), id(2), id(4)]);
        s.apply_box(&[id(9)], PLAIN);
        assert_eq!(s.iter().collect::<Vec<_>>(), vec![id(9)]);
        s.apply_box(&[], PLAIN);
        assert!(s.is_empty());
    }

    #[test]
    fn retain_drops_vanished_entities() {
        let mut s = Selection::default();
        s.set_all([id(1), id(2), id(3)]);
        s.retain(|i| i != id(2));
        assert_eq!(s.iter().collect::<Vec<_>>(), vec![id(1), id(3)]);
    }
}
