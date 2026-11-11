// SPDX-License-Identifier: AGPL-3.0-or-later
//! Constraint tools (#70, SPEC §5, §6.2), independent of the GUI: which
//! constraint a selection supports, and the dimension a selection implies.
//!
//! Everything returns a [`Command`] for the caller to run through the
//! `Sketcher` (SPEC §11); the solver's pre-check refuses conflicting
//! constraints there. Nothing here touches the document.

use locus_doc::{Command, Constraint, Dimension, Document, Edge, EntityId, EntityKind};
use locus_geom::Vec2;

/// Lines closer than this (radians) to parallel are dimensioned by distance,
/// not angle.
const PARALLEL_TOL: f64 = 1e-3;

/// The geometric constraints, in menu order (SPEC §5 v1 list).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Coincident,
    Horizontal,
    Vertical,
    Parallel,
    Perpendicular,
    Tangent,
    Collinear,
    Concentric,
    Equal,
    Symmetric,
    Midpoint,
    Fix,
}

impl Kind {
    pub const ALL: [Kind; 12] = [
        Kind::Coincident,
        Kind::Horizontal,
        Kind::Vertical,
        Kind::Parallel,
        Kind::Perpendicular,
        Kind::Tangent,
        Kind::Collinear,
        Kind::Concentric,
        Kind::Equal,
        Kind::Symmetric,
        Kind::Midpoint,
        Kind::Fix,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Kind::Coincident => "Coincident",
            Kind::Horizontal => "Horizontal",
            Kind::Vertical => "Vertical",
            Kind::Parallel => "Parallel",
            Kind::Perpendicular => "Perpendicular",
            Kind::Tangent => "Tangent",
            Kind::Collinear => "Collinear",
            Kind::Concentric => "Concentric",
            Kind::Equal => "Equal",
            Kind::Symmetric => "Symmetric",
            Kind::Midpoint => "Midpoint",
            Kind::Fix => "Fix",
        }
    }

    /// Single-key shortcut (Select mode, pointer over the canvas).
    pub fn key(self) -> char {
        match self {
            Kind::Coincident => 'K',
            Kind::Horizontal => 'H',
            Kind::Vertical => 'V',
            Kind::Parallel => 'Q',
            Kind::Perpendicular => 'W',
            Kind::Tangent => 'T',
            Kind::Collinear => 'X',
            Kind::Concentric => 'O',
            Kind::Equal => 'E',
            Kind::Symmetric => 'Y',
            Kind::Midpoint => 'M',
            Kind::Fix => 'F',
        }
    }

    /// What to select, for the hover text and the refusal message.
    pub fn needs(self) -> &'static str {
        match self {
            Kind::Coincident => "two points, or a point and a curve",
            Kind::Horizontal | Kind::Vertical => "one or more lines",
            Kind::Parallel | Kind::Perpendicular | Kind::Collinear => "two lines",
            Kind::Tangent => "two curves (line, circle, arc or Bézier)",
            Kind::Concentric => "two circles or arcs",
            Kind::Equal => "two or more lines, or two or more circles/arcs",
            Kind::Symmetric => "two points and a line to mirror across",
            Kind::Midpoint => "a point and a line",
            Kind::Fix => "one or more points",
        }
    }
}

/// The selection sorted by what it is.
#[derive(Debug, Default)]
struct Sorted {
    points: Vec<EntityId>,
    lines: Vec<EntityId>,
    circles: Vec<EntityId>,
    arcs: Vec<EntityId>,
    beziers: Vec<EntityId>,
}

impl Sorted {
    fn new(doc: &Document, sel: &[EntityId]) -> Self {
        let mut s = Self::default();
        for &id in sel {
            match doc.entity(id).map(|e| e.geom.kind()) {
                Some(EntityKind::Point) => s.points.push(id),
                Some(EntityKind::Line) => s.lines.push(id),
                Some(EntityKind::Circle) => s.circles.push(id),
                Some(EntityKind::Arc) => s.arcs.push(id),
                Some(EntityKind::Bezier) => s.beziers.push(id),
                // Text is a container; constraints reach it through its points.
                Some(EntityKind::Text) => {}
                None => {}
            }
        }
        s
    }

