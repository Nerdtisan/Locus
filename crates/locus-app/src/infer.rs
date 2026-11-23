// SPDX-License-Identifier: AGPL-3.0-or-later
//! Auto-constraint inference while drawing (#69, SPEC §6.2), independent of
//! the GUI.
//!
//! Given what is under the cursor, propose the constraints the shape being
//! drawn should be created with, Fusion-style: coincident (snap onto an
//! existing point), point-on-curve, horizontal, vertical and tangent. The
//! result is a [`Pick`]: an adjusted position plus the hints that become
//! constraints in the same command as the shape (SPEC §11).

use crate::scene::{Item, Shape};
use crate::tools::{Host, LineHint, Pick, Tool};
use locus_doc::EntityId;
use locus_geom::Vec2;

/// A line within this angle of horizontal / vertical / tangent snaps to it.
pub const ANGLE_TOL_DEG: f64 = 4.0;

#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// The global auto-constrain toggle. Off still snaps onto points (that is
    /// sharing, not a constraint); everything else is suppressed. Holding the
    /// suppress modifier is the same as off for one click.
    pub enabled: bool,
    /// Snap distance in mm.
    pub tol: f64,
    /// Snap onto points and curves (Grid and snaps ▾ Snap to geometry,
    /// M5 design §2.4). Off, the cursor never lands on existing geometry.
    pub geometry: bool,
    /// Snap free picks to this grid step in mm (Snap to grid). Geometry
    /// wins where both apply, as in Fusion.
    pub grid: Option<f64>,
}

/// `p` rounded to the nearest multiple of `step` (mm) on each axis.
fn on_grid(p: Vec2, step: Option<f64>) -> Vec2 {
    match step.filter(|s| *s > 0.0 && s.is_finite()) {
        Some(s) => Vec2::new((p.x / s).round() * s, (p.y / s).round() * s),
        None => p,
    }
}

/// The start of the line being drawn.
#[derive(Clone, Copy, Debug)]
pub struct Anchor {
    pub pos: Vec2,
    /// The existing point entity it is, if it snapped onto one.
    pub point: Option<EntityId>,
}

/// What the cursor at `cursor` designates for `tool`.
///
/// `anchor` is the line's start when a line is being drawn; `curve_snap` says
/// whether the point being placed may snap onto a curve (the radius click of
/// a circle, say, may not).
pub fn infer(
    items: &[Item],
    opts: Options,
    tool: Tool,
    anchor: Option<Anchor>,
    curve_snap: bool,
    cursor: Vec2,
) -> Pick {
    if opts.geometry
        && let Some((id, pos)) = crate::hit::pick_point(items, cursor, opts.tol)
    {
        let mut pick = Pick::at_point(pos, id);
        if tool == Tool::ArcTangent {
            pick.host = host(items, id);
        }
        return pick;
    }
    // The sketch origin is a snap point for every drawing tool (#335).
    if opts.geometry && cursor.length() <= opts.tol {
        return Pick::at_origin();
    }
    let gridded = on_grid(cursor, opts.grid);
    let mut pick = Pick::free(gridded);
    if !opts.enabled {
        return pick;
    }
    if opts.geometry
        && tool == Tool::Line
        && let Some(a) = anchor
        && let Some((pos, arc)) = tangent(items, a, cursor)
    {
        pick.pos = pos;
        pick.line = Some(LineHint::Tangent(arc));
        return pick;
    }
    // Snaps to existing geometry (#357), each creating its constraint:
    // an intersection (on both curves), a line's midpoint, and from the
    // line being drawn, the foot of a perpendicular or a tangent point.
    if opts.geometry && curve_snap {
        if let Some((a, b, pos)) = intersection(items, cursor, opts.tol) {
            pick.pos = pos;
            pick.on_curve = Some(a);
            pick.on_curve2 = Some(b);
            return pick;
        }
        if let Some((line, pos)) = midpoint(items, cursor, opts.tol) {
            pick.pos = pos;
            pick.midpoint_of = Some(line);
            return pick;
        }
    }
    if opts.geometry
        && curve_snap
        && tool == Tool::Line
        && let Some(a) = anchor
    {
        if let Some((line, pos)) = perpendicular_foot(items, a.pos, cursor, opts.tol) {
            pick.pos = pos;
            pick.on_curve = Some(line);
            pick.line = Some(LineHint::Perpendicular(line));
            return pick;
        }
        if let Some((curve, pos)) = tangent_point(items, a.pos, cursor, opts.tol) {
            pick.pos = pos;
            pick.on_curve = Some(curve);
            pick.line = Some(LineHint::TangentTo(curve));
            return pick;
        }
    }
    if opts.geometry
        && curve_snap
        && let Some((curve, pos)) = nearest_curve(items, cursor, opts.tol)
    {
        pick.pos = pos;
        pick.on_curve = Some(curve);
        return pick;
    }
    // Inference along an existing line: its extension (#357).
    if opts.geometry
        && curve_snap
        && let Some((line, pos, from)) = extension(items, cursor, opts.tol)
    {
        pick.pos = pos;
        pick.on_curve = Some(line);
        pick.guide = Some((from, pos));
        return pick;
    }
    if tool == Tool::Line
        && let Some(a) = anchor
    {
        if let Some((pos, hint)) = axis(a.pos, gridded) {
            pick.pos = pos;
            pick.line = Some(hint);
        } else if opts.geometry
            && let Some((line, pos)) = parallel(items, a.pos, gridded)
        {
            // Parallel to an existing line (#357).
            pick.pos = pos;
            pick.line = Some(LineHint::Parallel(line));
        }
    }
    pick
}

