// SPDX-License-Identifier: AGPL-3.0-or-later

//! Operation transforms (SPEC §8 "Apply operation transforms", §7): kerf
//! compensation on Cut layers, blade offset on drag-knife layers and hatch
//! fill on Engrave layers. Each rewrites an [`ExportLayer`]'s paths; the
//! document is never touched (SPEC §11).
//!
//! Offsets use the Locus-owned engine in `locus-region` (ADR 0010), so arcs
//! come out as arcs.

use crate::pipeline::{ExportLayer, ExportModel, ExportPath, PathSeg, Shape};
use core::f64::consts::TAU;
use locus_doc::{Compensation, EntityId, Operation};
use locus_geom::{Arc, Circle, EXPORT_TOL, Line, Vec2};
use locus_region::{Area, Edge, Join, Loop, Seg};

/// Applies every layer's operation transform.
pub fn apply_operations(model: &mut ExportModel) {
    for layer in &mut model.layers {
        match layer.operation {
            Operation::Cut(p) if p.compensation != Compensation::None && p.kerf_width > 0.0 => {
                kerf(layer, p.kerf_width / 2.0, p.compensation);
            }
            Operation::DragKnife(p) if p.blade_offset > 0.0 => blade(layer, p.blade_offset),
            Operation::Engrave(p) if p.hatch_spacing > 0.0 => {
                hatch(layer, p.hatch_spacing, p.hatch_angle, p.crosshatch);
            }
            _ => {}
        }
    }
}

// ── Conversions ─────────────────────────────────────────────────────────

/// A path segment as region pieces in travel order (a Bézier as its biarcs).
fn segs_of(s: &PathSeg) -> Vec<Seg> {
    let one = |shape: &Shape| -> Option<Seg> {
        match *shape {
            Shape::Line(l) => Some(Seg::Line { a: l.a, b: l.b }),
            Shape::Arc(a) => Some(Seg::Arc {
                center: a.center,
                radius: a.radius(),
                start: a.start_angle(),
                sweep: a.sweep(),
            }),
            Shape::Circle(c) => Some(Seg::Arc {
                center: c.center,
                radius: c.radius,
                start: 0.0,
                sweep: TAU,
            }),
            Shape::Cubic(_) => None,
        }
    };
    let mut out: Vec<Seg> = s.shape.pieces().iter().filter_map(one).collect();
    if s.reversed {
        out.reverse();
        for p in &mut out {
            *p = p.reversed();
        }
    }
    out
}

/// A closed path as a loop. Gaps the chaining allowed (up to `EXPORT_TOL`)
/// are bridged with short lines so the loop is exactly closed.
fn loop_of(p: &ExportPath) -> Loop {
    let mut edges: Vec<Edge> = Vec::new();
    for s in &p.segs {
        for seg in segs_of(s) {
            if let Some(last) = edges.last()
                && last.seg.end().distance(seg.start()) > locus_region::SNAP
            {
                edges.push(Edge {
                    seg: Seg::Line {
                        a: last.seg.end(),
                        b: seg.start(),
                    },
                    source: s.entity.get(),
                });
            }
            edges.push(Edge {
                seg,
                source: s.entity.get(),
            });
        }
    }
    if let (Some(first), Some(last)) = (edges.first(), edges.last()) {
        let (a, b) = (last.seg.end(), first.seg.start());
        if a.distance(b) > locus_region::SNAP {
            let source = last.source;
            edges.push(Edge {
                seg: Seg::Line { a, b },
                source,
            });
        }
    }
    Loop { edges }
}

