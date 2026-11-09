// SPDX-License-Identifier: AGPL-3.0-or-later

//! First export stage: collect exportable geometry and chain it into paths
//! (SPEC §8 pipeline, #125).
//!
//! The result, [`ExportModel`], is a plain value owned by the exporter. Every
//! later stage (validation, auto-fix, ordering, writers) reads or edits this
//! model and never the [`Document`] (SPEC §11).

use locus_doc::{Document, EntityId, Geom, LayerId, Operation};
use locus_geom::{Aabb, Arc, Circle, Cubic, EXPORT_TOL, Line, Piece, Vec2};
use std::collections::VecDeque;

/// One drawable primitive, in document millimetres (Y-up).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Shape {
    Line(Line),
    /// Counter-clockwise from `start` to `end` (SPEC §4).
    Arc(Arc),
    Circle(Circle),
    /// Cubic Bézier (SPEC §4). SVG and PDF write it as a curve; DXF R12 and the
    /// geometric checks use [`Shape::pieces`].
    Cubic(Cubic),
}

impl Shape {
    #[must_use]
    pub fn bbox(&self) -> Aabb {
        match self {
            Shape::Line(l) => l.bbox(),
            Shape::Arc(a) => a.bbox(),
            Shape::Circle(c) => c.bbox(),
            Shape::Cubic(c) => c.bbox(),
        }
    }

    #[must_use]
    pub fn length(&self) -> f64 {
        match self {
            Shape::Line(l) => l.length(),
            Shape::Arc(a) => a.length(),
            Shape::Circle(c) => c.length(),
            Shape::Cubic(c) => c.length(),
        }
    }

    /// Too small to cut: shorter than the export tolerance (SPEC §8).
    #[must_use]
    pub fn is_degenerate(&self) -> bool {
        match self {
            Shape::Line(l) => l.length() <= EXPORT_TOL,
            Shape::Arc(a) => a.is_degenerate() || a.length() <= EXPORT_TOL,
            Shape::Circle(c) => c.radius <= EXPORT_TOL,
            Shape::Cubic(c) => c.length() <= EXPORT_TOL,
        }
    }

    /// Natural endpoints (circles have none).
    pub(crate) fn ends(&self) -> Option<(Vec2, Vec2)> {
        match self {
            Shape::Line(l) => Some((l.a, l.b)),
            Shape::Arc(a) => Some((a.start, a.end)),
            Shape::Circle(_) => None,
            Shape::Cubic(c) => Some((c.p0, c.p1)),
        }
    }

    /// A Bézier as lines and arcs within [`EXPORT_TOL`]; any other shape is
    /// its own single piece (a circle has none, see [`Shape::Circle`]).
    #[must_use]
    pub fn pieces(&self) -> Vec<Shape> {
        match self {
            Shape::Cubic(c) => c
                .to_pieces(EXPORT_TOL)
                .into_iter()
                .map(|p| match p {
                    Piece::Line(l) => Shape::Line(l),
                    Piece::Arc(a) => Shape::Arc(a),
                })
                .collect(),
            other => vec![*other],
        }
    }

    /// Intersection points with another shape (collinear overlaps excluded).
    /// A Bézier is compared through its arc approximation.
    #[must_use]
    pub fn intersections(&self, o: &Shape) -> Vec<Vec2> {
        if matches!(self, Shape::Cubic(_)) || matches!(o, Shape::Cubic(_)) {
            let (a, b) = (self.pieces(), o.pieces());
            return a
                .iter()
                .flat_map(|x| b.iter().flat_map(move |y| x.intersections(y)))
                .collect();
        }
        match (self, o) {
            (Shape::Line(a), Shape::Line(b)) => a.intersect_line(b).into_iter().collect(),
            (Shape::Line(a), Shape::Arc(b)) | (Shape::Arc(b), Shape::Line(a)) => a.intersect_arc(b),
            (Shape::Line(a), Shape::Circle(b)) | (Shape::Circle(b), Shape::Line(a)) => {
                a.intersect_circle(b)
            }
            (Shape::Arc(a), Shape::Arc(b)) => a.intersect_arc(b),
            (Shape::Arc(a), Shape::Circle(b)) | (Shape::Circle(b), Shape::Arc(a)) => {
                a.intersect_circle(b)
            }
            (Shape::Circle(a), Shape::Circle(b)) => a.intersect_circle(b),
            (Shape::Cubic(_), _) | (_, Shape::Cubic(_)) => Vec::new(),
        }
    }
}

