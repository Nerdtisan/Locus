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
use locus_doc::{Command, Constraint, Dimension, Document, EntityId, Geom};
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
}

impl Tool {
    pub const DRAWING: [Tool; 5] = [
        Tool::Point,
        Tool::Line,
        Tool::Circle,
        Tool::ArcThreePoint,
        Tool::ArcCentre,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Tool::Select => "Select",
            Tool::Point => "Point",
            Tool::Line => "Line",
            Tool::Circle => "Circle",
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

/// What the cursor designates: a position (already adjusted by inference),
/// the existing point entity it snapped onto, the curve it lies on, and the
/// constraint the line being drawn gets (#69).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pick {
    pub pos: Vec2,
    pub point: Option<EntityId>,
    pub on_curve: Option<EntityId>,
    pub line: Option<LineHint>,
}

impl Pick {
    pub fn free(pos: Vec2) -> Self {
        Self {
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
    commands: Vec<Command>,
}

impl Plan {
    fn new(doc_next_id: u64) -> Self {
        Self {
            next: doc_next_id,
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
            layer: Document::default_layer(),
            construction: false,
            geom,
        });
        EntityId::new(self.alloc())
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
}

impl Draft {
    pub fn new(tool: Tool) -> Self {
        Self {
            tool,
            nodes: Vec::new(),
            chain_first: None,
        }
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
            Tool::Circle | Tool::ArcCentre => self.nodes.is_empty(),
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
    }

    /// Enter: end the polyline (or abandon a half-drawn shape).
    pub fn finish(&mut self) {
        self.cancel();
    }

    pub fn click(&mut self, pick: Pick, doc_next_id: u64) -> Outcome {
        let here = Node::from_pick(pick);
        let mut plan = Plan::new(doc_next_id);
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

    /// A number typed while drawing: the line's length, the circle's radius,
    /// or the centre-arc's radius, in the direction of the cursor. Lengths
    /// and radii typed this way become dimensions (SPEC §6.1).
    pub fn typed(&mut self, value: f64, cursor: Pick, doc_next_id: u64) -> Outcome {
        if !(value.is_finite() && value > MIN_SIZE_MM) {
            return Outcome::Rejected("Enter a positive number");
        }
        let Some(&from) = self.nodes.first() else {
            return Outcome::Pending;
        };
        let dir = (cursor.pos - from.pos())
            .normalized(MIN_SIZE_MM)
            .unwrap_or(Vec2::new(1.0, 0.0));
        let at = Node::New(from.pos() + dir * value);
        let mut plan = Plan::new(doc_next_id);
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
            _ => Outcome::Pending,
        }
    }

    /// The numeric value a typed entry would set, for the on-canvas readout:
    /// `(what, value in mm)`.
    pub fn readout(&self, cursor: Vec2) -> Option<(&'static str, f64)> {
        let from = self.nodes.first()?.pos();
        match (self.tool, self.nodes.len()) {
            (Tool::Line, _) => Some(("Length", from.distance(cursor))),
            (Tool::Circle, _) | (Tool::ArcCentre, 1) => Some(("Radius", from.distance(cursor))),
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
}
