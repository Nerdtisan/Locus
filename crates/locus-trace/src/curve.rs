// SPDX-License-Identifier: AGPL-3.0-or-later
//! Stages 2 and 3 of the Potrace method (Selinger 2003, §2.2–§2.3),
//! written from the paper: a lattice path becomes the polygon with the
//! fewest straight edges (ties broken by squared distance), its vertices
//! move onto the least-squares lines of their edges, and each vertex
//! becomes a corner or a cubic Bézier between the edges' midpoints by the
//! paper's α rule.
//!
//! Work happens in y-up coordinates (the paper's), so the straightness
//! cones keep their orientation; results go back to the image's y-down.

use crate::decompose::LatticePath;
use crate::{Budget, Seg, TraceError};
use locus_geom::Vec2;

/// Curve parameters (the dialog's Corners and Smoothing).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CurveParams {
    /// Potrace's `alphamax`: vertices with α at or above it are corners.
    /// 0 makes every vertex a corner (a polygon); 4/3 none.
    pub alpha_max: f64,
}

impl Default for CurveParams {
    fn default() -> Self {
        Self { alpha_max: 1.0 }
    }
}

type P = (i64, i64);

fn cross(a: P, b: P) -> i64 {
    a.0 * b.1 - a.1 * b.0
}

/// For each start `i`, how many steps along the path stay straight: the
/// points fit in a band one unit wide and the steps don't use all four
/// directions (the paper's straightness, by its constraint cones).
fn straight_runs(pts: &[P], budget: &mut Budget) -> Result<Vec<usize>, TraceError> {
    let n = pts.len();
    let mut lon = vec![1usize; n];
    for (i, run) in lon.iter_mut().enumerate() {
        let (mut c0, mut c1) = ((0i64, 0i64), (0i64, 0i64));
        let mut dirs = [false; 4];
        let mut best = 1;
        for len in 1..n {
            budget.spend(1)?;
            let a = pts[(i + len - 1) % n];
            let b = pts[(i + len) % n];
            let d = (b.0 - a.0, b.1 - a.1);
            let k = match d {
                (1, 0) => 0,
                (0, 1) => 1,
                (-1, 0) => 2,
                _ => 3,
            };
            dirs[k] = true;
            if dirs.iter().all(|x| *x) {
                break;
            }
            let p0 = pts[i];
            let cur = (b.0 - p0.0, b.1 - p0.1);
            if cross(c0, cur) < 0 || cross(c1, cur) > 0 {
                break;
            }
            best = len;
            if cur.0.abs() <= 1 && cur.1.abs() <= 1 {
                continue;
            }
            let off = (
                cur.0
                    + if cur.1 >= 0 && (cur.1 > 0 || cur.0 < 0) {
                        1
                    } else {
                        -1
                    },
                cur.1
                    + if cur.0 <= 0 && (cur.0 < 0 || cur.1 < 0) {
                        1
                    } else {
                        -1
                    },
            );
            if cross(c0, off) >= 0 {
                c0 = off;
            }
            let off = (
                cur.0
                    + if cur.1 <= 0 && (cur.1 < 0 || cur.0 < 0) {
                        1
                    } else {
                        -1
                    },
                cur.1
                    + if cur.0 >= 0 && (cur.0 > 0 || cur.1 < 0) {
                        1
                    } else {
                        -1
                    },
            );
            if cross(c1, off) <= 0 {
                c1 = off;
            }
        }
        *run = best;
    }
    Ok(lon)
}

/// Prefix sums over the path walked twice, for O(1) segment penalties.
struct Sums {
    x: Vec<f64>,
    y: Vec<f64>,
    xx: Vec<f64>,
    xy: Vec<f64>,
    yy: Vec<f64>,
    pts: Vec<Vec2>,
}

impl Sums {
    fn new(pts: &[P]) -> Self {
        let n = pts.len();
        let unrolled: Vec<Vec2> = (0..=2 * n)
            .map(|k| {
                let p = pts[k % n];
                Vec2::new(p.0 as f64, p.1 as f64)
            })
            .collect();
        let mut s = Self {
            x: vec![0.0],
            y: vec![0.0],
            xx: vec![0.0],
            xy: vec![0.0],
            yy: vec![0.0],
            pts: unrolled.clone(),
        };
        for p in &unrolled {
            let last = s.x.len() - 1;
            s.x.push(s.x[last] + p.x);
            s.y.push(s.y[last] + p.y);
            s.xx.push(s.xx[last] + p.x * p.x);
            s.xy.push(s.xy[last] + p.x * p.y);
            s.yy.push(s.yy[last] + p.y * p.y);
        }
        s
    }

