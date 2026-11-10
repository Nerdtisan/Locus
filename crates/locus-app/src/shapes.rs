// SPDX-License-Identifier: AGPL-3.0-or-later
//! The geometry behind the sketch tools (#155–#158, SPEC §6.1): pure maths on
//! plain vectors, independent of the GUI and the document. The tools in
//! [`crate::tools`] turn these results into one command each.

use locus_geom::Vec2;

/// Shorter than this is "the same place".
const MIN: f64 = 1e-6;

/// Fewest and most sides of a polygon (#157).
pub const POLYGON_SIDES: std::ops::RangeInclusive<usize> = 3..=64;

/// Corners, counter-clockwise from `c0`, of the axis-aligned rectangle with
/// opposite corners `c0` and `c1`.
pub fn rect_corners(c0: Vec2, c1: Vec2) -> [Vec2; 4] {
    [c0, Vec2::new(c1.x, c0.y), c1, Vec2::new(c0.x, c1.y)]
}

/// The rectangle with centre `c` and one corner `corner`, corners ordered
/// counter-clockwise starting from the corner opposite `corner`.
pub fn rect_corners_centred(c: Vec2, corner: Vec2) -> [Vec2; 4] {
    rect_corners(c * 2.0 - corner, corner)
}

/// The rotated rectangle whose first edge is `a → b` and whose far edge
/// passes through `height_pt`. Corners: `a`, `b`, then the two on the far
/// side. `None` when the edge or the height is zero.
pub fn rect_corners_rotated(a: Vec2, b: Vec2, height_pt: Vec2) -> Option<[Vec2; 4]> {
    let u = (b - a).normalized(MIN)?;
    let n = u.perp();
    let h = (height_pt - a).dot(n);
    (h.abs() > MIN).then(|| [a, b, b + n * h, a + n * h])
}

/// Vertices of a regular polygon and the radius of the construction circle
/// about `centre` that it is inscribed in (`false`) or circumscribed about
/// (`true`). `towards` sets the rotation: it is a vertex when inscribed, the
/// middle of a side when circumscribed. `None` for a zero radius or `sides`
/// outside [`POLYGON_SIDES`].
pub fn polygon(
    centre: Vec2,
    towards: Vec2,
    sides: usize,
    circumscribed: bool,
) -> Option<(Vec<Vec2>, f64)> {
    if !POLYGON_SIDES.contains(&sides) {
        return None;
    }
    let d = towards - centre;
    let r = d.length();
    if r < MIN {
        return None;
    }
    let step = core::f64::consts::TAU / sides as f64;
    // The circle's radius is what was dragged; the vertices sit on it, or
    // further out when it is the inscribed circle of the sides.
    let (vertex_r, first) = if circumscribed {
        (r / (step / 2.0).cos(), d.angle() - step / 2.0)
    } else {
        (r, d.angle())
    };
    let pts = (0..sides)
        .map(|k| centre + Vec2::from_angle(first + step * k as f64) * vertex_r)
        .collect();
    Some((pts, r))
}

/// A slot (#158): two end circles of radius `half_width` about `a` and `b`
/// joined by two tangent lines.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Slot {
    pub half_width: f64,
    /// Tangent points `[a + n·w, b + n·w, b − n·w, a − n·w]`, `n` being the
    /// left normal of `a → b`. Lines run `[0]→[1]` and `[2]→[3]`; the arc about
    /// `b` runs counter-clockwise `[2]→[1]`, the one about `a` `[0]→[3]`.
    pub tangent_points: [Vec2; 4],
}

/// The slot between centres `a` and `b` whose half width is the distance of
/// `side_pt` from the axis. `None` when the centres coincide or the width is 0.
pub fn slot(a: Vec2, b: Vec2, side_pt: Vec2) -> Option<Slot> {
    let u = (b - a).normalized(MIN)?;
    let n = u.perp();
    let w = (side_pt - a).dot(n).abs();
    (w > MIN).then(|| Slot {
        half_width: w,
        tangent_points: [a + n * w, b + n * w, b - n * w, a - n * w],
    })
}

/// The circle's centre and whether the arc from `start`, leaving along the
/// unit direction `dir`, to `end` runs counter-clockwise (#156). `None` when
/// `end` lies on the tangent line (no arc, a straight line).
pub fn tangent_arc(start: Vec2, dir: Vec2, end: Vec2) -> Option<(Vec2, bool)> {
    let n = dir.perp();
    let w = end - start;
    let side = w.dot(n);
    if side.abs() < MIN * w.length().max(1.0) {
        return None;
    }
    let r = w.length_sq() / (2.0 * side);
    let centre = start + n * r;
    (centre.is_finite() && r.abs() < 1e6).then_some((centre, r > 0.0))
}

