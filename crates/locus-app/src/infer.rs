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
    if let Some((id, pos)) = crate::hit::pick_point(items, cursor, opts.tol) {
        let mut pick = Pick::at_point(pos, id);
        if tool == Tool::ArcTangent {
            pick.host = host(items, id);
        }
        return pick;
    }
    let mut pick = Pick::free(cursor);
    if !opts.enabled {
        return pick;
    }
    if tool == Tool::Line
        && let Some(a) = anchor
        && let Some((pos, arc)) = tangent(items, a, cursor)
    {
        pick.pos = pos;
        pick.line = Some(LineHint::Tangent(arc));
        return pick;
    }
    if curve_snap && let Some((curve, pos)) = nearest_curve(items, cursor, opts.tol) {
        pick.pos = pos;
        pick.on_curve = Some(curve);
        return pick;
    }
    if tool == Tool::Line
        && let Some(a) = anchor
        && let Some((pos, hint)) = axis(a.pos, cursor)
    {
        pick.pos = pos;
        pick.line = Some(hint);
    }
    pick
}

/// The curve nearest `p` within `tol`, and the closest point on it. Lines and
/// arcs and circles; points are handled before this.
fn nearest_curve(items: &[Item], p: Vec2, tol: f64) -> Option<(EntityId, Vec2)> {
    let mut best: Option<(f64, EntityId, Vec2)> = None;
    for item in items {
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
            ends: None,
        }
    }

    const ON: Options = Options {
        enabled: true,
        tol: 1.0,
    };

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