/// The lines, circles and arcs among `items`, by id. Text outlines aren't
/// curves to snap onto (#473).
fn curves(items: &[Item]) -> impl Iterator<Item = (EntityId, Shape)> + '_ {
    items
        .iter()
        .filter(|i| !i.text)
        .filter(|i| matches!(i.shape, Shape::Line(_) | Shape::Circle(_) | Shape::Arc(_)))
        .map(|i| (i.id, i.shape))
}

/// The intersection of two curves nearest `p`, within `tol` (#357).
fn intersection(items: &[Item], p: Vec2, tol: f64) -> Option<(EntityId, EntityId, Vec2)> {
    // Only curves passing near the cursor can meet there.
    let near: Vec<(EntityId, Shape)> = curves(items)
        .filter(|(_, s)| s.bbox().contains(p, tol))
        .collect();
    let mut best: Option<(f64, EntityId, EntityId, Vec2)> = None;
    for (i, (ia, a)) in near.iter().enumerate() {
        for (ib, b) in near.iter().skip(i + 1) {
            if ia == ib {
                continue;
            }
            let pts: Vec<Vec2> = match (a, b) {
                (Shape::Line(x), Shape::Line(y)) => x.intersect_line(y).into_iter().collect(),
                (Shape::Line(x), Shape::Circle(c)) | (Shape::Circle(c), Shape::Line(x)) => {
                    x.intersect_circle(c)
                }
                (Shape::Line(x), Shape::Arc(c)) | (Shape::Arc(c), Shape::Line(x)) => {
                    x.intersect_arc(c)
                }
                (Shape::Circle(x), Shape::Circle(y)) => x.intersect_circle(y),
                (Shape::Circle(x), Shape::Arc(y)) | (Shape::Arc(y), Shape::Circle(x)) => {
                    x.intersect_arc(y)
                }
                (Shape::Arc(x), Shape::Arc(y)) => x.intersect_arc(y),
                _ => Vec::new(),
            };
            for q in pts {
                let d = q.distance(p);
                if d <= tol && best.is_none_or(|(bd, ..)| d < bd) {
                    best = Some((d, *ia, *ib, q));
                }
            }
        }
    }
    best.map(|(_, a, b, q)| (a, b, q))
}

/// A line's midpoint within `tol` of `p` (#357).
fn midpoint(items: &[Item], p: Vec2, tol: f64) -> Option<(EntityId, Vec2)> {
    curves(items)
        .filter_map(|(id, s)| match s {
            Shape::Line(l) => Some((id, l.eval(0.5))),
            _ => None,
        })
        .filter(|(_, m)| m.distance(p) <= tol)
        .min_by(|a, b| a.1.distance(p).total_cmp(&b.1.distance(p)))
}

/// The foot of the perpendicular from `from` onto a line, near `p`.
fn perpendicular_foot(items: &[Item], from: Vec2, p: Vec2, tol: f64) -> Option<(EntityId, Vec2)> {
    curves(items)
        .filter_map(|(id, s)| {
            let Shape::Line(l) = s else { return None };
            let u = (l.b - l.a).normalized(1e-12)?;
            let foot = l.a + u * (from - l.a).dot(u);
            // Not from a point on the line itself, and on the segment.
            let on = (foot - l.a).dot(u) >= -tol && (foot - l.a).dot(u) <= l.length() + tol;
            (from.distance(foot) > tol && on && foot.distance(p) <= tol).then_some((id, foot))
        })
        .min_by(|a, b| a.1.distance(p).total_cmp(&b.1.distance(p)))
}