    fn round(&self) -> Vec<EntityId> {
        self.circles.iter().chain(&self.arcs).copied().collect()
    }

    fn curves(&self) -> Vec<EntityId> {
        self.lines
            .iter()
            .chain(&self.circles)
            .chain(&self.arcs)
            .copied()
            .collect()
    }

    fn total(&self) -> usize {
        self.points.len()
            + self.lines.len()
            + self.circles.len()
            + self.arcs.len()
            + self.beziers.len()
    }
}

/// One constraint, or several (one undo step) when the selection has several
/// targets.
fn commands(kind: Kind, cs: Vec<Constraint>) -> Command {
    let mut cmds: Vec<Command> = cs
        .into_iter()
        .map(|constraint| Command::AddConstraint { constraint })
        .collect();
    if cmds.len() == 1 {
        cmds.remove(0)
    } else {
        Command::Compound {
            label: format!("Add {} constraints", kind.label().to_lowercase()),
            commands: cmds,
        }
    }
}

/// The command applying `kind` to the selection, or why it can't.
///
/// # Errors
/// A message naming what the constraint needs selected.
pub fn plan(kind: Kind, doc: &Document, sel: &[EntityId]) -> Result<Command, String> {
    let s = Sorted::new(doc, sel);
    let bad = || format!("{} needs {} selected", kind.label(), kind.needs());
    let only = |n: usize, f: &dyn Fn(&Sorted) -> usize| s.total() == n && f(&s) == n;
    let cs = match kind {
        Kind::Coincident => {
            if only(2, &|s| s.points.len()) {
                vec![Constraint::Coincident {
                    a: s.points[0],
                    b: s.points[1],
                }]
            } else if s.points.len() == 1 && s.total() == 2 {
                vec![Constraint::PointOnCurve {
                    point: s.points[0],
                    curve: s.curves()[0],
                }]
            } else {
                return Err(bad());
            }
        }
        Kind::Horizontal | Kind::Vertical => {
            if s.lines.is_empty() || s.lines.len() != s.total() {
                return Err(bad());
            }
            s.lines
                .iter()
                .map(|&line| {
                    if kind == Kind::Horizontal {
                        Constraint::Horizontal { line }
                    } else {
                        Constraint::Vertical { line }
                    }
                })
                .collect()
        }
        Kind::Parallel | Kind::Perpendicular | Kind::Collinear => {
            if !only(2, &|s| s.lines.len()) {
                return Err(bad());
            }
            let (a, b) = (s.lines[0], s.lines[1]);
            vec![match kind {
                Kind::Parallel => Constraint::Parallel { a, b },
                Kind::Perpendicular => Constraint::Perpendicular { a, b },
                _ => Constraint::Collinear { a, b },
            }]
        }
        Kind::Tangent if !s.beziers.is_empty() => {
            // G1 at a shared end point: a Bézier with a line, arc or Bézier.
            let c: Vec<EntityId> = s
                .beziers
                .iter()
                .chain(&s.lines)
                .chain(&s.arcs)
                .copied()
                .collect();
            if s.total() != 2 || c.len() != 2 {
                return Err(bad());
            }
            vec![Constraint::Tangent { a: c[0], b: c[1] }]
        }
        Kind::Tangent => {
            let c = s.curves();
            if s.total() != 2 || c.len() != 2 || s.lines.len() == 2 {
                return Err(bad());
            }
            vec![Constraint::Tangent { a: c[0], b: c[1] }]
        }
        Kind::Concentric => {
            if !(s.total() == 2 && s.round().len() == 2) {
                return Err(bad());
            }
            let r = s.round();
            vec![Constraint::Concentric { a: r[0], b: r[1] }]
        }
        Kind::Equal => {
            let group = if s.lines.len() == s.total() {
                s.lines.clone()
            } else if s.round().len() == s.total() {
                s.round()
            } else {
                return Err(bad());
            };
            if group.len() < 2 {
                return Err(bad());
            }
            group[1..]
                .iter()
                .map(|&b| Constraint::Equal { a: group[0], b })
                .collect()
        }
        Kind::Symmetric => {
            if !(s.total() == 3 && s.points.len() == 2 && s.lines.len() == 1) {
                return Err(bad());
            }
            vec![Constraint::Symmetric {
                a: s.points[0],
                b: s.points[1],
                axis: s.lines[0],
            }]
        }
        Kind::Midpoint => {
            if !(s.total() == 2 && s.points.len() == 1 && s.lines.len() == 1) {
                return Err(bad());
            }
            vec![Constraint::Midpoint {
                point: s.points[0],
                line: s.lines[0],
            }]
        }
        Kind::Fix => {
            if s.points.is_empty() || s.points.len() != s.total() {
                return Err(bad());
            }
            s.points
                .iter()
                .map(|&point| Constraint::Fix { point })
                .collect()
        }
    };
    Ok(commands(kind, cs))
}

