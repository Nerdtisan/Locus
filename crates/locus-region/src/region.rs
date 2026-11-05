// SPDX-License-Identifier: AGPL-3.0-or-later

//! Automatic closed-profile (region) detection (SPEC §6.3, #121), as Fusion
//! does it: the faces of the planar arrangement of the sketch's curves, with
//! holes nested inside them. Regions are derived data, recomputed after each
//! solve; construction geometry is left out by the caller.

use crate::arrange::{Arrangement, GEdge, arrange, in_dir, merge_carriers, out_dir};
use crate::seg::{Area, Edge, Loop, Seg};
use core::f64::consts::TAU;
use locus_geom::{Arc, Circle, Line, Vec2};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Faces smaller than this (mm²) are slivers of the arrangement, not regions.
const MIN_AREA: f64 = 1e-9;

/// A sketch curve offered to region detection.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Shape {
    Line(Line),
    Circle(Circle),
    Arc(Arc),
}

/// A curve and the entity it belongs to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Curve {
    pub key: u64,
    pub shape: Shape,
}

impl Curve {
    fn to_edge(self) -> Option<Edge> {
        let seg = match self.shape {
            Shape::Line(l) => Seg::Line { a: l.a, b: l.b },
            Shape::Circle(c) => Seg::Arc {
                center: c.center,
                radius: c.radius,
                start: 0.0,
                sweep: TAU,
            },
            Shape::Arc(a) => {
                if a.is_degenerate() {
                    return None;
                }
                Seg::Arc {
                    center: a.center,
                    radius: a.radius(),
                    start: a.start_angle(),
                    sweep: a.sweep(),
                }
            }
        };
        Some(Edge {
            seg,
            source: self.key,
        })
    }
}

/// Stable identity of a region: the entities on its outer boundary (sorted),
/// plus an index telling apart regions bounded by the same entities (e.g.
/// the two halves of a circle cut by a line), ordered by centroid x then y.
/// It survives any edit that keeps the same entities around the region, and
/// adding holes inside it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct RegionKey {
    pub bounds: Vec<u64>,
    pub index: u32,
}

/// A closed region: an outer loop (CCW) and the holes inside it (CW).
#[derive(Debug, Clone, PartialEq)]
pub struct Region {
    pub key: RegionKey,
    pub outer: Loop,
    pub holes: Vec<Loop>,
}

impl Region {
    /// Net area in mm² (holes removed).
    #[must_use]
    pub fn area(&self) -> f64 {
        self.outer.area() + self.holes.iter().map(Loop::area).sum::<f64>()
    }

    /// Whether `q` lies inside the region (not in a hole).
    #[must_use]
    pub fn contains(&self, q: Vec2) -> bool {
        self.outer.winding(q) != 0 && self.holes.iter().all(|h| h.winding(q) == 0)
    }

    /// The region as a filled area, for booleans.
    #[must_use]
    pub fn to_area(&self) -> Area {
        let mut loops = vec![self.outer.clone()];
        loops.extend(self.holes.iter().cloned());
        Area { loops }
    }
}

