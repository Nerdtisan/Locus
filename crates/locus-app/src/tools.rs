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
use locus_doc::{
    Command, Constraint, Dimension, Document, EntityId, Geom, LayerId, Style, TextData,
};
use locus_geom::{Arc, Circle, Cubic, Line, Vec2};

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
    /// One side's two ends, then which side of it the polygon lies on
    /// (Fusion's Edge Polygon, #356).
    PolygonEdge,
    /// Centre, centre, then the width (#158).
    Slot,
    /// Bézier pen: click for corners, click-drag for smooth points (#160).
    Bezier,
    /// Text: one click places the anchor; content and font come from the tool strip (#180).
    Text,
    /// Inspect ▸ Measure (#386, design §2): two snapped clicks; the distance,
    /// Δx, Δy and angle go to the status bar. Never changes the document.
    Measure,
}

impl Tool {
    #[cfg(test)]
    pub const DRAWING: [Tool; 16] = [
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
        Tool::PolygonEdge,
        Tool::Slot,
        Tool::Bezier,
        Tool::Text,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Tool::Select => "Select",
            Tool::Point => "Point",
            Tool::Line => "Line",
            Tool::Circle => "Center Diameter Circle",
            Tool::RectCorner => "2-Point Rectangle",
            Tool::RectCentre => "Center Rectangle",
            Tool::RectThree => "3-Point Rectangle",
            Tool::Circle2 => "2-Point Circle",
            Tool::Circle3 => "3-Point Circle",
            Tool::ArcTangent => "Tangent Arc",
            Tool::Polygon => "Polygon",
            Tool::PolygonEdge => "Edge Polygon",
            Tool::Slot => "Slot",
            Tool::Bezier => "Bézier Pen",
            Tool::Text => "Text",
            Tool::Measure => "Measure",
            Tool::ArcThreePoint => "3-Point Arc",
            Tool::ArcCentre => "Center Point Arc",
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
                "Rectangle: click the opposite corner, or type the width, Tab, the height, Enter"
            }
            (Tool::RectCentre, 0) => "Rectangle: click the centre",
            (Tool::RectCentre, _) => {
                "Rectangle: click a corner, or type the width, Tab, the height, Enter"
            }
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
            (Tool::Polygon, _) => {
                "Polygon: click a vertex (or side middle), or type the radius; Tab for sides"
            }
            (Tool::PolygonEdge, 0) => "Edge polygon: click one end of a side",
            (Tool::PolygonEdge, 1) => {
                "Edge polygon: click the side's other end, or type its length; Tab for sides"
            }
            (Tool::PolygonEdge, _) => "Edge polygon: click the side of the edge it goes on",
            (Tool::Slot, 0) => "Slot: click the first centre",
            (Tool::Slot, 1) => "Slot: click the second centre",
            (Tool::Slot, _) => "Slot: click to set the width, or type it",
            (Tool::Text, _) => "Text: set the text in the dialog, then click to place it",
            (Tool::Measure, 0) => "Measure: click the first point (snaps to points and curves)",
            (Tool::Measure, _) => "Measure: click the second point (Esc cancels)",
            (Tool::Bezier, 0) => "Bézier: click the start (drag for a smooth point)",
            (Tool::Bezier, _) => {
                "Bézier: click a corner, drag for a smooth point, click the start to close (Enter finishes)"
            }
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
    /// Ends at the foot of the perpendicular onto this line (#357).
    Perpendicular(EntityId),
    /// Ends where it touches this circle or arc, tangent to it (#357).
    TangentTo(EntityId),
    /// Runs parallel to this line (#357).
    Parallel(EntityId),
}