/// The same dimension as a driven (reference) one: measured, not enforced
/// (SPEC §5). `None` for a constraint that has no value.
pub fn as_driven(c: &Constraint) -> Option<Constraint> {
    let mut c = c.clone();
    let dim = match &mut c {
        Constraint::Distance { dim, .. }
        | Constraint::HorizontalDistance { dim, .. }
        | Constraint::VerticalDistance { dim, .. }
        | Constraint::Angle { dim, .. }
        | Constraint::Radius { dim, .. }
        | Constraint::Diameter { dim, .. } => dim,
        _ => return None,
    };
    dim.driven = true;
    Some(c)
}

/// "Line 6", "Point 2": how the constraint list names an entity.
pub fn entity_name(doc: &Document, id: EntityId) -> String {
    let kind = match doc.entity(id).map(|e| e.geom.kind()) {
        Some(EntityKind::Point) => "Point",
        Some(EntityKind::Line) => "Line",
        Some(EntityKind::Circle) => "Circle",
        Some(EntityKind::Arc) => "Arc",
        Some(EntityKind::Bezier) => "Bézier",
        Some(EntityKind::Text) => "Text",
        None => "Missing",
    };
    format!("{kind} {id}")
}

/// One row of the constraints panel: the constraint, its value if it has
/// one, and what it ties together.
pub fn describe(doc: &Document, c: &Constraint, units: crate::settings::Units) -> String {
    let refs = c
        .references()
        .into_iter()
        .map(|r| entity_name(doc, r))
        .collect::<Vec<_>>()
        .join(", ");
    let value = c.dimension().map(|d| {
        let v = if matches!(c, Constraint::Angle { .. }) {
            format!("{:.2}°", d.value.to_degrees())
        } else {
            units.format_len(d.value)
        };
        if d.driven {
            format!(" ({v})")
        } else {
            format!(" {v}")
        }
    });
    format!("{}{}: {refs}", c.name(), value.unwrap_or_default())
}

/// What a dimension measures, before its value is chosen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DimSpec {
    /// Distance between two points, a point and a line, or two parallel lines.
    Distance(EntityId, EntityId),
    Radius(EntityId),
    Diameter(EntityId),
    /// Angle between two lines.
    Angle(EntityId, EntityId),
}

impl DimSpec {
    /// The constraint with `value` (mm, or radians for an angle).
    pub fn constraint(self, value: f64, driven: bool) -> Constraint {
        let dim = Dimension::new(value, driven, Edge::Centre);
        match self {
            DimSpec::Distance(a, b) => Constraint::Distance { a, b, dim },
            DimSpec::Radius(curve) => Constraint::Radius { curve, dim },
            DimSpec::Diameter(curve) => Constraint::Diameter { curve, dim },
            DimSpec::Angle(a, b) => Constraint::Angle { a, b, dim },
        }
    }

    pub fn is_angle(self) -> bool {
        matches!(self, DimSpec::Angle(..))
    }

    pub fn label(self) -> &'static str {
        match self {
            DimSpec::Distance(..) => "Distance",
            DimSpec::Radius(_) => "Radius",
            DimSpec::Diameter(_) => "Diameter",
            DimSpec::Angle(..) => "Angle",
        }
    }
}

