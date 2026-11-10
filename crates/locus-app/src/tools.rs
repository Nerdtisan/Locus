// SPDX-License-Identifier: AGPL-3.0-or-later
//! The M1 drawing tools (#68, SPEC §6.1), independent of the GUI.
//!
//! A [`Draft`] is the in-progress state of one tool: the clicks so far. Each
//! click, typed value or finish returns an [`Outcome`]; a completed shape is
//! a single [`Command`] (a labelled compound where it needs several entities)
//! for the caller to run through the `Sketcher` (SPEC §11). The draft never
//! touches the document.
//!
//! A compound cannot refer to entities another command in it creates, so the
//! plan predicts their IDs: the document hands out IDs sequentially
//! (ADR 0003), and a compound runs its commands in order.

use crate::scene::Shape;
use crate::shapes;
use locus_doc::{Command, Constraint, Dimension, Document, EntityId, Geom, LayerId};
use locus_geom::{Arc, Circle, Line, Vec2};

/// Shorter than this is "the same place": no zero-length lines or radii.
const MIN_SIZE_MM: f64 = 1e-6;
/// A three-point arc through nearly collinear points would be a huge circle.
const MAX_ARC_RADIUS_MM: f64 = 1e6;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Select,
    Point,
    Line,
    Circle,
    /// Start, end, then a point the arc passes through.
    ArcThreePoint,
    /// Centre, start (sets the radius), then end.
    ArcCentre,
    /// Two opposite corners (#155).
    RectCorner,
    /// Centre, then a corner (#155).
    RectCentre,
    /// An edge, then a point on the far edge: a rotated rectangle (#155).
    RectThree,
    /// The two ends of a diameter (#156).
    Circle2,
    /// Three points on the circle (#156).
    Circle3,
    /// From the end of a line or arc, tangent to it (#156).
    ArcTangent,
    /// Centre, then a vertex or a side's middle; sides set in the tool bar (#157).
    Polygon,
    /// Centre, centre, then the width (#158).
    Slot,
}

impl Tool {
    pub const DRAWING: [Tool; 13] = [
        Tool::Point,
        Tool::Line,
        Tool::RectCorner,
        Tool::RectCentre,
        Tool::RectThree,
        Tool::Circle,
        Tool::Circle2,
        Tool::Circle3,
        Tool::ArcThreePoint,
        Tool::ArcCentre,
        Tool::ArcTangent,
        Tool::Polygon,
        Tool::Slot,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Tool::Select => "Select",
            Tool::Point => "Point",
            Tool::Line => "Line",
            Tool::Circle => "Circle (centre)",
            Tool::RectCorner => "Rectangle (2-point)",
            Tool::RectCentre => "Rectangle (centre)",
            Tool::RectThree => "Rectangle (3-point)",
            Tool::Circle2 => "Circle (2-point)",
            Tool::Circle3 => "Circle (3-point)",
            Tool::ArcTangent => "Arc (tangent)",
            Tool::Polygon => "Polygon",
            Tool::Slot => "Slot",
            Tool::ArcThreePoint => "Arc (3-point)",
            Tool::ArcCentre => "Arc (centre)",
        }
    }

    /// Single-key shortcut shown in menus and tooltips.
    pub fn key(self) -> Option<char> {
        match self {
            Tool::Select => Some('S'),
            Tool::Point => Some('P'),
            Tool::Line => Some('L'),
            Tool::Circle => Some('C'),
            Tool::ArcThreePoint => Some('A'),
            Tool::ArcCentre => Some('R'),
            Tool::RectCorner => Some('B'),
            Tool::ArcTangent => Some('I'),
            Tool::Polygon => Some('G'),
            Tool::Slot => Some('U'),
            Tool::RectCentre | Tool::RectThree | Tool::Circle2 | Tool::Circle3 => None,
        }
    }

    /// What the next click does, for the status line.
    pub fn prompt(self, clicks: usize) -> &'static str {
        match (self, clicks) {
            (Tool::Select, _) => "Click to select, drag for a box",
            (Tool::Point, _) => "Click to place a point",
            (Tool::Line, 0) => "Line: click the start point",
            (Tool::Line, _) => "Line: click the next point (Enter finishes, Esc cancels)",
            (Tool::Circle, 0) => "Circle: click the centre",
            (Tool::Circle, _) => "Circle: click a point on the circle, or type a radius",
            (Tool::ArcThreePoint, 0) => "Arc: click the start point",
            (Tool::ArcThreePoint, 1) => "Arc: click the end point",
            (Tool::ArcThreePoint, _) => "Arc: click a point the arc passes through",
            (Tool::ArcCentre, 0) => "Arc: click the centre",
            (Tool::ArcCentre, 1) => "Arc: click the start point, or type a radius",
            (Tool::ArcCentre, _) => "Arc: click the end angle",
            (Tool::RectCorner, 0) => "Rectangle: click a corner",
            (Tool::RectCorner, _) => {
                "Rectangle: click the opposite corner, or type width, then height"
            }
            (Tool::RectCentre, 0) => "Rectangle: click the centre",
            (Tool::RectCentre, _) => "Rectangle: click a corner, or type width, then height",
            (Tool::RectThree, 0) => "Rectangle: click the first edge's start",
            (Tool::RectThree, 1) => "Rectangle: click the edge's end, or type its length",
            (Tool::RectThree, _) => "Rectangle: click a point on the far edge, or type the height",
            (Tool::Circle2, 0) => "Circle: click one end of a diameter",
            (Tool::Circle2, _) => "Circle: click the other end",
            (Tool::Circle3, 0) => "Circle: click the first point",
            (Tool::Circle3, 1) => "Circle: click the second point",
            (Tool::Circle3, _) => "Circle: click the third point",
            (Tool::ArcTangent, 0) => "Arc: click the end of a line or arc to continue",
            (Tool::ArcTangent, _) => "Arc: click the arc's end",
            (Tool::Polygon, 0) => "Polygon: click the centre",
            (Tool::Polygon, _) => "Polygon: click a vertex (or side middle), or type a radius",
            (Tool::Slot, 0) => "Slot: click the first centre",
            (Tool::Slot, 1) => "Slot: click the second centre",
            (Tool::Slot, _) => "Slot: click to set the width, or type it",
        }
    }
}

/// A constraint the line being drawn should be created with (#69).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LineHint {
    Horizontal,
    Vertical,
    /// Tangent to this arc at the line's start point.
    Tangent(EntityId),
}

impl LineHint {
    /// Badge shown beside the cursor.
    pub fn badge(self) -> &'static str {
        match self {
            LineHint::Horizontal => "H",
            LineHint::Vertical => "V",
            LineHint::Tangent(_) => "T",
        }
    }
}

/// A curve ending at a picked point and the direction it leaves the curve
/// there (unit, pointing away from the curve): where a tangent arc continues.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Host {
    pub curve: EntityId,
    pub dir: Vec2,
}

/// What the cursor designates: a position (already adjusted by inference),
/// the existing point entity it snapped onto, the curve it lies on, and the
/// constraint the line being drawn gets (#69).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pick {
    /// The line or arc that ends at `point`, for the tangent arc tool (#156).
    pub host: Option<Host>,
    pub pos: Vec2,
    pub point: Option<EntityId>,
    pub on_curve: Option<EntityId>,
    pub line: Option<LineHint>,
}

impl Pick {
    pub fn free(pos: Vec2) -> Self {
        Self {
            host: None,
            pos,
            point: None,
            on_curve: None,
            line: None,
        }
    }

    pub fn at_point(pos: Vec2, id: EntityId) -> Self {
        Self {
            point: Some(id),
            ..Self::free(pos)
        }
    }
}

/// A clicked location: an existing point entity (reused, so the shapes share
/// it), or a position that becomes a new point when the shape is committed.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Node {
    Existing(EntityId, Vec2),
    New(Vec2),
    /// A new point constrained onto an existing curve.
    OnCurve(Vec2, EntityId),
}