/// A shape placed in a path, possibly traversed backwards.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PathSeg {
    pub entity: EntityId,
    pub shape: Shape,
    /// Traversed end → start (arcs keep their CCW definition; writers use [`PathSeg::start`] / [`PathSeg::end`]).
    pub reversed: bool,
}

impl PathSeg {
    /// Where the pen enters this segment. For a circle, the angle-0 point.
    #[must_use]
    pub fn start(&self) -> Vec2 {
        match (self.shape.ends(), self.reversed) {
            (Some((_, b)), true) => b,
            (Some((a, _)), false) => a,
            (None, _) => self.circle_point(),
        }
    }

    /// Where the pen leaves this segment.
    #[must_use]
    pub fn end(&self) -> Vec2 {
        match (self.shape.ends(), self.reversed) {
            (Some((a, _)), true) => a,
            (Some((_, b)), false) => b,
            (None, _) => self.circle_point(),
        }
    }

    fn circle_point(&self) -> Vec2 {
        match self.shape {
            Shape::Circle(c) => c.center + Vec2::new(c.radius, 0.0),
            _ => Vec2::ZERO,
        }
    }

    fn flipped(mut self) -> Self {
        self.reversed = !self.reversed;
        self
    }
}

/// Segments joined end to end. A circle is a one-segment closed path.
#[derive(Debug, Clone, PartialEq)]
pub struct ExportPath {
    pub segs: Vec<PathSeg>,
    pub closed: bool,
}

impl ExportPath {
    #[must_use]
    pub fn start(&self) -> Vec2 {
        self.segs.first().map_or(Vec2::ZERO, PathSeg::start)
    }

    #[must_use]
    pub fn end(&self) -> Vec2 {
        self.segs.last().map_or(Vec2::ZERO, PathSeg::end)
    }

    #[must_use]
    pub fn entities(&self) -> Vec<EntityId> {
        self.segs.iter().map(|s| s.entity).collect()
    }

    #[must_use]
    pub fn bbox(&self) -> Option<Aabb> {
        self.segs.iter().map(|s| s.shape.bbox()).reduce(Aabb::union)
    }
}

/// One exported layer.
#[derive(Debug, Clone, PartialEq)]
pub struct ExportLayer {
    pub id: LayerId,
    pub name: String,
    pub colour: [u8; 3],
    pub operation: Operation,
    pub paths: Vec<ExportPath>,
    /// Zero-length entities set aside during collection; never written.
    pub degenerate: Vec<EntityId>,
}

impl ExportLayer {
    /// Rebuild `paths` from the current segments (after an edit).
    pub fn rechain(&mut self) {
        let segs: Vec<PathSeg> = self
            .paths
            .drain(..)
            .flat_map(|p| p.segs)
            .map(|mut s| {
                s.reversed = false;
                s
            })
            .collect();
        self.paths = chain(segs);
    }
}

/// Everything the exporters need, layers in export order.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ExportModel {
    pub layers: Vec<ExportLayer>,
}

impl ExportModel {
    #[must_use]
    pub fn bounds(&self) -> Option<Aabb> {
        self.layers
            .iter()
            .flat_map(|l| &l.paths)
            .filter_map(ExportPath::bbox)
            .reduce(Aabb::union)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.layers.iter().all(|l| l.paths.is_empty())
    }
}

/// Is this layer written at all? (SPEC §7, §8: export-disabled and Reference
/// layers are never exported.)
fn layer_exported(layer: &locus_doc::Layer) -> bool {
    layer.export_enabled && layer.operation.is_exportable()
}