    /// Sum of squared distances of points `a..=b` to the line through
    /// points `a` and `b`.
    fn penalty(&self, a: usize, b: usize) -> f64 {
        let (pa, pb) = (self.pts[a], self.pts[b]);
        let e = pb - pa;
        let len2 = e.length_sq();
        if len2 == 0.0 {
            return 0.0;
        }
        // cross(e, p - pa) = ex*py - ey*px + c, with c = ey*ax - ex*ay.
        let c = e.y * pa.x - e.x * pa.y;
        let k = (b + 1 - a) as f64;
        let r = |v: &Vec<f64>| v[b + 1] - v[a];
        let (sx, sy, sxx, sxy, syy) = (
            r(&self.x),
            r(&self.y),
            r(&self.xx),
            r(&self.xy),
            r(&self.yy),
        );
        let sum = e.x * e.x * syy + e.y * e.y * sxx - 2.0 * e.x * e.y * sxy + 2.0 * e.x * c * sy
            - 2.0 * e.y * c * sx
            + k * c * c;
        sum.max(0.0) / len2
    }

    /// The least-squares line through points `a..=b`: centroid and
    /// direction.
    fn fit(&self, a: usize, b: usize) -> (Vec2, Vec2) {
        let k = (b + 1 - a) as f64;
        let r = |v: &Vec<f64>| v[b + 1] - v[a];
        let (mx, my) = (r(&self.x) / k, r(&self.y) / k);
        let cxx = r(&self.xx) / k - mx * mx;
        let cxy = r(&self.xy) / k - mx * my;
        let cyy = r(&self.yy) / k - my * my;
        // Principal axis of the 2×2 covariance.
        let angle = 0.5 * (2.0 * cxy).atan2(cxx - cyy);
        let fallback = self.pts[b] - self.pts[a];
        let dir = if cxx + cyy > 1e-12 {
            Vec2::new(angle.cos(), angle.sin())
        } else {
            fallback.normalized(1e-12).unwrap_or(Vec2::new(1.0, 0.0))
        };
        (Vec2::new(mx, my), dir)
    }
}

/// The polygon with the fewest edges, then the least penalty: the indices
/// of its vertices on the path.
fn best_polygon(
    pts: &[P],
    lon: &[usize],
    sums: &Sums,
    budget: &mut Budget,
) -> Result<Vec<usize>, TraceError> {
    let n = pts.len();
    // best[b] = (edges, penalty, previous) for reaching unrolled index b
    // from 0; the polygon closes at n.
    let mut best: Vec<(usize, f64, usize)> = vec![(usize::MAX, f64::INFINITY, 0); n + 1];
    best[0] = (0, 0.0, 0);
    for a in 0..n {
        let (edges, pen, _) = best[a];
        if edges == usize::MAX {
            continue;
        }
        let reach = (a + lon[a % n]).min(n);
        budget.spend(reach - a)?;
        for (b, slot) in best.iter_mut().enumerate().take(reach + 1).skip(a + 1) {
            let cand = (edges + 1, pen + sums.penalty(a, b));
            if cand.0 < slot.0 || (cand.0 == slot.0 && cand.1 < slot.1) {
                *slot = (cand.0, cand.1, a);
            }
        }
    }
    let mut v = Vec::new();
    let mut b = n;
    while b > 0 {
        let a = best[b].2;
        v.push(a);
        if a == b {
            break;
        }
        b = a;
    }
    v.reverse();
    Ok(v)
}

/// Each vertex moved to where its two edges' fitted lines meet, kept
/// within half a pixel of the lattice vertex.
fn adjust(verts: &[usize], sums: &Sums, n: usize) -> Vec<Vec2> {
    let m = verts.len();
    let lines: Vec<(Vec2, Vec2)> = (0..m)
        .map(|k| {
            let a = verts[k];
            let mut b = verts[(k + 1) % m];
            if b <= a {
                b += n;
            }
            sums.fit(a, b)
        })
        .collect();
    (0..m)
        .map(|k| {
            let orig = sums.pts[verts[k]];
            let (c0, d0) = lines[(k + m - 1) % m];
            let (c1, d1) = lines[k];
            let den = d0.cross(d1);
            let hit = if den.abs() > 1e-9 {
                let t = (c1 - c0).cross(d1) / den;
                c0 + d0 * t
            } else {
                orig
            };
            Vec2::new(
                hit.x.clamp(orig.x - 0.5, orig.x + 0.5),
                hit.y.clamp(orig.y - 0.5, orig.y + 0.5),
            )
        })
        .collect()
}

/// The paper's α for vertex `p1` between `p0` and `p2`.
fn alpha(p0: Vec2, p1: Vec2, p2: Vec2) -> f64 {
    let d = p2 - p0;
    // The chord's length in the L1 norm.
    let denom = d.x.abs() + d.y.abs();
    if denom == 0.0 {
        return 4.0 / 3.0;
    }
    let dd = ((p1 - p0).cross(d) / denom).abs();
    let a = if dd > 1.0 { 1.0 - 1.0 / dd } else { 0.0 };
    a / 0.75
}

