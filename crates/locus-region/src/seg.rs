// SPDX-License-Identifier: AGPL-3.0-or-later

//! Boundary pieces: line segments and arcs with a signed sweep (CCW > 0), and
//! closed loops of them (SPEC §6.3, ADR 0008).

use core::f64::consts::{FRAC_PI_2, PI, TAU};
use locus_geom::{Aabb, Vec2};
use serde::{Deserialize, Serialize};

/// A line segment or an arc. Unlike `locus_geom::Arc` an arc here may run
/// either way round, so a piece can be reversed without changing shape.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Seg {
    Line {
        a: Vec2,
        b: Vec2,
    },
    /// Centre, radius, start angle (radians), signed sweep (CCW > 0).
    Arc {
        center: Vec2,
        radius: f64,
        start: f64,
        sweep: f64,
    },
}

impl Seg {
    #[must_use]
    pub fn is_arc(&self) -> bool {
        matches!(self, Seg::Arc { .. })
    }

    /// Point at `t` (0 = start, 1 = end).
    #[must_use]
    pub fn at(&self, t: f64) -> Vec2 {
        match *self {
            Seg::Line { a, b } => a.lerp(b, t),
            Seg::Arc {
                center,
                radius,
                start,
                sweep,
            } => center + Vec2::from_angle(start + sweep * t) * radius,
        }
    }

    #[must_use]
    pub fn start(&self) -> Vec2 {
        match *self {
            Seg::Line { a, .. } => a,
            Seg::Arc { .. } => self.at(0.0),
        }
    }

    #[must_use]
    pub fn end(&self) -> Vec2 {
        match *self {
            Seg::Line { b, .. } => b,
            Seg::Arc { .. } => self.at(1.0),
        }
    }

    #[must_use]
    pub fn length(&self) -> f64 {
        match *self {
            Seg::Line { a, b } => a.distance(b),
            Seg::Arc { radius, sweep, .. } => radius * sweep.abs(),
        }
    }

    /// The same piece traversed the other way.
    #[must_use]
    pub fn reversed(&self) -> Seg {
        match *self {
            Seg::Line { a, b } => Seg::Line { a: b, b: a },
            Seg::Arc {
                center,
                radius,
                start,
                sweep,
            } => Seg::Arc {
                center,
                radius,
                start: start + sweep,
                sweep: -sweep,
            },
        }
    }

    /// Unit tangent in the direction of travel at `t` (zero when degenerate).
    #[must_use]
    pub fn tangent(&self, t: f64) -> Vec2 {
        match *self {
            Seg::Line { a, b } => (b - a).normalized(0.0).unwrap_or(Vec2::ZERO),
            Seg::Arc { start, sweep, .. } => {
                let d = Vec2::from_angle(start + sweep * t).perp();
                if sweep >= 0.0 { d } else { -d }
            }
        }
    }

    /// The part between parameters `t0` and `t1`.
    #[must_use]
    pub fn sub(&self, t0: f64, t1: f64) -> Seg {
        match *self {
            Seg::Line { .. } => Seg::Line {
                a: self.at(t0),
                b: self.at(t1),
            },
            Seg::Arc {
                center,
                radius,
                start,
                sweep,
            } => Seg::Arc {
                center,
                radius,
                start: start + sweep * t0,
                sweep: sweep * (t1 - t0),
            },
        }
    }

    /// Angle of `q` about `center` from `start`, in the sweep's direction, in
    /// [0, 2π).
    fn rel_angle(center: Vec2, start: f64, sweep: f64, q: Vec2) -> f64 {
        let ang = (q - center).angle();
        if sweep >= 0.0 {
            (ang - start).rem_euclid(TAU)
        } else {
            (start - ang).rem_euclid(TAU)
        }
    }