/// Where a line from `from` touches a circle or arc tangentially, near `p`.
fn tangent_point(items: &[Item], from: Vec2, p: Vec2, tol: f64) -> Option<(EntityId, Vec2)> {
    let mut best: Option<(f64, EntityId, Vec2)> = None;
    for (id, s) in curves(items) {
        let (c, r, arc) = match s {
            Shape::Circle(c) => (c.center, c.radius, None),
            Shape::Arc(a) => (a.center, a.radius(), Some(a)),
            _ => continue,
        };
        let d = from.distance(c);
        if d <= r + tol {
            continue;
        }
        let Some(u) = (from - c).normalized(1e-12) else {
            continue;
        };
        let t = (r / d).acos();
        for q in [c + u.rotated(t) * r, c + u.rotated(-t) * r] {
            if arc.is_some_and(|a| !a.contains_point_angle(q)) {
                continue;
            }
            let dist = q.distance(p);
            if dist <= tol && best.is_none_or(|(bd, ..)| dist < bd) {
                best = Some((dist, id, q));
            }
        }
    }
    best.map(|(_, id, q)| (id, q))
}

/// A point on a line's extension beyond its ends near `p` (#357): the
/// point, and the end the dashed guide starts from.
fn extension(items: &[Item], p: Vec2, tol: f64) -> Option<(EntityId, Vec2, Vec2)> {
    curves(items)
        .filter_map(|(id, s)| {
            let Shape::Line(l) = s else { return None };
            let len = l.length();
            let u = (l.b - l.a).normalized(1e-12)?;
            let t = (p - l.a).dot(u);
            let q = l.a + u * t;
            let beyond = t < -tol || t > len + tol;
            let from = if t < 0.0 { l.a } else { l.b };
            (beyond && q.distance(p) <= tol).then_some((id, q, from))
        })
        .min_by(|a, b| a.1.distance(p).total_cmp(&b.1.distance(p)))
}

/// From `from` towards `to`, parallel to an existing line if close in
/// angle (#357): the point projected onto that direction.
fn parallel(items: &[Item], from: Vec2, to: Vec2) -> Option<(EntityId, Vec2)> {
    let d = to - from;
    if d.length() < 1e-9 {
        return None;
    }
    let tol = ANGLE_TOL_DEG.to_radians().sin();
    curves(items)
        .filter_map(|(id, s)| {
            let Shape::Line(l) = s else { return None };
            let u = (l.b - l.a).normalized(1e-12)?;
            let sin = (u.cross(d) / d.length()).abs();
            (sin < tol).then(|| (sin, id, from + u * d.dot(u)))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, id, q)| (id, q))
}

/// The curve nearest `p` within `tol`, and the closest point on it. Lines and
/// arcs and circles; points are handled before this.
fn nearest_curve(items: &[Item], p: Vec2, tol: f64) -> Option<(EntityId, Vec2)> {
    let mut best: Option<(f64, EntityId, Vec2)> = None;
    for item in items.iter().filter(|i| !i.text) {
        let q = match item.shape {
            Shape::Point(_) => continue,
            Shape::Line(l) => l.closest_point(p),
            Shape::Arc(a) => a.closest_point(p),
            Shape::Cubic(c) => c.closest_point(p),
            Shape::Circle(c) => {
                let Some(u) = (p - c.center).normalized(1e-12) else {
                    continue;
                };
                c.center + u * c.radius
            }
        };
        let d = p.distance(q);
        if d <= tol && best.is_none_or(|(bd, bid, _)| d < bd || (d == bd && item.id < bid)) {
            best = Some((d, item.id, q));
        }
    }
    best.map(|(_, id, q)| (id, q))
}

/// Horizontal or vertical from `from` to `to`, if close enough in angle.
fn axis(from: Vec2, to: Vec2) -> Option<(Vec2, LineHint)> {
    let d = to - from;
    let len = d.length();
    if len < 1e-9 {
        return None;
    }
    let tol = ANGLE_TOL_DEG.to_radians().sin();
    if (d.y / len).abs() < tol {
        Some((Vec2::new(to.x, from.y), LineHint::Horizontal))
    } else if (d.x / len).abs() < tol {
        Some((Vec2::new(from.x, to.y), LineHint::Vertical))
    } else {
        None
    }
}

