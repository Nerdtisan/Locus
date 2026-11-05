// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared spike geometry: points, line/arc segments, loops, shapes, area and
//! winding. Arcs carry a signed sweep (CCW positive), as in a polyline bulge.

use std::f64::consts::{PI, TAU};
use std::ops::{Add, Mul, Neg, Sub};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct P {
    pub x: f64,
    pub y: f64,
}

#[must_use]
pub const fn p(x: f64, y: f64) -> P {
    P { x, y }
}

impl Add for P {
    type Output = P;
    fn add(self, o: P) -> P {
        p(self.x + o.x, self.y + o.y)
    }
}
impl Sub for P {
    type Output = P;
    fn sub(self, o: P) -> P {
        p(self.x - o.x, self.y - o.y)
    }
}
impl Mul<f64> for P {
    type Output = P;
    fn mul(self, k: f64) -> P {
        p(self.x * k, self.y * k)
    }
}
impl Neg for P {
    type Output = P;
    fn neg(self) -> P {
        p(-self.x, -self.y)
    }
}

impl P {
    pub fn dot(self, o: P) -> f64 {
        self.x * o.x + self.y * o.y
    }
    pub fn cross(self, o: P) -> f64 {
        self.x * o.y - self.y * o.x
    }
    pub fn len(self) -> f64 {
        self.x.hypot(self.y)
    }
    pub fn dist(self, o: P) -> f64 {
        (self - o).len()
    }
    /// Rotated +90°.
    pub fn perp(self) -> P {
        p(-self.y, self.x)
    }
    pub fn unit(self) -> P {
        let l = self.len();
        if l > 0.0 { self * (1.0 / l) } else { p(0.0, 0.0) }
    }
    pub fn angle(self) -> f64 {
        self.y.atan2(self.x)
    }
    pub fn dir(a: f64) -> P {
        p(a.cos(), a.sin())
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Seg {
    Line { a: P, b: P },
    /// Centre, radius, start angle, signed sweep (CCW > 0).
    Arc { c: P, r: f64, a0: f64, sweep: f64 },
}

impl Seg {
    pub fn is_arc(&self) -> bool {
        matches!(self, Seg::Arc { .. })
    }
    pub fn at(&self, t: f64) -> P {
        match *self {
            Seg::Line { a, b } => a + (b - a) * t,
            Seg::Arc { c, r, a0, sweep } => c + P::dir(a0 + sweep * t) * r,
        }
    }
    pub fn start(&self) -> P {
        match *self {
            Seg::Line { a, .. } => a,
            _ => self.at(0.0),
        }
    }
    pub fn end(&self) -> P {
        match *self {
            Seg::Line { b, .. } => b,
            _ => self.at(1.0),
        }
    }
    pub fn len(&self) -> f64 {
        match *self {
            Seg::Line { a, b } => a.dist(b),
            Seg::Arc { r, sweep, .. } => r * sweep.abs(),
        }
    }
    pub fn rev(&self) -> Seg {
        match *self {
            Seg::Line { a, b } => Seg::Line { a: b, b: a },
            Seg::Arc { c, r, a0, sweep } => Seg::Arc { c, r, a0: a0 + sweep, sweep: -sweep },
        }
    }
    /// Unit tangent in the direction of travel.
    pub fn tangent(&self, t: f64) -> P {
        match *self {
            Seg::Line { a, b } => (b - a).unit(),
            Seg::Arc { a0, sweep, .. } => {
                let d = P::dir(a0 + sweep * t).perp();
                if sweep >= 0.0 { d } else { -d }
            }
        }
    }
    pub fn sub(&self, t0: f64, t1: f64) -> Seg {
        match *self {
            Seg::Line { .. } => Seg::Line { a: self.at(t0), b: self.at(t1) },
            Seg::Arc { c, r, a0, sweep } => Seg::Arc { c, r, a0: a0 + sweep * t0, sweep: sweep * (t1 - t0) },
        }
    }
    /// Angle of `q` about the arc centre measured from `a0` in the sweep's
    /// direction, in [0, 2π).
    fn rel_angle(c: P, a0: f64, sweep: f64, q: P) -> f64 {
        let ang = (q - c).angle();
        if sweep >= 0.0 { (ang - a0).rem_euclid(TAU) } else { (a0 - ang).rem_euclid(TAU) }
    }
    /// Parameter of the closest point (approximately 0..1; slightly outside
    /// for points just past an end).
    pub fn param_of(&self, q: P) -> f64 {
        match *self {
            Seg::Line { a, b } => {
                let d = b - a;
                let l2 = d.dot(d);
                if l2 == 0.0 { 0.0 } else { (q - a).dot(d) / l2 }
            }
            Seg::Arc { c, a0, sweep, .. } => {
                let rel = Self::rel_angle(c, a0, sweep, q);
                let s = sweep.abs();
                if rel <= s {
                    rel / s
                } else if rel - s < TAU - rel {
                    rel / s
                } else {
                    (rel - TAU) / s
                }
            }
        }
    }
    /// Exact distance from `q` to the segment.
    pub fn dist(&self, q: P) -> f64 {
        match *self {
            Seg::Line { a, b } => {
                let t = self.param_of(q).clamp(0.0, 1.0);
                q.dist(a + (b - a) * t)
            }
            Seg::Arc { c, r, a0, sweep } => {
                if Self::rel_angle(c, a0, sweep, q) <= sweep.abs() {
                    (q.dist(c) - r).abs()
                } else {
                    q.dist(self.start()).min(q.dist(self.end()))
                }
            }
        }
    }
    pub fn bbox(&self) -> (P, P) {
        let (s, e) = (self.start(), self.end());
        let mut lo = p(s.x.min(e.x), s.y.min(e.y));
        let mut hi = p(s.x.max(e.x), s.y.max(e.y));
        if let Seg::Arc { c, r, a0, sweep } = *self {
            for k in 0..4 {
                let ang = f64::from(k) * PI / 2.0;
                let q = c + P::dir(ang) * r;
                if Self::rel_angle(c, a0, sweep, q) <= sweep.abs() {
                    lo = p(lo.x.min(q.x), lo.y.min(q.y));
                    hi = p(hi.x.max(q.x), hi.y.max(q.y));
                }
            }
        }
        (lo, hi)
    }
    /// Pushes the start point and interior points (not the end) of a chord
    /// approximation whose sagitta is at most `sag`.
    pub fn flatten(&self, sag: f64, out: &mut Vec<P>) {
        match *self {
            Seg::Line { a, .. } => out.push(a),
            Seg::Arc { r, sweep, .. } => {
                let step = 2.0 * (1.0 - (sag / r).min(1.0)).acos();
                let n = ((sweep.abs() / step).ceil() as usize).max(1);
                for i in 0..n {
                    out.push(self.at(i as f64 / n as f64));
                }
            }
        }
    }
}

/// Arc from `p0` to `p1` with signed sweep `sweep` (|sweep| < 2π).
pub fn arc_from_chord(p0: P, p1: P, sweep: f64) -> Seg {
    let ch = p1 - p0;
    let half = ch.len() / 2.0;
    let m = p0 + ch * 0.5;
    let c = m + ch.unit().perp() * (half / (sweep / 2.0).tan());
    Seg::Arc { c, r: c.dist(p0), a0: (p0 - c).angle(), sweep }
}

pub type Loop = Vec<Seg>;

#[derive(Clone, Debug, Default)]
pub struct Shape {
    pub loops: Vec<Loop>,
}

impl Shape {
    pub fn new(loops: Vec<Loop>) -> Self {
        Self { loops }
    }
    pub fn area(&self) -> f64 {
        self.loops.iter().map(|l| loop_area(l)).sum()
    }
    pub fn segs(&self) -> impl Iterator<Item = &Seg> {
        self.loops.iter().flatten()
    }
    pub fn count(&self) -> (usize, usize) {
        let arcs = self.segs().filter(|s| s.is_arc()).count();
        (arcs, self.segs().count() - arcs)
    }
    pub fn perimeter(&self) -> f64 {
        self.segs().map(Seg::len).sum()
    }
    pub fn bbox(&self) -> (P, P) {
        let mut lo = p(f64::INFINITY, f64::INFINITY);
        let mut hi = p(f64::NEG_INFINITY, f64::NEG_INFINITY);
        for s in self.segs() {
            let (a, b) = s.bbox();
            lo = p(lo.x.min(a.x), lo.y.min(a.y));
            hi = p(hi.x.max(b.x), hi.y.max(b.y));
        }
        (lo, hi)
    }
}

/// Signed area (CCW positive) by Green's theorem; arcs exactly.
pub fn loop_area(l: &[Seg]) -> f64 {
    l.iter()
        .map(|s| match *s {
            Seg::Line { a, b } => 0.5 * a.cross(b),
            Seg::Arc { r, sweep, .. } => 0.5 * s.start().cross(s.end()) + 0.5 * r * r * (sweep - sweep.sin()),
        })
        .sum()
}

/// Winding number of the loops around `q` (half-open ray-crossing rule; arcs
/// split into y-monotone pieces). Each loop vertex has one canonical position
/// shared by both segments meeting there, so a ray through a vertex is never
/// counted twice or missed when an arc's computed end differs by an ulp.
pub fn winding(loops: &[Loop], q: P) -> i32 {
    let mut w = 0;
    let mut cross = |a: P, b: P, x_at: &dyn Fn() -> f64| {
        if (a.y <= q.y) != (b.y <= q.y) && x_at() > q.x {
            w += if b.y > a.y { 1 } else { -1 };
        }
    };
    for l in loops {
        let n = l.len();
        let vert = |i: usize| -> P {
            let (prev, cur) = (&l[(i + n - 1) % n], &l[i % n]);
            match (prev, cur) {
                (_, Seg::Line { a, .. }) => *a,
                (Seg::Line { b, .. }, _) => *b,
                _ => cur.start(),
            }
        };
        for (i, s) in l.iter().enumerate() {
            let (v0, v1) = (vert(i), vert(i + 1));
            match *s {
                Seg::Line { .. } => cross(v0, v1, &|| v0.x + (q.y - v0.y) / (v1.y - v0.y) * (v1.x - v0.x)),
                Seg::Arc { c, r, a0, sweep } => {
                    let end = a0 + sweep;
                    let mut angs = vec![a0];
                    if sweep > 0.0 {
                        let mut k = ((a0 - PI / 2.0) / PI).floor() + 1.0;
                        while PI / 2.0 + k * PI < end {
                            angs.push(PI / 2.0 + k * PI);
                            k += 1.0;
                        }
                    } else {
                        let mut k = ((a0 - PI / 2.0) / PI).ceil() - 1.0;
                        while PI / 2.0 + k * PI > end {
                            angs.push(PI / 2.0 + k * PI);
                            k -= 1.0;
                        }
                    }
                    angs.push(end);
                    let m = angs.len() - 1;
                    for (k, win) in angs.windows(2).enumerate() {
                        let (t0, t1) = (win[0], win[1]);
                        let pa = if k == 0 { v0 } else { c + P::dir(t0) * r };
                        let pb = if k + 1 == m { v1 } else { c + P::dir(t1) * r };
                        let side = ((t0 + t1) / 2.0).cos().signum();
                        cross(pa, pb, &|| {
                            let dy = q.y - c.y;
                            c.x + side * (r * r - dy * dy).max(0.0).sqrt()
                        });
                    }
                }
            }
        }
    }
    w
}

// ── Fixture builders ─────────────────────────────────────────────────────

pub fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Loop {
    let (a, b, c, d) = (p(x0, y0), p(x1, y0), p(x1, y1), p(x0, y1));
    vec![Seg::Line { a, b }, Seg::Line { a: b, b: c }, Seg::Line { a: c, b: d }, Seg::Line { a: d, b: a }]
}

pub fn circle(c: P, r: f64) -> Loop {
    vec![Seg::Arc { c, r, a0: 0.0, sweep: TAU }]
}

/// Obround: two semicircles of radius `r` around `c1`, `c2`, joined by lines.
pub fn slot(c1: P, c2: P, r: f64) -> Loop {
    let d = (c2 - c1).unit();
    let n = d.perp();
    let na = n.angle();
    vec![
        Seg::Line { a: c1 - n * r, b: c2 - n * r },
        Seg::Arc { c: c2, r, a0: na - PI, sweep: PI },
        Seg::Line { a: c2 + n * r, b: c1 + n * r },
        Seg::Arc { c: c1, r, a0: na, sweep: PI },
    ]
}

pub fn rounded_rect(x0: f64, y0: f64, x1: f64, y1: f64, r: f64) -> Loop {
    let h = PI / 2.0;
    vec![
        Seg::Line { a: p(x0 + r, y0), b: p(x1 - r, y0) },
        Seg::Arc { c: p(x1 - r, y0 + r), r, a0: -h, sweep: h },
        Seg::Line { a: p(x1, y0 + r), b: p(x1, y1 - r) },
        Seg::Arc { c: p(x1 - r, y1 - r), r, a0: 0.0, sweep: h },
        Seg::Line { a: p(x1 - r, y1), b: p(x0 + r, y1) },
        Seg::Arc { c: p(x0 + r, y1 - r), r, a0: h, sweep: h },
        Seg::Line { a: p(x0, y1 - r), b: p(x0, y0 + r) },
        Seg::Arc { c: p(x0 + r, y0 + r), r, a0: PI, sweep: h },
    ]
}

pub fn reversed(l: &Loop) -> Loop {
    l.iter().rev().map(Seg::rev).collect()
}
