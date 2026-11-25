// SPDX-License-Identifier: AGPL-3.0-or-later

//! First export stage: collect exportable geometry and chain it into paths
//! (SPEC §8 pipeline, #125).
//!
//! The result, [`ExportModel`], is a plain value owned by the exporter. Every
//! later stage (validation, auto-fix, ordering, writers) reads or edits this
//! model and never the [`Document`] (SPEC §11).

use locus_doc::{
    Align, Document, EntityId, Geom, LayerId, LineStyle, Operation, Stroke, StrokeStyles, Style,
};
use locus_geom::{Aabb, Arc, Circle, Cubic, EXPORT_TOL, Line, Piece, Vec2};
use std::collections::{BTreeMap, VecDeque};

/// The stroke width machine layers export with when the profile sets none,
/// mm: the hairline LightBurn and the vinyl-cutter apps read as a cut line
/// (SPEC §7, #284).
pub const DEFAULT_HAIRLINE_MM: f64 = 0.1;

/// How one path's stroke is written (SPEC §7, §8; #284): resolved from the
/// object's style on a Draw layer, or the layer colour and hairline on a
/// machine layer. Paths have no fill: fills belong to regions (#451,
/// [`ExportFill`]) and are written under every stroke.
#[derive(Debug, Clone, PartialEq)]
pub struct Paint {
    pub stroke: [u8; 3],
    /// Stroke width, mm.
    pub width: f64,
    /// `0..=1`.
    pub opacity: f64,
    /// The plotter pen, on Draw layers.
    pub pen: Option<u32>,
    /// Caps, joins and alignment (#428): the object's on a Draw layer,
    /// the default elsewhere (machine software reads the hairline only).
    pub line: LineStyle,
}

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
    /// How this path is written when it isn't its object's own stroke: an
    /// extra stroke (#429) or a dash already split off (#427). `None` uses
    /// the object's style.
    pub paint: Option<Paint>,
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
    /// Stroke width of a machine layer, mm: the profile's hairline, or
    /// [`DEFAULT_HAIRLINE_MM`] (SPEC §7, #284).
    pub hairline: f64,
    /// Each exported entity's own style (SPEC §4, #287); used on Draw layers.
    pub styles: BTreeMap<EntityId, Style>,
    /// Machine layers: cut the centreline or the styled outline (#428).
    pub stroke_styles: StrokeStyles,
}

impl ExportLayer {
    /// A layer with nothing in it yet, default hairline and no styles.
    #[must_use]
    pub fn new(id: LayerId, name: String, colour: [u8; 3], operation: Operation) -> Self {
        Self {
            id,
            name,
            colour,
            operation,
            paths: Vec::new(),
            degenerate: Vec::new(),
            hairline: DEFAULT_HAIRLINE_MM,
            styles: BTreeMap::new(),
            stroke_styles: StrokeStyles::default(),
        }
    }

    /// Does this layer write each object's own style? Only Draw does; the
    /// machine layers write the profile's colour and hairline, because
    /// LightBurn and the vinyl-cutter apps read colour as the operation
    /// (SPEC §7, owner decision 2026-11-13, #284).
    #[must_use]
    pub fn uses_object_style(&self) -> bool {
        matches!(self.operation, Operation::Draw)
    }

    fn style_of(&self, entity: EntityId) -> Style {
        self.styles.get(&entity).cloned().unwrap_or_default()
    }

    /// How `path` is written. A path is chained from segments of one style
    /// ([`ExportLayer::chain`]), so its first segment speaks for it.
    #[must_use]
    pub fn paint(&self, path: &ExportPath) -> Paint {
        if let Some(p) = &path.paint {
            return p.clone();
        }
        if !self.uses_object_style() {
            return Paint {
                stroke: self.colour,
                width: self.hairline,
                opacity: 1.0,
                pen: None,
                line: LineStyle::default(),
            };
        }
        let s = path
            .segs
            .first()
            .map_or_else(Style::default, |seg| self.style_of(seg.entity));
        Paint {
            stroke: match s.stroke {
                Stroke::ByLayer => self.colour,
                Stroke::Colour(c) => c,
            },
            width: s.width,
            opacity: s.opacity,
            pen: Some(s.pen),
            line: s.line,
        }
    }