/// The line, arc or Bézier that ends at `point`, and the direction it
/// continues in beyond that end (for a tangent arc, #156).
fn host(items: &[Item], point: EntityId) -> Option<Host> {
    items.iter().find_map(|item| {
        let (s, e) = item.ends?;
        if point != s && point != e {
            return None;
        }
        let at_end = point == e;
        let forward = match item.shape {
            Shape::Line(l) => l.b - l.a,
            Shape::Arc(a) => a.tangent(if at_end { 1.0 } else { 0.0 })?,
            Shape::Cubic(c) => {
                if at_end {
                    c.end_tangent()?
                } else {
                    c.start_tangent()?
                }
            }
            _ => return None,
        };
        // Line and Bézier-start tangents point along the curve's travel; the
        // continuation beyond the start is the opposite way.
        let dir = match item.shape {
            Shape::Cubic(c) if !at_end => -c.start_tangent()?,
            _ if at_end => forward,
            _ => -forward,
        };
        Some(Host {
            curve: item.id,
            dir: dir.normalized(1e-12)?,
        })
    })
}

/// A line leaving an arc's end point along the arc's tangent (either way).
fn tangent(items: &[Item], anchor: Anchor, to: Vec2) -> Option<(Vec2, EntityId)> {
    let point = anchor.point?;
    let d = to - anchor.pos;
    let len = d.length();
    if len < 1e-9 {
        return None;
    }
    let tol = ANGLE_TOL_DEG.to_radians().sin();
    items.iter().find_map(|item| {
        let Shape::Arc(arc) = item.shape else {
            return None;
        };
        let (s, e) = item.ends?;
        if point != s && point != e {
            return None;
        }
        let t = (anchor.pos - arc.center).perp().normalized(1e-12)?;
        if (d.cross(t) / len).abs() >= tol {
            return None;
        }
        // Project onto the tangent line, keeping the side the cursor is on.
        Some((anchor.pos + t * d.dot(t), item.id))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use locus_geom::{Arc, Circle, Line};

    fn v(x: f64, y: f64) -> Vec2 {
        Vec2::new(x, y)
    }

    fn item(id: u64, shape: Shape) -> Item {
        Item {
            id: EntityId::new(id),
            bbox: shape.bbox(),
            shape,
            construction: false,
            constrained: false,
            owned_point: false,
            derived_point: false,
            ends: None,
            text: false,
        }
    }

    const ON: Options = Options {
        enabled: true,
        tol: 1.0,
        geometry: true,
        grid: None,
    };

    /// #335: near (0, 0) the pick is the origin; an existing point there
    /// wins (it is the origin once fixed); Snap to geometry off ignores it.
    #[test]
    fn the_origin_is_a_snap_point() {
        let p = infer(&[], ON, Tool::Line, None, true, v(0.4, -0.3));
        assert!(p.origin && p.pos == v(0.0, 0.0) && p.point.is_none());
        let far = infer(&[], ON, Tool::Line, None, true, v(3.0, 0.0));
        assert!(!far.origin);
        let items = [item(9, Shape::Point(v(0.0, 0.0)))];
        let p = infer(&items, ON, Tool::Line, None, true, v(0.2, 0.2));
        assert_eq!(p.point, Some(EntityId::new(9)));
        assert!(!p.origin);
        let off = Options {
            geometry: false,
            ..ON
        };
        assert!(!infer(&[], off, Tool::Line, None, true, v(0.2, 0.2)).origin);
    }

    fn line_item(id: u64, a: Vec2, b: Vec2) -> Item {
        item(id, Shape::Line(Line::new(a, b)))
    }

    /// Regression (#473): a text's outline and path guide draw as line
    /// segments carrying the text's id. Snapping onto them (on, at a
    /// crossing, a midpoint or an extension) designated "a point on the
    /// text", which the Text tool's click couldn't place. They never snap.
    #[test]
    fn text_outlines_are_not_snapped_onto() {
        let text = |a, b| Item {
            text: true,
            ..line_item(7, a, b)
        };
        let items = [
            text(v(-10.0, 0.0), v(10.0, 0.0)),
            text(v(3.0, -10.0), v(3.0, 10.0)),
        ];
        for at in [v(3.4, 0.3), v(-5.0, 0.4), v(0.3, 0.2), v(25.0, 0.6)] {
            let p = infer(&items, ON, Tool::Text, None, true, at);
            assert_eq!(
                (p.on_curve, p.on_curve2, p.midpoint_of),
                (None, None, None),
                "{at:?}"
            );
            assert_eq!(p.guide, None);
        }
    }

    /// #357: each snap to existing geometry, and what it designates.
    #[test]
    fn snaps_to_existing_geometry() {
        // Two crossing lines: the crossing.
        let items = [
            line_item(1, v(-10.0, 0.0), v(10.0, 0.0)),
            line_item(2, v(3.0, -10.0), v(3.0, 10.0)),
        ];
        let p = infer(&items, ON, Tool::Point, None, true, v(3.4, 0.3));
        assert_eq!(
            (p.on_curve, p.on_curve2),
            (Some(EntityId::new(1)), Some(EntityId::new(2)))
        );
        assert!(p.pos.distance(v(3.0, 0.0)) < 1e-9);
        // A line's midpoint.
        let items = [line_item(1, v(0.0, 0.0), v(20.0, 0.0))];
        let p = infer(&items, ON, Tool::Point, None, true, v(10.3, 0.4));
        assert_eq!(p.midpoint_of, Some(EntityId::new(1)));
        assert!(p.pos.distance(v(10.0, 0.0)) < 1e-9);
        // Perpendicular foot from the line being drawn.
        let items = [line_item(1, v(0.0, 0.0), v(20.0, 20.0))];
        let p = infer(&items, ON, Tool::Line, anchor(0.0, 10.0), true, v(5.4, 4.6));
        assert_eq!(p.line, Some(LineHint::Perpendicular(EntityId::new(1))));
        assert!(p.pos.distance(v(5.0, 5.0)) < 1e-9);
        // Tangent point on a circle.
        let items = [item(1, Shape::Circle(Circle::new(v(0.0, 0.0), 5.0)))];
        let t = (5.0f64 / 10.0).acos();
        let touch = v(0.0, 0.0) + v(1.0, 0.0).rotated(t) * 5.0;
        let p = infer(
            &items,
            ON,
            Tool::Line,
            anchor(10.0, 0.0),
            true,
            touch + v(0.3, 0.2),
        );
        assert_eq!(p.line, Some(LineHint::TangentTo(EntityId::new(1))));
        assert!(p.pos.distance(touch) < 1e-9);
        // Along a line's extension, with a guide from its end.
        let items = [line_item(1, v(0.0, 0.0), v(10.0, 0.0))];
        let p = infer(&items, ON, Tool::Point, None, true, v(25.0, 0.6));
        assert_eq!(p.on_curve, Some(EntityId::new(1)));
        assert_eq!(p.guide, Some((v(10.0, 0.0), v(25.0, 0.0))));
        // Parallel to an existing diagonal.
        let items = [line_item(1, v(0.0, 0.0), v(10.0, 10.0))];
        let p = infer(
            &items,
            ON,
            Tool::Line,
            anchor(20.0, 0.0),
            true,
            v(30.2, 9.9),
        );
        assert_eq!(p.line, Some(LineHint::Parallel(EntityId::new(1))));
        let d = p.pos - v(20.0, 0.0);
        assert!((d.x - d.y).abs() < 1e-9);
    }

    /// Snap to grid rounds free picks to the step; geometry still wins,
    /// and Snap to geometry off ignores points (#328, design §2.4).
    #[test]
    fn grid_and_geometry_snaps() {
        let grid = Options {
            grid: Some(5.0),
            ..ON
        };
        let p = infer(&[], grid, Tool::Point, None, true, v(7.4, -3.0));
        assert_eq!(p.pos, v(5.0, -5.0));
        let items = [item(7, Shape::Point(v(7.0, 0.0)))];
        let p = infer(&items, grid, Tool::Point, None, true, v(7.4, 0.3));
        assert_eq!(p.point, Some(EntityId::new(7)));
        let no_geo = Options {
            geometry: false,
            ..ON
        };
        let p = infer(&items, no_geo, Tool::Point, None, true, v(7.4, 0.3));
        assert_eq!(p.point, None);
        assert_eq!(p.pos, v(7.4, 0.3));
    }

    fn anchor(x: f64, y: f64) -> Option<Anchor> {
        Some(Anchor {
            pos: v(x, y),
            point: None,
        })
    }

    #[test]
    fn near_horizontal_and_vertical_snap_and_say_so() {
        let p = infer(&[], ON, Tool::Line, anchor(0.0, 0.0), true, v(10.0, 0.4));
        assert_eq!(p.line, Some(LineHint::Horizontal));
        assert_eq!(p.pos, v(10.0, 0.0));
        let p = infer(&[], ON, Tool::Line, anchor(0.0, 0.0), true, v(-0.3, 10.0));
        assert_eq!(p.line, Some(LineHint::Vertical));
        assert_eq!(p.pos, v(0.0, 10.0));
    }

    #[test]
    fn a_clearly_diagonal_line_gets_no_hint() {
        let p = infer(&[], ON, Tool::Line, anchor(0.0, 0.0), true, v(10.0, 5.0));
        assert_eq!(p.line, None);
        assert_eq!(p.pos, v(10.0, 5.0));
    }

    #[test]
    fn the_toggle_suppresses_everything_but_point_sharing() {
        let off = Options {
            enabled: false,
            ..ON
        };
        let items = [item(7, Shape::Point(v(10.0, 0.2)))];
        let p = infer(
            &items,
            off,
            Tool::Line,
            anchor(0.0, 0.0),
            true,
            v(10.0, 0.4),
        );
        assert_eq!(p.point, Some(EntityId::new(7)));
        let p = infer(&[], off, Tool::Line, anchor(0.0, 0.0), true, v(10.0, 0.4));
        assert_eq!((p.line, p.on_curve, p.pos), (None, None, v(10.0, 0.4)));
    }

    #[test]
    fn existing_points_beat_curves_and_axis_hints() {
        let items = [
            item(1, Shape::Line(Line::new(v(0.0, 0.0), v(20.0, 0.0)))),
            item(2, Shape::Point(v(10.0, 0.3))),
        ];
        let p = infer(&items, ON, Tool::Line, anchor(0.0, 5.0), true, v(10.0, 0.0));
        assert_eq!(p.point, Some(EntityId::new(2)));
        assert_eq!(p.pos, v(10.0, 0.3));
        assert_eq!((p.on_curve, p.line), (None, None));
    }

    #[test]
    fn a_point_near_a_curve_lands_on_it() {
        let items = [
            item(1, Shape::Line(Line::new(v(0.0, 0.0), v(20.0, 0.0)))),
            item(3, Shape::Circle(Circle::new(v(0.0, 30.0), 5.0))),
        ];
        let p = infer(&items, ON, Tool::Point, None, true, v(8.0, 0.6));
        assert_eq!((p.on_curve, p.pos), (Some(EntityId::new(1)), v(8.0, 0.0)));
        let p = infer(&items, ON, Tool::Point, None, true, v(0.0, 35.4));
        assert_eq!(p.on_curve, Some(EntityId::new(3)));
        assert!((p.pos.distance(v(0.0, 30.0)) - 5.0).abs() < 1e-9);
        // …unless the tool says this click may not.
        let p = infer(&items, ON, Tool::Point, None, false, v(8.0, 0.6));
        assert_eq!(p.on_curve, None);
    }

    #[test]
    fn a_line_from_an_arc_end_snaps_to_the_tangent() {
        // Quarter arc about the origin, end point (0, 10) with id 12.
        let mut arc = item(
            10,
            Shape::Arc(Arc::new(v(0.0, 0.0), v(10.0, 0.0), v(0.0, 10.0))),
        );
        arc.ends = Some((EntityId::new(11), EntityId::new(12)));
        let a = Some(Anchor {
            pos: v(0.0, 10.0),
            point: Some(EntityId::new(12)),
        });
        // The tangent at (0, 10) is horizontal; leave to the left, a bit off.
        let items = [arc];
        let p = infer(&items, ON, Tool::Line, a, false, v(-20.0, 10.8));
        assert_eq!(p.line, Some(LineHint::Tangent(EntityId::new(10))));
        assert!((p.pos.y - 10.0).abs() < 1e-9 && (p.pos.x + 20.0).abs() < 0.1);
        // Not tangent: no hint from the arc.
        let p = infer(&items, ON, Tool::Line, a, false, v(-20.0, 25.0));
        assert_eq!(p.line, None);
        // A line from some other point is never tangent to it.
        let other = Some(Anchor {
            pos: v(0.0, 10.0),
            point: Some(EntityId::new(99)),
        });
        let p = infer(&items, ON, Tool::Line, other, false, v(-20.0, 10.8));
        assert_ne!(p.line, Some(LineHint::Tangent(EntityId::new(10))));
    }

    #[test]
    fn only_lines_get_line_hints() {
        let p = infer(&[], ON, Tool::Circle, anchor(0.0, 0.0), false, v(10.0, 0.2));
        assert_eq!(p.line, None);
    }
}