/// Collect exportable geometry per layer and chain it into paths.
///
/// Skipped: construction geometry, bare points, export-disabled and Reference layers, and
/// geometry that references a missing point (an exporter must not panic on a
/// document it did not validate). Layers with nothing to write are omitted.
#[must_use]
pub fn collect(doc: &Document) -> ExportModel {
    let mut layers: Vec<(ExportLayer, Vec<PathSeg>)> = doc
        .layers()
        .iter()
        .filter(|(_, l)| layer_exported(l))
        .map(|(id, l)| {
            (
                ExportLayer {
                    id: *id,
                    name: l.name.clone(),
                    colour: l.colour,
                    operation: l.operation,
                    paths: Vec::new(),
                    degenerate: Vec::new(),
                },
                Vec::new(),
            )
        })
        .collect();
    layers.sort_by_key(|(l, _)| {
        let order = doc.layer(l.id).map_or(0, |d| d.export_order);
        (order, l.id.get())
    });

    for (eid, e) in doc.entities().iter() {
        if e.construction {
            continue;
        }
        let shape = match &e.geom {
            Geom::Point { .. } => continue,
            Geom::Line { .. } => doc.line(eid).map(Shape::Line),
            Geom::Circle { .. } => doc.circle(eid).map(Shape::Circle),
            Geom::Arc { .. } => doc.arc(eid).map(Shape::Arc),
            Geom::Bezier { .. } => doc.cubic(eid).map(Shape::Cubic),
        };
        let Some(shape) = shape else { continue };
        let Some((layer, segs)) = layers.iter_mut().find(|(l, _)| l.id == e.layer) else {
            continue;
        };
        if shape.is_degenerate() || !shape.bbox().min.is_finite() || !shape.bbox().max.is_finite() {
            layer.degenerate.push(eid);
        } else {
            segs.push(PathSeg {
                entity: eid,
                shape,
                reversed: false,
            });
        }
    }

    ExportModel {
        layers: layers
            .into_iter()
            .filter_map(|(mut l, segs)| {
                l.paths = chain(segs);
                (!l.paths.is_empty() || !l.degenerate.is_empty()).then_some(l)
            })
            .collect(),
    }
}

fn near(a: Vec2, b: Vec2) -> bool {
    a.distance(b) <= EXPORT_TOL
}

/// Join segments end to end where endpoints lie within [`EXPORT_TOL`].
///
/// Greedy and deterministic: paths start from the lowest-index unused segment
/// and grow at the tail then the head, always taking the nearest match
/// (ties by input order). Where three or more segments meet at one point the
/// chain simply continues with the nearest and leaves the rest as further paths.
#[must_use]
pub fn chain(segs: Vec<PathSeg>) -> Vec<ExportPath> {
    let mut used = vec![false; segs.len()];
    let mut out = Vec::new();
    for i in 0..segs.len() {
        if used[i] {
            continue;
        }
        used[i] = true;
        if matches!(segs[i].shape, Shape::Circle(_)) {
            out.push(ExportPath {
                segs: vec![segs[i]],
                closed: true,
            });
            continue;
        }
        let mut path: VecDeque<PathSeg> = VecDeque::from([segs[i]]);
        let closed = loop {
            if path.len() >= 2 && near(path[path.len() - 1].end(), path[0].start()) {
                break true;
            }
            let tail = path[path.len() - 1].end();
            let Some((j, flip)) = nearest_join(&segs, &used, tail) else {
                break false;
            };
            used[j] = true;
            path.push_back(if flip { segs[j].flipped() } else { segs[j] });
        };
        if !closed {
            // Grow the head too; a chain may have been started mid-way.
            loop {
                let head = path[0].start();
                let Some((j, flip)) = nearest_join(&segs, &used, head) else {
                    break;
                };
                used[j] = true;
                // Joining at our head: the new segment must END at `head`.
                path.push_front(if flip { segs[j] } else { segs[j].flipped() });
                if path.len() >= 2 && near(path[path.len() - 1].end(), path[0].start()) {
                    break;
                }
            }
        }
        let closed = path.len() >= 2 && near(path[path.len() - 1].end(), path[0].start());
        out.push(ExportPath {
            segs: path.into(),
            closed,
        });
    }
    out
}

/// Nearest unused non-circle segment with an endpoint at `p`.
/// `flip` = the segment must be reversed to *start* at `p`.
fn nearest_join(segs: &[PathSeg], used: &[bool], p: Vec2) -> Option<(usize, bool)> {
    let mut best: Option<(usize, bool, f64)> = None;
    for (j, s) in segs.iter().enumerate() {
        if used[j] {
            continue;
        }
        let Some((a, b)) = s.shape.ends() else {
            continue;
        };
        for (d, flip) in [(a.distance(p), false), (b.distance(p), true)] {
            if d <= EXPORT_TOL && best.is_none_or(|(_, _, bd)| d < bd) {
                best = Some((j, flip, d));
            }
        }
    }
    best.map(|(j, f, _)| (j, f))
}

