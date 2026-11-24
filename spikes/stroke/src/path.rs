// SPDX-License-Identifier: AGPL-3.0-or-later

//! The centreline (lines, arcs, cubics) and the stroke models: each model is a
//! pair of side functions L(k, t), R(k, t) per centreline piece k, t ∈ [0, 1].

use locus_geom::{Cubic, Vec2};
use locus_region::Seg;

/// One centreline piece, in travel order.
#[derive(Debug, Clone, Copy)]
pub enum Piece {
    Seg(Seg),
    Cubic(Cubic),
}

impl Piece {
    pub fn at(&self, t: f64) -> Vec2 {
        match self {
            Piece::Seg(s) => s.at(t),
            Piece::Cubic(c) => c.eval(t),
        }
    }

    /// Unit tangent; cubics fall back to the chord at a degenerate derivative.
    pub fn tangent(&self, t: f64) -> Vec2 {
        match self {
            Piece::Seg(s) => s.tangent(t),
            Piece::Cubic(c) => {
                let d = c.derivative(t);
                if let Some(u) = d.normalized(1e-12) {
                    return u;
                }
                // Coincident control point: the second derivative's direction.
                let h = if t < 0.5 { 1e-4 } else { -1e-4 };
                let d2 = (c.eval(t + h) - c.eval(t)) * h.signum();
                d2.normalized(0.0).unwrap_or(Vec2::new(1.0, 0.0))
            }
        }
    }
}

/// A centreline with an arc-length table per piece, so a profile can be a
/// function of the fraction of total length.
#[derive(Debug, Clone)]
pub struct Path {
    pub pieces: Vec<Piece>,
    pub closed: bool,
    /// Per piece: cumulative length at t = i / TABLE.
    table: Vec<[f64; TABLE + 1]>,
    pub length: f64,
}

const TABLE: usize = 64;

impl Path {
    pub fn new(pieces: Vec<Piece>, closed: bool) -> Self {
        let mut table = Vec::with_capacity(pieces.len());
        let mut acc = 0.0;
        for p in &pieces {
            let mut row = [0.0; TABLE + 1];
            row[0] = acc;
            let mut prev = p.at(0.0);
            for (i, r) in row.iter_mut().enumerate().skip(1) {
                // 8 sub-steps per table step: length error far below 1e-6.
                for k in 1..=8 {
                    let t = ((i - 1) as f64 + k as f64 / 8.0) / TABLE as f64;
                    let q = p.at(t);
                    acc += prev.distance(q);
                    prev = q;
                }
                *r = acc;
            }
            table.push(row);
        }
        Self {
            pieces,
            closed,
            table,
            length: acc,
        }
    }

    /// Fraction of total length at piece `k`, parameter `t`.
    pub fn frac(&self, k: usize, t: f64) -> f64 {
        let row = &self.table[k];
        let x = (t.clamp(0.0, 1.0) * TABLE as f64).min(TABLE as f64 - 1e-9);
        let i = x.floor() as usize;
        let s = row[i] + (row[i + 1] - row[i]) * (x - i as f64);
        if self.length > 0.0 {
            s / self.length
        } else {
            0.0
        }
    }
}

/// Width profile (#426): points (t of length, left, right) in half-width mm,
/// smoothstep between points (C1, the Illustrator look).
#[derive(Debug, Clone)]
pub struct Profile {
    pub pts: Vec<(f64, f64, f64)>,
}

impl Profile {
    pub fn constant(w: f64) -> Self {
        Self {
            pts: vec![(0.0, w, w), (1.0, w, w)],
        }
    }

    pub fn at(&self, u: f64) -> (f64, f64) {
        let p = &self.pts;
        if u <= p[0].0 {
            return (p[0].1, p[0].2);
        }
        for w in p.windows(2) {
            let (a, b) = (w[0], w[1]);
            if u <= b.0 {
                let x = if b.0 > a.0 {
                    (u - a.0) / (b.0 - a.0)
                } else {
                    1.0
                };
                let s = x * x * (3.0 - 2.0 * x);
                return (a.1 + (b.1 - a.1) * s, a.2 + (b.2 - a.2) * s);
            }
        }
        let l = p[p.len() - 1];
        (l.1, l.2)
    }
}

/// A calligraphic nib (#430): ellipse with semi-axes `a` (along `angle`) and
/// `b` = a × roundness.
#[derive(Debug, Clone, Copy)]
pub struct Nib {
    pub a: f64,
    pub b: f64,
    pub angle: f64,
}

impl Nib {
    pub fn axes(&self) -> (Vec2, Vec2) {
        let u = Vec2::from_angle(self.angle);
        (u, u.perp())
    }

    /// Support function h(n): half-width of the nib across unit normal n.
    pub fn support(&self, n: Vec2) -> f64 {
        let (u, v) = self.axes();
        ((self.a * n.dot(u)).powi(2) + (self.b * n.dot(v)).powi(2)).sqrt()
    }

    /// Support point: the point of the nib farthest along n.
    pub fn support_point(&self, n: Vec2) -> Vec2 {
        let (u, v) = self.axes();
        let phi = (self.b * n.dot(v)).atan2(self.a * n.dot(u));
        u * (self.a * phi.cos()) + v * (self.b * phi.sin())
    }

    pub fn point(&self, phi: f64) -> Vec2 {
        let (u, v) = self.axes();
        u * (self.a * phi.cos()) + v * (self.b * phi.sin())
    }
}

/// The stroke model being expanded.
#[derive(Debug, Clone)]
pub enum Model {
    /// Normal offset by the profile's left/right half-widths (#426).
    Profile(Profile),
    /// ADR 0019's formula: normal offset by the nib's support h(n).
    NibNormal(Nib),
    /// Minkowski boundary: the nib's support point (S2 candidate).
    NibSupport(Nib),
}

impl Model {
    /// Left and right side points of piece `k` at `t`.
    pub fn sides(&self, path: &Path, k: usize, t: f64) -> (Vec2, Vec2) {
        let p = &path.pieces[k];
        let c = p.at(t);
        let tan = p.tangent(t);
        let n = tan.perp();
        match self {
            Model::Profile(pr) => {
                let (l, r) = pr.at(path.frac(k, t));
                (c + n * l, c - n * r)
            }
            Model::NibNormal(nib) => {
                let h = nib.support(n);
                (c + n * h, c - n * h)
            }
            Model::NibSupport(nib) => {
                let s = nib.support_point(n);
                (c + s, c - s)
            }
        }
    }

    /// Left and right half-widths at piece `k`, `t` (joins and caps).
    pub fn widths(&self, path: &Path, k: usize, t: f64) -> (f64, f64) {
        match self {
            Model::Profile(pr) => pr.at(path.frac(k, t)),
            Model::NibNormal(nib) | Model::NibSupport(nib) => {
                let n = path.pieces[k].tangent(t).perp();
                let h = nib.support(n);
                (h, h)
            }
        }
    }

    pub fn nib(&self) -> Option<Nib> {
        match self {
            Model::Profile(_) => None,
            Model::NibNormal(n) | Model::NibSupport(n) => Some(*n),
        }
    }
}