/// Every region of the sketch, sorted by key. Faces are disjoint, so at most
/// one region contains any point off the curves.
#[must_use]
pub fn detect(curves: &[Curve]) -> Vec<Region> {
    let input: Vec<Edge> = curves.iter().filter_map(|c| c.to_edge()).collect();
    let mut arr = arrange(&input);
    prune(&mut arr);
    let comp = components(&arr);
    let cycles = faces(&arr);

    // Bounded faces are CCW cycles; each CW cycle is the outside of one
    // connected component and becomes a hole of the smallest face of another
    // component around it (or bounds the unbounded face).
    let mut regions: Vec<(Loop, usize, Vec<Loop>)> = Vec::new();
    let mut outsides: Vec<(Loop, usize)> = Vec::new();
    for c in cycles {
        let lp = Loop {
            edges: c.iter().map(|&h| half_edge(&arr, h)).collect(),
        };
        let a = lp.area();
        let cid = comp[arr.edges[c[0] / 2].v0];
        if a > MIN_AREA {
            regions.push((lp, cid, Vec::new()));
        } else if a < -MIN_AREA {
            outsides.push((lp, cid));
        }
    }
    for (lp, cid) in outsides {
        let probe = lp.edges[0].seg.start();
        let host = regions
            .iter()
            .enumerate()
            .filter(|(_, (r, rc, _))| *rc != cid && r.winding(probe) != 0)
            .min_by(|(_, (a, ..)), (_, (b, ..))| a.area().total_cmp(&b.area()))
            .map(|(i, _)| i);
        if let Some(i) = host {
            regions[i].2.push(lp);
        }
    }

    let mut out: Vec<Region> = regions
        .into_iter()
        .map(|(outer, _, holes)| {
            let outer = Loop {
                edges: merge_carriers(outer.edges, true),
            };
            let holes = holes
                .into_iter()
                .map(|h| Loop {
                    edges: merge_carriers(h.edges, true),
                })
                .collect();
            let mut bounds: Vec<u64> = outer.edges.iter().map(|e| e.source).collect();
            bounds.sort_unstable();
            bounds.dedup();
            Region {
                key: RegionKey { bounds, index: 0 },
                outer,
                holes,
            }
        })
        .collect();
    assign_indices(&mut out);
    out.sort_by(|a, b| a.key.cmp(&b.key));
    out
}

fn assign_indices(regions: &mut [Region]) {
    let mut groups: BTreeMap<Vec<u64>, Vec<usize>> = BTreeMap::new();
    for (i, r) in regions.iter().enumerate() {
        groups.entry(r.key.bounds.clone()).or_default().push(i);
    }
    for ids in groups.values_mut() {
        if ids.len() < 2 {
            continue;
        }
        let cent: Vec<Vec2> = regions.iter().map(|r| centroid(&r.outer)).collect();
        ids.sort_by(|&a, &b| {
            cent[a]
                .x
                .total_cmp(&cent[b].x)
                .then(cent[a].y.total_cmp(&cent[b].y))
        });
        for (k, &i) in ids.iter().enumerate() {
            regions[i].key.index = u32::try_from(k).unwrap_or(u32::MAX);
        }
    }
}

/// Centroid of a loop's flattened outline (enough to order regions).
fn centroid(l: &Loop) -> Vec2 {
    let pts = l.flatten(1e-3);
    let (mut a, mut c) = (0.0, Vec2::ZERO);
    for i in 0..pts.len() {
        let (p, q) = (pts[i], pts[(i + 1) % pts.len()]);
        let k = p.cross(q);
        a += k;
        c += (p + q) * k;
    }
    if a.abs() > 0.0 {
        c / (3.0 * a)
    } else {
        pts.first().copied().unwrap_or(Vec2::ZERO)
    }
}

/// Half-edge `h` of the arrangement: edge `h / 2`, reversed when `h` is odd.
fn half_edge(arr: &Arrangement, h: usize) -> Edge {
    let e = arr.edges[h / 2].edge;
    if h.is_multiple_of(2) {
        e
    } else {
        Edge {
            seg: e.seg.reversed(),
            source: e.source,
        }
    }
}

fn head(e: &GEdge, h: usize) -> usize {
    if h.is_multiple_of(2) { e.v1 } else { e.v0 }
}

fn tail(e: &GEdge, h: usize) -> usize {
    if h.is_multiple_of(2) { e.v0 } else { e.v1 }
}

/// Removes dangling edges (an end of degree one) and bridges (both sides in
/// the same face), repeatedly, so every edge left bounds two different faces.
fn prune(arr: &mut Arrangement) {
    loop {
        let mut degree = vec![0usize; arr.verts.len()];
        for e in &arr.edges {
            degree[e.v0] += 1;
            degree[e.v1] += 1;
        }
        let before = arr.edges.len();
        arr.edges.retain(|e| degree[e.v0] > 1 && degree[e.v1] > 1);
        if arr.edges.len() != before {
            continue;
        }
        let cycles = faces(arr);
        let mut face_of = vec![0usize; arr.edges.len() * 2];
        for (f, c) in cycles.iter().enumerate() {
            for &h in c {
                face_of[h] = f;
            }
        }
        let keep: Vec<bool> = (0..arr.edges.len())
            .map(|e| face_of[2 * e] != face_of[2 * e + 1])
            .collect();
        if keep.iter().all(|&k| k) {
            return;
        }
        let mut k = keep.into_iter();
        arr.edges.retain(|_| k.next().unwrap_or(true));
    }
}

