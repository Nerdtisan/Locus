// SPDX-License-Identifier: AGPL-3.0-or-later
//! Stage 1 of the Potrace method (Selinger 2003, §2.1): the bitmap's
//! boundaries as closed paths on the lattice of pixel corners.
//!
//! The first ink pixel in reading order starts a path along its top edge,
//! walked with the ink on the right. At each corner the two pixels ahead
//! decide the turn: ink on both sides turns left, paper on both turns
//! right, ink only on the right goes straight, and the ambiguous diagonal
//! follows the turn policy (here: keep ink connected). Once a path closes,
//! every pixel inside it is inverted, so the next path found is a hole (or
//! an island in it), and the bitmap is empty when everything is traced.
//! Paths enclosing fewer than `speckle` pixels are dropped ("turdsize").

use crate::bitmap::Bitmap;
use crate::{Budget, TraceError};

/// One closed boundary: lattice points (pixel corners, y down), and
/// whether it bounds ink (`outer`) or a hole in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LatticePath {
    pub points: Vec<(i64, i64)>,
    pub outer: bool,
    /// Enclosed pixels.
    pub area: u64,
}

/// Every boundary of `bitmap` larger than `speckle` pixels.
pub fn decompose(
    bitmap: &Bitmap,
    speckle: u64,
    budget: &mut Budget,
) -> Result<Vec<LatticePath>, TraceError> {
    let mut work = bitmap.clone();
    let mut out = Vec::new();
    let (w, h) = (work.width as i64, work.height as i64);
    // Resume the scan where the last path started: everything before it is
    // paper by now.
    let mut from = 0i64;
    while let Some((sx, sy)) = next_ink(&work, from, budget)? {
        from = sy * w + sx;
        let points = walk(&work, sx, sy, budget)?;
        let area = invert_inside(&mut work, &points, budget)?;
        if area > speckle {
            out.push(LatticePath {
                points,
                // The original image says which side the path bounds.
                outer: bitmap.get(sx, sy),
                area,
            });
        }
        if from >= w * h {
            break;
        }
    }
    Ok(out)
}

fn next_ink(b: &Bitmap, from: i64, budget: &mut Budget) -> Result<Option<(i64, i64)>, TraceError> {
    let w = b.width as i64;
    let total = w * b.height as i64;
    let mut i = from;
    while i < total {
        if i % 4096 == 0 {
            budget.spend(4096)?;
        }
        let (x, y) = (i % w, i / w);
        if b.get(x, y) {
            return Ok(Some((x, y)));
        }
        i += 1;
    }
    Ok(None)
}

/// Directions, y down: right of (dx, dy) is (-dy, dx).
fn right((dx, dy): (i64, i64)) -> (i64, i64) {
    (-dy, dx)
}
fn left((dx, dy): (i64, i64)) -> (i64, i64) {
    (dy, -dx)
}

/// The closed path starting at the top-left corner of ink pixel (x, y),
/// heading east with the ink on the right.
fn walk(b: &Bitmap, x: i64, y: i64, budget: &mut Budget) -> Result<Vec<(i64, i64)>, TraceError> {
    let start = (x, y);
    let mut p = start;
    let mut d = (1i64, 0i64);
    let mut points = Vec::new();
    loop {
        budget.spend(1)?;
        points.push(p);
        p = (p.0 + d.0, p.1 + d.1);
        if p == start {
            return Ok(points);
        }
        // The pixels ahead of the corner, to the right and the left.
        let pixel = |side: (i64, i64)| {
            b.get(
                p.0 + (d.0 + side.0 - 1).div_euclid(2),
                p.1 + (d.1 + side.1 - 1).div_euclid(2),
            )
        };
        let (r, l) = (pixel(right(d)), pixel(left(d)));
        d = match (r, l) {
            (true, true) => left(d),
            (true, false) => d,
            (false, false) => right(d),
            // A diagonal: keep the ink pixels connected.
            (false, true) => left(d),
        };
    }
}

/// Invert every pixel inside `points` (each vertical edge flips the rest of
/// its row up to the path's right edge). Returns how many pixels it
/// encloses.
fn invert_inside(
    b: &mut Bitmap,
    points: &[(i64, i64)],
    budget: &mut Budget,
) -> Result<u64, TraceError> {
    let max_x = points.iter().map(|p| p.0).max().unwrap_or(0);
    let mut area: i64 = 0;
    let n = points.len();
    for i in 0..n {
        let (x0, y0) = points[i];
        let (x1, y1) = points[(i + 1) % n];
        if x0 != x1 {
            continue;
        }
        // A vertical edge: going down or up past row min(y0, y1).
        let row = y0.min(y1);
        let width = (max_x - x0).max(0);
        budget.spend(width as usize + 1)?;
        for x in x0..max_x {
            b.flip(x as usize, row as usize);
        }
        area += if y1 > y0 { width } else { -width };
    }
    Ok(area.unsigned_abs())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bitmap(rows: &[&str]) -> Bitmap {
        let mut b = Bitmap::new(rows[0].len(), rows.len());
        for (y, r) in rows.iter().enumerate() {
            for (x, c) in r.chars().enumerate() {
                b.set(x, y, c == '#');
            }
        }
        b
    }

    #[test]
    fn a_square_is_one_outer_path_round_its_pixels() {
        let b = bitmap(&["....", ".##.", ".##.", "...."]);
        let paths = decompose(&b, 0, &mut Budget::new(u64::MAX)).unwrap_or_default();
        assert_eq!(paths.len(), 1);
        assert!(paths[0].outer);
        assert_eq!(paths[0].area, 4);
        assert_eq!(paths[0].points.len(), 8);
        assert_eq!(paths[0].points[0], (1, 1));
    }

    #[test]
    fn a_ring_has_a_hole_and_speckles_go() {
        let b = bitmap(&[
            "#.......", "........", ".#####..", ".#...#..", ".#...#..", ".#####..",
        ]);
        let paths = decompose(&b, 1, &mut Budget::new(u64::MAX)).unwrap_or_default();
        // The lone pixel (area 1) is a speckle; the ring is outer + hole.
        assert_eq!(paths.len(), 2);
        assert_eq!(paths.iter().filter(|p| p.outer).count(), 1);
        let hole = paths.iter().find(|p| !p.outer).map(|p| p.area);
        assert_eq!(hole, Some(6));
    }

    #[test]
    fn diagonal_pixels_stay_one_shape() {
        let b = bitmap(&["##..", "##..", "..##", "..##"]);
        let paths = decompose(&b, 0, &mut Budget::new(u64::MAX)).unwrap_or_default();
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].area, 8);
    }

    #[test]
    fn the_budget_stops_the_work() {
        let b = bitmap(&["####", "####"]);
        assert!(matches!(
            decompose(&b, 0, &mut Budget::new(3)),
            Err(TraceError::Budget)
        ));
    }
}