#[cfg(test)]
pub(crate) mod testutil {
    use super::*;

    pub fn seg(id: u64, a: (f64, f64), b: (f64, f64)) -> PathSeg {
        PathSeg {
            entity: EntityId::new(id),
            shape: Shape::Line(Line::new(Vec2::new(a.0, a.1), Vec2::new(b.0, b.1))),
            reversed: false,
        }
    }

    pub fn layer(op: locus_doc::OperationKind, segs: Vec<PathSeg>) -> ExportLayer {
        ExportLayer {
            id: LayerId::new(1),
            name: "L".into(),
            colour: [0, 0, 0],
            operation: op.default_operation(),
            paths: chain(segs),
            degenerate: Vec::new(),
        }
    }

    /// Axis-aligned rectangle as four lines with ids `base..base+4`.
    pub fn rect(base: u64, x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<PathSeg> {
        vec![
            seg(base, (x0, y0), (x1, y0)),
            seg(base + 1, (x1, y0), (x1, y1)),
            seg(base + 2, (x1, y1), (x0, y1)),
            seg(base + 3, (x0, y1), (x0, y0)),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::testutil::*;
    use super::*;

    #[test]
    fn out_of_order_and_reversed_segments_form_one_closed_path() {
        let segs = vec![
            seg(1, (0.0, 0.0), (10.0, 0.0)),
            seg(3, (10.0, 10.0), (0.0, 10.0)),
            seg(2, (10.0, 10.0), (10.0, 0.0)), // reversed
            seg(4, (0.0, 0.0), (0.0, 10.0)),   // reversed
        ];
        let paths = chain(segs);
        assert_eq!(paths.len(), 1);
        assert!(paths[0].closed);
        assert_eq!(paths[0].segs.len(), 4);
        for w in paths[0].segs.windows(2) {
            assert!(near(w[0].end(), w[1].start()));
        }
    }

    #[test]
    fn near_coincident_endpoints_join_but_gaps_do_not() {
        let joined = chain(vec![
            seg(1, (0.0, 0.0), (10.0, 0.0)),
            seg(2, (10.0 + 0.0005, 0.0), (10.0, 10.0)),
        ]);
        assert_eq!(joined.len(), 1);
        let gap = chain(vec![
            seg(1, (0.0, 0.0), (10.0, 0.0)),
            seg(2, (10.01, 0.0), (10.0, 10.0)),
        ]);
        assert_eq!(gap.len(), 2);
        assert!(gap.iter().all(|p| !p.closed));
    }

    #[test]
    fn open_chain_started_mid_way_grows_at_the_head() {
        let paths = chain(vec![
            seg(2, (10.0, 0.0), (20.0, 0.0)),
            seg(1, (0.0, 0.0), (10.0, 0.0)),
            seg(3, (20.0, 0.0), (30.0, 0.0)),
        ]);
        assert_eq!(paths.len(), 1);
        assert!(!paths[0].closed);
        assert_eq!(paths[0].start(), Vec2::new(0.0, 0.0));
        assert_eq!(paths[0].end(), Vec2::new(30.0, 0.0));
    }

    #[test]
    fn circle_is_its_own_closed_path() {
        let c = PathSeg {
            entity: EntityId::new(1),
            shape: Shape::Circle(Circle::new(Vec2::new(5.0, 5.0), 2.0)),
            reversed: false,
        };
        let paths = chain(vec![c]);
        assert_eq!(paths.len(), 1);
        assert!(paths[0].closed);
    }

    #[test]
    fn arc_and_line_join() {
        let arc = PathSeg {
            entity: EntityId::new(1),
            shape: Shape::Arc(Arc::new(
                Vec2::new(0.0, 0.0),
                Vec2::new(5.0, 0.0),
                Vec2::new(0.0, 5.0),
            )),
            reversed: false,
        };
        let paths = chain(vec![seg(2, (0.0, 5.0), (0.0, 0.0)), arc]);
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].segs.len(), 2);
    }
}