/// Connected component of every vertex.
fn components(arr: &Arrangement) -> Vec<usize> {
    let mut parent: Vec<usize> = (0..arr.verts.len()).collect();
    fn find(p: &mut [usize], mut x: usize) -> usize {
        while p[x] != x {
            p[x] = p[p[x]];
            x = p[x];
        }
        x
    }
    for e in &arr.edges {
        let (a, b) = (find(&mut parent, e.v0), find(&mut parent, e.v1));
        parent[a] = b;
    }
    (0..arr.verts.len()).map(|v| find(&mut parent, v)).collect()
}

/// Traces every face cycle (half-edge ids) with the face on the left: at each
/// vertex, leave by the first outgoing half-edge clockwise from the way back
/// (the twin is taken only at a dead end).
fn faces(arr: &Arrangement) -> Vec<Vec<usize>> {
    let n = arr.edges.len() * 2;
    let mut out: Vec<Vec<usize>> = vec![Vec::new(); arr.verts.len()];
    for h in 0..n {
        out[tail(&arr.edges[h / 2], h)].push(h);
    }
    let dirs: Vec<(Vec2, Vec2)> = (0..n)
        .map(|h| {
            let s = half_edge(arr, h).seg;
            (out_dir(&s), in_dir(&s))
        })
        .collect();
    let next = |h: usize| -> usize {
        let back = -dirs[h].1;
        let twin = h ^ 1;
        out[head(&arr.edges[h / 2], h)]
            .iter()
            .copied()
            .max_by(|&x, &y| {
                ccw_from(back, dirs[x].0, x == twin).total_cmp(&ccw_from(
                    back,
                    dirs[y].0,
                    y == twin,
                ))
            })
            .unwrap_or(twin)
    };
    let mut seen = vec![false; n];
    let mut cycles = Vec::new();
    for start in 0..n {
        if seen[start] {
            continue;
        }
        let mut c = Vec::new();
        let mut h = start;
        // Every half-edge has exactly one successor, so this closes; the
        // bound only guards against a logic error.
        while !seen[h] && c.len() <= n {
            seen[h] = true;
            c.push(h);
            h = next(h);
        }
        cycles.push(c);
    }
    cycles
}