impl LineHint {
    /// Symbol shown beside the cursor (#329: symbols, not letters).
    pub fn badge(self) -> crate::scene::Symbol {
        use crate::scene::Symbol;
        match self {
            LineHint::Horizontal => Symbol::HORIZONTAL,
            LineHint::Vertical => Symbol::VERTICAL,
            LineHint::Tangent(_) | LineHint::TangentTo(_) => Symbol::TANGENT,
            LineHint::Perpendicular(_) => Symbol::PERPENDICULAR,
            LineHint::Parallel(_) => Symbol::PARALLEL,
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
    /// The sketch origin (0, 0), where no point exists yet (#335).
    pub origin: bool,
    /// The midpoint of this line (#357).
    pub midpoint_of: Option<EntityId>,
    /// With `on_curve`: the intersection of two curves (#357).
    pub on_curve2: Option<EntityId>,
    /// A dashed inference guide to draw, in world mm (#357): from a line's
    /// end along its extension to the pick.
    pub guide: Option<(Vec2, Vec2)>,
}

impl Pick {
    pub fn free(pos: Vec2) -> Self {
        Self {
            host: None,
            pos,
            point: None,
            on_curve: None,
            line: None,
            origin: false,
            midpoint_of: None,
            on_curve2: None,
            guide: None,
        }
    }

    /// The sketch origin (#335): the new point is fixed there.
    pub fn at_origin() -> Self {
        Self {
            origin: true,
            ..Self::free(Vec2::ZERO)
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
    /// A new point at a line's midpoint (#357).
    Midpoint(Vec2, EntityId),
    /// A new point on two curves: their intersection (#357).
    OnTwo(Vec2, EntityId, EntityId),
    /// A new point on the sketch origin, fixed there: Fusion's coincident
    /// to the origin (#335). The document has no origin entity, so a point
    /// fixed at (0, 0) is the origin; later picks snap to it.
    Origin,
}

impl Node {
    fn from_pick(p: Pick) -> Self {
        match (p.point, p.on_curve) {
            (Some(id), _) => Node::Existing(id, p.pos),
            (None, _) if p.origin => Node::Origin,
            (None, _) if p.midpoint_of.is_some() => {
                Node::Midpoint(p.pos, p.midpoint_of.unwrap_or(EntityId::new(0)))
            }
            (None, Some(a)) if p.on_curve2.is_some() => {
                Node::OnTwo(p.pos, a, p.on_curve2.unwrap_or(a))
            }
            (None, Some(curve)) => Node::OnCurve(p.pos, curve),
            (None, None) => Node::New(p.pos),
        }
    }

    fn pos(self) -> Vec2 {
        match self {
            Node::Existing(_, p)
            | Node::New(p)
            | Node::OnCurve(p, _)
            | Node::Midpoint(p, _)
            | Node::OnTwo(p, _, _) => p,
            Node::Origin => Vec2::ZERO,
        }
    }

    fn id(self) -> Option<EntityId> {
        match self {
            Node::Existing(id, _) => Some(id),
            Node::New(_)
            | Node::OnCurve(..)
            | Node::Midpoint(..)
            | Node::OnTwo(..)
            | Node::Origin => None,
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
    style: Style,
    /// Everything is construction (the Linetype switch, #289).
    construction: bool,
    commands: Vec<Command>,
}

impl Plan {
    fn new(doc_next_id: u64, layer: LayerId, style: Style, construction: bool) -> Self {
        Self {
            next: doc_next_id,
            layer,
            style,
            construction,
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
            construction: self.construction,
            geom,
            style: self.style,
        });
        EntityId::new(self.alloc())
    }

    fn construction(&mut self, geom: Geom) -> EntityId {
        self.commands.push(Command::AddEntity {
            layer: self.layer,
            construction: true,
            geom,
            style: self.style,
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
            Node::Midpoint(pos, line) => {
                let point = self.point(pos);
                self.constrain(Constraint::Midpoint { point, line });
                point
            }
            Node::OnTwo(pos, a, b) => {
                let point = self.point(pos);
                self.constrain(Constraint::PointOnCurve { point, curve: a });
                self.constrain(Constraint::PointOnCurve { point, curve: b });
                point
            }
            Node::Origin => {
                let point = self.point(Vec2::ZERO);
                self.constrain(Constraint::Fix { point });
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

/// One heads-up field while sizing a shape (#334).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Field {
    pub name: &'static str,
    /// The locked value, or what the cursor gives now (mm).
    pub mm: f64,
    pub locked: bool,
    /// Typing goes here.
    pub active: bool,
    /// A whole number (a polygon's sides, #356), not a length: typed as
    /// is, whatever the display units.
    pub count: bool,
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
    /// Dimensions typed so far for the current shape (width, height),
    /// each locked until the shape is placed (#334).
    dims: [Option<f64>; 2],
    /// The heads-up field typing goes into: 0 width, 1 height (Tab
    /// switches, design §3 step 2).
    field: usize,
    /// The curve a tangent arc continues.
    host: Option<Host>,
    sides: usize,
    circumscribed: bool,
    /// Pen: the last anchor's outgoing handle (set when it is smooth), and
    /// the curve just drawn, so the next one is made tangent to it.
    out: Option<Vec2>,
    last_curve: Option<EntityId>,
    /// The style new geometry gets (design §6.2; #287).
    style: Style,
    /// New geometry is construction (#289).
    construction: bool,
    /// Text tool: the text to place, laid out by the canvas before each click.
    text: Option<Box<TextData>>,
    /// The text entity the last click created, for the canvas to select.
    placed: Option<EntityId>,
    /// Measure: the last two points measured, drawn until the next
    /// measurement, and whether the status bar has been told yet.
    measured: Option<(Vec2, Vec2)>,
    measured_new: bool,
}

impl Draft {
    pub fn new(tool: Tool) -> Self {
        Self {
            tool,
            nodes: Vec::new(),
            chain_first: None,
            layer: Document::default_layer(),
            dims: [None, None],
            field: 0,
            host: None,
            sides: 6,
            circumscribed: false,
            out: None,
            last_curve: None,
            style: Style::default(),
            construction: false,
            text: None,
            placed: None,
            measured: None,
            measured_new: false,
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

    /// Give new geometry `style` (the current style, #287).
    pub fn set_style(&mut self, style: Style) {
        self.style = style;
    }

    /// Make new geometry construction or normal (the Linetype switch, #289).
    pub fn set_construction(&mut self, on: bool) {
        self.construction = on;
    }

    pub fn tool(&self) -> Tool {
        self.tool
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
            Tool::PolygonEdge => self.nodes.len() < 2,
            Tool::RectCorner | Tool::RectCentre | Tool::RectThree | Tool::Slot => {
                self.nodes.len() < 2
            }
            Tool::Circle2 | Tool::Circle3 | Tool::Bezier | Tool::Text | Tool::Measure => true,
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
        self.field = 0;
        self.host = None;
        self.out = None;
        self.last_curve = None;
    }

    /// Enter: end the polyline (or abandon a half-drawn shape).
    pub fn finish(&mut self) {
        self.cancel();
    }

    pub fn click(&mut self, pick: Pick, doc_next_id: u64) -> Outcome {
        let here = Node::from_pick(pick);
        let mut plan = Plan::new(doc_next_id, self.layer, self.style, self.construction);
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
            Tool::PolygonEdge => self.click_edge_polygon(here, plan),
            Tool::Slot => self.click_slot(here, plan, None),
            Tool::Bezier => self.pen(pick, None, doc_next_id),
            Tool::Text => self.click_text(here, plan),
            Tool::Measure => {
                match self.nodes.first() {
                    None => self.nodes.push(here),
                    Some(from) => {
                        self.measured = Some((from.pos(), here.pos()));
                        self.measured_new = true;
                        self.nodes.clear();
                    }
                }
                Outcome::Pending
            }
        }
    }

    /// Measure: the measurement the last click completed, once (#386).
    pub fn take_measured(&mut self) -> Option<(Vec2, Vec2)> {
        if std::mem::take(&mut self.measured_new) {
            self.measured
        } else {
            None
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
            Some(LineHint::Tangent(arc) | LineHint::TangentTo(arc)) => {
                plan.constrain(Constraint::Tangent { a: line, b: arc });
            }
            Some(LineHint::Perpendicular(l)) => {
                plan.constrain(Constraint::Perpendicular { a: line, b: l });
            }
            Some(LineHint::Parallel(l)) => plan.constrain(Constraint::Parallel { a: line, b: l }),
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
        let plan = Plan::new(next_id, self.layer, self.style, self.construction);
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
        // Enter with a value (#334, Fusion): it locks the field being
        // typed in, and the rectangle is placed with the mouse setting any
        // side still free. Tab locks a field without placing.
        self.dims[self.field] = Some(value);
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

    /// The text the next Text-tool click places (laid out by the canvas).
    pub fn set_text(&mut self, text: Option<TextData>) {
        self.text = text.map(Box::new);
    }

    /// The text entity the last click created, once.
    pub fn take_placed(&mut self) -> Option<EntityId> {
        self.placed.take()
    }

    /// Text tool (SPEC §9, #180): an anchor, a point to its right that sets
    /// the baseline direction, a construction baseline between them held
    /// horizontal, and the text, as one undo step. Delete the Horizontal
    /// constraint to turn the text; constrain the anchor like any point.
    fn click_text(&mut self, here: Node, mut plan: Plan) -> Outcome {
        let Some(text) = self.text.clone() else {
            return Outcome::Rejected("Type some text and pick a font first");
        };
        // The direction point is only a direction; keep it clear of the
        // text so it neither clutters nor snaps onto it.
        let reach = (text.size * 3.0).max(10.0);
        let anchor = plan.node(here);
        let dir = plan.point(here.pos() + Vec2::new(reach, 0.0));
        let base = plan.construction(Geom::Line {
            start: anchor,
            end: dir,
        });
        plan.constrain(Constraint::Horizontal { line: base });
        self.placed = Some(plan.add(Geom::Text { anchor, dir, text }));
        Outcome::Commit(plan.finish("Add text"))
    }

    fn click_polygon(&mut self, here: Node, mut plan: Plan, radius: Option<f64>) -> Outcome {
        let Some(&centre) = self.nodes.first() else {
            self.nodes.push(here);
            return Outcome::Pending;
        };
        // A radius typed and locked with Tab (#356) holds; the click turns it.
        let radius = radius.or(self.dims[0]);
        let towards = match radius {
            Some(r) => {
                let d = (here.pos() - centre.pos())
                    .normalized(MIN_SIZE_MM)
                    .unwrap_or(Vec2::new(1.0, 0.0));
                centre.pos() + d * r
            }
            None => here.pos(),
        };
        let Some((pts, r)) = shapes::polygon(centre.pos(), towards, self.sides, self.circumscribed)
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

    /// Edge polygon (#356): two clicks for one side, a third for the side
    /// of it the polygon lies on. Built like an inscribed polygon: its
    /// vertices on a construction circle, every side equal.
    fn click_edge_polygon(&mut self, here: Node, plan: Plan) -> Outcome {
        match self.nodes.len() {
            0 => {
                self.nodes.push(here);
                Outcome::Pending
            }
            1 => {
                // A length typed and locked with Tab holds; the click aims it.
                let at = match self.dims[0] {
                    Some(len) => {
                        let from = self.nodes[0].pos();
                        let d = (here.pos() - from)
                            .normalized(MIN_SIZE_MM)
                            .unwrap_or(Vec2::new(1.0, 0.0));
                        Node::New(from + d * len)
                    }
                    None => here,
                };
                if at.pos().distance(self.nodes[0].pos()) <= MIN_SIZE_MM {
                    return Outcome::Pending;
                }
                self.nodes.push(at);
                self.field = 0;
                Outcome::Pending
            }
            _ => self.edge_polygon(here.pos(), plan),
        }
    }

    fn edge_polygon(&mut self, side: Vec2, mut plan: Plan) -> Outcome {
        let (a, b) = (self.nodes[0], self.nodes[1]);
        let Some((pts, centre, r)) = shapes::edge_polygon(a.pos(), b.pos(), side, self.sides)
        else {
            return Outcome::Pending;
        };
        let c = plan.point(centre);
        let circle = plan.construction(Geom::Circle {
            center: c,
            radius: r,
        });
        let mut ps: Vec<EntityId> = vec![plan.node(a), plan.node(b)];
        ps.extend(pts[2..].iter().map(|p| plan.point(*p)));
        let n = ps.len();
        let lines: Vec<EntityId> = (0..n).map(|i| plan.line(ps[i], ps[(i + 1) % n])).collect();
        for p in &ps {
            plan.constrain(Constraint::PointOnCurve {
                point: *p,
                curve: circle,
            });
        }
        for w in lines.windows(2) {
            plan.constrain(Constraint::Equal { a: w[0], b: w[1] });
        }
        // A typed side length is a dimension (SPEC §6.1).
        if let Some(len) = self.dims[0] {
            plan.constrain(Constraint::Distance {
                a: ps[0],
                b: ps[1],
                dim: Dimension::driving(len),
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

    /// One pen click (`handle` None: a corner) or click-drag (the handle
    /// pulled out of the anchor: a smooth point, G1 with the previous curve).
    /// Each curve after the first anchor is one undo step (#160).
    pub fn pen(&mut self, pick: Pick, handle: Option<Vec2>, doc_next_id: u64) -> Outcome {
        let here = Node::from_pick(pick);
        let handle = handle.filter(|h| h.length() > MIN_SIZE_MM);
        let Some(&from) = self.nodes.first() else {
            self.nodes.push(here);
            self.chain_first = here.id();
            self.out = handle;
            self.last_curve = None;
            return Outcome::Pending;
        };
        if from.pos().distance(here.pos()) < MIN_SIZE_MM
            || (from.id().is_some() && from.id() == here.id())
        {
            return Outcome::Pending;
        }
        let closing = here.id().is_some() && here.id() == self.chain_first;
        let handle = if closing { None } else { handle };
        let mut plan = Plan::new(doc_next_id, self.layer, self.style, self.construction);
        // Corners get handles a third of the way along the chord.
        let third = (here.pos() - from.pos()) / 3.0;
        let c0 = from.pos() + self.out.unwrap_or(third);
        let c1 = here.pos() - handle.unwrap_or(third);
        let p = plan.node(from);
        let h0 = plan.point(c0);
        let h1 = plan.point(c1);
        let q = plan.node(here);
        let curve = plan.add(Geom::Bezier {
            start: p,
            start_ctrl: h0,
            end_ctrl: h1,
            end: q,
        });
        if self.out.is_some()
            && let Some(prev) = self.last_curve
        {
            plan.constrain(Constraint::Tangent { a: prev, b: curve });
        }
        if closing {
            self.cancel();
        } else {
            self.chain_first = self.chain_first.or(Some(p));
            self.nodes = vec![Node::Existing(q, here.pos())];
            self.out = handle;
            self.last_curve = Some(curve);
        }
        Outcome::Commit(plan.finish("Add curve"))
    }

    /// Rubber band for the pen while a handle is being dragged out of the
    /// anchor at `anchor` to `now`.
    pub fn preview_pen(&self, anchor: Vec2, now: Vec2) -> Vec<Shape> {
        let mut out = vec![Shape::Line(Line::new(anchor - (now - anchor), now))];
        if let Some(from) = self.nodes.first().map(|n| n.pos()) {
            let third = (anchor - from) / 3.0;
            out.push(Shape::Cubic(Cubic::new(
                from,
                from + self.out.unwrap_or(third),
                anchor - (now - anchor),
                anchor,
            )));
        }
        out
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
            // The Sides field takes a count, not a length (#356).
            Tool::Polygon | Tool::PolygonEdge if self.two_fields() && self.field == 1 => {
                self.set_sides(value);
                self.field = 0;
                return Outcome::Pending;
            }
            Tool::PolygonEdge if self.nodes.len() == 1 => {
                self.dims[0] = Some(value);
                let at = cursor.pos;
                return self.click_edge_polygon(
                    Node::New(at),
                    Plan::new(doc_next_id, self.layer, self.style, self.construction),
                );
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
        let mut plan = Plan::new(doc_next_id, self.layer, self.style, self.construction);
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

    /// A shape with two heads-up fields is being sized: a rectangle's width
    /// and height, a polygon's radius (or side length) and sides (#356).
    fn two_fields(&self) -> bool {
        matches!(
            self.tool,
            Tool::RectCorner | Tool::RectCentre | Tool::Polygon | Tool::PolygonEdge
        ) && self.nodes.len() == 1
    }

    /// The side count from a typed number (#356): whole, within range.
    fn set_sides(&mut self, value: f64) {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let n = value.round().clamp(0.0, 1000.0) as usize;
        self.set_polygon(n, self.circumscribed);
    }

    /// Tab while sizing (#334, design §3 step 2): a typed `value` locks the
    /// field it was typed in, then the other field takes the typing.
    /// Returns false when the shape has a single field.
    pub fn tab(&mut self, value: Option<f64>) -> bool {
        if !self.two_fields() {
            return false;
        }
        let polygon = matches!(self.tool, Tool::Polygon | Tool::PolygonEdge);
        if let Some(v) = value.filter(|v| v.is_finite() && *v > MIN_SIZE_MM) {
            if polygon && self.field == 1 {
                self.set_sides(v);
            } else {
                self.dims[self.field] = Some(v);
            }
        }
        self.field = 1 - self.field;
        true
    }

    /// The heads-up fields while sizing a two-field shape: name, value at
    /// the cursor (or the locked value), locked, and which takes typing.
    pub fn fields(&self, cursor: Vec2) -> Option<[Field; 2]> {
        if !self.two_fields() {
            return None;
        }
        let d = cursor - self.nodes[0].pos();
        if matches!(self.tool, Tool::Polygon | Tool::PolygonEdge) {
            let name = match self.tool {
                Tool::PolygonEdge => "Length",
                // The radius to a side's middle when circumscribed.
                _ if self.circumscribed => "Apothem",
                _ => "Radius",
            };
            #[allow(clippy::cast_precision_loss)]
            let sides = self.sides as f64;
            return Some([
                Field {
                    name,
                    mm: self.dims[0].unwrap_or(d.length()),
                    locked: self.dims[0].is_some(),
                    active: self.field == 0,
                    count: false,
                },
                Field {
                    name: "Sides",
                    mm: sides,
                    locked: false,
                    active: self.field == 1,
                    count: true,
                },
            ]);
        }
        // A centred rectangle's sides are twice the cursor's offset.
        let k = if self.tool == Tool::RectCentre {
            2.0
        } else {
            1.0
        };
        let f = |i: usize, name, live: f64| Field {
            name,
            mm: self.dims[i].unwrap_or(live * k),
            locked: self.dims[i].is_some(),
            active: self.field == i,
            count: false,
        };
        Some([f(0, "Width", d.x.abs()), f(1, "Height", d.y.abs())])
    }

    /// The numeric value a typed entry would set, for the on-canvas readout:
    /// `(what, value in mm)`.
    pub fn readout(&self, cursor: Vec2) -> Option<(&'static str, f64)> {
        let from = self.nodes.first()?.pos();
        match (self.tool, self.nodes.len()) {
            (Tool::Line, _) => Some(("Length", from.distance(cursor))),
            (Tool::Measure, _) => Some(("Distance", from.distance(cursor))),
            (Tool::Polygon | Tool::PolygonEdge, 1) => {
                let f = self.fields(cursor)?;
                let a = &f[self.field];
                Some((a.name, a.mm))
            }
            (Tool::Circle, _) | (Tool::ArcCentre, 1) => Some(("Radius", from.distance(cursor))),
            (Tool::Circle2, _) => Some(("Diameter", from.distance(cursor))),
            (Tool::RectCorner | Tool::RectCentre, _) => {
                let f = self.fields(cursor)?;
                let a = &f[self.field];
                Some((a.name, a.mm))
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
            // The last measurement stays drawn until the next one starts.
            return match (self.tool, self.measured) {
                (Tool::Measure, Some((a, b))) => vec![guide(a, b)],
                _ => Vec::new(),
            };
        };
        match (self.tool, self.nodes.len()) {
            (Tool::Line | Tool::Measure, _) => vec![guide(first, cursor)],
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
            (Tool::PolygonEdge, 1) => {
                let end = match self.dims[0] {
                    Some(len) => {
                        first
                            + (cursor - first)
                                .normalized(MIN_SIZE_MM)
                                .unwrap_or(Vec2::new(1.0, 0.0))
                                * len
                    }
                    None => cursor,
                };
                vec![guide(first, end)]
            }
            (Tool::PolygonEdge, _) => {
                match shapes::edge_polygon(first, self.nodes[1].pos(), cursor, self.sides) {
                    Some((pts, c, r)) => {
                        let mut v = outline(&pts);
                        v.push(Shape::Circle(Circle::new(c, r)));
                        v
                    }
                    None => Vec::new(),
                }
            }
            (Tool::Polygon, _) => {
                let towards = match self.dims[0] {
                    Some(r) => {
                        first
                            + (cursor - first)
                                .normalized(MIN_SIZE_MM)
                                .unwrap_or(Vec2::new(1.0, 0.0))
                                * r
                    }
                    None => cursor,
                };
                match shapes::polygon(first, towards, self.sides, self.circumscribed) {
                    Some((pts, r)) => {
                        let mut v = outline(&pts);
                        v.push(Shape::Circle(Circle::new(first, r)));
                        v
                    }
                    None => Vec::new(),
                }
            }
            (Tool::Bezier, _) => {
                let third = (cursor - first) / 3.0;
                vec![Shape::Cubic(Cubic::new(
                    first,
                    first + self.out.unwrap_or(third),
                    cursor - third,
                    cursor,
                ))]
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

    fn sample_text(content: &str) -> TextData {
        use locus_text::{Align, BundledFont, BundledProvider, FontRef};
        TextData::new(
            content,
            FontRef::bundled(BundledFont::HersheySans),
            8.0,
            Align::Left,
            1.0,
            &BundledProvider,
        )
        .unwrap()
    }

    #[test]
    fn text_tool_needs_text_then_places_it_as_one_undo_step() {
        let mut s = Sketcher::new(Document::new());
        let mut d = Draft::new(Tool::Text);
        assert!(matches!(
            d.click(at(5.0, 5.0), s.document().next_id()),
            Outcome::Rejected(_)
        ));
        d.set_text(Some(sample_text("Hi")));
        let a = click(&mut d, &mut s, at(5.0, 5.0)).unwrap();
        let id = d.take_placed().unwrap();
        assert!(a.entities.contains(&id));
        let (t, anchor, dir) = s.document().text(id).unwrap();
        assert_eq!(t.content, "Hi");
        // Anchor where clicked; the baseline is held horizontal.
        assert_eq!(s.document().point_pos(anchor), Some(Vec2::new(5.0, 5.0)));
        let pd = s.document().point_pos(dir).unwrap();
        assert!((pd.y - 5.0).abs() < 1e-6 && pd.x > 5.0);
        assert_eq!(count(&s, |g| matches!(g, Geom::Line { .. })), 1);
        // The whole placement is one undo step.
        s.undo().unwrap();
        assert!(s.document().entities().is_empty());
    }

    #[test]
    fn text_tool_reuses_an_existing_point_as_the_anchor() {
        let mut s = Sketcher::new(Document::new());
        let p = click(&mut Draft::new(Tool::Point), &mut s, at(2.0, 3.0))
            .unwrap()
            .entities[0];
        let mut d = Draft::new(Tool::Text);
        d.set_text(Some(sample_text("A")));
        click(&mut d, &mut s, Pick::at_point(Vec2::new(2.0, 3.0), p)).unwrap();
        let id = d.take_placed().unwrap();
        assert_eq!(s.document().text(id).unwrap().1, p);
        assert_eq!(count(&s, |g| matches!(g, Geom::Point { .. })), 2);
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
        // Width, Tab, height, Enter (#334): nothing is placed until Enter.
        assert!(d.tab(Some(40.0)));
        let f = d.fields(Vec2::new(5.0, 5.0)).expect("two fields");
        assert!(f[0].locked && (f[0].mm - 40.0).abs() < 1e-9 && f[1].active);
        assert!((f[1].mm - 5.0).abs() < 1e-9, "the mouse sets the height");
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

    /// #356: while sizing a polygon, Tab moves between the radius and the
    /// side count; a typed count sets the sides, a typed radius holds.
    #[test]
    fn polygon_fields_take_a_radius_and_a_side_count() {
        let mut s = Sketcher::new(Document::new());
        let mut d = Draft::new(Tool::Polygon);
        click(&mut d, &mut s, at(0.0, 0.0));
        let f = d.fields(Vec2::new(3.0, 4.0)).expect("two fields");
        assert_eq!((f[0].name, f[0].mm), ("Radius", 5.0));
        assert!(f[1].count && f[1].name == "Sides" && !f[0].count);
        // Lock a radius, then type the sides into the second field.
        assert!(d.tab(Some(20.0)));
        let out = d.typed(8.0, at(1.0, 0.0), s.document().next_id());
        assert!(matches!(out, Outcome::Pending));
        assert_eq!(d.polygon().0, 8);
        // The click turns the polygon; the locked radius holds.
        click(&mut d, &mut s, at(1.0, 1.0)).expect("placed");
        let radius = s
            .document()
            .entities()
            .iter()
            .find_map(|(_, e)| match e.geom {
                Geom::Circle { radius, .. } => Some(radius),
                _ => None,
            })
            .expect("the construction circle");
        assert!((radius - 20.0).abs() < 1e-9);
        assert_eq!(count(&s, |g| matches!(g, Geom::Line { .. })), 8);
    }

    /// #356: an edge polygon from two clicks and a side; a typed length is
    /// a dimension on the first side.
    #[test]
    fn edge_polygon_is_built_on_the_clicked_side() {
        let mut s = Sketcher::new(Document::new());
        let mut d = Draft::new(Tool::PolygonEdge);
        d.set_polygon(5, false);
        click(&mut d, &mut s, at(0.0, 0.0));
        assert_eq!(
            d.fields(Vec2::new(12.0, 0.0)).expect("fields")[0].name,
            "Length"
        );
        let out = d.typed(12.0, at(1.0, 0.0), s.document().next_id());
        assert!(matches!(out, Outcome::Pending));
        assert!(d.fields(Vec2::ZERO).is_none(), "the side is placed");
        click(&mut d, &mut s, at(6.0, 5.0)).expect("placed");
        let doc = s.document();
        let lengths: Vec<f64> = doc
            .entities()
            .iter()
            .filter_map(|(_, e)| match e.geom {
                Geom::Line { start, end } => Some(
                    doc.point_pos(start)
                        .unwrap()
                        .distance(doc.point_pos(end).unwrap()),
                ),
                _ => None,
            })
            .collect();
        assert_eq!(lengths.len(), 5);
        assert!(
            lengths.iter().all(|l| (l - 12.0).abs() < 1e-6),
            "{lengths:?}"
        );
        assert!(doc.constraints().values().any(|c| matches!(
            c,
            Constraint::Distance { dim, .. } if (dim.value - 12.0).abs() < 1e-9
        )));
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

    // ── Bézier pen (#160) ───────────────────────────────────────────────

    fn pen(
        d: &mut Draft,
        s: &mut Sketcher,
        x: f64,
        y: f64,
        handle: Option<Vec2>,
    ) -> Option<locus_doc::Applied> {
        match d.pen(at(x, y), handle, s.document().next_id()) {
            Outcome::Commit(cmd) => Some(s.apply(&cmd).unwrap()),
            _ => None,
        }
    }

    fn beziers(s: &Sketcher) -> Vec<EntityId> {
        pts_of(s, |g| matches!(g, Geom::Bezier { .. }))
    }

    #[test]
    fn pen_clicks_make_corner_curves_one_undo_step_each() {
        let mut s = Sketcher::new(Document::new());
        let mut d = Draft::new(Tool::Bezier);
        assert!(pen(&mut d, &mut s, 0.0, 0.0, None).is_none());
        let a = pen(&mut d, &mut s, 30.0, 0.0, None).unwrap();
        assert_eq!(a.entities.len(), 5); // two ends, two handles, the curve
        pen(&mut d, &mut s, 30.0, 30.0, None).unwrap();
        assert_eq!(beziers(&s).len(), 2);
        assert!(
            s.document().constraints().is_empty(),
            "corners carry no tangency"
        );
        s.undo().unwrap();
        assert_eq!(beziers(&s).len(), 1);
    }

    #[test]
    fn pen_drag_makes_a_smooth_point_that_stays_tangent() {
        let mut s = Sketcher::new(Document::new());
        let mut d = Draft::new(Tool::Bezier);
        pen(&mut d, &mut s, 0.0, 0.0, None);
        pen(&mut d, &mut s, 30.0, 0.0, Some(Vec2::new(10.0, 8.0)));
        let a = pen(&mut d, &mut s, 60.0, -10.0, None).unwrap();
        assert!(a.constraints.len() == 1);
        assert!(matches!(
            s.document().constraint(a.constraints[0]),
            Some(Constraint::Tangent { .. })
        ));
        // Drag a handle of the first curve: the joint stays smooth.
        let bs = beziers(&s);
        let (j, h_in, h_out) = {
            let first = s.document().entity(bs[0]).unwrap().geom.clone();
            let second = s.document().entity(bs[1]).unwrap().geom.clone();
            match (first, second) {
                (Geom::Bezier { end_ctrl, end, .. }, Geom::Bezier { start_ctrl, .. }) => {
                    (end, end_ctrl, start_ctrl)
                }
                _ => unreachable!(),
            }
        };
        s.begin_drag(h_in).unwrap();
        let from = s.document().point_pos(h_in).unwrap();
        for k in 1..=10 {
            s.drag_to(from + Vec2::new(0.5 * f64::from(k), 1.2 * f64::from(k)))
                .unwrap();
            let p = |id| s.document().point_pos(id).unwrap();
            let (a, b) = (p(j) - p(h_in), p(h_out) - p(j));
            assert!(
                a.cross(b).abs() / (a.length() * b.length()) < 1e-5,
                "frame {k}"
            );
        }
        s.end_drag().unwrap();
    }

    #[test]
    fn pen_closes_on_the_first_point_and_ends_the_chain() {
        let mut s = Sketcher::new(Document::new());
        let mut d = Draft::new(Tool::Bezier);
        pen(&mut d, &mut s, 0.0, 0.0, None);
        let first = pen(&mut d, &mut s, 20.0, 0.0, None).unwrap().entities[0];
        pen(&mut d, &mut s, 20.0, 20.0, None);
        let out = d.pen(
            Pick::at_point(Vec2::new(0.0, 0.0), first),
            None,
            s.document().next_id(),
        );
        run(&mut s, out);
        assert_eq!(beziers(&s).len(), 3);
        assert!(!d.is_drawing());
    }

    /// #334 (Fusion): a value typed then Enter locks the field being typed
    /// in and places the rectangle, the mouse setting the other side; Tab
    /// switches fields without placing; a centred rectangle's fields read
    /// its full sides; other tools have no Tab.
    #[test]
    fn tab_switches_fields_and_enter_places_with_the_mouse_for_the_rest() {
        let mut s = Sketcher::new(Document::new());
        let mut d = Draft::new(Tool::RectCorner);
        click(&mut d, &mut s, at(0.0, 0.0));
        assert!(d.tab(None), "Tab with nothing typed just switches");
        assert!(d.fields(Vec2::new(1.0, 1.0)).expect("fields")[1].active);
        let out = d.typed(12.0, at(30.0, 8.0), s.document().next_id());
        run(&mut s, out);
        assert_rectangle(&s);
        let l = lines(&s);
        let w = l.iter().map(|l| l.length()).fold(0.0, f64::max);
        let h = l.iter().map(|l| l.length()).fold(f64::MAX, f64::min);
        assert!(
            (w - 30.0).abs() < 1e-6 && (h - 12.0).abs() < 1e-6,
            "{w} {h}"
        );

        let mut c = Draft::new(Tool::RectCentre);
        c.nodes.push(Node::New(Vec2::ZERO));
        let f = c.fields(Vec2::new(3.0, -2.0)).expect("fields");
        assert!((f[0].mm - 6.0).abs() < 1e-9 && (f[1].mm - 4.0).abs() < 1e-9);

        let mut line = Draft::new(Tool::Line);
        line.nodes.push(Node::New(Vec2::ZERO));
        assert!(!line.tab(Some(5.0)));
        assert!(line.fields(Vec2::new(1.0, 0.0)).is_none());
    }

    /// #335: a shape started on the origin gets a point fixed at (0, 0)
    /// (coincident to the origin); a rectangle's corner on it too.
    #[test]
    fn a_shape_started_on_the_origin_is_fixed_there() {
        let mut s = Sketcher::new(Document::new());
        let mut d = Draft::new(Tool::Line);
        click(&mut d, &mut s, Pick::at_origin());
        click(&mut d, &mut s, at(10.0, 4.0)).expect("a line");
        let doc = s.document();
        let fixed: Vec<EntityId> = doc
            .constraints()
            .values()
            .filter_map(|c| match c {
                Constraint::Fix { point } => Some(*point),
                _ => None,
            })
            .collect();
        assert_eq!(fixed.len(), 1);
        assert_eq!(doc.point_pos(fixed[0]), Some(Vec2::ZERO));
        // A rectangle whose first corner is the origin.
        let mut s = Sketcher::new(Document::new());
        let mut r = Draft::new(Tool::RectCorner);
        click(&mut r, &mut s, Pick::at_origin());
        click(&mut r, &mut s, at(20.0, 10.0)).expect("a rectangle");
        assert_rectangle(&s);
        let doc = s.document();
        assert!(doc.constraints().values().any(|c| matches!(
            c,
            Constraint::Fix { point } if doc.point_pos(*point) == Some(Vec2::ZERO)
        )));
    }

    /// #357: snaps onto existing geometry create their constraints, and the
    /// result solves: a midpoint, an intersection, a perpendicular foot.
    #[test]
    fn geometry_snaps_create_their_constraints() {
        let mut s = Sketcher::new(Document::new());
        let mut d = Draft::new(Tool::Line);
        click(&mut d, &mut s, at(0.0, 0.0));
        click(&mut d, &mut s, at(20.0, 0.0)).expect("a base line");
        d.cancel();
        let base = s
            .document()
            .entities()
            .iter()
            .find(|(_, e)| matches!(e.geom, Geom::Line { .. }))
            .map(|(id, _)| id)
            .expect("the base line");
        let kinds = |s: &Sketcher| -> Vec<&'static str> {
            s.document()
                .constraints()
                .values()
                .map(|c| c.name())
                .collect()
        };
        // From the base's midpoint, up, then a line ending perpendicular
        // on the base from a point off it.
        let mid = Pick {
            midpoint_of: Some(base),
            ..Pick::free(Vec2::new(10.0, 0.0))
        };
        click(&mut d, &mut s, mid);
        click(&mut d, &mut s, at(10.0, 8.0)).expect("a line from the midpoint");
        assert!(kinds(&s).contains(&"midpoint"), "{:?}", kinds(&s));
        d.cancel();
        click(&mut d, &mut s, at(4.0, 6.0));
        let foot = Pick {
            on_curve: Some(base),
            line: Some(LineHint::Perpendicular(base)),
            ..Pick::free(Vec2::new(4.0, 0.0))
        };
        click(&mut d, &mut s, foot).expect("a perpendicular line");
        let k = kinds(&s);
        assert!(
            k.contains(&"perpendicular") && k.contains(&"point-on-curve"),
            "{k:?}"
        );
        assert!(s.state().converged);
    }
}