impl Node {
    fn from_pick(p: Pick) -> Self {
        match (p.point, p.on_curve) {
            (Some(id), _) => Node::Existing(id, p.pos),
            (None, Some(curve)) => Node::OnCurve(p.pos, curve),
            (None, None) => Node::New(p.pos),
        }
    }

    fn pos(self) -> Vec2 {
        match self {
            Node::Existing(_, p) | Node::New(p) | Node::OnCurve(p, _) => p,
        }
    }

    fn id(self) -> Option<EntityId> {
        match self {
            Node::Existing(id, _) => Some(id),
            Node::New(_) | Node::OnCurve(..) => None,
        }
    }
}

/// The result of a click, typed value or finish.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// Nothing to run yet (or the input was ignored).
    Pending,
    /// A finished shape: run it as one undo step.
    Commit(Command),
    /// The input can't form a shape; say why.
    Rejected(&'static str),
}

/// Builds one compound, predicting the IDs of what it creates.
struct Plan {
    next: u64,
    layer: LayerId,
    commands: Vec<Command>,
}

impl Plan {
    fn new(doc_next_id: u64, layer: LayerId) -> Self {
        Self {
            next: doc_next_id,
            layer,
            commands: Vec::new(),
        }
    }

    fn alloc(&mut self) -> u64 {
        let id = self.next;
        self.next += 1;
        id
    }

    fn add(&mut self, geom: Geom) -> EntityId {
        self.commands.push(Command::AddEntity {
            layer: self.layer,
            construction: false,
            geom,
        });
        EntityId::new(self.alloc())
    }

    fn construction(&mut self, geom: Geom) -> EntityId {
        self.commands.push(Command::AddEntity {
            layer: self.layer,
            construction: true,
            geom,
        });
        EntityId::new(self.alloc())
    }

    fn line(&mut self, a: EntityId, b: EntityId) -> EntityId {
        self.add(Geom::Line { start: a, end: b })
    }

    fn point(&mut self, pos: Vec2) -> EntityId {
        self.add(Geom::Point { pos })
    }

    fn node(&mut self, n: Node) -> EntityId {
        match n {
            Node::Existing(id, _) => id,
            Node::New(pos) => self.point(pos),
            Node::OnCurve(pos, curve) => {
                let point = self.point(pos);
                self.constrain(Constraint::PointOnCurve { point, curve });
                point
            }
        }
    }

    fn constrain(&mut self, constraint: Constraint) {
        self.commands.push(Command::AddConstraint { constraint });
        self.alloc();
    }

    fn finish(self, label: &str) -> Command {
        Command::Compound {
            label: label.to_owned(),
            commands: self.commands,
        }
    }
}

/// The clicks so far of the active drawing tool.
#[derive(Clone, Debug)]
pub struct Draft {
    tool: Tool,
    nodes: Vec<Node>,
    /// First point of the current polyline, so a click on it closes the loop.
    chain_first: Option<EntityId>,
    /// Layer new geometry goes on (the active layer, SPEC §4).
    layer: LayerId,
    /// Dimensions typed so far for the current shape (width, then height).
    dims: [Option<f64>; 2],
    /// The curve a tangent arc continues.
    host: Option<Host>,
    sides: usize,
    circumscribed: bool,
}

impl Draft {
    pub fn new(tool: Tool) -> Self {
        Self {
            tool,
            nodes: Vec::new(),
            chain_first: None,
            layer: Document::default_layer(),
            dims: [None, None],
            host: None,
            sides: 6,
            circumscribed: false,
        }
    }

    /// Sides and kind of the polygon tool (#157).
    pub fn polygon(&self) -> (usize, bool) {
        (self.sides, self.circumscribed)
    }

    pub fn set_polygon(&mut self, sides: usize, circumscribed: bool) {
        self.sides = sides.clamp(*shapes::POLYGON_SIDES.start(), *shapes::POLYGON_SIDES.end());
        self.circumscribed = circumscribed;
    }

    /// Put new geometry on `layer`.
    pub fn set_layer(&mut self, layer: LayerId) {
        self.layer = layer;
    }

    pub fn tool(&self) -> Tool {
        self.tool
    }

    /// Switch tools, dropping any half-drawn shape.
    pub fn set_tool(&mut self, tool: Tool) {
        self.tool = tool;
        self.cancel();
    }

    /// The start of the line being drawn, for inference (#69).
    pub fn line_anchor(&self) -> Option<(Vec2, Option<EntityId>)> {
        match (self.tool, self.nodes.first()) {
            (Tool::Line, Some(n)) => Some((n.pos(), n.id())),
            _ => None,
        }
    }

    /// May the next point snap onto a curve (becoming point-on-curve)?
    pub fn curve_snap(&self) -> bool {
        match self.tool {
            Tool::Select => false,
            Tool::Point | Tool::Line => true,
            Tool::Circle | Tool::ArcCentre | Tool::Polygon => self.nodes.is_empty(),
            Tool::RectCorner | Tool::RectCentre | Tool::RectThree | Tool::Slot => {
                self.nodes.len() < 2
            }
            Tool::Circle2 | Tool::Circle3 => true,
            Tool::ArcTangent => false,
            // The "through" click is only a guide.
            Tool::ArcThreePoint => self.nodes.len() < 2,
        }
    }