/// Centre and radius of the circle with diameter `p`–`q`.
pub fn circle_by_diameter(p: Vec2, q: Vec2) -> Option<(Vec2, f64)> {
    let r = p.distance(q) / 2.0;
    (r > MIN).then(|| (p.lerp(q, 0.5), r))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use core::f64::consts::{FRAC_PI_2, TAU};

    fn v(x: f64, y: f64) -> Vec2 {
        Vec2::new(x, y)
    }

    #[test]
    fn rectangles_have_the_right_corners() {
        let r = rect_corners(v(0.0, 0.0), v(4.0, 3.0));
        assert_eq!(r, [v(0.0, 0.0), v(4.0, 0.0), v(4.0, 3.0), v(0.0, 3.0)]);
        let c = rect_corners_centred(v(5.0, 5.0), v(7.0, 8.0));
        assert_eq!(c[0], v(3.0, 2.0));
        assert_eq!(c[2], v(7.0, 8.0));
    }

    #[test]
    fn rotated_rectangle_is_square_on_either_side() {
        for h in [3.0, -3.0] {
            let r =
                rect_corners_rotated(v(0.0, 0.0), v(10.0, 10.0), v(0.0, 0.0) + v(-h, h)).unwrap();
            let e = |i: usize, j: usize| r[j] - r[i];
            assert!(e(0, 1).dot(e(1, 2)).abs() < 1e-9, "right angle");
            assert!((e(0, 1) - (r[2] - r[3])).length() < 1e-9, "parallel, equal");
            assert!((e(1, 2).length() - h.abs() * 2f64.sqrt()).abs() < 1e-9);
        }
        assert!(rect_corners_rotated(v(0.0, 0.0), v(0.0, 0.0), v(1.0, 1.0)).is_none());
        assert!(rect_corners_rotated(v(0.0, 0.0), v(5.0, 0.0), v(2.0, 0.0)).is_none());
    }

    #[test]
    fn inscribed_polygon_is_regular() {
        let (pts, r) = polygon(v(1.0, 2.0), v(6.0, 2.0), 6, false).unwrap();
        assert!((r - 5.0).abs() < 1e-9);
        assert_eq!(pts.len(), 6);
        assert!(pts[0].approx_eq(v(6.0, 2.0), 1e-9));
        for i in 0..6 {
            assert!((pts[i].distance(v(1.0, 2.0)) - 5.0).abs() < 1e-9);
            assert!((pts[i].distance(pts[(i + 1) % 6]) - 5.0).abs() < 1e-9);
        }
    }

    #[test]
    fn circumscribed_polygon_has_the_circle_as_its_incircle() {
        let (pts, r) = polygon(v(0.0, 0.0), v(0.0, 4.0), 5, true).unwrap();
        assert!((r - 4.0).abs() < 1e-9);
        for i in 0..5 {
            let (a, b) = (pts[i], pts[(i + 1) % 5]);
            let dist = (a.cross(b) / a.distance(b)).abs();
            assert!(
                (dist - 4.0).abs() < 1e-9,
                "side {i} is {dist} from the centre"
            );
        }
        // The dragged point is the middle of a side.
        let mid = pts[0].lerp(pts[1], 0.5);
        assert!(mid.approx_eq(v(0.0, 4.0), 1e-9) || pts.iter().any(|p| p.lerp(mid, 0.0) == *p));
    }

    #[test]
    fn polygon_sides_are_limited() {
        assert!(polygon(v(0.0, 0.0), v(1.0, 0.0), 2, false).is_none());
        assert!(polygon(v(0.0, 0.0), v(1.0, 0.0), 65, false).is_none());
        assert!(polygon(v(0.0, 0.0), v(1.0, 0.0), 64, true).is_some());
        assert!(polygon(v(0.0, 0.0), v(0.0, 0.0), 5, false).is_none());
    }

    #[test]
    fn slot_points_are_tangent_and_ordered() {
        let s = slot(v(0.0, 0.0), v(10.0, 0.0), v(3.0, -2.0)).unwrap();
        assert!((s.half_width - 2.0).abs() < 1e-9);
        let [p0, p1, p2, p3] = s.tangent_points;
        assert_eq!(
            (p0, p1, p2, p3),
            (v(0.0, 2.0), v(10.0, 2.0), v(10.0, -2.0), v(0.0, -2.0))
        );
        assert!(slot(v(1.0, 1.0), v(1.0, 1.0), v(2.0, 2.0)).is_none());
        assert!(slot(v(0.0, 0.0), v(5.0, 0.0), v(2.0, 0.0)).is_none());
    }

    #[test]
    fn tangent_arc_leaves_along_the_direction_and_picks_the_side() {
        // Leaving along +x towards a point above: a left turn, CCW about (0, 5).
        let (c, ccw) = tangent_arc(v(0.0, 0.0), v(1.0, 0.0), v(5.0, 5.0)).unwrap();
        assert!(ccw && c.approx_eq(v(0.0, 5.0), 1e-9));
        assert!((c.distance(v(5.0, 5.0)) - 5.0).abs() < 1e-9);
        let (c, ccw) = tangent_arc(v(0.0, 0.0), v(1.0, 0.0), v(5.0, -5.0)).unwrap();
        assert!(!ccw && c.approx_eq(v(0.0, -5.0), 1e-9));
        // Straight ahead is no arc.
        assert!(tangent_arc(v(0.0, 0.0), v(1.0, 0.0), v(5.0, 0.0)).is_none());
        // Behind the start works too (a U-turn).
        assert!(tangent_arc(v(0.0, 0.0), v(0.0, 1.0), v(-3.0, 3.0)).is_some());
        let _ = (FRAC_PI_2, TAU);
    }

    #[test]
    fn diameter_circle() {
        let (c, r) = circle_by_diameter(v(0.0, 0.0), v(6.0, 8.0)).unwrap();
        assert_eq!((c, r), (v(3.0, 4.0), 5.0));
        assert!(circle_by_diameter(v(1.0, 1.0), v(1.0, 1.0)).is_none());
    }
}