    /// The dashes of `path` as paths of their own (pen-down strokes, #427),
    /// when it has a dash pattern; `None` otherwise or past the dash limit
    /// (the path is then written solid; export reports nothing extra).
    #[must_use]
    pub fn dash_paths(&self, path: &ExportPath) -> Option<Vec<ExportPath>> {
        let entity = path.segs.first()?.entity;
        let paint = self.paint(path);
        let d = paint.line.dash?;
        let runs =
            locus_stroke::dashes(&path_chain(path), &d, &locus_stroke::Limits::default()).ok()?;
        // Each dash is drawn solid, in the stroke's paint.
        let solid = Paint {
            line: LineStyle {
                dash: None,
                ..paint.line.clone()
            },
            ..paint
        };
        Some(
            runs.iter()
                .filter(|c| !c.segs.is_empty())
                .map(|c| ExportPath {
                    segs: c.segs.iter().map(|s| seg_path(*s, entity)).collect(),
                    closed: false,
                    paint: Some(solid.clone()),
                })
                .collect(),
        )
    }

    /// The extra strokes of `path` (#429) as paths of their own, each at its
    /// offset with its own width, colour, pen, dash and cap; the object's
    /// own stroke is `path` itself. Empty without extras, or when they can't
    /// be built.
    #[must_use]
    pub fn extra_stroke_paths(&self, path: &ExportPath) -> Vec<ExportPath> {
        let Some(entity) = path.segs.first().map(|s| s.entity) else {
            return Vec::new();
        };
        let base = self.paint(path);
        if base.line.extra.is_empty() {
            return Vec::new();
        }
        let Ok(strokes) = locus_stroke::layer_strokes(
            &path_chain(path),
            base.width,
            &base.line,
            &locus_stroke::Limits::default(),
        ) else {
            return Vec::new();
        };
        strokes
            .into_iter()
            .skip(1)
            .flat_map(|ls| {
                let paint = Paint {
                    stroke: ls.colour.unwrap_or(base.stroke),
                    width: ls.width,
                    opacity: base.opacity,
                    pen: base.pen.map(|p| ls.pen.unwrap_or(p)),
                    line: ls.line,
                };
                ls.chains.into_iter().map(move |c| ExportPath {
                    segs: c.segs.iter().map(|s| seg_path(*s, entity)).collect(),
                    closed: c.closed,
                    paint: Some(paint.clone()),
                })
            })
            .collect()
    }

    /// The stroke of `path` as filled outline loops, when a writer can't
    /// draw it as a stroke: on a Draw layer, a closed path aligned inside or
    /// outside (SVG, PDF and PNG centre every stroke), or any path with a
    /// width profile (no format has variable-width strokes, #426), written
    /// with its fitted cubics. `None` otherwise, or when the outline can't
    /// be built (the stroke is then written centred, at its plain width).
    #[must_use]
    pub fn outline(&self, path: &ExportPath) -> Option<Vec<ExportPath>> {
        if !self.uses_object_style() {
            return None;
        }
        let p = self.paint(path);
        if p.line.varying().is_some() {
            return outline_curves(path, p.width, &p.line);
        }
        if !path.closed || p.line.align == Align::Centre {
            return None;
        }
        outline_paths(path, p.width, &p.line)
    }

    /// Chain segments into paths, never joining two objects whose styles
    /// differ on a layer that writes object styles (they would be written
    /// as one path, in one style). Elsewhere this is plain [`chain`].
    #[must_use]
    pub fn chain(&self, segs: Vec<PathSeg>) -> Vec<ExportPath> {
        if !self.uses_object_style() {
            return chain(segs);
        }
        // Group by style in first-seen order, so the output stays deterministic.
        let mut groups: Vec<(Style, Vec<PathSeg>)> = Vec::new();
        for s in segs {
            let st = self.style_of(s.entity);
            match groups.iter_mut().find(|(g, _)| *g == st) {
                Some((_, v)) => v.push(s),
                None => groups.push((st, vec![s])),
            }
        }
        groups.into_iter().flat_map(|(_, v)| chain(v)).collect()
    }

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
        self.paths = self.chain(segs);
    }
}