    /// Parameter of the closest point; slightly outside [0, 1] for points just
    /// past an end.
    #[must_use]
    pub fn param_of(&self, q: Vec2) -> f64 {
        match *self {
            Seg::Line { a, b } => {
                let d = b - a;
                let l2 = d.length_sq();
                if l2 == 0.0 { 0.0 } else { (q - a).dot(d) / l2 }
            }
            Seg::Arc {
                center,
                start,
                sweep,
                ..
            } => {
                let s = sweep.abs();
                if s == 0.0 {
                    return 0.0;
                }
                let rel = Self::rel_angle(center, start, sweep, q);
                // Past the end, or (nearer) before the start.
                if rel <= s || rel - s < TAU - rel {
                    rel / s
                } else {
                    (rel - TAU) / s
                }
            }
        }
    }

    /// Exact distance from `q` to the piece.
    #[must_use]
    pub fn distance(&self, q: Vec2) -> f64 {
        match *self {
            Seg::Line { a, b } => q.distance(a.lerp(b, self.param_of(q).clamp(0.0, 1.0))),
            Seg::Arc {
                center,
                radius,
                start,
                sweep,
            } => {
                if Self::rel_angle(center, start, sweep, q) <= sweep.abs() {
                    (q.distance(center) - radius).abs()
                } else {
                    q.distance(self.start()).min(q.distance(self.end()))
                }
            }
        }
    }

    #[must_use]
    pub fn bbox(&self) -> Aabb {
        let mut bb = Aabb::from_points(self.start(), self.end());
        if let Seg::Arc {
            center,
            radius,
            start,
            sweep,
        } = *self
        {
            for k in 0..4 {
                let q = center + Vec2::from_angle(f64::from(k) * FRAC_PI_2) * radius;
                if Self::rel_angle(center, start, sweep, q) <= sweep.abs() {
                    bb = bb.include(q);
                }
            }
        }
        bb
    }

    /// Exact signed-area contribution (Green's theorem) of this piece of a
    /// closed loop.
    #[must_use]
    pub fn area_term(&self) -> f64 {
        match *self {
            Seg::Line { a, b } => 0.5 * a.cross(b),
            Seg::Arc { radius, sweep, .. } => {
                0.5 * self.start().cross(self.end()) + 0.5 * radius * radius * (sweep - sweep.sin())
            }
        }
    }

    /// Appends points of a chord approximation with sagitta at most `sag`:
    /// the start and interior points, not the end.
    pub fn flatten_into(&self, sag: f64, out: &mut Vec<Vec2>) {
        match *self {
            Seg::Line { a, .. } => out.push(a),
            Seg::Arc { radius, sweep, .. } => {
                let step = 2.0 * (1.0 - (sag / radius).min(1.0)).acos();
                // Capped so an absurd tolerance can't allocate without bound.
                let n = if step > 0.0 {
                    (sweep.abs() / step).ceil().clamp(1.0, 4096.0) as usize
                } else {
                    1
                };
                out.extend((0..n).map(|i| self.at(i as f64 / n as f64)));
            }
        }
    }
}

/// A boundary piece and the sketch entity it lies on.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Edge {
    pub seg: Seg,
    /// The entity (curve key) the piece lies on.
    pub source: u64,
}

/// A closed loop: each edge ends where the next starts. Counter-clockwise
/// loops bound an area, clockwise ones are holes.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Loop {
    pub edges: Vec<Edge>,
}

impl Loop {
    /// Signed area, exact for arcs (CCW positive).
    #[must_use]
    pub fn area(&self) -> f64 {
        self.edges.iter().map(|e| e.seg.area_term()).sum()
    }

    #[must_use]
    pub fn bbox(&self) -> Option<Aabb> {
        self.edges.iter().map(|e| e.seg.bbox()).reduce(Aabb::union)
    }

    /// Chord approximation of the loop (closed implicitly) for drawing and
    /// hit-testing.
    #[must_use]
    pub fn flatten(&self, sag: f64) -> Vec<Vec2> {
        let mut out = Vec::new();
        for e in &self.edges {
            e.seg.flatten_into(sag, &mut out);
        }
        out
    }

    /// The loop traversed the other way.
    #[must_use]
    pub fn reversed(&self) -> Loop {
        Loop {
            edges: self
                .edges
                .iter()
                .rev()
                .map(|e| Edge {
                    seg: e.seg.reversed(),
                    source: e.source,
                })
                .collect(),
        }
    }