    /// Clicks made so far for the current shape.
    pub fn clicks(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_drawing(&self) -> bool {
        !self.nodes.is_empty()
    }

    /// Esc: throw the half-drawn shape away. Shapes already committed stay.
    pub fn cancel(&mut self) {
        self.nodes.clear();
        self.chain_first = None;
        self.dims = [None, None];
        self.host = None;
    }

    /// Enter: end the polyline (or abandon a half-drawn shape).
    pub fn finish(&mut self) {
        self.cancel();
    }

    pub fn click(&mut self, pick: Pick, doc_next_id: u64) -> Outcome {
        let here = Node::from_pick(pick);
        let mut plan = Plan::new(doc_next_id, self.layer);
        match self.tool {
            Tool::Select => Outcome::Pending,
            Tool::Point => {
                // A point already there needs no second one.
                if here.id().is_some() {
                    return Outcome::Pending;
                }
                plan.node(here);
                Outcome::Commit(plan.finish("Add point"))
            }
            Tool::Line => self.click_line(here, plan, pick.line),
            Tool::Circle => self.click_circle(here, plan),
            Tool::ArcCentre => self.click_arc_centre(pick, here, plan),
            Tool::ArcThreePoint => self.click_arc_three(here, plan),
            Tool::RectCorner | Tool::RectCentre | Tool::RectThree => {
                self.click_rect(here, pick.pos, plan)
            }
            Tool::Circle2 => self.click_circle2(here, plan),
            Tool::Circle3 => self.click_circle3(here, plan),
            Tool::ArcTangent => self.click_arc_tangent(pick, here, plan),
            Tool::Polygon => self.click_polygon(here, plan, None),
            Tool::Slot => self.click_slot(here, plan, None),
        }
    }

    fn click_line(&mut self, here: Node, plan: Plan, hint: Option<LineHint>) -> Outcome {
        let Some(&from) = self.nodes.first() else {
            self.nodes.push(here);
            self.chain_first = here.id();
            return Outcome::Pending;
        };
        self.segment(from, here, plan, None, hint)
    }

    /// Commit the segment `from → to` and carry on from `to` (or close).
    fn segment(
        &mut self,
        from: Node,
        to: Node,
        mut plan: Plan,
        length: Option<f64>,
        hint: Option<LineHint>,
    ) -> Outcome {
        if from.pos().distance(to.pos()) < MIN_SIZE_MM
            || (from.id().is_some() && from.id() == to.id())
        {
            return Outcome::Pending;
        }
        let a = plan.node(from);
        let b = plan.node(to);
        let line = plan.add(Geom::Line { start: a, end: b });
        match hint {
            Some(LineHint::Horizontal) => plan.constrain(Constraint::Horizontal { line }),
            Some(LineHint::Vertical) => plan.constrain(Constraint::Vertical { line }),
            Some(LineHint::Tangent(arc)) => plan.constrain(Constraint::Tangent { a: line, b: arc }),
            None => {}
        }
        if let Some(d) = length {
            plan.constrain(Constraint::Distance {
                a,
                b,
                dim: Dimension::driving(d),
            });
        }
        let closing = to.id().is_some() && to.id() == self.chain_first;
        if closing {
            self.cancel();
        } else {
            self.chain_first = self.chain_first.or(Some(a));
            self.nodes = vec![Node::Existing(b, to.pos())];
        }
        Outcome::Commit(plan.finish("Add line"))
    }

    fn click_circle(&mut self, here: Node, mut plan: Plan) -> Outcome {
        let Some(&centre) = self.nodes.first() else {
            self.nodes.push(here);
            return Outcome::Pending;
        };
        let radius = centre.pos().distance(here.pos());
        if radius < MIN_SIZE_MM {
            return Outcome::Pending;
        }
        let c = plan.node(centre);
        plan.add(Geom::Circle { center: c, radius });
        self.cancel();
        Outcome::Commit(plan.finish("Add circle"))
    }

    fn click_arc_centre(&mut self, pick: Pick, here: Node, mut plan: Plan) -> Outcome {
        match self.nodes.len() {
            0 => {
                self.nodes.push(here);
                Outcome::Pending
            }
            1 => {
                if self.nodes[0].pos().distance(here.pos()) < MIN_SIZE_MM {
                    return Outcome::Pending;
                }
                self.nodes.push(here);
                Outcome::Pending
            }
            _ => {
                let (centre, start) = (self.nodes[0], self.nodes[1]);
                let r = centre.pos().distance(start.pos());
                let dir = pick.pos - centre.pos();
                let Some(unit) = dir.normalized(MIN_SIZE_MM) else {
                    return Outcome::Pending;
                };
                // The end point is always new, on the circle; its angle is
                // the cursor's.
                let end = Node::New(centre.pos() + unit * r);
                let (c, s, e) = (plan.node(centre), plan.node(start), plan.node(end));
                plan.add(Geom::Arc {
                    center: c,
                    start: s,
                    end: e,
                });
                self.cancel();
                Outcome::Commit(plan.finish("Add arc"))
            }
        }
    }

    fn click_arc_three(&mut self, here: Node, mut plan: Plan) -> Outcome {
        if self.nodes.len() < 2 {
            if self
                .nodes
                .iter()
                .any(|n| n.pos().distance(here.pos()) < MIN_SIZE_MM)
            {
                return Outcome::Pending;
            }
            self.nodes.push(here);
            return Outcome::Pending;
        }
        let (a, b) = (self.nodes[0], self.nodes[1]);
        let Some((centre, ccw)) = circle_through(a.pos(), here.pos(), b.pos()) else {
            return Outcome::Rejected(
                "Those three points are in a line: no arc passes through them",
            );
        };
        // An arc runs counter-clockwise from its start to its end; if the
        // clicks went clockwise round the circle, swap the ends.
        let (start, end) = if ccw { (a, b) } else { (b, a) };
        let c = plan.point(centre);
        let (s, e) = (plan.node(start), plan.node(end));
        plan.add(Geom::Arc {
            center: c,
            start: s,
            end: e,
        });
        // The "through" click is only a guide, not geometry.
        self.cancel();
        Outcome::Commit(plan.finish("Add arc"))
    }

    /// The second corner of a corner/centre rectangle, honouring typed sizes.
    fn rect_corner(&self, cursor: Vec2) -> Vec2 {
        let c0 = self.nodes[0].pos();
        let d = cursor - c0;
        let k = if self.tool == Tool::RectCentre {
            0.5
        } else {
            1.0
        };
        let sign = |v: f64| if v < 0.0 { -1.0 } else { 1.0 };
        Vec2::new(
            c0.x + self.dims[0].map_or(d.x, |w| sign(d.x) * w * k),
            c0.y + self.dims[1].map_or(d.y, |h| sign(d.y) * h * k),
        )
    }

    fn click_rect(&mut self, here: Node, cursor: Vec2, plan: Plan) -> Outcome {
        match (self.tool, self.nodes.len()) {
            (_, 0) => {
                self.nodes.push(here);
                Outcome::Pending
            }
            (Tool::RectThree, 1) => {
                let a = self.nodes[0];
                if a.pos().distance(here.pos()) < MIN_SIZE_MM
                    || (a.id().is_some() && a.id() == here.id())
                {
                    return Outcome::Pending;
                }
                self.nodes.push(here);
                Outcome::Pending
            }
            (Tool::RectThree, _) => self.commit_rect3(cursor, plan),
            _ => {
                let second = if self.dims == [None, None] {
                    here
                } else {
                    Node::New(self.rect_corner(cursor))
                };
                self.commit_rect(second, plan)
            }
        }
    }

    fn commit_rect(&mut self, second: Node, mut plan: Plan) -> Outcome {
        let first = self.nodes[0];
        let (c0, c1) = (first.pos(), second.pos());
        let centred = self.tool == Tool::RectCentre;
        let k = if centred {
            shapes::rect_corners_centred(c0, c1)
        } else {
            shapes::rect_corners(c0, c1)
        };
        if k[0].distance(k[1]) < MIN_SIZE_MM || k[0].distance(k[3]) < MIN_SIZE_MM {
            return Outcome::Pending;
        }
        let (nodes, centre) = if centred {
            (
                [Node::New(k[0]), Node::New(k[1]), second, Node::New(k[3])],
                Some(first),
            )
        } else {
            ([first, Node::New(k[1]), second, Node::New(k[3])], None)
        };
        let dims = self.dims;
        build_rect(&mut plan, nodes, false, centre, dims);
        self.cancel();
        Outcome::Commit(plan.finish("Add rectangle"))
    }

    fn commit_rect3(&mut self, height_pt: Vec2, mut plan: Plan) -> Outcome {
        let (a, b) = (self.nodes[0], self.nodes[1]);
        let Some(k) = shapes::rect_corners_rotated(a.pos(), b.pos(), height_pt) else {
            return Outcome::Pending;
        };
        let dims = self.dims;
        build_rect(
            &mut plan,
            [a, b, Node::New(k[2]), Node::New(k[3])],
            true,
            None,
            dims,
        );
        self.cancel();
        Outcome::Commit(plan.finish("Add rectangle"))
    }

    /// A size typed for a rectangle: width then height (corner and centre
    /// variants), or edge length then height (3-point).
    fn typed_rect(&mut self, value: f64, cursor: Pick, next_id: u64) -> Outcome {
        let Some(&first) = self.nodes.first() else {
            return Outcome::Pending;
        };
        let plan = Plan::new(next_id, self.layer);
        if self.tool == Tool::RectThree {
            if self.nodes.len() == 1 {
                let dir = (cursor.pos - first.pos())
                    .normalized(MIN_SIZE_MM)
                    .unwrap_or(Vec2::new(1.0, 0.0));
                self.nodes.push(Node::New(first.pos() + dir * value));
                self.dims[0] = Some(value);
                return Outcome::Pending;
            }
            let (a, b) = (first.pos(), self.nodes[1].pos());
            let Some(u) = (b - a).normalized(MIN_SIZE_MM) else {
                return Outcome::Pending;
            };
            let side = if (cursor.pos - a).dot(u.perp()) < 0.0 {
                -1.0
            } else {
                1.0
            };
            self.dims[1] = Some(value);
            return self.commit_rect3(a + u.perp() * (side * value), plan);
        }
        let slot = usize::from(self.dims[0].is_some());
        self.dims[slot] = Some(value);
        if slot == 0 {
            return Outcome::Pending;
        }
        let second = Node::New(self.rect_corner(cursor.pos));
        self.commit_rect(second, plan)
    }

    fn click_circle2(&mut self, here: Node, mut plan: Plan) -> Outcome {
        let Some(&a) = self.nodes.first() else {
            self.nodes.push(here);
            return Outcome::Pending;
        };
        let Some((centre, radius)) = shapes::circle_by_diameter(a.pos(), here.pos()) else {
            return Outcome::Pending;
        };
        let (p, q) = (plan.node(a), plan.node(here));
        let c = plan.point(centre);
        let circle = plan.add(Geom::Circle { center: c, radius });
        // The centre is the midpoint of the diameter and one end is on the
        // circle; the other end follows (a second on-circle row would be
        // redundant and refused).
        let diameter = plan.construction(Geom::Line { start: p, end: q });
        plan.constrain(Constraint::Midpoint {
            point: c,
            line: diameter,
        });
        plan.constrain(Constraint::PointOnCurve {
            point: p,
            curve: circle,
        });
        self.cancel();
        Outcome::Commit(plan.finish("Add circle"))
    }

    fn click_circle3(&mut self, here: Node, mut plan: Plan) -> Outcome {
        if self.nodes.len() < 2 {
            if self
                .nodes
                .iter()
                .any(|n| n.pos().distance(here.pos()) < MIN_SIZE_MM)
            {
                return Outcome::Pending;
            }
            self.nodes.push(here);
            return Outcome::Pending;
        }
        let (a, b) = (self.nodes[0], self.nodes[1]);
        let Some((centre, _)) = circle_through(a.pos(), b.pos(), here.pos()) else {
            return Outcome::Rejected(
                "Those three points are in a line: no circle passes through them",
            );
        };
        let radius = centre.distance(a.pos());
        let (pa, pb, pc) = (plan.node(a), plan.node(b), plan.node(here));
        let c = plan.point(centre);
        let circle = plan.add(Geom::Circle { center: c, radius });
        for point in [pa, pb, pc] {
            plan.constrain(Constraint::PointOnCurve {
                point,
                curve: circle,
            });
        }
        self.cancel();
        Outcome::Commit(plan.finish("Add circle"))
    }

    fn click_arc_tangent(&mut self, pick: Pick, here: Node, mut plan: Plan) -> Outcome {
        let Some(&start) = self.nodes.first() else {
            return match (pick.host, here) {
                (Some(host), Node::Existing(..)) => {
                    self.nodes.push(here);
                    self.host = Some(host);
                    Outcome::Pending
                }
                _ => Outcome::Rejected("Click the end of a line or arc to continue it"),
            };
        };
        let Some(host) = self.host else {
            self.cancel();
            return Outcome::Pending;
        };
        if start.pos().distance(here.pos()) < MIN_SIZE_MM {
            return Outcome::Pending;
        }
        let Some((centre, ccw)) = shapes::tangent_arc(start.pos(), host.dir, here.pos()) else {
            return Outcome::Rejected("That point is straight ahead: a tangent arc has to turn");
        };
        let (s, e) = (plan.node(start), plan.node(here));
        let c = plan.point(centre);
        let arc = plan.add(if ccw {
            Geom::Arc {
                center: c,
                start: s,
                end: e,
            }
        } else {
            Geom::Arc {
                center: c,
                start: e,
                end: s,
            }
        });
        plan.constrain(Constraint::Tangent {
            a: arc,
            b: host.curve,
        });
        self.cancel();
        Outcome::Commit(plan.finish("Add tangent arc"))
    }

    fn click_polygon(&mut self, here: Node, mut plan: Plan, radius: Option<f64>) -> Outcome {
        let Some(&centre) = self.nodes.first() else {
            self.nodes.push(here);
            return Outcome::Pending;
        };
        let Some((pts, r)) =
            shapes::polygon(centre.pos(), here.pos(), self.sides, self.circumscribed)
        else {
            return Outcome::Pending;
        };
        let c = plan.node(centre);
        let circle = plan.construction(Geom::Circle {
            center: c,
            radius: r,
        });
        let ps: Vec<EntityId> = pts.iter().map(|p| plan.point(*p)).collect();
        let n = ps.len();
        let lines: Vec<EntityId> = (0..n).map(|i| plan.line(ps[i], ps[(i + 1) % n])).collect();
        for i in 0..n {
            plan.constrain(if self.circumscribed {
                Constraint::Tangent {
                    a: lines[i],
                    b: circle,
                }
            } else {
                Constraint::PointOnCurve {
                    point: ps[i],
                    curve: circle,
                }
            });
        }
        // n sides, n − 1 equalities chaining them all: regular, free to
        // rotate and scale.
        for w in lines.windows(2) {
            plan.constrain(Constraint::Equal { a: w[0], b: w[1] });
        }
        if let Some(r) = radius {
            plan.constrain(Constraint::Radius {
                curve: circle,
                dim: Dimension::driving(r),
            });
        }
        self.cancel();
        Outcome::Commit(plan.finish("Add polygon"))
    }

    fn click_slot(&mut self, here: Node, mut plan: Plan, width: Option<f64>) -> Outcome {
        match self.nodes.len() {
            0 => {
                self.nodes.push(here);
                Outcome::Pending
            }
            1 => {
                let a = self.nodes[0];
                if a.pos().distance(here.pos()) < MIN_SIZE_MM
                    || (a.id().is_some() && a.id() == here.id())
                {
                    return Outcome::Pending;
                }
                self.nodes.push(here);
                Outcome::Pending
            }
            _ => {
                let (a, b) = (self.nodes[0], self.nodes[1]);
                let Some(slot) = shapes::slot(a.pos(), b.pos(), here.pos()) else {
                    return Outcome::Pending;
                };
                let (pa, pb) = (plan.node(a), plan.node(b));
                let t: Vec<EntityId> = slot.tangent_points.iter().map(|p| plan.point(*p)).collect();
                let top = plan.line(t[0], t[1]);
                let bottom = plan.line(t[2], t[3]);
                let arc_b = plan.add(Geom::Arc {
                    center: pb,
                    start: t[2],
                    end: t[1],
                });
                let arc_a = plan.add(Geom::Arc {
                    center: pa,
                    start: t[0],
                    end: t[3],
                });
                for (line, arc) in [(top, arc_a), (top, arc_b), (bottom, arc_a), (bottom, arc_b)] {
                    plan.constrain(Constraint::Tangent { a: line, b: arc });
                }
                plan.constrain(Constraint::Equal { a: arc_a, b: arc_b });
                if let Some(w) = width {
                    plan.constrain(Constraint::Diameter {
                        curve: arc_a,
                        dim: Dimension::driving(w),
                    });
                }
                self.cancel();
                Outcome::Commit(plan.finish("Add slot"))
            }
        }
    }

    /// A number typed while drawing: the line's length, the circle's radius,
    /// or the centre-arc's radius, in the direction of the cursor. Lengths
    /// and radii typed this way become dimensions (SPEC §6.1).
    pub fn typed(&mut self, value: f64, cursor: Pick, doc_next_id: u64) -> Outcome {
        if !(value.is_finite() && value > MIN_SIZE_MM) {
            return Outcome::Rejected("Enter a positive number");
        }
        match self.tool {
            Tool::RectCorner | Tool::RectCentre | Tool::RectThree => {
                return self.typed_rect(value, cursor, doc_next_id);
            }
            _ => {}
        }
        let Some(&from) = self.nodes.first() else {
            return Outcome::Pending;
        };
        let dir = (cursor.pos - from.pos())
            .normalized(MIN_SIZE_MM)
            .unwrap_or(Vec2::new(1.0, 0.0));
        let at = Node::New(from.pos() + dir * value);
        let mut plan = Plan::new(doc_next_id, self.layer);
        match self.tool {
            // A typed length keeps the inferred direction and its constraint.
            Tool::Line => self.segment(from, at, plan, Some(value), cursor.line),
            Tool::Circle => {
                let c = plan.node(from);
                let circle = plan.add(Geom::Circle {
                    center: c,
                    radius: value,
                });
                plan.constrain(Constraint::Radius {
                    curve: circle,
                    dim: Dimension::driving(value),
                });
                self.cancel();
                Outcome::Commit(plan.finish("Add circle"))
            }
            Tool::ArcCentre if self.nodes.len() == 1 => {
                self.nodes.push(at);
                Outcome::Pending
            }
            Tool::Polygon => self.click_polygon(at, plan, Some(value)),
            Tool::Slot if self.nodes.len() == 2 => {
                let (a, b) = (self.nodes[0].pos(), self.nodes[1].pos());
                let Some(u) = (b - a).normalized(MIN_SIZE_MM) else {
                    return Outcome::Pending;
                };
                let side = Node::New(a + u.perp() * (value / 2.0));
                self.click_slot(side, plan, Some(value))
            }
            _ => Outcome::Pending,
        }
    }

    /// The numeric value a typed entry would set, for the on-canvas readout:
    /// `(what, value in mm)`.
    pub fn readout(&self, cursor: Vec2) -> Option<(&'static str, f64)> {
        let from = self.nodes.first()?.pos();
        match (self.tool, self.nodes.len()) {
            (Tool::Line, _) => Some(("Length", from.distance(cursor))),
            (Tool::Circle, _) | (Tool::ArcCentre, 1) | (Tool::Polygon, _) => {
                Some(("Radius", from.distance(cursor)))
            }
            (Tool::Circle2, _) => Some(("Diameter", from.distance(cursor))),
            (Tool::RectCorner | Tool::RectCentre, _) => {
                let d = cursor - from;
                Some(if self.dims[0].is_none() {
                    ("Width", d.x.abs())
                } else {
                    ("Height", d.y.abs())
                })
            }
            (Tool::RectThree, 1) => Some(("Length", from.distance(cursor))),
            (Tool::RectThree, _) => {
                let u = (self.nodes[1].pos() - from).normalized(MIN_SIZE_MM)?;
                Some(("Height", (cursor - from).dot(u.perp()).abs()))
            }
            (Tool::Slot, 2) => {
                let u = (self.nodes[1].pos() - from).normalized(MIN_SIZE_MM)?;
                Some(("Width", 2.0 * (cursor - from).dot(u.perp()).abs()))
            }
            _ => None,
        }
    }

    /// Rubber-band shapes for the cursor at `cursor`.
    pub fn preview(&self, cursor: Vec2) -> Vec<Shape> {
        let guide = |a: Vec2, b: Vec2| Shape::Line(Line::new(a, b));
        let Some(first) = self.nodes.first().map(|n| n.pos()) else {
            return Vec::new();
        };
        match (self.tool, self.nodes.len()) {
            (Tool::Line, _) => vec![guide(first, cursor)],
            (Tool::Circle, _) => {
                let r = first.distance(cursor);
                if r < MIN_SIZE_MM {
                    Vec::new()
                } else {
                    vec![Shape::Circle(Circle::new(first, r)), guide(first, cursor)]
                }
            }
            (Tool::ArcCentre, 1) => vec![guide(first, cursor)],
            (Tool::ArcCentre, _) => {
                let start = self.nodes[1].pos();
                let r = first.distance(start);
                match (cursor - first).normalized(MIN_SIZE_MM) {
                    Some(u) => vec![
                        guide(first, start),
                        Shape::Arc(Arc::new(first, start, first + u * r)),
                    ],
                    None => vec![guide(first, start)],
                }
            }
            (Tool::RectCorner | Tool::RectCentre, _) => {
                let c1 = self.rect_corner(cursor);
                let k = if self.tool == Tool::RectCentre {
                    shapes::rect_corners_centred(first, c1)
                } else {
                    shapes::rect_corners(first, c1)
                };
                outline(&k)
            }
            (Tool::RectThree, 1) => vec![guide(first, cursor)],
            (Tool::RectThree, _) => {
                let b = self.nodes[1].pos();
                match shapes::rect_corners_rotated(first, b, cursor) {
                    Some(k) => outline(&k),
                    None => vec![guide(first, b)],
                }
            }
            (Tool::Circle2, _) => match shapes::circle_by_diameter(first, cursor) {
                Some((c, r)) => vec![Shape::Circle(Circle::new(c, r)), guide(first, cursor)],
                None => Vec::new(),
            },
            (Tool::Circle3, 1) => vec![guide(first, cursor)],
            (Tool::Circle3, _) => {
                let b = self.nodes[1].pos();
                match circle_through(first, b, cursor) {
                    Some((c, _)) => vec![Shape::Circle(Circle::new(c, c.distance(first)))],
                    None => vec![guide(first, b)],
                }
            }
            (Tool::ArcTangent, _) => match self.host {
                Some(h) => match shapes::tangent_arc(first, h.dir, cursor) {
                    Some((c, ccw)) => vec![Shape::Arc(if ccw {
                        Arc::new(c, first, cursor)
                    } else {
                        Arc::new(c, cursor, first)
                    })],
                    None => vec![guide(first, cursor)],
                },
                None => Vec::new(),
            },
            (Tool::Polygon, _) => {
                match shapes::polygon(first, cursor, self.sides, self.circumscribed) {
                    Some((pts, r)) => {
                        let mut v = outline(&pts);
                        v.push(Shape::Circle(Circle::new(first, r)));
                        v
                    }
                    None => Vec::new(),
                }
            }
            (Tool::Slot, 1) => vec![guide(first, cursor)],
            (Tool::Slot, _) => {
                let b = self.nodes[1].pos();
                match shapes::slot(first, b, cursor) {
                    Some(sl) => {
                        let [t0, t1, t2, t3] = sl.tangent_points;
                        vec![
                            guide(t0, t1),
                            guide(t2, t3),
                            Shape::Arc(Arc::new(b, t2, t1)),
                            Shape::Arc(Arc::new(first, t0, t3)),
                        ]
                    }
                    None => vec![guide(first, b)],
                }
            }
            (Tool::ArcThreePoint, 1) => vec![guide(first, cursor)],
            (Tool::ArcThreePoint, _) => {
                let end = self.nodes[1].pos();
                match circle_through(first, cursor, end) {
                    Some((c, ccw)) => {
                        let (s, e) = if ccw { (first, end) } else { (end, first) };
                        vec![Shape::Arc(Arc::new(c, s, e))]
                    }
                    None => vec![guide(first, end)],
                }
            }
            _ => Vec::new(),
        }
    }
}

fn outline(pts: &[Vec2]) -> Vec<Shape> {
    (0..pts.len())
        .map(|i| Shape::Line(Line::new(pts[i], pts[(i + 1) % pts.len()])))
        .collect()
}

/// A rectangle from four corners (in order round it): points, four lines and
/// the constraints that keep it a rectangle. Axis-aligned ones get
/// horizontal/vertical lines, rotated ones parallel and perpendicular pairs
/// (5 degrees of freedom left, SPEC §6.1). `centre` adds a construction
/// diagonal with the centre at its midpoint. Typed sizes become dimensions.
fn build_rect(
    plan: &mut Plan,
    corners: [Node; 4],
    rotated: bool,
    centre: Option<Node>,
    dims: [Option<f64>; 2],
) {
    let p: Vec<EntityId> = corners.iter().map(|n| plan.node(*n)).collect();
    let l: Vec<EntityId> = (0..4).map(|i| plan.line(p[i], p[(i + 1) % 4])).collect();
    if rotated {
        plan.constrain(Constraint::Parallel { a: l[0], b: l[2] });
        plan.constrain(Constraint::Parallel { a: l[1], b: l[3] });
        plan.constrain(Constraint::Perpendicular { a: l[0], b: l[1] });
    } else {
        plan.constrain(Constraint::Horizontal { line: l[0] });
        plan.constrain(Constraint::Horizontal { line: l[2] });
        plan.constrain(Constraint::Vertical { line: l[1] });
        plan.constrain(Constraint::Vertical { line: l[3] });
    }
    for (value, (a, b)) in dims.into_iter().zip([(p[0], p[1]), (p[1], p[2])]) {
        if let Some(d) = value {
            plan.constrain(Constraint::Distance {
                a,
                b,
                dim: Dimension::driving(d),
            });
        }
    }
    if let Some(c) = centre {
        let c = plan.node(c);
        let diagonal = plan.construction(Geom::Line {
            start: p[0],
            end: p[2],
        });
        plan.constrain(Constraint::Midpoint {
            point: c,
            line: diagonal,
        });
    }
}

/// The centre of the circle through `a`, `via`, `b`, and whether going
/// `a → via → b` runs counter-clockwise round it. `None` when the points are
/// (nearly) collinear.
pub fn circle_through(a: Vec2, via: Vec2, b: Vec2) -> Option<(Vec2, bool)> {
    let d = 2.0 * (a.x * (via.y - b.y) + via.x * (b.y - a.y) + b.x * (a.y - via.y));
    if d.abs() < 1e-12 {
        return None;
    }
    let (a2, v2, b2) = (a.length_sq(), via.length_sq(), b.length_sq());
    let centre = Vec2::new(
        (a2 * (via.y - b.y) + v2 * (b.y - a.y) + b2 * (a.y - via.y)) / d,
        (a2 * (b.x - via.x) + v2 * (a.x - b.x) + b2 * (via.x - a.x)) / d,
    );
    if !centre.is_finite() || centre.distance(a) > MAX_ARC_RADIUS_MM {
        return None;
    }
    let ccw = (via - a).cross(b - via) > 0.0;
    Some((centre, ccw))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use locus_doc::Sketcher;

    fn at(x: f64, y: f64) -> Pick {
        Pick::free(Vec2::new(x, y))
    }

    fn run(s: &mut Sketcher, out: Outcome) -> locus_doc::Applied {
        match out {
            Outcome::Commit(cmd) => s.apply(&cmd).unwrap(),
            other => panic!("expected a commit, got {other:?}"),
        }
    }

    fn click(d: &mut Draft, s: &mut Sketcher, p: Pick) -> Option<locus_doc::Applied> {
        match d.click(p, s.document().next_id()) {
            Outcome::Commit(cmd) => Some(s.apply(&cmd).unwrap()),
            _ => None,
        }
    }

    fn count(s: &Sketcher, f: impl Fn(&Geom) -> bool) -> usize {
        s.document()
            .entities()
            .iter()
            .filter(|(_, e)| f(&e.geom))
            .count()
    }

    #[test]
    fn new_geometry_goes_on_the_active_layer() {
        let mut s = Sketcher::new(Document::new());
        let l2 = s
            .apply(&Command::AddLayer {
                name: "Score".to_owned(),
                colour: [0; 3],
                operation: locus_doc::Operation::Score,
            })
            .unwrap()
            .layers[0];
        let mut d = Draft::new(Tool::Point);
        d.set_layer(l2);
        let a = click(&mut d, &mut s, at(1.0, 1.0)).unwrap();
        assert_eq!(s.document().entity(a.entities[0]).unwrap().layer, l2);
    }

    #[test]
    fn point_tool_adds_one_point_per_click_and_skips_existing() {
        let mut s = Sketcher::new(Document::new());
        let mut d = Draft::new(Tool::Point);
        let a = click(&mut d, &mut s, at(3.0, 4.0)).unwrap();
        assert_eq!(a.entities.len(), 1);
        let again = Pick::at_point(Vec2::new(3.0, 4.0), a.entities[0]);
        assert!(click(&mut d, &mut s, again).is_none());
        assert_eq!(s.document().entities().len(), 1);
    }

    #[test]
    fn polyline_shares_joints_and_each_segment_is_one_undo_step() {
        let mut s = Sketcher::new(Document::new());
        let mut d = Draft::new(Tool::Line);
        assert!(click(&mut d, &mut s, at(0.0, 0.0)).is_none());
        let a = click(&mut d, &mut s, at(10.0, 0.0)).unwrap();
        assert_eq!(a.entities.len(), 3); // two points and the line
        let b = click(&mut d, &mut s, at(10.0, 10.0)).unwrap();
        assert_eq!(b.entities.len(), 2); // the joint is reused
        assert_eq!(count(&s, |g| matches!(g, Geom::Point { .. })), 3);
        assert_eq!(count(&s, |g| matches!(g, Geom::Line { .. })), 2);
        s.undo().unwrap();
        assert_eq!(count(&s, |g| matches!(g, Geom::Line { .. })), 1);
        assert_eq!(count(&s, |g| matches!(g, Geom::Point { .. })), 2);
    }

    #[test]
    fn closing_a_loop_reuses_the_first_point_and_ends_the_chain() {
        let mut s = Sketcher::new(Document::new());
        let mut d = Draft::new(Tool::Line);
        click(&mut d, &mut s, at(0.0, 0.0));
        let first = click(&mut d, &mut s, at(10.0, 0.0)).unwrap().entities[0];
        click(&mut d, &mut s, at(10.0, 10.0));
        let close = Pick::at_point(Vec2::new(0.0, 0.0), first);
        let a = click(&mut d, &mut s, close).unwrap();
        assert_eq!(a.entities.len(), 1);
        assert!(!d.is_drawing());
        assert_eq!(count(&s, |g| matches!(g, Geom::Point { .. })), 3);
        assert_eq!(count(&s, |g| matches!(g, Geom::Line { .. })), 3);
    }

    #[test]
    fn typed_length_places_the_end_and_adds_a_dimension() {
        let mut s = Sketcher::new(Document::new());
        let mut d = Draft::new(Tool::Line);
        click(&mut d, &mut s, at(0.0, 0.0));
        let out = d.typed(25.0, at(3.0, 0.0), s.document().next_id());
        run(&mut s, out);
        let line = s
            .document()
            .entities()
            .iter()
            .find_map(|(id, e)| matches!(e.geom, Geom::Line { .. }).then_some(id))
            .unwrap();
        assert!((s.document().line(line).unwrap().length() - 25.0).abs() < 1e-6);
        assert_eq!(s.document().constraints().len(), 1);
    }

    #[test]
    fn circle_by_click_and_by_typed_radius() {
        let mut s = Sketcher::new(Document::new());
        let mut d = Draft::new(Tool::Circle);
        click(&mut d, &mut s, at(0.0, 0.0));
        click(&mut d, &mut s, at(3.0, 4.0)).unwrap();
        let c = s
            .document()
            .entities()
            .iter()
            .find_map(|(id, e)| matches!(e.geom, Geom::Circle { .. }).then_some(id))
            .unwrap();
        assert!((s.document().circle(c).unwrap().radius - 5.0).abs() < 1e-9);
        click(&mut d, &mut s, at(50.0, 0.0));
        let out = d.typed(7.5, at(60.0, 0.0), s.document().next_id());
        run(&mut s, out);
        assert_eq!(count(&s, |g| matches!(g, Geom::Circle { .. })), 2);
        assert_eq!(s.document().constraints().len(), 1);
    }

    #[test]
    fn centre_arc_is_one_command_with_three_points() {
        let mut s = Sketcher::new(Document::new());
        let mut d = Draft::new(Tool::ArcCentre);
        click(&mut d, &mut s, at(0.0, 0.0));
        click(&mut d, &mut s, at(10.0, 0.0));
        let a = click(&mut d, &mut s, at(0.0, 30.0)).unwrap();
        assert_eq!(a.entities.len(), 4);
        let arc = *a.entities.last().unwrap();
        let arc = s.document().arc(arc).unwrap();
        assert!((arc.radius() - 10.0).abs() < 1e-6);
        assert!((arc.sweep() - std::f64::consts::FRAC_PI_2).abs() < 1e-6);
        s.undo().unwrap();
        assert!(s.document().entities().is_empty());
    }

    #[test]
    fn three_point_arc_passes_through_the_third_click_either_way_round() {
        for (a, via, b) in [
            ((10.0, 0.0), (0.0, 10.0), (-10.0, 0.0)),
            ((-10.0, 0.0), (0.0, 10.0), (10.0, 0.0)),
        ] {
            let mut s = Sketcher::new(Document::new());
            let mut d = Draft::new(Tool::ArcThreePoint);
            click(&mut d, &mut s, at(a.0, a.1));
            click(&mut d, &mut s, at(b.0, b.1));
            let out = click(&mut d, &mut s, at(via.0, via.1)).unwrap();
            let arc = s.document().arc(*out.entities.last().unwrap()).unwrap();
            assert!((arc.radius() - 10.0).abs() < 1e-6);
            assert!(arc.contains_point_angle(Vec2::new(via.0, via.1)));
        }
    }

    #[test]
    fn collinear_three_point_arc_is_rejected() {
        let mut s = Sketcher::new(Document::new());
        let mut d = Draft::new(Tool::ArcThreePoint);
        click(&mut d, &mut s, at(0.0, 0.0));
        click(&mut d, &mut s, at(10.0, 0.0));
        let out = d.click(at(5.0, 0.0), s.document().next_id());
        assert!(matches!(out, Outcome::Rejected(_)));
    }

    #[test]
    fn escape_and_enter_drop_the_half_drawn_shape() {
        let mut s = Sketcher::new(Document::new());
        let mut d = Draft::new(Tool::Line);
        click(&mut d, &mut s, at(0.0, 0.0));
        assert!(d.is_drawing());
        d.cancel();
        assert!(!d.is_drawing() && s.document().entities().is_empty());
        click(&mut d, &mut s, at(0.0, 0.0));
        click(&mut d, &mut s, at(5.0, 0.0));
        d.finish();
        assert!(!d.is_drawing());
        assert_eq!(count(&s, |g| matches!(g, Geom::Line { .. })), 1);
    }

    #[test]
    fn degenerate_input_is_ignored_or_rejected() {
        let mut s = Sketcher::new(Document::new());
        let mut d = Draft::new(Tool::Line);
        click(&mut d, &mut s, at(1.0, 1.0));
        assert!(click(&mut d, &mut s, at(1.0, 1.0)).is_none());
        let bad = d.typed(-3.0, at(5.0, 1.0), s.document().next_id());
        assert!(matches!(bad, Outcome::Rejected(_)));
        assert!(s.document().entities().is_empty());
    }

    #[test]
    fn hints_become_constraints_in_the_same_undo_step() {
        let mut s = Sketcher::new(Document::new());
        let mut d = Draft::new(Tool::Line);
        click(&mut d, &mut s, at(0.0, 0.0));
        let mut end = at(10.0, 0.0);
        end.line = Some(LineHint::Horizontal);
        let a = click(&mut d, &mut s, end).unwrap();
        assert_eq!(a.constraints.len(), 1);
        assert!(matches!(
            s.document().constraint(a.constraints[0]),
            Some(Constraint::Horizontal { .. })
        ));
        s.undo().unwrap();
        assert!(s.document().entities().is_empty() && s.document().constraints().is_empty());
    }

    #[test]
    fn a_point_on_a_curve_is_constrained_onto_it() {
        let mut s = Sketcher::new(Document::new());
        let mut d = Draft::new(Tool::Line);
        click(&mut d, &mut s, at(0.0, 0.0));
        click(&mut d, &mut s, at(10.0, 0.0));
        d.cancel();
        let line = s
            .document()
            .entities()
            .iter()
            .find_map(|(id, e)| matches!(e.geom, Geom::Line { .. }).then_some(id))
            .unwrap();
        let mut d = Draft::new(Tool::Point);
        let mut on = at(5.0, 0.0);
        on.on_curve = Some(line);
        let a = click(&mut d, &mut s, on).unwrap();
        assert_eq!(a.constraints.len(), 1);
        assert!(matches!(
            s.document().constraint(a.constraints[0]),
            Some(Constraint::PointOnCurve { curve, .. }) if *curve == line
        ));
    }

    #[test]
    fn preview_follows_the_cursor() {
        let mut d = Draft::new(Tool::Circle);
        assert!(d.preview(Vec2::new(1.0, 1.0)).is_empty());
        d.nodes.push(Node::New(Vec2::ZERO));
        let shapes = d.preview(Vec2::new(3.0, 4.0));
        assert!(
            shapes
                .iter()
                .any(|s| matches!(s, Shape::Circle(c) if (c.radius - 5.0).abs() < 1e-9))
        );
    }

    // ── Sketch tools (#155–#158) ────────────────────────────────────────

    fn pts_of(s: &Sketcher, geom: impl Fn(&Geom) -> bool) -> Vec<EntityId> {
        s.document()
            .entities()
            .iter()
            .filter(|(_, e)| geom(&e.geom))
            .map(|(id, _)| id)
            .collect()
    }

    fn lines(s: &Sketcher) -> Vec<locus_geom::Line> {
        pts_of(s, |g| matches!(g, Geom::Line { .. }))
            .into_iter()
            .filter(|id| !s.document().entity(*id).unwrap().construction)
            .filter_map(|id| s.document().line(id))
            .collect()
    }

    /// Four lines forming a rectangle: consecutive edges perpendicular.
    fn assert_rectangle(s: &Sketcher) {
        let l = lines(s);
        assert_eq!(l.len(), 4);
        for i in 0..4 {
            let (a, b) = (l[i], l[(i + 1) % 4]);
            let (da, db) = (a.b - a.a, b.b - b.a);
            assert!(
                da.dot(db).abs() / (da.length() * db.length()) < 1e-6,
                "edges {i} and {} are not perpendicular",
                (i + 1) % 4
            );
        }
    }

    fn drag_first_point_by(s: &mut Sketcher, dx: f64, dy: f64) {
        let p = pts_of(s, |g| matches!(g, Geom::Point { .. }))[0];
        let from = s.document().point_pos(p).unwrap();
        s.begin_drag(p).unwrap();
        for k in 1..=10 {
            let t = f64::from(k) / 10.0;
            s.drag_to(from + Vec2::new(dx * t, dy * t)).unwrap();
        }
        s.end_drag().unwrap();
    }

    #[test]
    fn two_point_rectangle_is_one_undo_step_and_stays_a_rectangle_when_dragged() {
        let mut s = Sketcher::new(Document::new());
        let mut d = Draft::new(Tool::RectCorner);
        assert!(click(&mut d, &mut s, at(0.0, 0.0)).is_none());
        click(&mut d, &mut s, at(30.0, 20.0)).unwrap();
        assert_eq!(count(&s, |g| matches!(g, Geom::Point { .. })), 4);
        assert_rectangle(&s);
        drag_first_point_by(&mut s, -7.0, 5.0);
        assert_rectangle(&s);
        s.undo().unwrap(); // the drag
        s.undo().unwrap(); // the rectangle
        assert!(s.document().entities().is_empty());
    }

    #[test]
    fn typed_width_and_height_become_dimensions() {
        let mut s = Sketcher::new(Document::new());
        let mut d = Draft::new(Tool::RectCorner);
        click(&mut d, &mut s, at(0.0, 0.0));
        assert!(matches!(
            d.typed(40.0, at(5.0, 5.0), s.document().next_id()),
            Outcome::Pending
        ));
        let out = d.typed(25.0, at(5.0, 5.0), s.document().next_id());
        run(&mut s, out);
        assert_rectangle(&s);
        let l = lines(&s);
        let w = l.iter().map(|l| l.length()).fold(0.0, f64::max);
        let h = l.iter().map(|l| l.length()).fold(f64::MAX, f64::min);
        assert!(
            (w - 40.0).abs() < 1e-6 && (h - 25.0).abs() < 1e-6,
            "{w} {h}"
        );
        assert_eq!(s.document().constraints().len(), 6);
    }

    #[test]
    fn centred_rectangle_keeps_its_centre() {
        let mut s = Sketcher::new(Document::new());
        let mut d = Draft::new(Tool::RectCentre);
        click(&mut d, &mut s, at(10.0, 10.0));
        click(&mut d, &mut s, at(16.0, 14.0)).unwrap();
        assert_rectangle(&s);
        let l = lines(&s);
        let w = l.iter().map(|l| l.length()).fold(0.0, f64::max);
        assert!((w - 12.0).abs() < 1e-6);
        drag_first_point_by(&mut s, 3.0, 3.0);
        assert_rectangle(&s);
    }

    #[test]
    fn three_point_rectangle_is_rotated_and_stays_rectangular() {
        let mut s = Sketcher::new(Document::new());
        let mut d = Draft::new(Tool::RectThree);
        click(&mut d, &mut s, at(0.0, 0.0));
        click(&mut d, &mut s, at(10.0, 10.0));
        click(&mut d, &mut s, at(-4.0, 4.0)).unwrap();
        assert_rectangle(&s);
        drag_first_point_by(&mut s, 2.0, -3.0);
        assert_rectangle(&s);
    }

    #[test]
    fn two_point_circle_has_the_clicks_as_a_diameter() {
        let mut s = Sketcher::new(Document::new());
        let mut d = Draft::new(Tool::Circle2);
        click(&mut d, &mut s, at(0.0, 0.0));
        click(&mut d, &mut s, at(10.0, 0.0)).unwrap();
        let c = pts_of(&s, |g| matches!(g, Geom::Circle { .. }))[0];
        let c = s.document().circle(c).unwrap();
        assert!((c.radius - 5.0).abs() < 1e-6 && c.center.distance(Vec2::new(5.0, 0.0)) < 1e-6);
        // Dragging one end moves the circle with the diameter.
        drag_first_point_by(&mut s, 0.0, 6.0);
        let c = s
            .document()
            .circle(pts_of(&s, |g| matches!(g, Geom::Circle { .. }))[0])
            .unwrap();
        let ends: Vec<_> = pts_of(&s, |g| matches!(g, Geom::Point { .. }))
            .into_iter()
            .map(|p| s.document().point_pos(p).unwrap())
            .collect();
        assert!((ends[0].distance(c.center) - c.radius).abs() < 1e-5);
        assert!((ends[1].distance(c.center) - c.radius).abs() < 1e-5);
    }

    #[test]
    fn three_point_circle_passes_through_the_clicks() {
        let mut s = Sketcher::new(Document::new());
        let mut d = Draft::new(Tool::Circle3);
        click(&mut d, &mut s, at(10.0, 0.0));
        click(&mut d, &mut s, at(0.0, 10.0));
        click(&mut d, &mut s, at(-10.0, 0.0)).unwrap();
        let c = s
            .document()
            .circle(pts_of(&s, |g| matches!(g, Geom::Circle { .. }))[0])
            .unwrap();
        assert!((c.radius - 10.0).abs() < 1e-6);
        let mut d = Draft::new(Tool::Circle3);
        click(&mut d, &mut s, at(0.0, 50.0));
        click(&mut d, &mut s, at(10.0, 50.0));
        assert!(matches!(
            d.click(at(20.0, 50.0), s.document().next_id()),
            Outcome::Rejected(_)
        ));
    }

    #[test]
    fn tangent_arc_continues_a_line_with_a_tangent_constraint() {
        let mut s = Sketcher::new(Document::new());
        let mut d = Draft::new(Tool::Line);
        click(&mut d, &mut s, at(0.0, 0.0));
        click(&mut d, &mut s, at(10.0, 0.0)).unwrap();
        d.cancel();
        let line = pts_of(&s, |g| matches!(g, Geom::Line { .. }))[0];
        let end = match s.document().entity(line).unwrap().geom {
            Geom::Line { end, .. } => end,
            _ => unreachable!(),
        };
        let mut t = Draft::new(Tool::ArcTangent);
        let mut start = Pick::at_point(Vec2::new(10.0, 0.0), end);
        start.host = Some(Host {
            curve: line,
            dir: Vec2::new(1.0, 0.0),
        });
        assert!(click(&mut t, &mut s, start).is_none());
        let a = click(&mut t, &mut s, at(20.0, 10.0)).unwrap();
        let arc = s.document().arc(*a.entities.last().unwrap()).unwrap();
        assert!((arc.radius() - 10.0).abs() < 1e-6);
        assert!(arc.center.distance(Vec2::new(10.0, 10.0)) < 1e-6);
        assert!(matches!(
            s.document().constraint(a.constraints[0]),
            Some(Constraint::Tangent { .. })
        ));
        // A free first click is not a continuation.
        let mut t = Draft::new(Tool::ArcTangent);
        assert!(matches!(
            t.click(at(3.0, 3.0), s.document().next_id()),
            Outcome::Rejected(_)
        ));
    }

    #[test]
    fn polygons_stay_regular_when_a_vertex_is_dragged() {
        for circumscribed in [false, true] {
            let mut s = Sketcher::new(Document::new());
            let mut d = Draft::new(Tool::Polygon);
            d.set_polygon(5, circumscribed);
            click(&mut d, &mut s, at(0.0, 0.0));
            click(&mut d, &mut s, at(0.0, 10.0)).unwrap();
            let regular = |s: &Sketcher| {
                let l = lines(s);
                assert_eq!(l.len(), 5);
                let len = l[0].length();
                assert!(
                    l.iter().all(|x| (x.length() - len).abs() < 1e-5),
                    "{circumscribed}"
                );
            };
            regular(&s);
            let vertex = pts_of(&s, |g| matches!(g, Geom::Point { .. }))[1];
            let from = s.document().point_pos(vertex).unwrap();
            s.begin_drag(vertex).unwrap();
            for k in 1..=10 {
                s.drag_to(from + Vec2::new(0.6 * f64::from(k), 0.3 * f64::from(k)))
                    .unwrap();
            }
            s.end_drag().unwrap();
            regular(&s);
        }
    }

    #[test]
    fn polygon_sides_are_clamped_and_typed_radius_is_a_dimension() {
        let mut d = Draft::new(Tool::Polygon);
        d.set_polygon(1000, false);
        assert_eq!(d.polygon().0, 64);
        d.set_polygon(0, false);
        assert_eq!(d.polygon().0, 3);
        let mut s = Sketcher::new(Document::new());
        click(&mut d, &mut s, at(0.0, 0.0));
        let out = d.typed(12.0, at(1.0, 0.0), s.document().next_id());
        run(&mut s, out);
        assert_eq!(lines(&s).len(), 3);
        assert!(
            s.document()
                .constraints()
                .values()
                .any(|c| matches!(c, Constraint::Radius { .. }))
        );
    }

    #[test]
    fn slot_stays_a_slot_when_dragged_or_redimensioned() {
        let mut s = Sketcher::new(Document::new());
        let mut d = Draft::new(Tool::Slot);
        click(&mut d, &mut s, at(0.0, 0.0));
        click(&mut d, &mut s, at(20.0, 0.0));
        click(&mut d, &mut s, at(5.0, 4.0)).unwrap();
        assert_eq!(count(&s, |g| matches!(g, Geom::Arc { .. })), 2);
        assert_eq!(lines(&s).len(), 2);
        let check = |s: &Sketcher| {
            let arcs: Vec<_> = pts_of(s, |g| matches!(g, Geom::Arc { .. }))
                .into_iter()
                .map(|a| s.document().arc(a).unwrap())
                .collect();
            assert!((arcs[0].radius() - arcs[1].radius()).abs() < 1e-5);
            for l in lines(s) {
                // Each line is tangent to both end circles.
                for a in &arcs {
                    let d = (l.b - l.a).cross(a.center - l.a).abs() / l.length();
                    assert!(
                        (d - a.radius()).abs() < 1e-5,
                        "tangent: {d} vs {}",
                        a.radius()
                    );
                }
            }
        };
        check(&s);
        let centre = pts_of(&s, |g| matches!(g, Geom::Point { .. }))[0];
        let from = s.document().point_pos(centre).unwrap();
        s.begin_drag(centre).unwrap();
        for k in 1..=10 {
            s.drag_to(from + Vec2::new(-f64::from(k), 1.5 * f64::from(k)))
                .unwrap();
        }
        s.end_drag().unwrap();
        check(&s);
    }

    #[test]
    fn typed_slot_width_is_a_diameter_dimension() {
        let mut s = Sketcher::new(Document::new());
        let mut d = Draft::new(Tool::Slot);
        click(&mut d, &mut s, at(0.0, 0.0));
        click(&mut d, &mut s, at(20.0, 0.0));
        let out = d.typed(8.0, at(5.0, 3.0), s.document().next_id());
        run(&mut s, out);
        let arc = s
            .document()
            .arc(pts_of(&s, |g| matches!(g, Geom::Arc { .. }))[0])
            .unwrap();
        assert!((arc.radius() - 4.0).abs() < 1e-6);
    }
}