/// One filled region (SPEC §6.3, #451): its outer loop and holes as closed
/// paths, filled even-odd so the holes stay open, written under every stroke.
#[derive(Debug, Clone, PartialEq)]
pub struct ExportFill {
    /// The layer of the region's outline: the file and group it goes in.
    pub layer: LayerId,
    /// Outer loop first, then the holes; each closed.
    pub paths: Vec<ExportPath>,
    pub colour: [u8; 3],
    /// `0..=1`.
    pub opacity: f64,
    /// A gradient instead of `colour` (#436), with its frame on the page.
    pub gradient: Option<(locus_doc::gradient::Gradient, locus_doc::gradient::Frame)>,
}

/// Everything the exporters need, layers in export order.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ExportModel {
    pub layers: Vec<ExportLayer>,
    /// Region fills (#451), written before any stroke so no fill hides a line.
    pub fills: Vec<ExportFill>,
    /// Overrides the bounds the writers place the drawing in. Per-layer export
    /// (#291) sets it to the whole design's bounds, so every layer's file has
    /// the same origin and canvas and the files line up when re-imported.
    pub frame: Option<Aabb>,
}

impl ExportModel {
    #[must_use]
    pub fn bounds(&self) -> Option<Aabb> {
        if self.frame.is_some() {
            return self.frame;
        }
        self.layers
            .iter()
            .flat_map(|l| &l.paths)
            .filter_map(ExportPath::bbox)
            .reduce(Aabb::union)
    }

    /// What is drawn, ignoring the frame: each path's bounds grown by its
    /// stroke's reach, caps and miters included (#450, #428), so a thick
    /// stroke is not cut off at the page edge.
    #[must_use]
    pub fn visual_bounds(&self) -> Option<Aabb> {
        self.layers
            .iter()
            .flat_map(|l| l.paths.iter().map(move |p| (l, p)))
            .filter_map(|(l, p)| {
                let b = p.bbox()?;
                let paint = l.paint(p);
                let w = paint.width.max(0.0);
                if l.uses_object_style() && !paint.line.is_default() {
                    return locus_stroke::bounds(&path_chain(p), w, &paint.line);
                }
                let r = Vec2::new(1.0, 1.0) * (w / 2.0);
                Some(Aabb::from_points(b.min - r, b.max + r))
            })
            .reduce(Aabb::union)
    }

    /// The page the writers draw on, as `(anchor, page)` (#450). Coordinates
    /// are measured from the anchor: the frame when there is one (the #443
    /// bed, or the per-layer frame), else the visual bounds. The page is the
    /// anchor grown to everything drawn, so padding for a stroke never moves
    /// the anchor: SVG writes it as a negative `viewBox` origin, PDF as its
    /// `MediaBox` origin.
    #[must_use]
    pub fn page(&self) -> Option<(Aabb, Aabb)> {
        match (self.frame, self.visual_bounds()) {
            (Some(f), Some(v)) => Some((f, f.union(v))),
            (Some(f), None) => Some((f, f)),
            (None, Some(v)) => Some((v, v)),
            (None, None) => None,
        }
    }