    /// Position shared by edges `i - 1` and `i`. Lines store their ends, arcs
    /// compute them; one canonical value per vertex keeps the winding test
    /// from counting a ray through a vertex twice or not at all.
    fn vertex(&self, i: usize) -> Vec2 {
        let n = self.edges.len();
        let (prev, cur) = (&self.edges[(i + n - 1) % n].seg, &self.edges[i % n].seg);
        match (prev, cur) {
            (_, Seg::Line { a, .. }) => *a,
            (Seg::Line { b, .. }, _) => *b,
            _ => cur.start(),
        }
    }

    /// Winding number of the loop around `q` (half-open ray rule; arcs split
    /// into y-monotone pieces). Undefined exactly on the boundary.
    #[must_use]
    pub fn winding(&self, q: Vec2) -> i32 {
        self.winding_boxed(q, None)
    }

    /// [`Loop::winding`], skipping edges whose box (`boxes[i]`, when given,
    /// one per edge) the ray can't meet.
    pub(crate) fn winding_boxed(&self, q: Vec2, boxes: Option<&[Aabb]>) -> i32 {
        let mut w = 0;
        let mut cross = |a: Vec2, b: Vec2, x_at: &dyn Fn() -> f64| {
            if (a.y <= q.y) != (b.y <= q.y) && x_at() > q.x {
                w += if b.y > a.y { 1 } else { -1 };
            }
        };
        let reach = crate::arrange::SNAP;
        for (i, e) in self.edges.iter().enumerate() {
            // The ray can't meet an edge that is clearly above, below or left
            // of the probe; checked before the vertices, which cost
            // trigonometry. The margin keeps canonical vertices, which may
            // differ from an arc's own ends by an ulp, on the full test.
            if let Some(bb) = boxes.and_then(|b| b.get(i)) {
                if bb.min.y > q.y + reach || bb.max.y < q.y - reach || bb.max.x < q.x - reach {
                    continue;
                }
            } else if let Seg::Arc { center, radius, .. } = e.seg {
                let reach = radius + reach;
                if (q.y - center.y).abs() > reach || center.x + reach < q.x {
                    continue;
                }
            }
            let (v0, v1) = (self.vertex(i), self.vertex(i + 1));
            match e.seg {
                Seg::Line { .. } => cross(v0, v1, &|| {
                    v0.x + (q.y - v0.y) / (v1.y - v0.y) * (v1.x - v0.x)
                }),
                Seg::Arc {
                    center,
                    radius,
                    start,
                    sweep,
                } => {
                    let (angs, len) = monotone_breaks(start, sweep);
                    let angs = &angs[..len];
                    let m = len - 1;
                    for (k, win) in angs.windows(2).enumerate() {
                        let (t0, t1) = (win[0], win[1]);
                        let pa = if k == 0 {
                            v0
                        } else {
                            center + Vec2::from_angle(t0) * radius
                        };
                        let pb = if k + 1 == m {
                            v1
                        } else {
                            center + Vec2::from_angle(t1) * radius
                        };
                        let side = ((t0 + t1) / 2.0).cos().signum();
                        cross(pa, pb, &|| {
                            let dy = q.y - center.y;
                            center.x + side * (radius * radius - dy * dy).max(0.0).sqrt()
                        });
                    }
                }
            }
        }
        w
    }
}

/// Most arcs in a loop sweep at most a full turn, so they have at most three
/// top/bottom breaks; longer sweeps are cut short at this many angles.
const MAX_BREAKS: usize = 8;

/// `start`, every top/bottom angle (π/2 + kπ) strictly inside the sweep in
/// order, and the end angle, with the count used.
fn monotone_breaks(start: f64, sweep: f64) -> ([f64; MAX_BREAKS], usize) {
    let end = start + sweep;
    let mut angs = [0.0; MAX_BREAKS];
    angs[0] = start;
    let mut len = 1;
    if sweep > 0.0 {
        let mut k = ((start - FRAC_PI_2) / PI).floor() + 1.0;
        while FRAC_PI_2 + k * PI < end && len < MAX_BREAKS - 1 {
            angs[len] = FRAC_PI_2 + k * PI;
            len += 1;
            k += 1.0;
        }
    } else {
        let mut k = ((start - FRAC_PI_2) / PI).ceil() - 1.0;
        while FRAC_PI_2 + k * PI > end && len < MAX_BREAKS - 1 {
            angs[len] = FRAC_PI_2 + k * PI;
            len += 1;
            k -= 1.0;
        }
    }
    angs[len] = end;
    (angs, len + 1)
}

