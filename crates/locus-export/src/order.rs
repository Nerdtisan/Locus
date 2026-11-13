// SPDX-License-Identifier: AGPL-3.0-or-later

//! Cut ordering and path optimisation (SPEC §8 Optimisation, #129).
//!
//! 1. Contained contours are cut before the contours around them.
//! 2. Within that constraint, nearest-neighbour travel ordering; paths are
//!    reversed (open) or re-started at the closest vertex (closed) so the pen
//!    lifts as little as possible. Paths that join end to end were already
//!    chained by the collection stage.

use crate::pipeline::{ExportLayer, ExportModel, ExportPath, PathSeg, Shape};
use locus_geom::Vec2;
use std::f64::consts::PI;

/// Reorder every layer in place. The pen starts at the origin and carries on
/// from layer to layer, in export order.
pub fn order(model: &mut ExportModel) {
    let mut pen = Vec2::ZERO;
    for layer in &mut model.layers {
        order_layer(layer, &mut pen);
    }
}

/// Total pen-up distance if the model were drawn in its current order.
#[must_use]
pub fn travel_length(model: &ExportModel) -> f64 {
    let mut pen = Vec2::ZERO;
    let mut total = 0.0;
    for p in model.layers.iter().flat_map(|l| &l.paths) {
        total += pen.distance(p.start());
        pen = p.end();
    }
    total
}

fn order_layer(layer: &mut ExportLayer, pen: &mut Vec2) {
    let paths = std::mem::take(&mut layer.paths);
    let polys: Vec<Option<Vec<Vec2>>> =
        paths.iter().map(|p| p.closed.then(|| polygon(p))).collect();
    // Depth = number of closed contours that contain the path.
    let depth: Vec<usize> = (0..paths.len())
        .map(|i| {
            let probe = probe_point(&paths[i]);
            polys
                .iter()
                .enumerate()
                .filter(|(j, poly)| {
                    *j != i && poly.as_ref().is_some_and(|poly| contains(poly, probe))
                })
                .count()
        })
        .collect();

    let mut remaining: Vec<(usize, ExportPath)> = depth.into_iter().zip(paths).collect();
    let mut out = Vec::with_capacity(remaining.len());
    while !remaining.is_empty() {
        // Deepest level first; nearest neighbour inside the level.
        let level = remaining.iter().map(|(d, _)| *d).max().unwrap_or(0);
        let (idx, _) = remaining
            .iter()
            .enumerate()
            .filter(|(_, (d, _))| *d == level)
            .map(|(k, (_, p))| (k, entry_cost(p, *pen)))
            .fold(
                (0, f64::INFINITY),
                |best, (k, c)| if c < best.1 { (k, c) } else { best },
            );
        let (_, path) = remaining.remove(idx);
        let path = orient(path, *pen);
        *pen = path.end();
        out.push(path);
    }
    layer.paths = out;
}

/// Cheapest way to start this path from `pen`.
fn entry_cost(p: &ExportPath, pen: Vec2) -> f64 {
    if p.closed {
        p.segs
            .iter()
            .map(|s| s.start().distance(pen))
            .fold(f64::INFINITY, f64::min)
    } else {
        pen.distance(p.start()).min(pen.distance(p.end()))
    }
}

fn orient(mut p: ExportPath, pen: Vec2) -> ExportPath {
    if p.closed {
        let best = p
            .segs
            .iter()
            .enumerate()
            .map(|(k, s)| (k, s.start().distance(pen)))
            .fold(
                (0, f64::INFINITY),
                |b, (k, d)| if d < b.1 { (k, d) } else { b },
            )
            .0;
        p.segs.rotate_left(best);
    } else if pen.distance(p.end()) < pen.distance(p.start()) {
        p.segs.reverse();
        for s in &mut p.segs {
            s.reversed = !s.reversed;
        }
    }
    p
}

fn probe_point(p: &ExportPath) -> Vec2 {
    match p.segs.first() {
        Some(s) => match s.shape {
            Shape::Line(l) => l.eval(0.5),
            Shape::Arc(a) => a.eval(0.5),
            Shape::Circle(_) => s.start(),
            Shape::Cubic(c) => c.eval(0.5),
        },
        None => Vec2::ZERO,
    }
}

