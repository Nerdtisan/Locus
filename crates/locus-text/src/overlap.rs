// SPDX-License-Identifier: AGPL-3.0-or-later

//! Overlapping outline contours, merged (#222, SPEC §8, §9).
//!
//! Fonts may draw a glyph as contours that overlap, filled by the non-zero
//! rule. Variable fonts such as the bundled Figtree do this: `#` is two bars
//! crossing two bars. That fills correctly on screen, but a cutter follows
//! every contour, so it cuts through the glyph, and export validation
//! reports "segments intersect". Contours that really cross (each other or
//! themselves) are therefore unioned into the glyph's outline. Curves enter
//! the union as biarcs within a fine tolerance and come back as cubics.
//! Contours that don't cross keep their exact curves.

use crate::geometry::{Contour, Seg};
use locus_geom::{Aabb, Cubic, Piece, Vec2};
use locus_region::{Area, Edge, Loop, Seg as RSeg, union_all};

/// Biarc tolerance for the union, mm: well inside the export tolerance.
const FIT_TOL: f64 = 1e-4;

/// A contour flattened for the crossing test.
struct Flat {
    pts: Vec<Vec2>,
    bbox: Aabb,
}

fn flatten(c: &Contour, sag: f64) -> Flat {
    let mut pts = vec![c.start];
    for cub in c.cubics() {
        let flat = cub.flatten(sag);
        pts.extend(flat.into_iter().skip(1));
    }
    let bbox = pts
        .iter()
        .fold(Aabb::from_point(c.start), |b, p| b.include(*p));
    Flat { pts, bbox }
}

/// Whether segments `a0a1` and `b0b1` cross properly (touching ends don't
/// count).
fn cross(a0: Vec2, a1: Vec2, b0: Vec2, b1: Vec2) -> bool {
    let d = |p: Vec2, q: Vec2, r: Vec2| (q - p).cross(r - p);
    let (d1, d2) = (d(b0, b1, a0), d(b0, b1, a1));
    let (d3, d4) = (d(a0, a1, b0), d(a0, a1, b1));
    (d1 > 0.0) != (d2 > 0.0)
        && (d3 > 0.0) != (d4 > 0.0)
        && d1 != 0.0
        && d2 != 0.0
        && d3 != 0.0
        && d4 != 0.0
}

fn edges(f: &Flat) -> impl Iterator<Item = (usize, Vec2, Vec2)> + '_ {
    f.pts.windows(2).enumerate().map(|(i, w)| (i, w[0], w[1]))
}

/// Whether two boxes overlap (touching counts).
fn meets(a: &Aabb, b: &Aabb) -> bool {
    a.min.x <= b.max.x && b.min.x <= a.max.x && a.min.y <= b.max.y && b.min.y <= a.max.y
}

fn seg_box(a: Vec2, b: Vec2) -> Aabb {
    Aabb::from_points(a, b)
}

/// Two contours cross each other.
fn crosses(a: &Flat, b: &Flat) -> bool {
    if !meets(&a.bbox, &b.bbox) {
        return false;
    }
    edges(a).any(|(_, a0, a1)| {
        let ab = seg_box(a0, a1);
        meets(&ab, &b.bbox)
            && edges(b).any(|(_, b0, b1)| meets(&ab, &seg_box(b0, b1)) && cross(a0, a1, b0, b1))
    })
}

/// A contour crosses itself (non-adjacent pieces cross).
fn crosses_itself(f: &Flat) -> bool {
    let n = f.pts.len().saturating_sub(1);
    edges(f).any(|(i, a0, a1)| {
        let ab = seg_box(a0, a1);
        edges(f)
            .skip(i + 2)
            .filter(|(j, ..)| !(i == 0 && *j == n - 1))
            .any(|(_, b0, b1)| meets(&ab, &seg_box(b0, b1)) && cross(a0, a1, b0, b1))
    })
}

/// A closed contour as a region loop of lines and arcs.
fn to_loop(c: &Contour) -> Loop {
    let mut out = Vec::new();
    for cub in c.cubics() {
        for (p, back) in cub.to_pieces_oriented(FIT_TOL) {
            let seg = match p {
                Piece::Line(l) => RSeg::Line { a: l.a, b: l.b },
                Piece::Arc(a) => {
                    let s = RSeg::Arc {
                        center: a.center,
                        radius: a.radius(),
                        start: a.start_angle(),
                        sweep: a.sweep(),
                    };
                    if back { s.reversed() } else { s }
                }
            };
            out.push(Edge { seg, source: 0 });
        }
    }
    Loop { edges: out }
}

/// Cubics for an arc of `sweep` radians (either sign) from `start`.
fn arc_cubics(center: Vec2, radius: f64, start: f64, sweep: f64) -> Vec<Cubic> {
    let n = (sweep.abs() / core::f64::consts::FRAC_PI_2).ceil().max(1.0) as usize;
    let step = sweep / n as f64;
    let k = 4.0 / 3.0 * (step / 4.0).tan();
    (0..n)
        .map(|i| {
            let a0 = start + step * i as f64;
            let a1 = a0 + step;
            let p = |a: f64| center + Vec2::new(a.cos(), a.sin()) * radius;
            let t = |a: f64| Vec2::new(-a.sin(), a.cos()) * radius;
            let (p0, p1) = (p(a0), p(a1));
            Cubic::new(p0, p0 + t(a0) * k, p1 - t(a1) * k, p1)
        })
        .collect()
}

