// SPDX-License-Identifier: AGPL-3.0-or-later
//! Centreline trace (ADR 0019 decision 8): single-line paths for plotters
//! and engraving. The ink is thinned to a one-pixel skeleton with the
//! Zhang–Suen algorithm (Zhang & Suen 1984), the skeleton is walked from
//! its ends and junctions into polylines (closed loops last), and each
//! polyline is simplified to `tolerance` pixels (Ramer–Douglas–Peucker).

use crate::bitmap::Bitmap;
use crate::{Budget, Seg, TraceError};
use locus_geom::Vec2;

/// The 8 neighbours, clockwise from north, as Zhang–Suen numbers them
/// (P2..P9).
const RING: [(i64, i64); 8] = [
    (0, -1),
    (1, -1),
    (1, 0),
    (1, 1),
    (0, 1),
    (-1, 1),
    (-1, 0),
    (-1, -1),
];

/// Thin `b` in place to a one-pixel skeleton.
pub(crate) fn thin(b: &mut Bitmap, budget: &mut Budget) -> Result<(), TraceError> {
    let mut ink: Vec<(usize, usize)> = (0..b.height)
        .flat_map(|y| (0..b.width).map(move |x| (x, y)))
        .filter(|&(x, y)| b.get(x as i64, y as i64))
        .collect();
    loop {
        let mut changed = false;
        for pass in 0..2 {
            budget.spend(ink.len())?;
            let remove: Vec<(usize, usize)> = ink
                .iter()
                .copied()
                .filter(|&(x, y)| {
                    let p: Vec<bool> = RING
                        .iter()
                        .map(|(dx, dy)| b.get(x as i64 + dx, y as i64 + dy))
                        .collect();
                    let count = p.iter().filter(|v| **v).count();
                    let transitions = (0..8).filter(|i| !p[*i] && p[(i + 1) % 8]).count();
                    let (p2, p4, p6, p8) = (p[0], p[2], p[4], p[6]);
                    let side = if pass == 0 {
                        !(p2 && p4 && p6) && !(p4 && p6 && p8)
                    } else {
                        !(p2 && p4 && p8) && !(p2 && p6 && p8)
                    };
                    (2..=6).contains(&count) && transitions == 1 && side
                })
                .collect();
            for &(x, y) in &remove {
                b.set(x, y, false);
            }
            changed |= !remove.is_empty();
            ink.retain(|&(x, y)| b.get(x as i64, y as i64));
        }
        if !changed {
            return Ok(());
        }
    }
}