/// A region piece back as an export segment.
fn path_seg(e: &Edge) -> PathSeg {
    let entity = EntityId::new(e.source);
    match e.seg {
        Seg::Line { a, b } => PathSeg {
            entity,
            shape: Shape::Line(Line::new(a, b)),
            reversed: false,
        },
        Seg::Arc {
            center,
            radius,
            sweep,
            ..
        } if sweep.abs() >= TAU - 1e-12 => PathSeg {
            entity,
            shape: Shape::Circle(Circle::new(center, radius)),
            reversed: sweep < 0.0,
        },
        Seg::Arc { center, sweep, .. } => {
            let (s, t) = (e.seg.start(), e.seg.end());
            if sweep >= 0.0 {
                PathSeg {
                    entity,
                    shape: Shape::Arc(Arc::new(center, s, t)),
                    reversed: false,
                }
            } else {
                PathSeg {
                    entity,
                    shape: Shape::Arc(Arc::new(center, t, s)),
                    reversed: true,
                }
            }
        }
    }
}

fn path_of(l: &Loop) -> ExportPath {
    ExportPath {
        segs: l.edges.iter().map(path_seg).collect(),
        closed: true,
    }
}

/// A point just inside the path's first segment, for nesting tests: its
/// midpoint (off every other path unless they overlap).
fn probe(l: &Loop) -> Option<Vec2> {
    l.edges.first().map(|e| e.seg.at(0.5))
}

/// How many other closed loops each loop lies inside (0 = outer contour,
/// 1 = hole, 2 = part inside a hole, …).
fn depths(loops: &[Loop]) -> Vec<usize> {
    loops
        .iter()
        .enumerate()
        .map(|(i, l)| {
            probe(l).map_or(0, |q| {
                loops
                    .iter()
                    .enumerate()
                    .filter(|(j, o)| *j != i && o.winding(q) != 0)
                    .count()
            })
        })
        .collect()
}

// ── Kerf (#173) ─────────────────────────────────────────────────────────

/// Kerf compensation (SPEC §7 Cut): each closed path is offset on its own by
/// half the kerf, with round joins (the beam is round). With `Outside`
/// compensation the parts are the material inside outer contours, so outer
/// contours (even nesting depth) grow and holes (odd depth) shrink; `Inside`
/// is the reverse. Open paths are left alone (validation reports them). A
/// hole that closes up entirely disappears.
fn kerf(layer: &mut ExportLayer, half: f64, comp: Compensation) {
    let (closed, open): (Vec<ExportPath>, Vec<ExportPath>) =
        layer.paths.drain(..).partition(|p| p.closed);
    let loops: Vec<Loop> = closed.iter().map(loop_of).collect();
    let depth = depths(&loops);
    let mut out = open;
    for (l, d) in loops.iter().zip(depth) {
        let ccw = if l.area() < 0.0 {
            l.reversed()
        } else {
            l.clone()
        };
        let grow = (d % 2 == 0) == (comp == Compensation::Outside);
        let r = locus_region::offset(
            &Area { loops: vec![ccw] },
            if grow { half } else { -half },
            Join::Round,
        );
        out.extend(r.loops.iter().map(path_of));
    }
    layer.paths = out;
}

// ── Drag-knife blade offset (#174) ──────────────────────────────────────

/// Turns sharper than this (radians) get a swivel arc; gentler ones are
/// joined straight, as the blade follows them on its own. 10° is a common
/// cutter-driver default.
pub const SWIVEL_MIN: f64 = 10.0 * core::f64::consts::PI / 180.0;

/// Blade-offset compensation (SPEC §7 drag-knife). A drag knife's tip trails
/// the holder by the blade offset `o`, so the holder is driven ahead of the
/// tip along the direction of travel: lines shift by `o` along themselves,
/// arcs ride on a circle of radius √(r² + o²) turned ahead by atan(o/r), and
/// at turns sharper than [`SWIVEL_MIN`] the holder swivels on an arc of
/// radius `o` about the corner, which brings the tip round to the new
/// direction and leaves a sharp corner in the vinyl.
fn blade(layer: &mut ExportLayer, o: f64) {
    for p in &mut layer.paths {
        *p = blade_path(p, o);
    }
}