/// Counter-clockwise angle from `from` to `to` in [0, 2π). The largest is
/// the first edge clockwise from `from`; the twin (straight back) ranks last.
fn ccw_from(from: Vec2, to: Vec2, twin: bool) -> f64 {
    if twin {
        return -1.0;
    }
    from.cross(to).atan2(from.dot(to)).rem_euclid(TAU)
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::f64::consts::PI;

    fn v(x: f64, y: f64) -> Vec2 {
        Vec2::new(x, y)
    }

    fn line(key: u64, a: Vec2, b: Vec2) -> Curve {
        Curve {
            key,
            shape: Shape::Line(Line::new(a, b)),
        }
    }

    fn circle(key: u64, c: Vec2, r: f64) -> Curve {
        Curve {
            key,
            shape: Shape::Circle(Circle::new(c, r)),
        }
    }

    /// Rectangle as four lines keyed `k..k+4`.
    fn rect(k: u64, x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<Curve> {
        let (a, b, c, d) = (v(x0, y0), v(x1, y0), v(x1, y1), v(x0, y1));
        vec![
            line(k, a, b),
            line(k + 1, b, c),
            line(k + 2, c, d),
            line(k + 3, d, a),
        ]
    }

    fn areas(rs: &[Region]) -> Vec<f64> {
        let mut a: Vec<f64> = rs.iter().map(Region::area).collect();
        a.sort_by(f64::total_cmp);
        a
    }

    fn close(a: &[f64], b: &[f64]) -> bool {
        a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-6)
    }

    #[test]
    fn rectangle() {
        let rs = detect(&rect(1, 0.0, 0.0, 4.0, 3.0));
        assert_eq!(rs.len(), 1);
        assert!((rs[0].area() - 12.0).abs() < 1e-12);
        assert_eq!(
            rs[0].key,
            RegionKey {
                bounds: vec![1, 2, 3, 4],
                index: 0
            }
        );
        assert!(rs[0].outer.area() > 0.0);
        assert!(rs[0].contains(v(1.0, 1.0)) && !rs[0].contains(v(5.0, 1.0)));
    }

    #[test]
    fn circle_alone_is_one_region_with_one_arc() {
        let rs = detect(&[circle(7, v(0.0, 0.0), 2.0)]);
        assert_eq!(rs.len(), 1);
        assert_eq!(rs[0].outer.edges.len(), 1);
        assert!((rs[0].area() - PI * 4.0).abs() < 1e-9);
    }

    #[test]
    fn slot() {
        // Two lines and two semicircles sharing endpoints.
        let (r, a0, a1) = (1.0, v(0.0, 0.0), v(5.0, 0.0));
        let arc = |k, c: Vec2, s: Vec2, e: Vec2| Curve {
            key: k,
            shape: Shape::Arc(Arc::new(c, s, e)),
        };
        let cs = vec![
            line(1, v(0.0, -r), v(5.0, -r)),
            arc(2, a1, v(5.0, -r), v(5.0, r)),
            line(3, v(5.0, r), v(0.0, r)),
            arc(4, a0, v(0.0, r), v(0.0, -r)),
        ];
        let rs = detect(&cs);
        assert_eq!(rs.len(), 1);
        assert!(close(&areas(&rs), &[10.0 + PI]));
        assert_eq!(
            rs[0].outer.edges.iter().filter(|e| e.seg.is_arc()).count(),
            2
        );
    }

    #[test]
    fn overlapping_shapes_make_three_faces() {
        let mut cs = rect(1, 0.0, 0.0, 4.0, 4.0);
        cs.extend(rect(5, 2.0, 2.0, 6.0, 6.0));
        assert!(close(&areas(&detect(&cs)), &[4.0, 12.0, 12.0]));
        // Rectangle and circle.
        let mut cs = rect(1, 0.0, 0.0, 4.0, 4.0);
        cs.push(circle(9, v(4.0, 4.0), 2.0));
        let rs = detect(&cs);
        assert!(close(&areas(&rs), &[PI, 3.0 * PI, 16.0 - PI]));
    }

    #[test]
    fn t_junction_splits_a_rectangle() {
        let mut cs = rect(1, 0.0, 0.0, 4.0, 2.0);
        cs.push(line(9, v(1.0, 0.0), v(1.0, 2.0)));
        assert!(close(&areas(&detect(&cs)), &[2.0, 6.0]));
    }

    #[test]
    fn touching_corners_are_separate_regions() {
        let mut cs = rect(1, 0.0, 0.0, 2.0, 2.0);
        cs.extend(rect(5, 2.0, 2.0, 5.0, 5.0));
        assert!(close(&areas(&detect(&cs)), &[4.0, 9.0]));
    }

    #[test]
    fn shared_edge_from_two_rectangles() {
        let mut cs = rect(1, 0.0, 0.0, 2.0, 2.0);
        cs.extend(rect(5, 2.0, 0.0, 5.0, 2.0));
        assert!(close(&areas(&detect(&cs)), &[4.0, 6.0]));
    }

    #[test]
    fn nested_holes() {
        // Plate with a circular hole holding an island with its own hole.
        let mut cs = rect(1, 0.0, 0.0, 20.0, 20.0);
        cs.push(circle(10, v(10.0, 10.0), 8.0));
        cs.extend(rect(20, 7.0, 7.0, 13.0, 13.0));
        cs.push(circle(30, v(10.0, 10.0), 1.0));
        let rs = detect(&cs);
        let a = areas(&rs);
        assert!(
            close(&a, &[PI, 36.0 - PI, 64.0 * PI - 36.0, 400.0 - 64.0 * PI]),
            "{a:?}"
        );
        let plate = rs
            .iter()
            .find(|r| r.key.bounds == vec![1, 2, 3, 4])
            .expect("plate");
        assert_eq!(plate.holes.len(), 1);
        assert!(!plate.contains(v(10.0, 10.0)) && plate.contains(v(1.0, 1.0)));
        // Exactly one region contains each probe.
        for q in [v(1.0, 1.0), v(10.0, 4.0), v(8.0, 8.0), v(10.0, 10.0)] {
            assert_eq!(rs.iter().filter(|r| r.contains(q)).count(), 1);
        }
    }

    #[test]
    fn dangling_and_bridging_lines_are_ignored() {
        let mut cs = rect(1, 0.0, 0.0, 4.0, 4.0);
        cs.push(line(9, v(2.0, 2.0), v(3.0, 3.0))); // floating inside
        cs.push(line(10, v(4.0, 2.0), v(6.0, 2.0))); // antenna outside
        cs.extend(rect(20, 6.0, 0.0, 8.0, 4.0)); // bridged by the antenna
        let rs = detect(&cs);
        assert!(close(&areas(&rs), &[8.0, 16.0]));
        assert!(rs.iter().all(|r| r.holes.is_empty()));
    }

    #[test]
    fn open_shapes_make_no_region() {
        assert!(
            detect(&[
                line(1, v(0.0, 0.0), v(1.0, 0.0)),
                line(2, v(1.0, 0.0), v(1.0, 1.0))
            ])
            .is_empty()
        );
        assert!(detect(&[]).is_empty());
    }

    #[test]
    fn circle_cut_by_a_line_has_two_keyed_halves() {
        let cs = vec![
            circle(1, v(0.0, 0.0), 2.0),
            line(2, v(0.0, -3.0), v(0.0, 3.0)),
        ];
        let rs = detect(&cs);
        assert_eq!(rs.len(), 2);
        assert_eq!(
            rs[0].key,
            RegionKey {
                bounds: vec![1, 2],
                index: 0
            }
        );
        assert_eq!(
            rs[1].key,
            RegionKey {
                bounds: vec![1, 2],
                index: 1
            }
        );
        // Index 0 is the left half (smaller centroid x).
        assert!(rs[0].contains(v(-1.0, 0.0)));
    }

    #[test]
    fn keys_survive_moving_geometry() {
        let k = |dx: f64| {
            let mut cs = rect(1, 0.0, 0.0, 4.0, 4.0);
            cs.push(circle(9, v(4.0 + dx, 4.0), 2.0));
            detect(&cs).into_iter().map(|r| r.key).collect::<Vec<_>>()
        };
        assert_eq!(k(0.0), k(0.3));
    }

    #[test]
    fn near_coincident_edges_within_solver_tolerance() {
        let mut cs = rect(1, 0.0, 0.0, 2.0, 2.0);
        cs.extend(rect(5, 2.0 + 1e-7, 0.0, 5.0, 2.0));
        assert!(close(&areas(&detect(&cs)), &[4.0, 6.0]));
    }

    #[test]
    fn tangent_circles() {
        let cs = vec![
            circle(1, v(0.0, 0.0), 1.0),
            circle(2, v(2.0, 0.0), 1.0),
            circle(3, v(0.5, 0.0), 0.5),
        ];
        // The small circle is internally tangent to the first.
        assert!(close(&areas(&detect(&cs)), &[0.25 * PI, 0.75 * PI, PI]));
    }

    #[test]
    fn bad_input_never_panics() {
        let cs = vec![
            line(1, v(f64::NAN, 0.0), v(1.0, 1.0)),
            circle(2, v(0.0, 0.0), 0.0),
            circle(3, v(0.0, 0.0), f64::INFINITY),
            line(4, v(1.0, 1.0), v(1.0, 1.0)),
        ];
        assert!(detect(&cs).is_empty());
    }
}