/// A region loop back as a closed contour.
fn from_loop(l: &Loop) -> Option<Contour> {
    let first = l.edges.first()?;
    let mut c = Contour::new(first.seg.start(), true);
    for e in &l.edges {
        match e.seg {
            RSeg::Line { b, .. } => c.segs.push(Seg::Line { end: b }),
            RSeg::Arc {
                center,
                radius,
                start,
                sweep,
            } => {
                for k in arc_cubics(center, radius, start, sweep) {
                    c.segs.push(Seg::Cubic {
                        c0: k.c0,
                        c1: k.c1,
                        end: k.p1,
                    });
                }
            }
        }
    }
    Some(c)
}

/// Signed area of a flattened contour (CCW positive).
fn signed_area(f: &Flat) -> f64 {
    f.pts.windows(2).map(|w| w[0].cross(w[1])).sum::<f64>() / 2.0
}

/// Merge the closed contours in `contours[from..]` that cross, keeping
/// the rest as they are. `size` (the em, mm) scales the crossing test.
pub(crate) fn merge_overlaps(contours: &mut Vec<Contour>, from: usize, size: f64) {
    let tail = &contours[from..];
    if tail.is_empty() {
        return;
    }
    let sag = (size * 2e-4).max(1e-5);
    let flats: Vec<Flat> = tail.iter().map(|c| flatten(c, sag)).collect();
    let n = tail.len();
    // Union-find over the contours that cross.
    let mut parent: Vec<usize> = (0..n).collect();
    fn root(p: &mut [usize], mut i: usize) -> usize {
        while p[i] != i {
            p[i] = p[p[i]];
            i = p[i];
        }
        i
    }
    let mut dirty = vec![false; n];
    for i in 0..n {
        if !tail[i].closed {
            continue;
        }
        if crosses_itself(&flats[i]) {
            dirty[i] = true;
        }
        for j in i + 1..n {
            if tail[j].closed && crosses(&flats[i], &flats[j]) {
                dirty[i] = true;
                dirty[j] = true;
                let (a, b) = (root(&mut parent, i), root(&mut parent, j));
                parent[a] = b;
            }
        }
    }
    if !dirty.iter().any(|d| *d) {
        return;
    }
    // Each cluster of crossing contours becomes its union.
    let mut clusters: Vec<Vec<usize>> = Vec::new();
    let mut cluster_of = vec![usize::MAX; n];
    for i in (0..n).filter(|i| dirty[*i]) {
        let r = root(&mut parent, i);
        let k = match cluster_of[r] {
            usize::MAX => {
                cluster_of[r] = clusters.len();
                clusters.push(Vec::new());
                clusters.len() - 1
            }
            k => k,
        };
        clusters[k].push(i);
    }
    let mut merged: Vec<Contour> = Vec::new();
    for members in &clusters {
        let area = Area {
            loops: members.iter().map(|i| to_loop(&tail[*i])).collect(),
        };
        let union = union_all(&[area]);
        // Keep the font's winding convention: the union's outer loops run
        // CCW; reverse them all when the font's largest contour runs CW.
        let cw = members
            .iter()
            .map(|i| signed_area(&flats[*i]))
            .max_by(|a, b| a.abs().total_cmp(&b.abs()))
            .is_some_and(|a| a < 0.0);
        for l in &union.loops {
            let l = if cw { l.reversed() } else { l.clone() };
            if let Some(c) = from_loop(&l) {
                merged.push(c);
            }
        }
    }
    let kept: Vec<Contour> = tail
        .iter()
        .enumerate()
        .filter(|(i, _)| !dirty[*i])
        .map(|(_, c)| c.clone())
        .collect();
    contours.truncate(from);
    contours.extend(kept);
    contours.extend(merged);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(x0: f64, y0: f64, s: f64) -> Contour {
        let mut c = Contour::new(Vec2::new(x0, y0), true);
        for p in [(x0 + s, y0), (x0 + s, y0 + s), (x0, y0 + s), (x0, y0)] {
            c.segs.push(Seg::Line {
                end: Vec2::new(p.0, p.1),
            });
        }
        c
    }

    #[test]
    fn crossing_contours_merge_and_separate_ones_stay() {
        // Two overlapping squares (a "+"-like overlap) and one apart.
        let mut cs = vec![
            square(0.0, 0.0, 2.0),
            square(1.0, 1.0, 2.0),
            square(10.0, 0.0, 1.0),
        ];
        merge_overlaps(&mut cs, 0, 10.0);
        assert_eq!(cs.len(), 2, "the overlapping pair is one outline now");
        let flats: Vec<Flat> = cs.iter().map(|c| flatten(c, 1e-3)).collect();
        for (i, a) in flats.iter().enumerate() {
            assert!(!crosses_itself(a));
            for b in &flats[i + 1..] {
                assert!(!crosses(a, b));
            }
        }
        // The merged outline's area is the union's: 4 + 4 - 1.
        let areas: Vec<f64> = flats.iter().map(signed_area).map(f64::abs).collect();
        assert!(areas.iter().any(|a| (a - 7.0).abs() < 1e-6), "{areas:?}");
    }

    #[test]
    fn a_hole_that_crosses_nothing_is_left_exact() {
        let mut outer = square(0.0, 0.0, 4.0);
        // A clockwise hole inside.
        let mut hole = Contour::new(Vec2::new(1.0, 1.0), true);
        for p in [(1.0, 2.0), (2.0, 2.0), (2.0, 1.0), (1.0, 1.0)] {
            hole.segs.push(Seg::Line {
                end: Vec2::new(p.0, p.1),
            });
        }
        let before = vec![outer.clone(), hole.clone()];
        let mut cs = before.clone();
        merge_overlaps(&mut cs, 0, 10.0);
        assert_eq!(cs, before);
        outer.closed = false;
        let mut open = vec![outer, hole];
        merge_overlaps(&mut open, 0, 10.0);
        assert_eq!(open.len(), 2, "open strokes are never merged");
    }
}