/// Infer the dimension a selection asks for and its current value (mm, or
/// radians for an angle). One line → its length; two points → their
/// distance; a circle → diameter; an arc → radius; two lines → the angle
/// between them (or the distance, when parallel); a point and a line → the
/// distance from the point to the line.
///
/// # Errors
/// A message when the selection doesn't suggest a dimension.
pub fn infer_dimension(doc: &Document, sel: &[EntityId]) -> Result<(DimSpec, f64), String> {
    let s = Sorted::new(doc, sel);
    let pos = |id| doc.point_pos(id).ok_or_else(|| "Unknown point".to_owned());
    let err = || {
        "Select a line, two points, a circle or arc, two lines, or a point and a line to dimension"
            .to_owned()
    };
    if s.total() == 1 && s.lines.len() == 1 {
        let l = doc.line(s.lines[0]).ok_or_else(err)?;
        let (a, b) = endpoints(doc, s.lines[0]).ok_or_else(err)?;
        return Ok((DimSpec::Distance(a, b), l.length()));
    }
    if s.total() == 2 && s.points.len() == 2 {
        let d = pos(s.points[0])?.distance(pos(s.points[1])?);
        return Ok((DimSpec::Distance(s.points[0], s.points[1]), d));
    }
    if s.total() == 1 && s.circles.len() == 1 {
        let c = doc.circle(s.circles[0]).ok_or_else(err)?;
        return Ok((DimSpec::Diameter(s.circles[0]), c.radius * 2.0));
    }
    if s.total() == 1 && s.arcs.len() == 1 {
        let a = doc.arc(s.arcs[0]).ok_or_else(err)?;
        return Ok((DimSpec::Radius(s.arcs[0]), a.radius()));
    }
    if s.total() == 2 && s.lines.len() == 2 {
        let (l, m) = (doc.line(s.lines[0]), doc.line(s.lines[1]));
        let (l, m) = l.zip(m).ok_or_else(err)?;
        let (dl, dm) = (l.b - l.a, m.b - m.a);
        let angle = dl.cross(dm).atan2(dl.dot(dm));
        let off_parallel = angle.sin().abs();
        return if off_parallel < PARALLEL_TOL {
            let n = dl.perp().normalized(1e-12).ok_or_else(err)?;
            let d = ((m.a - l.a).dot(n)).abs();
            Ok((DimSpec::Distance(s.lines[0], s.lines[1]), d))
        } else {
            Ok((DimSpec::Angle(s.lines[0], s.lines[1]), angle))
        };
    }
    if s.total() == 2 && s.points.len() == 1 && s.lines.len() == 1 {
        let l = doc.line(s.lines[0]).ok_or_else(err)?;
        let p = pos(s.points[0])?;
        // Perpendicular distance to the infinite line, as the solver measures it.
        let n = (l.b - l.a).perp().normalized(1e-12).ok_or_else(err)?;
        return Ok((
            DimSpec::Distance(s.points[0], s.lines[0]),
            (p - l.a).dot(n).abs(),
        ));
    }
    Err(err())
}

fn endpoints(doc: &Document, line: EntityId) -> Option<(EntityId, EntityId)> {
    match doc.entity(line)?.geom {
        locus_doc::Geom::Line { start, end } => Some((start, end)),
        _ => None,
    }
}