    /// Only the geometry of `keep` (an export of the selection, #332): each
    /// layer keeps the segments drawn from those entities, chained again.
    /// The frame is dropped, so the bounds are the selection's.
    #[must_use]
    pub fn only(&self, keep: &std::collections::BTreeSet<EntityId>) -> ExportModel {
        let layers = self
            .layers
            .iter()
            .map(|l| {
                let segs: Vec<PathSeg> = l
                    .paths
                    .iter()
                    .flat_map(|p| p.segs.iter())
                    .filter(|s| keep.contains(&s.entity))
                    .cloned()
                    .collect();
                let mut out = l.clone();
                out.paths = l.chain(segs);
                out.degenerate.retain(|e| keep.contains(e));
                out
            })
            .collect();
        let fills = self
            .fills
            .iter()
            .filter(|f| {
                f.paths
                    .iter()
                    .flat_map(|p| &p.segs)
                    .all(|s| keep.contains(&s.entity))
            })
            .cloned()
            .collect();
        ExportModel {
            layers,
            fills,
            frame: None,
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.layers.iter().all(|l| l.paths.is_empty())
    }

    /// This model with dashed Draw-layer paths split into one path per dash
    /// (#427): every dash for writers without dashes (DXF, plugins such as
    /// HPGL: one pen-down per dash), or only corner-aligned ones for SVG,
    /// PDF and PNG, which draw exact dashes natively.
    #[must_use]
    pub fn dashes_as_paths(&self, corners_only: bool) -> ExportModel {
        let mut m = self.with_extra_strokes();
        for l in &mut m.layers {
            if !l.uses_object_style() {
                continue;
            }
            let mut out = Vec::new();
            for p in std::mem::take(&mut l.paths) {
                let line = l.paint(&p).line;
                // A visual writer draws a profiled stroke's dashes in its
                // outline (#426): each dash takes its part of the profile.
                let split = line.dash.is_some_and(|d| {
                    !corners_only || d.align == locus_stroke::DashAlign::Corners
                }) && !(corners_only && line.varying().is_some());
                match split.then(|| l.dash_paths(&p)).flatten() {
                    Some(ds) => out.extend(ds),
                    None => out.push(p),
                }
            }
            l.paths = out;
        }
        m
    }

    /// This model with every Draw-layer path's extra strokes (#429) added as
    /// paths of their own, after the path, in paint order.
    #[must_use]
    pub fn with_extra_strokes(&self) -> ExportModel {
        let mut m = self.clone();
        for l in &mut m.layers {
            if !l.uses_object_style() {
                continue;
            }
            let mut out = Vec::new();
            for p in std::mem::take(&mut l.paths) {
                let extra = if p.paint.is_none() {
                    l.extra_stroke_paths(&p)
                } else {
                    Vec::new()
                };
                out.push(p);
                out.extend(extra);
            }
            l.paths = out;
        }
        m
    }

    /// Move every path (and the frame) by `d`, mm: the job placement (#443).
    pub fn translate(&mut self, d: Vec2) {
        for seg in self
            .layers
            .iter_mut()
            .flat_map(|l| l.paths.iter_mut())
            .flat_map(|p| p.segs.iter_mut())
        {
            seg.shape = translated(seg.shape, d);
        }
        for seg in self
            .fills
            .iter_mut()
            .flat_map(|f| f.paths.iter_mut())
            .flat_map(|p| p.segs.iter_mut())
        {
            seg.shape = translated(seg.shape, d);
        }
        // A gradient's frame moves with its region (#436).
        for f in &mut self.fills {
            if let Some((_, frame)) = &mut f.gradient {
                frame.origin += d;
            }
        }
        if let Some(f) = &mut self.frame {
            *f = Aabb::from_points(f.min + d, f.max + d);
        }
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
            let mut el = ExportLayer::new(*id, l.name.clone(), l.colour, l.operation);
            el.stroke_styles = l.stroke_styles;
            (el, Vec::new())
        })
        .collect();
    layers.sort_by_key(|(l, _)| {
        let order = doc.layer(l.id).map_or(0, |d| d.export_order);
        (order, l.id.get())
    });