/// Polyline outline of a closed path (arcs sampled every ~10°).
fn polygon(p: &ExportPath) -> Vec<Vec2> {
    let mut pts = Vec::new();
    for s in &p.segs {
        sample(s, &mut pts);
    }
    pts
}

fn sample(s: &PathSeg, pts: &mut Vec<Vec2>) {
    match s.shape {
        Shape::Line(_) => pts.push(s.start()),
        Shape::Arc(a) => {
            let n = ((a.sweep() / (PI / 18.0)).ceil() as usize).max(2);
            for k in 0..n {
                let t = k as f64 / n as f64;
                pts.push(a.eval(if s.reversed { 1.0 - t } else { t }));
            }
        }
        Shape::Circle(c) => {
            for k in 0..72 {
                pts.push(c.eval(f64::from(k) * PI / 36.0));
            }
        }
        Shape::Cubic(c) => {
            // Points in travel order, without the end (the next segment's start).
            let mut flat = c.flatten(0.01);
            if s.reversed {
                flat.reverse();
            }
            flat.pop();
            pts.extend(flat);
        }
    }
}

/// Even-odd ray cast.
fn contains(poly: &[Vec2], p: Vec2) -> bool {
    let mut inside = false;
    let mut j = poly.len().wrapping_sub(1);
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[j]);
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            inside = !inside;
        }
        j = i;
    }
    inside
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::testutil::*;
    use locus_doc::OperationKind;

    fn first_x(p: &ExportPath) -> f64 {
        p.bbox().unwrap().min.x
    }

    #[test]
    fn inner_contours_are_cut_before_the_outer() {
        // Outer listed first; a hole and a second, disjoint part follow.
        let mut segs = rect(1, 0.0, 0.0, 100.0, 100.0);
        segs.extend(rect(5, 40.0, 40.0, 60.0, 60.0));
        segs.extend(rect(9, 200.0, 0.0, 220.0, 20.0));
        let mut m = ExportModel {
            layers: vec![layer(OperationKind::Cut, segs)],
            ..Default::default()
        };
        order(&mut m);
        let xs: Vec<f64> = m.layers[0].paths.iter().map(first_x).collect();
        let outer = xs.iter().position(|&x| x == 0.0).unwrap();
        let hole = xs.iter().position(|&x| x == 40.0).unwrap();
        assert!(hole < outer, "{xs:?}");
    }

    #[test]
    fn nested_three_deep_goes_inside_out() {
        let mut segs = rect(1, 0.0, 0.0, 100.0, 100.0);
        segs.extend(rect(5, 10.0, 10.0, 90.0, 90.0));
        segs.extend(rect(9, 20.0, 20.0, 80.0, 80.0));
        let mut m = ExportModel {
            layers: vec![layer(OperationKind::Cut, segs)],
            ..Default::default()
        };
        order(&mut m);
        let xs: Vec<f64> = m.layers[0].paths.iter().map(first_x).collect();
        assert_eq!(xs, vec![20.0, 10.0, 0.0]);
    }

    #[test]
    fn travel_is_shorter_than_input_order() {
        // Squares along x, listed in a zig-zag order.
        let mut segs = Vec::new();
        for (k, x) in [0.0, 500.0, 50.0, 450.0, 100.0, 400.0].iter().enumerate() {
            segs.extend(rect(1 + 4 * k as u64, *x, 0.0, x + 10.0, 10.0));
        }
        let mut m = ExportModel {
            layers: vec![layer(OperationKind::Cut, segs)],
            ..Default::default()
        };
        let before = travel_length(&m);
        order(&mut m);
        let after = travel_length(&m);
        assert!(after < before * 0.5, "{after} vs {before}");
    }

    #[test]
    fn open_paths_are_reversed_when_that_is_closer() {
        let segs = vec![
            seg(1, (0.0, 0.0), (1.0, 0.0)),
            seg(2, (100.0, 0.0), (50.0, 0.0)),
        ];
        let mut m = ExportModel {
            layers: vec![layer(OperationKind::Draw, segs)],
            ..Default::default()
        };
        order(&mut m);
        let p = &m.layers[0].paths;
        assert_eq!(p[0].start(), Vec2::new(0.0, 0.0));
        assert_eq!(p[1].start(), Vec2::new(50.0, 0.0));
        assert_eq!(p[1].end(), Vec2::new(100.0, 0.0));
    }
}