/// Where the inline value editor for a new dimension appears (world mm).
pub fn dim_anchor(doc: &Document, spec: DimSpec) -> Vec2 {
    let mid = |a: Vec2, b: Vec2| a.lerp(b, 0.5);
    let anchor = |id| -> Option<Vec2> {
        match doc.entity(id)?.geom {
            locus_doc::Geom::Point { pos } => Some(pos),
            locus_doc::Geom::Line { .. } => doc.line(id).map(|l| l.eval(0.5)),
            locus_doc::Geom::Circle { .. } => doc.circle(id).map(|c| c.center),
            locus_doc::Geom::Arc { .. } => doc.arc(id).map(|a| a.center),
            locus_doc::Geom::Bezier { .. } => doc.cubic(id).map(|c| c.eval(0.5)),
            locus_doc::Geom::Text { anchor, .. } => doc.point_pos(anchor),
        }
    };
    match spec {
        DimSpec::Distance(a, b) | DimSpec::Angle(a, b) => anchor(a)
            .zip(anchor(b))
            .map_or(Vec2::ZERO, |(p, q)| mid(p, q)),
        DimSpec::Radius(c) | DimSpec::Diameter(c) => anchor(c).unwrap_or(Vec2::ZERO),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use locus_doc::{Geom, Sketcher};

    struct Fix {
        s: Sketcher,
        p: Vec<EntityId>,
        l: Vec<EntityId>,
        c: EntityId,
        a: EntityId,
    }

    fn add(s: &mut Sketcher, geom: Geom) -> EntityId {
        s.apply(&Command::AddEntity {
            layer: Document::default_layer(),
            construction: false,
            geom,
        })
        .unwrap()
        .entities[0]
    }

    fn fix() -> Fix {
        let mut s = Sketcher::new(Document::new());
        let pts: Vec<_> = [
            (0.0, 0.0),
            (10.0, 1.0),
            (10.0, 12.0),
            (-1.0, 14.0),
            (30.0, 30.0),
        ]
        .iter()
        .map(|&(x, y)| {
            add(
                &mut s,
                Geom::Point {
                    pos: Vec2::new(x, y),
                },
            )
        })
        .collect();
        let l = vec![
            add(
                &mut s,
                Geom::Line {
                    start: pts[0],
                    end: pts[1],
                },
            ),
            add(
                &mut s,
                Geom::Line {
                    start: pts[1],
                    end: pts[2],
                },
            ),
            add(
                &mut s,
                Geom::Line {
                    start: pts[2],
                    end: pts[3],
                },
            ),
        ];
        let c = add(
            &mut s,
            Geom::Circle {
                center: pts[4],
                radius: 4.0,
            },
        );
        let ac = add(
            &mut s,
            Geom::Point {
                pos: Vec2::new(50.0, 0.0),
            },
        );
        let a0 = add(
            &mut s,
            Geom::Point {
                pos: Vec2::new(55.0, 0.0),
            },
        );
        let a1 = add(
            &mut s,
            Geom::Point {
                pos: Vec2::new(50.0, 5.0),
            },
        );
        let a = add(
            &mut s,
            Geom::Arc {
                center: ac,
                start: a0,
                end: a1,
            },
        );
        Fix { s, p: pts, l, c, a }
    }

    fn name(c: &Command) -> String {
        match c {
            Command::AddConstraint { constraint } => constraint.name().to_owned(),
            Command::Compound { label, .. } => label.clone(),
            _ => "?".to_owned(),
        }
    }

    #[test]
    fn every_kind_is_reachable_with_the_right_selection() {
        let f = fix();
        let d = f.s.document();
        let cases: [(Kind, Vec<EntityId>, &str); 12] = [
            (Kind::Coincident, vec![f.p[0], f.p[3]], "coincident"),
            (Kind::Horizontal, vec![f.l[0]], "horizontal"),
            (Kind::Vertical, vec![f.l[1]], "vertical"),
            (Kind::Parallel, vec![f.l[0], f.l[2]], "parallel"),
            (Kind::Perpendicular, vec![f.l[0], f.l[1]], "perpendicular"),
            (Kind::Tangent, vec![f.l[0], f.c], "tangent"),
            (Kind::Collinear, vec![f.l[0], f.l[2]], "collinear"),
            (Kind::Concentric, vec![f.c, f.a], "concentric"),
            (Kind::Equal, vec![f.l[0], f.l[1]], "equal"),
            (Kind::Symmetric, vec![f.p[1], f.p[3], f.l[1]], "symmetric"),
            (Kind::Midpoint, vec![f.p[4], f.l[0]], "midpoint"),
            (Kind::Fix, vec![f.p[0]], "fix"),
        ];
        for (kind, sel, want) in cases {
            let cmd = plan(kind, d, &sel).unwrap();
            assert_eq!(name(&cmd), want, "{kind:?}");
        }
    }

    #[test]
    fn point_and_curve_is_point_on_curve_and_wrong_selections_explain() {
        let f = fix();
        let d = f.s.document();
        let cmd = plan(Kind::Coincident, d, &[f.p[4], f.l[0]]).unwrap();
        assert_eq!(name(&cmd), "point-on-curve");
        let e = plan(Kind::Parallel, d, &[f.l[0]]).unwrap_err();
        assert!(e.contains("two lines"), "{e}");
        assert!(plan(Kind::Tangent, d, &[f.l[0], f.l[1]]).is_err());
        assert!(plan(Kind::Horizontal, d, &[f.l[0], f.p[0]]).is_err());
        assert!(plan(Kind::Fix, d, &[]).is_err());
    }

    #[test]
    fn several_targets_become_one_labelled_undo_step() {
        let mut f = fix();
        let cmd = plan(Kind::Horizontal, f.s.document(), &[f.l[0], f.l[2]]).unwrap();
        assert_eq!(name(&cmd), "Add horizontal constraints");
        f.s.apply(&cmd).unwrap();
        assert_eq!(f.s.document().constraints().len(), 2);
        f.s.undo().unwrap();
        assert!(f.s.document().constraints().is_empty());
        let cmd = plan(Kind::Equal, f.s.document(), &[f.l[0], f.l[1], f.l[2]]).unwrap();
        f.s.apply(&cmd).unwrap();
        assert_eq!(f.s.document().constraints().len(), 2);
    }

    #[test]
    fn dimension_is_inferred_from_the_selection() {
        let f = fix();
        let d = f.s.document();
        let (spec, v) = infer_dimension(d, &[f.l[0]]).unwrap();
        assert!(matches!(spec, DimSpec::Distance(..)));
        assert!((v - (101.0f64).sqrt()).abs() < 1e-9);
        let (spec, v) = infer_dimension(d, &[f.c]).unwrap();
        assert_eq!((spec, v), (DimSpec::Diameter(f.c), 8.0));
        assert_eq!(
            infer_dimension(d, &[f.a]).unwrap(),
            (DimSpec::Radius(f.a), 5.0)
        );
        let (spec, v) = infer_dimension(d, &[f.l[0], f.l[1]]).unwrap();
        assert!(spec.is_angle());
        assert!(v.abs() > 0.1 && v.abs() < std::f64::consts::PI);
        let (spec, _) = infer_dimension(d, &[f.p[0], f.p[1]]).unwrap();
        assert!(matches!(spec, DimSpec::Distance(..)));
        let (spec, v) = infer_dimension(d, &[f.p[4], f.l[0]]).unwrap();
        assert!(matches!(spec, DimSpec::Distance(..)) && v > 0.0);
        assert!(infer_dimension(d, &[]).is_err());
    }

    #[test]
    fn as_driven_keeps_the_value_and_only_dimensions_have_one() {
        let d = Constraint::Radius {
            curve: EntityId::new(5),
            dim: Dimension::driving(3.0),
        };
        assert_eq!(
            as_driven(&d),
            Some(Constraint::Radius {
                curve: EntityId::new(5),
                dim: Dimension::new(3.0, true, Edge::Centre)
            })
        );
        assert_eq!(
            as_driven(&Constraint::Fix {
                point: EntityId::new(2)
            }),
            None
        );
    }

    #[test]
    fn rows_name_the_constraint_value_and_entities() {
        let f = fix();
        let c = Constraint::Distance {
            a: f.p[0],
            b: f.p[1],
            dim: Dimension::driving(12.5),
        };
        let row = describe(f.s.document(), &c, crate::settings::Units::Mm);
        assert!(row.starts_with("distance 12.500 mm: Point "), "{row}");
        let driven = as_driven(&c).unwrap();
        assert!(
            describe(f.s.document(), &driven, crate::settings::Units::Mm).contains("(12.500 mm)")
        );
    }

    #[test]
    fn a_dimension_drives_the_geometry_to_the_typed_value() {
        let mut f = fix();
        let (spec, _) = infer_dimension(f.s.document(), &[f.c]).unwrap();
        f.s.apply(&Command::AddConstraint {
            constraint: spec.constraint(20.0, false),
        })
        .unwrap();
        assert!((f.s.document().circle(f.c).unwrap().radius - 10.0).abs() < 1e-6);
    }

    #[test]
    fn a_conflicting_dimension_is_refused_with_the_constraints_named() {
        let mut f = fix();
        let (spec, _) = infer_dimension(f.s.document(), &[f.c]).unwrap();
        f.s.apply(&Command::AddConstraint {
            constraint: spec.constraint(20.0, false),
        })
        .unwrap();
        let again = f.s.apply(&Command::AddConstraint {
            constraint: spec.constraint(30.0, false),
        });
        assert!(matches!(
            again,
            Err(locus_doc::DocError::Conflict { .. } | locus_doc::DocError::Redundant { .. })
        ));
    }
}