/// The holder's path for one piece.
fn ahead(seg: &Seg, o: f64) -> Seg {
    match *seg {
        Seg::Line { a, b } => {
            let u = (b - a).normalized(0.0).unwrap_or(Vec2::ZERO) * o;
            Seg::Line { a: a + u, b: b + u }
        }
        Seg::Arc {
            center,
            radius,
            start,
            sweep,
        } => Seg::Arc {
            center,
            radius: radius.hypot(o),
            start: start + (o / radius).atan() * sweep.signum(),
            sweep,
        },
    }
}

fn blade_path(p: &ExportPath, o: f64) -> ExportPath {
    let pieces: Vec<(u64, Seg)> = p
        .segs
        .iter()
        .flat_map(|s| segs_of(s).into_iter().map(move |g| (s.entity.get(), g)))
        .collect();
    let n = pieces.len();
    let mut out: Vec<Edge> = Vec::new();
    for (i, &(source, seg)) in pieces.iter().enumerate() {
        let holder = ahead(&seg, o);
        if i > 0 || p.closed {
            let prev = pieces[(i + n - 1) % n].1;
            let corner = seg.start();
            let (t1, t2) = (prev.tangent(1.0), seg.tangent(0.0));
            let turn = t1.cross(t2).atan2(t1.dot(t2));
            let from = out
                .last()
                .map_or_else(|| ahead(&prev, o).end(), |e| e.seg.end());
            if turn.abs() > SWIVEL_MIN {
                out.push(Edge {
                    seg: Seg::Arc {
                        center: corner,
                        radius: o,
                        start: t1.angle(),
                        sweep: turn,
                    },
                    source,
                });
            } else if from.distance(holder.start()) > locus_region::SNAP {
                out.push(Edge {
                    seg: Seg::Line {
                        a: from,
                        b: holder.start(),
                    },
                    source,
                });
            }
        }
        out.push(Edge {
            seg: holder,
            source,
        });
    }
    ExportPath {
        segs: out.iter().map(path_seg).collect(),
        closed: p.closed,
    }
}

// ── Hatch fill (#175) ───────────────────────────────────────────────────

/// Hatch fill (SPEC §7 Engrave): the closed paths, combined even-odd by
/// nesting (holes stay empty, islands in holes are filled again), are
/// crossed by parallel lines `spacing` apart at `angle_deg`, and again at
/// +90° for a crosshatch. Lines sit on a global grid (multiples of the
/// spacing from the origin), so neighbouring shapes line up. Alternate lines
/// run in opposite directions; the order stage then minimises travel. The
/// outlines are kept.
fn hatch(layer: &mut ExportLayer, spacing: f64, angle_deg: f64, cross: bool) {
    let loops: Vec<Loop> = layer
        .paths
        .iter()
        .filter(|p| p.closed)
        .map(loop_of)
        .collect();
    if loops.is_empty() {
        return;
    }
    let depth = depths(&loops);
    let area = Area {
        loops: loops
            .iter()
            .zip(depth)
            .map(|(l, d)| {
                let ccw = l.area() >= 0.0;
                if ccw == (d % 2 == 0) {
                    l.clone()
                } else {
                    l.reversed()
                }
            })
            .collect(),
    };
    let a = angle_deg.to_radians();
    let mut lines = scan(&area, spacing, a);
    if cross {
        lines.extend(scan(&area, spacing, a + core::f64::consts::FRAC_PI_2));
    }
    layer.paths.extend(lines);
}

/// Most hatch lines in one fill, so a tiny spacing on a large part can't
/// exhaust memory (untrusted input, CLAUDE.md). Over it, no hatch is made.
pub const MAX_HATCH_LINES: usize = 200_000;

/// Where segment `l` crosses region piece `s`.
fn crossings(l: &Line, s: &Seg) -> Vec<Vec2> {
    match *s {
        Seg::Line { a, b } => l.intersect_line(&Line::new(a, b)).into_iter().collect(),
        Seg::Arc {
            center,
            radius,
            sweep,
            ..
        } if sweep.abs() >= TAU - 1e-12 => l.intersect_circle(&Circle::new(center, radius)),
        Seg::Arc { center, sweep, .. } => {
            let (p, q) = (s.start(), s.end());
            let arc = if sweep >= 0.0 {
                Arc::new(center, p, q)
            } else {
                Arc::new(center, q, p)
            };
            l.intersect_arc(&arc)
        }
    }
}