/// A filled area: loops combined by the non-zero winding rule (outer loops
/// CCW, holes CW).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Area {
    pub loops: Vec<Loop>,
}

impl Area {
    #[must_use]
    pub fn winding(&self, q: Vec2) -> i32 {
        self.loops.iter().map(|l| l.winding(q)).sum()
    }

    #[must_use]
    pub fn contains(&self, q: Vec2) -> bool {
        self.winding(q) != 0
    }

    /// Net area (holes subtract).
    #[must_use]
    pub fn area(&self) -> f64 {
        self.loops.iter().map(Loop::area).sum()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.loops.is_empty()
    }

    pub fn edges(&self) -> impl Iterator<Item = &Edge> {
        self.loops.iter().flat_map(|l| l.edges.iter())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: f64, y: f64) -> Vec2 {
        Vec2::new(x, y)
    }

    fn circle(c: Vec2, r: f64) -> Loop {
        Loop {
            edges: vec![Edge {
                seg: Seg::Arc {
                    center: c,
                    radius: r,
                    start: 0.0,
                    sweep: TAU,
                },
                source: 1,
            }],
        }
    }

    #[test]
    fn arc_area_and_winding() {
        let c = circle(v(1.0, 2.0), 3.0);
        assert!((c.area() - PI * 9.0).abs() < 1e-12);
        assert_eq!(c.winding(v(1.0, 2.0)), 1);
        assert_eq!(c.winding(v(4.5, 2.0)), 0);
        assert_eq!(c.reversed().winding(v(1.0, 2.0)), -1);
        assert!((c.reversed().area() + PI * 9.0).abs() < 1e-12);
    }

    #[test]
    fn params_and_distance() {
        let a = Seg::Arc {
            center: v(0.0, 0.0),
            radius: 2.0,
            start: 0.0,
            sweep: -FRAC_PI_2,
        };
        assert!(a.end().approx_eq(v(0.0, -2.0), 1e-12));
        assert!((a.param_of(v(2.0f64.sqrt(), -(2.0f64.sqrt()))) - 0.5).abs() < 1e-12);
        assert!(a.param_of(v(2.0, 0.01)) < 0.0);
        assert!((a.distance(v(3.0, -0.0001)) - 1.0).abs() < 1e-6);
        assert!(a.reversed().start().approx_eq(a.end(), 1e-12));
        let bb = Seg::Arc {
            center: v(0.0, 0.0),
            radius: 1.0,
            start: 0.0,
            sweep: PI,
        }
        .bbox();
        assert!(bb.max.approx_eq(v(1.0, 1.0), 1e-12) && bb.min.approx_eq(v(-1.0, 0.0), 1e-12));
    }

    #[test]
    fn ray_through_a_vertex_counts_once() {
        // Square whose right side has a vertex at the probe's height.
        let p = |x, y| v(x, y);
        let pts = [
            p(0.0, 0.0),
            p(2.0, 0.0),
            p(2.0, 1.0),
            p(2.0, 2.0),
            p(0.0, 2.0),
        ];
        let edges = (0..pts.len())
            .map(|i| Edge {
                seg: Seg::Line {
                    a: pts[i],
                    b: pts[(i + 1) % pts.len()],
                },
                source: 0,
            })
            .collect();
        assert_eq!(Loop { edges }.winding(p(1.0, 1.0)), 1);
    }

    #[test]
    fn flatten_respects_sagitta_and_caps() {
        let c = circle(v(0.0, 0.0), 10.0);
        let pts = c.flatten(0.01);
        assert!(pts.len() > 20 && pts.iter().all(|q| (q.length() - 10.0).abs() < 1e-9));
        assert!(c.flatten(0.0).len() <= 4096);
    }
}