/// One traced outline: lines and cubics in image pixels, y down, scaled by
/// `factor`, merging collinear runs.
pub fn outline(
    path: &LatticePath,
    params: &CurveParams,
    factor: f64,
    budget: &mut Budget,
) -> Result<Vec<Seg>, TraceError> {
    // y up for the paper's orientation.
    let pts: Vec<P> = path.points.iter().map(|&(x, y)| (x, -y)).collect();
    let n = pts.len();
    if n < 4 {
        return Ok(Vec::new());
    }
    let lon = straight_runs(&pts, budget)?;
    let sums = Sums::new(&pts);
    let verts = best_polygon(&pts, &lon, &sums, budget)?;
    if verts.len() < 2 {
        return Ok(Vec::new());
    }
    let v = adjust(&verts, &sums, n);
    let m = v.len();
    let out = |p: Vec2| Vec2::new(p.x * factor, -p.y * factor);
    let mid = |k: usize| v[k % m].lerp(v[(k + 1) % m], 0.5);
    let mut segs = Vec::with_capacity(2 * m);
    for k in 0..m {
        budget.spend(1)?;
        let (p0, p1, p2) = (v[(k + m - 1) % m], v[k], v[(k + 1) % m]);
        let (a, b) = (mid(k + m - 1), mid(k));
        let al = alpha(p0, p1, p2);
        if al >= params.alpha_max {
            segs.push(Seg::Line(out(a), out(p1)));
            segs.push(Seg::Line(out(p1), out(b)));
        } else {
            let t = 0.5 + 0.5 * al.clamp(0.55, 1.0);
            segs.push(Seg::Cubic(
                out(a),
                out(a.lerp(p1, t)),
                out(b.lerp(p1, t)),
                out(b),
            ));
        }
    }
    Ok(merge_lines(segs))
}

/// Consecutive collinear lines joined: a corner's half-edges meet at the
/// midpoints of straight edges.
pub(crate) fn merge_lines(segs: Vec<Seg>) -> Vec<Seg> {
    let mut out: Vec<Seg> = Vec::with_capacity(segs.len());
    for s in segs {
        if let (Some(Seg::Line(a, b)), Seg::Line(c, d)) = (out.last().copied(), s)
            && b.distance(c) < 1e-9
        {
            let (u, w) = (b - a, d - c);
            if u.cross(w).abs() <= 1e-9 * u.length() * w.length() && u.dot(w) > 0.0 {
                if let Some(last) = out.last_mut() {
                    *last = Seg::Line(a, d);
                }
                continue;
            }
        }
        out.push(s);
    }
    // The first and last of a closed path may join too.
    if out.len() > 2
        && let (Some(Seg::Line(a, b)), Some(Seg::Line(c, d))) =
            (out.last().copied(), out.first().copied())
        && b.distance(c) < 1e-9
    {
        let (u, w) = (b - a, d - c);
        if u.cross(w).abs() <= 1e-9 * u.length() * w.length() && u.dot(w) > 0.0 {
            out.pop();
            out[0] = Seg::Line(a, d);
        }
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::bitmap::Bitmap;
    use crate::decompose::decompose;

    fn disc(r: f64, size: usize) -> Bitmap {
        let mut b = Bitmap::new(size, size);
        let c = size as f64 / 2.0;
        for y in 0..size {
            for x in 0..size {
                let (dx, dy) = (x as f64 + 0.5 - c, y as f64 + 0.5 - c);
                b.set(x, y, dx * dx + dy * dy <= r * r);
            }
        }
        b
    }

    fn trace(b: &Bitmap) -> Vec<Vec<Seg>> {
        let mut budget = Budget::new(u64::MAX);
        decompose(b, 0, &mut budget)
            .unwrap()
            .iter()
            .map(|p| outline(p, &CurveParams::default(), 1.0, &mut budget).unwrap())
            .collect()
    }

    #[test]
    fn a_big_square_is_four_lines() {
        let mut b = Bitmap::new(40, 40);
        for y in 5..35 {
            for x in 5..35 {
                b.set(x, y, true);
            }
        }
        let paths = trace(&b);
        assert_eq!(paths.len(), 1);
        let segs = &paths[0];
        assert_eq!(segs.len(), 4, "{segs:?}");
        assert!(segs.iter().all(|s| matches!(s, Seg::Line(..))));
        // Its corners are the square's.
        for s in segs {
            if let Seg::Line(a, _) = s {
                assert!(
                    [5.0, 35.0].iter().any(|v| (a.x - v).abs() < 1e-6)
                        && [5.0, 35.0].iter().any(|v| (a.y - v).abs() < 1e-6),
                    "{a:?}"
                );
            }
        }
    }

    #[test]
    fn a_disc_is_a_few_curves_close_to_the_circle() {
        let paths = trace(&disc(30.0, 80));
        assert_eq!(paths.len(), 1);
        let segs = &paths[0];
        assert!(segs.len() <= 24, "{} segments", segs.len());
        assert!(segs.iter().all(|s| matches!(s, Seg::Cubic(..))));
        // Every curve point stays within a pixel of the true circle.
        for s in segs {
            for t in [0.0, 0.25, 0.5, 0.75, 1.0] {
                let p = s.at(t);
                let r = p.distance(Vec2::new(40.0, 40.0));
                assert!((r - 30.0).abs() < 1.0, "r = {r}");
            }
        }
    }
}