    let mut text_paths: Vec<(locus_doc::LayerId, ExportPath)> = Vec::new();
    for (eid, e) in doc.entities().iter() {
        if e.construction {
            continue;
        }
        if !matches!(e.geom, Geom::Point { .. })
            && !e.style.is_default()
            && let Some((layer, _)) = layers.iter_mut().find(|(l, _)| l.id == e.layer)
        {
            layer.styles.insert(eid, e.style.clone());
        }
        if matches!(e.geom, Geom::Text { .. }) {
            // Text exports as the geometry cached in the document (SPEC §9,
            // §10), so it needs no font. Each contour is already an ordered
            // path, so it bypasses `chain`, which could fuse touching glyphs.
            if let Some(geom) = doc.text_geometry(eid)
                && layers.iter().any(|(l, _)| l.id == e.layer)
            {
                text_paths.extend(
                    geom.contours
                        .iter()
                        .filter_map(|c| text_path(eid, c))
                        .map(|p| (e.layer, p)),
                );
            }
            continue;
        }
        let shape = match &e.geom {
            Geom::Point { .. } | Geom::Text { .. } => continue,
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

    let layers: Vec<ExportLayer> = layers
        .into_iter()
        .filter_map(|(mut l, segs)| {
            l.paths = l.chain(segs);
            let id = l.id;
            l.paths.extend(
                text_paths
                    .iter()
                    .filter(|(layer, _)| *layer == id)
                    .map(|(_, p)| p.clone()),
            );
            (!l.paths.is_empty() || !l.degenerate.is_empty()).then_some(l)
        })
        .collect();
    let fills = collect_fills(doc, &layers);
    ExportModel {
        layers,
        fills,
        ..Default::default()
    }
}

/// Machine layers set to **Styled** (#428, ADR 0019 decision 4) cut, score
/// or engrave the strokes of each object with a line style instead of its
/// centreline: dashes perforate, extra strokes cut in parallel. Plain
/// objects, however wide, keep their centreline (owner, 2026-11-25).
/// Centreline layers, Draw and Reference layers are left alone. A path
/// whose outline can't be built keeps its centreline.
pub fn apply_stroke_styles(model: &mut ExportModel) {
    for layer in &mut model.layers {
        let machine = matches!(
            layer.operation,
            Operation::Cut(_) | Operation::Score | Operation::Engrave(_) | Operation::DragKnife(_)
        );
        if !machine || layer.stroke_styles != StrokeStyles::Styled {
            continue;
        }
        let mut out = Vec::new();
        for p in std::mem::take(&mut layer.paths) {
            let st = p
                .segs
                .first()
                .map_or_else(Style::default, |s| layer.style_of(s.entity));
            // Only line styles (dashes, extra strokes) make an object follow
            // its strokes; a plain object, however wide, cuts its centreline
            // once (owner decision 2026-11-25). Expand to geometry's result is
            // plain, so it isn't outlined a second time (#504).
            if !st.line.machine_styled() {
                out.push(p);
                continue;
            }
            // Extra strokes become parallel cuts or marks (#429), each
            // dashed if it is; dashes become separate cuts (a perforation,
            // #427); otherwise the stroke's outline.
            let styled = if !st.line.extra.is_empty() {
                styled_strokes(&p, &st)
            } else if st.line.dash.is_some() {
                dash_runs(&p, &st.line)
            } else {
                outline_paths(&p, st.width, &st.line)
            };
            match styled {
                Some(paths) if !paths.is_empty() => out.extend(paths),
                _ => out.push(p),
            }
        }
        layer.paths = out;
    }
}

/// The dashes of `path` under `line`, as open centreline paths.
fn dash_runs(path: &ExportPath, line: &LineStyle) -> Option<Vec<ExportPath>> {
    let entity = path.segs.first()?.entity;
    let d = line.dash?;
    let runs =
        locus_stroke::dashes(&path_chain(path), &d, &locus_stroke::Limits::default()).ok()?;
    Some(
        runs.iter()
            .filter(|c| !c.segs.is_empty())
            .map(|c| ExportPath {
                segs: c.segs.iter().map(|s| seg_path(*s, entity)).collect(),
                closed: false,
                paint: None,
            })
            .collect(),
    )
}

/// Every stroke of `path` (its own and the extras, #429) as centreline
/// paths for a machine: each at its offset, dashes split.
fn styled_strokes(path: &ExportPath, st: &Style) -> Option<Vec<ExportPath>> {
    let entity = path.segs.first()?.entity;
    let strokes = locus_stroke::layer_strokes(
        &path_chain(path),
        st.width,
        &st.line,
        &locus_stroke::Limits::default(),
    )
    .ok()?;
    let mut out = Vec::new();
    for ls in strokes {
        for c in ls.chains {
            let p = ExportPath {
                segs: c.segs.iter().map(|s| seg_path(*s, entity)).collect(),
                closed: c.closed,
                paint: None,
            };
            match dash_runs(&p, &ls.line) {
                Some(ds) => out.extend(ds),
                None => out.push(p),
            }
        }
    }
    Some(out)
}

/// An export path as the pieces a stroke follows: lines and arcs in travel
/// order (Béziers as biarcs within [`EXPORT_TOL`]).
#[must_use]
pub fn path_chain(p: &ExportPath) -> locus_stroke::Chain {
    use locus_region::Seg;
    let arc = |a: &Arc| Seg::Arc {
        center: a.center,
        radius: a.radius(),
        start: a.start_angle(),
        sweep: a.sweep(),
    };
    let mut segs = Vec::new();
    for s in &p.segs {
        let mut part: Vec<Seg> = match s.shape {
            Shape::Line(l) => vec![Seg::Line { a: l.a, b: l.b }],
            Shape::Arc(a) => vec![arc(&a)],
            Shape::Circle(c) => vec![Seg::Arc {
                center: c.center,
                radius: c.radius,
                start: 0.0,
                sweep: core::f64::consts::TAU,
            }],
            // Biarc pieces are stored CCW: orient each along the curve.
            Shape::Cubic(c) => c
                .to_pieces_oriented(EXPORT_TOL)
                .into_iter()
                .map(|(pc, back)| match pc {
                    Piece::Line(l) => Seg::Line { a: l.a, b: l.b },
                    Piece::Arc(a) => {
                        if back {
                            arc(&a).reversed()
                        } else {
                            arc(&a)
                        }
                    }
                })
                .collect(),
        };
        if s.reversed {
            part.reverse();
            for q in &mut part {
                *q = q.reversed();
            }
        }
        segs.extend(part);
    }
    locus_stroke::Chain {
        segs,
        closed: p.closed,
    }
}

/// The outline of `path` stroked at `width` with `line`, as closed export
/// paths; `None` when it can't be built.
#[must_use]
pub fn outline_paths(path: &ExportPath, width: f64, line: &LineStyle) -> Option<Vec<ExportPath>> {
    let entity = path.segs.first()?.entity;
    let area = locus_stroke::expand(
        &path_chain(path),
        width,
        line,
        &locus_stroke::Limits::default(),
    )
    .ok()?;
    Some(
        area.loops
            .iter()
            .filter_map(loop_path)
            .map(|mut p| {
                // Keep the source entity, so validation and selection export
                // still know where each piece came from.
                for s in &mut p.segs {
                    s.entity = entity;
                }
                p
            })
            .collect(),
    )
}

/// The outline of `path` stroked at `width` with `line` for drawing (#426):
/// variable-width spans keep their fitted cubics within `EXPORT_TOL` (ADR
/// 0019 option (b)). `None` when it can't be built.
#[must_use]
pub fn outline_curves(path: &ExportPath, width: f64, line: &LineStyle) -> Option<Vec<ExportPath>> {
    let entity = path.segs.first()?.entity;
    let o = locus_stroke::outline(
        &path_chain(path),
        width,
        line,
        locus_geom::EXPORT_TOL,
        &locus_stroke::Limits::default(),
    )
    .ok()?;
    Some(
        o.loops
            .iter()
            .filter(|l| !l.is_empty())
            .map(|l| ExportPath {
                segs: l
                    .iter()
                    .map(|piece| match *piece {
                        locus_stroke::OutPiece::Seg(seg) => seg_path(seg, entity),
                        locus_stroke::OutPiece::Cubic(c) => PathSeg {
                            entity,
                            shape: Shape::Cubic(c),
                            reversed: false,
                        },
                    })
                    .collect(),
                closed: true,
                paint: None,
            })
            .collect(),
    )
}

/// `shape` moved by `d`.
fn translated(shape: Shape, d: Vec2) -> Shape {
    match shape {
        Shape::Line(l) => Shape::Line(Line::new(l.a + d, l.b + d)),
        Shape::Arc(a) => Shape::Arc(Arc {
            center: a.center + d,
            start: a.start + d,
            end: a.end + d,
        }),
        Shape::Circle(c) => Shape::Circle(Circle {
            center: c.center + d,
            radius: c.radius,
        }),
        Shape::Cubic(c) => Shape::Cubic(Cubic {
            p0: c.p0 + d,
            c0: c.c0 + d,
            c1: c.c1 + d,
            p1: c.p1 + d,
        }),
    }
}

/// One region piece as an export segment: a clockwise arc is the
/// counter-clockwise arc travelled backwards.
fn seg_path(seg: locus_region::Seg, entity: EntityId) -> PathSeg {
    let (shape, reversed) = match seg {
        locus_region::Seg::Line { a, b } => (Shape::Line(Line::new(a, b)), false),
        locus_region::Seg::Arc {
            center,
            radius,
            start,
            sweep,
        } => {
            let at = |t: f64| center + Vec2::from_angle(t) * radius;
            if sweep.abs() >= core::f64::consts::TAU - 1e-9 {
                (Shape::Circle(Circle { center, radius }), false)
            } else if sweep >= 0.0 {
                (
                    Shape::Arc(Arc {
                        center,
                        start: at(start),
                        end: at(start + sweep),
                    }),
                    false,
                )
            } else {
                (
                    Shape::Arc(Arc {
                        center,
                        start: at(start + sweep),
                        end: at(start),
                    }),
                    true,
                )
            }
        }
    };
    PathSeg {
        entity,
        shape,
        reversed,
    }
}

/// A region loop as a closed export path (#451). Region arcs carry a signed
/// sweep; a clockwise one is the counter-clockwise arc travelled backwards.
fn loop_path(l: &locus_region::Loop) -> Option<ExportPath> {
    let segs: Vec<PathSeg> = l
        .edges
        .iter()
        .map(|e| {
            let entity = EntityId::new(e.source);
            let (shape, reversed) = match e.seg {
                locus_region::Seg::Line { a, b } => (Shape::Line(Line::new(a, b)), false),
                locus_region::Seg::Arc {
                    center,
                    radius,
                    start,
                    sweep,
                } => {
                    let at = |t: f64| center + Vec2::from_angle(t) * radius;
                    if sweep.abs() >= core::f64::consts::TAU - 1e-9 {
                        (Shape::Circle(Circle { center, radius }), false)
                    } else if sweep >= 0.0 {
                        (
                            Shape::Arc(Arc {
                                center,
                                start: at(start),
                                end: at(start + sweep),
                            }),
                            false,
                        )
                    } else {
                        (
                            Shape::Arc(Arc {
                                center,
                                start: at(start + sweep),
                                end: at(start),
                            }),
                            true,
                        )
                    }
                }
            };
            PathSeg {
                entity,
                shape,
                reversed,
            }
        })
        .collect();
    (!segs.is_empty()).then_some(ExportPath {
        segs,
        closed: true,
        paint: Default::default(),
    })
}

/// The document's region fills (#451) on exported Draw layers. Machine
/// layers write no fills: their software reads every path as a cut, and
/// fills stay visual (SPEC §7). Regions are per layer and resolved by the
/// same function as the canvas, with the stored colour and opacity (#457).
fn collect_fills(doc: &Document, layers: &[ExportLayer]) -> Vec<ExportFill> {
    if doc.region_fills().is_empty() {
        return Vec::new();
    }
    let regions = locus_doc::regions::regions(doc);
    let fills = locus_doc::region_fill::resolve(doc, &regions);
    regions
        .into_iter()
        .zip(fills)
        .filter_map(|(r, fill)| {
            let fill = fill?;
            let colour = fill.painted()?;
            let layer = locus_doc::regions::layer_of(doc, &r)?;
            layers
                .iter()
                .find(|l| l.id == layer && l.uses_object_style())?;
            let mut paths: Vec<ExportPath> = loop_path(&r.outer).into_iter().collect();
            paths.extend(r.holes.iter().filter_map(loop_path));
            let gradient = fill
                .gradient
                .as_deref()
                .and_then(|g| locus_doc::gradient::region_frame(&r, g).map(|f| (g.clone(), f)));
            Some(ExportFill {
                layer,
                paths,
                colour,
                opacity: fill.opacity.clamp(0.0, 1.0),
                gradient,
            })
        })
        .collect()
}

/// One cached text contour as an export path (SPEC §9). Zero-length or
/// non-finite segments are dropped, so an exporter never sees a degenerate one.
fn text_path(entity: EntityId, c: &locus_text::Contour) -> Option<ExportPath> {
    let mut from = c.start;
    let mut segs = Vec::with_capacity(c.segs.len());
    for s in &c.segs {
        let shape = match *s {
            locus_text::Seg::Line { end } => Shape::Line(Line::new(from, end)),
            locus_text::Seg::Cubic { c0, c1, end } => Shape::Cubic(Cubic::new(from, c0, c1, end)),
        };
        from = s.end();
        let b = shape.bbox();
        if shape.is_degenerate() || !b.min.is_finite() || !b.max.is_finite() {
            continue;
        }
        segs.push(PathSeg {
            entity,
            shape,
            reversed: false,
        });
    }
    (!segs.is_empty()).then_some(ExportPath {
        segs,
        closed: c.closed,
        paint: Default::default(),
    })
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
                paint: Default::default(),
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
            paint: Default::default(),
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
        let mut l = ExportLayer::new(
            LayerId::new(1),
            "L".into(),
            [0, 0, 0],
            op.default_operation(),
        );
        l.paths = chain(segs);
        l
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