fn scan(area: &Area, spacing: f64, angle: f64) -> Vec<ExportPath> {
    let dir = Vec2::from_angle(angle);
    let nrm = dir.perp();
    let Some(bb) = area
        .loops
        .iter()
        .filter_map(Loop::bbox)
        .reduce(locus_geom::Aabb::union)
    else {
        return Vec::new();
    };
    let corners = [
        bb.min,
        Vec2::new(bb.max.x, bb.min.y),
        bb.max,
        Vec2::new(bb.min.x, bb.max.y),
    ];
    let span = |v: Vec2| {
        corners
            .iter()
            .map(|c| c.dot(v))
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), x| {
                (lo.min(x), hi.max(x))
            })
    };
    let ((lo, hi), (dlo, dhi)) = (span(nrm), span(dir));
    let first = (lo / spacing).ceil();
    let count = ((hi / spacing).floor() - first + 1.0).max(0.0);
    if !count.is_finite() || count > MAX_HATCH_LINES as f64 {
        return Vec::new();
    }
    let edges: Vec<&Edge> = area.edges().collect();
    let mut out = Vec::new();
    for k in 0..count as usize {
        let c = (first + k as f64) * spacing;
        let base = nrm * c;
        let l = Line::new(base + dir * (dlo - 1.0), base + dir * (dhi + 1.0));
        let mut hits: Vec<(f64, u64)> = edges
            .iter()
            .flat_map(|e| {
                crossings(&l, &e.seg)
                    .into_iter()
                    .map(|q| (q.dot(dir), e.source))
            })
            .collect();
        hits.sort_by(|x, y| x.0.total_cmp(&y.0));
        hits.dedup_by(|y, x| (y.0 - x.0).abs() <= 1e-9);
        // Inside spans along the row, touching ones merged.
        let mut spans: Vec<(f64, f64, u64)> = Vec::new();
        for w in hits.windows(2) {
            let (t0, t1) = (w[0].0, w[1].0);
            let mid = base + dir * ((t0 + t1) / 2.0);
            // Spans along an outline edge are already drawn by the outline.
            let on_edge = edges
                .iter()
                .any(|e| e.seg.distance(mid) <= EXPORT_TOL / 2.0);
            if t1 - t0 <= EXPORT_TOL || on_edge || !area.contains(mid) {
                continue;
            }
            match spans.last_mut() {
                Some(last) if (last.1 - t0).abs() <= 1e-9 => last.1 = t1,
                _ => spans.push((t0, t1, w[0].1)),
            }
        }
        let mut row: Vec<PathSeg> = spans
            .into_iter()
            .map(|(t0, t1, src)| PathSeg {
                entity: EntityId::new(src),
                shape: Shape::Line(Line::new(base + dir * t0, base + dir * t1)),
                reversed: false,
            })
            .collect();
        if k % 2 == 1 {
            row.reverse();
            for s in &mut row {
                s.reversed = true;
            }
        }
        out.extend(row.into_iter().map(|s| ExportPath {
            segs: vec![s],
            closed: false,
        }));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::chain;
    use locus_doc::{CutParams, LayerId};

    fn v(x: f64, y: f64) -> Vec2 {
        Vec2::new(x, y)
    }

    pub(super) fn layer(op: Operation, shapes: Vec<Shape>) -> ExportLayer {
        let segs = shapes
            .into_iter()
            .enumerate()
            .map(|(i, shape)| PathSeg {
                entity: EntityId::new(100 + i as u64),
                shape,
                reversed: false,
            })
            .collect();
        let mut l = ExportLayer::new(LayerId::new(1), "L".to_owned(), [0, 0, 0], op);
        l.paths = chain(segs);
        l
    }

    pub(super) fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<Shape> {
        let p = [v(x0, y0), v(x1, y0), v(x1, y1), v(x0, y1)];
        (0..4)
            .map(|i| Shape::Line(Line::new(p[i], p[(i + 1) % 4])))
            .collect()
    }

    fn cut(kerf: f64, comp: Compensation) -> Operation {
        Operation::Cut(CutParams {
            kerf_width: kerf,
            compensation: comp,
            passes: 1,
        })
    }

    fn area(l: &ExportLayer) -> f64 {
        l.paths.iter().map(|p| loop_of(p).area().abs()).sum()
    }

    #[test]
    fn outside_kerf_grows_parts_and_shrinks_holes() {
        // A 40×20 plate with a 10×10 hole and a circle part inside the hole.
        let mut shapes = rect(0.0, 0.0, 40.0, 20.0);
        shapes.extend(rect(5.0, 5.0, 15.0, 15.0));
        shapes.push(Shape::Circle(Circle::new(v(30.0, 10.0), 4.0)));
        let mut l = layer(cut(0.2, Compensation::Outside), shapes);
        let mut m = ExportModel {
            layers: vec![l.clone()],
            ..Default::default()
        };
        apply_operations(&mut m);
        l = m.layers.remove(0);
        assert_eq!(l.paths.len(), 3);
        let bbs: Vec<_> = l.paths.iter().map(|p| p.bbox().unwrap()).collect();
        // Plate grows by 0.1 all round (round corners).
        assert!(
            bbs.iter()
                .any(|b| (b.min.x + 0.1).abs() < 1e-9 && (b.max.x - 40.1).abs() < 1e-9)
        );
        // The hole shrinks by 0.1 with sharp corners.
        assert!(
            bbs.iter()
                .any(|b| (b.min.x - 5.1).abs() < 1e-9 && (b.max.x - 14.9).abs() < 1e-9)
        );
        // The circle is a hole in the plate (depth 1): it shrinks.
        let c = l
            .paths
            .iter()
            .find_map(|p| match p.segs[0].shape {
                Shape::Circle(c) => Some(c),
                _ => None,
            })
            .unwrap();
        assert!((c.radius - 3.9).abs() < 1e-9);
        // Arcs stay arcs: the plate's corners are four true arcs.
        assert!(
            l.paths
                .iter()
                .flat_map(|p| &p.segs)
                .filter(|s| matches!(s.shape, Shape::Arc(_)))
                .count()
                == 4
        );
    }

    #[test]
    fn inside_kerf_is_the_reverse_and_none_is_untouched() {
        let shapes = rect(0.0, 0.0, 10.0, 10.0);
        let mut m = ExportModel {
            layers: vec![layer(cut(0.4, Compensation::Inside), shapes.clone())],
            ..Default::default()
        };
        apply_operations(&mut m);
        assert!((area(&m.layers[0]) - 9.6 * 9.6).abs() < 1e-9);
        let before = layer(cut(0.4, Compensation::None), shapes);
        let mut m = ExportModel {
            layers: vec![before.clone()],
            ..Default::default()
        };
        apply_operations(&mut m);
        assert_eq!(m.layers[0], before);
    }

    #[test]
    fn nested_part_in_a_hole_grows() {
        // Plate, hole, and a part inside the hole (depth 2): the part grows.
        let mut shapes = rect(0.0, 0.0, 50.0, 50.0);
        shapes.extend(rect(10.0, 10.0, 40.0, 40.0));
        shapes.extend(rect(20.0, 20.0, 30.0, 30.0));
        let mut m = ExportModel {
            layers: vec![layer(cut(0.2, Compensation::Outside), shapes)],
            ..Default::default()
        };
        apply_operations(&mut m);
        let inner = m.layers[0]
            .paths
            .iter()
            .map(|p| p.bbox().unwrap())
            .find(|b| b.min.x > 15.0)
            .unwrap();
        assert!((inner.min.x - 19.9).abs() < 1e-9 && (inner.max.x - 30.1).abs() < 1e-9);
    }

    fn knife(o: f64) -> Operation {
        Operation::DragKnife(locus_doc::DragKnifeParams { blade_offset: o })
    }

    #[test]
    fn blade_offset_swivels_at_sharp_corners() {
        // A 10×10 square cut CCW: four 90° turns, each gets a swivel arc of
        // radius 0.25 about the corner; the sides shift 0.25 ahead.
        let mut m = ExportModel {
            layers: vec![layer(knife(0.25), rect(0.0, 0.0, 10.0, 10.0))],
            ..Default::default()
        };
        apply_operations(&mut m);
        let p = &m.layers[0].paths[0];
        assert!(p.closed);
        let arcs: Vec<Arc> = p
            .segs
            .iter()
            .filter_map(|s| match s.shape {
                Shape::Arc(a) => Some(a),
                _ => None,
            })
            .collect();
        assert_eq!(arcs.len(), 4);
        assert!(arcs.iter().all(|a| (a.radius() - 0.25).abs() < 1e-12));
        assert!(arcs.iter().any(|a| a.center.approx_eq(v(10.0, 0.0), 1e-12)));
        // Each side is the original shifted ahead by the offset.
        assert!(p.segs.iter().any(|s| matches!(s.shape, Shape::Line(l)
            if l.a.approx_eq(v(0.25, 0.0), 1e-12) && l.b.approx_eq(v(10.25, 0.0), 1e-12))));
        // The path is continuous.
        for w in p.segs.windows(2) {
            assert!(w[0].end().distance(w[1].start()) < 1e-9);
        }
    }

    #[test]
    fn blade_offset_on_arcs_and_gentle_turns() {
        // A circle: one bigger concentric circle, no swivel.
        let mut m = ExportModel {
            layers: vec![layer(
                knife(0.3),
                vec![Shape::Circle(Circle::new(v(0.0, 0.0), 4.0))],
            )],
            ..Default::default()
        };
        apply_operations(&mut m);
        let Shape::Circle(c) = m.layers[0].paths[0].segs[0].shape else {
            panic!()
        };
        assert!((c.radius - 4.0f64.hypot(0.3)).abs() < 1e-12);
        // A 5° bend in an open polyline: joined straight, no swivel arc.
        let a = v(0.0, 0.0);
        let b = v(10.0, 0.0);
        let c2 = b + Vec2::from_angle(5f64.to_radians()) * 10.0;
        let mut m = ExportModel {
            layers: vec![layer(
                knife(0.3),
                vec![Shape::Line(Line::new(a, b)), Shape::Line(Line::new(b, c2))],
            )],
            ..Default::default()
        };
        apply_operations(&mut m);
        let p = &m.layers[0].paths[0];
        assert!(!p.closed);
        assert!(p.segs.iter().all(|s| matches!(s.shape, Shape::Line(_))));
        assert_eq!(p.segs.len(), 3);
    }

    fn engrave(spacing: f64, angle: f64, cross: bool) -> Operation {
        Operation::Engrave(locus_doc::EngraveParams {
            hatch_spacing: spacing,
            hatch_angle: angle,
            crosshatch: cross,
        })
    }

    fn hatches(l: &ExportLayer) -> Vec<Line> {
        l.paths
            .iter()
            .filter(|p| !p.closed)
            .filter_map(|p| match p.segs[0].shape {
                Shape::Line(l) => Some(l),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn hatch_fills_and_respects_holes() {
        // 10×10 square with a 3.5×3.5 hole in the middle, spacing 1.
        let mut shapes = rect(0.0, 0.0, 10.0, 10.0);
        shapes.extend(rect(3.25, 3.25, 6.75, 6.75));
        let mut m = ExportModel {
            layers: vec![layer(engrave(1.0, 0.0, false), shapes)],
            ..Default::default()
        };
        apply_operations(&mut m);
        let hs = hatches(&m.layers[0]);
        // Lines at y = 1..9 (y = 0 and 10 lie on the boundary); the rows
        // through the hole (y = 4, 5, 6) are split in two.
        assert_eq!(hs.len(), 9 + 3);
        let total: f64 = hs.iter().map(Line::length).sum();
        assert!((total - (9.0 * 10.0 - 3.0 * 3.5)).abs() < 1e-9);
        // Nothing inside the hole.
        assert!(hs.iter().all(|l| {
            let m = l.eval(0.5);
            !(m.x > 3.25 && m.x < 6.75 && m.y > 3.25 && m.y < 6.75)
        }));
        // Alternate rows run opposite ways.
        let dirs: Vec<bool> = m.layers[0]
            .paths
            .iter()
            .filter(|p| !p.closed)
            .map(|p| p.start().x < p.end().x)
            .collect();
        assert!(dirs.windows(2).any(|w| w[0] != w[1]));
        // Outlines are kept.
        assert_eq!(m.layers[0].paths.iter().filter(|p| p.closed).count(), 2);
    }

    #[test]
    fn crosshatch_at_an_angle_and_round_shapes() {
        let shapes = vec![Shape::Circle(Circle::new(v(0.0, 0.0), 5.0))];
        let mut m = ExportModel {
            layers: vec![layer(engrave(0.5, 30.0, true), shapes)],
            ..Default::default()
        };
        apply_operations(&mut m);
        let hs = hatches(&m.layers[0]);
        // Every end lies on the circle; both directions are present.
        assert!(
            hs.iter()
                .all(|l| (l.a.length() - 5.0).abs() < 1e-9 && (l.b.length() - 5.0).abs() < 1e-9)
        );
        let dir = |l: &Line| (l.b - l.a).angle().rem_euclid(core::f64::consts::PI);
        let a30 = 30f64.to_radians();
        assert!(hs.iter().any(|l| (dir(l) - a30).abs() < 1e-9));
        assert!(
            hs.iter()
                .any(|l| (dir(l) - a30 - core::f64::consts::FRAC_PI_2).abs() < 1e-9)
        );
        // Total hatch length ≈ disc area / spacing, for each of the passes.
        let total: f64 = hs.iter().map(Line::length).sum();
        let want = 2.0 * core::f64::consts::PI * 25.0 / 0.5;
        assert!((total - want).abs() / want < 0.02, "{total} vs {want}");
    }

    #[test]
    fn a_row_along_an_edge_is_one_line() {
        // The hole's top and bottom edges lie on hatch rows y = 3 and 7.
        let mut shapes = rect(0.0, 0.0, 10.0, 10.0);
        shapes.extend(rect(3.0, 3.0, 7.0, 7.5));
        let mut m = ExportModel {
            layers: vec![layer(engrave(1.0, 0.0, false), shapes)],
            ..Default::default()
        };
        apply_operations(&mut m);
        let at = |y: f64| {
            hatches(&m.layers[0])
                .iter()
                .filter(|l| (l.a.y - y).abs() < 1e-9)
                .count()
        };
        assert!(at(3.0) <= 2);
        assert_eq!(at(2.0), 1);
    }

    #[test]
    fn absurd_hatch_density_is_refused() {
        let mut m = ExportModel {
            layers: vec![layer(
                engrave(1e-6, 0.0, false),
                rect(0.0, 0.0, 1000.0, 1000.0),
            )],
            ..Default::default()
        };
        apply_operations(&mut m);
        assert!(hatches(&m.layers[0]).is_empty());
    }

    #[test]
    fn a_hole_narrower_than_the_kerf_disappears_and_open_paths_stay() {
        let mut shapes = rect(0.0, 0.0, 20.0, 20.0);
        shapes.extend(rect(5.0, 5.0, 5.15, 15.0));
        shapes.push(Shape::Line(Line::new(v(30.0, 0.0), v(40.0, 0.0))));
        let mut m = ExportModel {
            layers: vec![layer(cut(0.2, Compensation::Outside), shapes)],
            ..Default::default()
        };
        apply_operations(&mut m);
        let l = &m.layers[0];
        assert_eq!(l.paths.iter().filter(|p| p.closed).count(), 1);
        assert_eq!(l.paths.iter().filter(|p| !p.closed).count(), 1);
    }
}