/// Drop the pixels thinning leaves that nothing needs: staircase corners
/// and 2×2 clumps. A pixel goes when its ink neighbours stay one connected
/// group without it (so no line breaks) and it isn't a line's end. Left
/// in, they make false junctions and tiny loops.
pub(crate) fn prune_simple(b: &mut Bitmap, budget: &mut Budget) -> Result<(), TraceError> {
    for _ in 0..4 {
        let mut changed = false;
        for y in 0..b.height {
            budget.spend(b.width)?;
            for x in 0..b.width {
                let (xi, yi) = (x as i64, y as i64);
                if !b.get(xi, yi) {
                    continue;
                }
                let on: Vec<bool> = RING
                    .iter()
                    .map(|(dx, dy)| b.get(xi + dx, yi + dy))
                    .collect();
                if on.iter().filter(|v| **v).count() >= 2 && one_group(&on) {
                    b.set(x, y, false);
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    Ok(())
}

/// Whether the ink among the 8 neighbours (in [`RING`] order) is one
/// 8-connected group: neighbours next in the ring touch, and so do two
/// edge neighbours either side of a corner (north and east).
fn one_group(on: &[bool]) -> bool {
    let touch = |i: usize, j: usize| {
        let d = (j + 8 - i) % 8;
        d == 1 || d == 7 || (d == 2 && i.is_multiple_of(2)) || (d == 6 && j.is_multiple_of(2))
    };
    let first = (0..8).find(|i| on[*i]);
    let Some(first) = first else { return true };
    let mut seen = [false; 8];
    let mut stack = vec![first];
    seen[first] = true;
    while let Some(i) = stack.pop() {
        for j in 0..8 {
            if on[j] && !seen[j] && touch(i, j) {
                seen[j] = true;
                stack.push(j);
            }
        }
    }
    (0..8).all(|i| !on[i] || seen[i])
}

/// The skeleton's polylines, in pixel centres.
pub(crate) fn walk(b: &Bitmap, budget: &mut Budget) -> Result<Vec<(Vec<Vec2>, bool)>, TraceError> {
    let (w, h) = (b.width, b.height);
    let neighbours = |x: usize, y: usize| -> Vec<(usize, usize)> {
        RING.iter()
            .map(|(dx, dy)| (x as i64 + dx, y as i64 + dy))
            .filter(|&(nx, ny)| b.get(nx, ny))
            .map(|(nx, ny)| (nx as usize, ny as usize))
            .collect()
    };
    let key = |x: usize, y: usize| y * w + x;
    let mut used = std::collections::HashSet::new();
    let edge = |a: usize, c: usize| (a.min(c), a.max(c));
    let mut out = Vec::new();
    let centre = |x: usize, y: usize| Vec2::new(x as f64 + 0.5, y as f64 + 0.5);
    let ink: Vec<(usize, usize)> = (0..h)
        .flat_map(|y| (0..w).map(move |x| (x, y)))
        .filter(|&(x, y)| b.get(x as i64, y as i64))
        .collect();
    // Nodes first (ends and junctions), so lines run node to node; what's
    // left is closed loops.
    let mut starts: Vec<(usize, usize)> = ink
        .iter()
        .copied()
        .filter(|&(x, y)| neighbours(x, y).len() != 2)
        .collect();
    starts.extend(ink.iter().copied());
    for (sx, sy) in starts {
        for first in neighbours(sx, sy) {
            if used.contains(&edge(key(sx, sy), key(first.0, first.1))) {
                continue;
            }
            let mut line = vec![centre(sx, sy)];
            let (mut prev, mut cur) = ((sx, sy), first);
            used.insert(edge(key(sx, sy), key(cur.0, cur.1)));
            loop {
                budget.spend(1)?;
                line.push(centre(cur.0, cur.1));
                let next: Vec<(usize, usize)> = neighbours(cur.0, cur.1)
                    .into_iter()
                    .filter(|n| {
                        *n != prev && !used.contains(&edge(key(cur.0, cur.1), key(n.0, n.1)))
                    })
                    .collect();
                // Stop at a node, or when nothing is left to follow.
                if neighbours(cur.0, cur.1).len() != 2 || next.is_empty() {
                    break;
                }
                let n = next[0];
                used.insert(edge(key(cur.0, cur.1), key(n.0, n.1)));
                prev = cur;
                cur = n;
            }
            let closed = line.len() > 2 && cur == (sx, sy);
            if closed {
                line.pop();
            }
            out.push((line, closed));
        }
    }
    Ok(out)
}

/// Ramer–Douglas–Peucker: the fewest points within `tol` of `pts`.
pub(crate) fn simplify(pts: &[Vec2], tol: f64) -> Vec<Vec2> {
    if pts.len() < 3 {
        return pts.to_vec();
    }
    let mut keep = vec![false; pts.len()];
    keep[0] = true;
    keep[pts.len() - 1] = true;
    let mut stack = vec![(0usize, pts.len() - 1)];
    while let Some((a, b)) = stack.pop() {
        let (pa, pb) = (pts[a], pts[b]);
        let e = pb - pa;
        let len = e.length();
        let (mut far, mut dmax) = (a, 0.0);
        for (i, p) in pts.iter().enumerate().take(b).skip(a + 1) {
            let d = if len > 1e-12 {
                (*p - pa).cross(e).abs() / len
            } else {
                p.distance(pa)
            };
            if d > dmax {
                dmax = d;
                far = i;
            }
        }
        if dmax > tol {
            keep[far] = true;
            stack.push((a, far));
            stack.push((far, b));
        }
    }
    pts.iter()
        .zip(keep)
        .filter_map(|(p, k)| k.then_some(*p))
        .collect()
}

/// Centrelines of `b` as line segments, scaled by `factor`.
pub(crate) fn centrelines(
    b: &Bitmap,
    tolerance: f64,
    speckle: usize,
    factor: f64,
    budget: &mut Budget,
) -> Result<Vec<(Vec<Seg>, bool)>, TraceError> {
    let mut sk = b.clone();
    thin(&mut sk, budget)?;
    prune_simple(&mut sk, budget)?;
    let mut out = Vec::new();
    for (line, closed) in walk(&sk, budget)? {
        // Short stubs are speckles.
        if line.len() <= speckle.max(1) {
            continue;
        }
        let mut pts = line;
        if closed {
            pts.push(pts[0]);
        }
        let pts = simplify(&pts, tolerance.max(0.1));
        let segs: Vec<Seg> = pts
            .windows(2)
            .map(|w| Seg::Line(w[0] * factor, w[1] * factor))
            .collect();
        if !segs.is_empty() {
            out.push((segs, closed));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_thick_bar_thins_to_one_line() {
        let mut b = Bitmap::new(40, 12);
        for y in 3..9 {
            for x in 4..36 {
                b.set(x, y, true);
            }
        }
        let lines = centrelines(&b, 1.0, 2, 1.0, &mut Budget::new(u64::MAX)).unwrap_or_default();
        assert_eq!(lines.len(), 1, "{lines:?}");
        let (segs, closed) = &lines[0];
        assert!(!closed);
        assert_eq!(segs.len(), 1, "{segs:?}");
        if let Seg::Line(a, c) = segs[0] {
            assert!((a.y - 6.0).abs() <= 1.0 && (c.y - 6.0).abs() <= 1.0);
            assert!(a.distance(c) > 20.0);
        }
    }

    #[test]
    fn a_ring_is_one_closed_loop() {
        let mut b = Bitmap::new(40, 40);
        for y in 0..40 {
            for x in 0..40 {
                let r = ((x as f64 - 20.0).powi(2) + (y as f64 - 20.0).powi(2)).sqrt();
                b.set(x, y, (12.0..15.0).contains(&r));
            }
        }
        let lines = centrelines(&b, 0.5, 2, 1.0, &mut Budget::new(u64::MAX)).unwrap_or_default();
        assert_eq!(lines.len(), 1, "{}", lines.len());
        assert!(lines[0].1, "closed");
    }

    #[test]
    fn simplify_keeps_corners() {
        let pts: Vec<Vec2> = (0..=10)
            .map(|i| Vec2::new(f64::from(i), 0.0))
            .chain((1..=10).map(|i| Vec2::new(10.0, f64::from(i))))
            .collect();
        let s = simplify(&pts, 0.1);
        assert_eq!(
            s,
            vec![
                Vec2::new(0.0, 0.0),
                Vec2::new(10.0, 0.0),
                Vec2::new(10.0, 10.0)
            ]
        );
    }
}
