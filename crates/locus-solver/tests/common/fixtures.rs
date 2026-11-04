// SPDX-License-Identifier: AGPL-3.0-or-later

//! Fixture sketches (issue #41, SPEC §13 M0): the M0 test sketch, its under-,
//! over- and redundantly-constrained variants, a ~500-entity generated sketch,
//! and one small case per §5 v1 constraint.

use locus_geom::Vec2;
use locus_solver::ConstraintKind::{self as Constraint, *};
use locus_solver::{ConstraintId, CurveId, CurveKind as Curve, PointId, Sketch};

/// A sketch with a known solution and a perturbed start.
#[derive(Clone, Debug)]
pub struct Fixture {
    pub name: &'static str,
    /// A configuration satisfying every constraint (for the conflict fixtures:
    /// the base sketch's solution, which the extra constraint then violates).
    pub exact: Sketch,
    /// `exact` with every free point and radius nudged: where a full solve starts.
    pub start: Sketch,
    /// Degrees of freedom left, worked out by hand (see each constructor).
    pub expected_dof: usize,
    /// Constraints that a conflict report should name (empty: consistent sketch).
    pub conflict: Vec<ConstraintId>,
    /// Interactive drags along feasible paths, from `exact`.
    pub drags: Vec<Drag>,
}

/// A drag: point `point` follows `path` one target per frame. Every target is
/// reachable without violating a constraint, so "the point reached the target and
/// everything still holds" is a fair check for every backend.
#[derive(Clone, Debug)]
pub struct Drag {
    pub name: &'static str,
    pub point: PointId,
    pub path: Vec<Vec2>,
}

/// One §5 constraint on a tiny sketch that starts off violating it.
#[derive(Clone, Debug)]
pub struct CoverageCase {
    pub name: &'static str,
    pub start: Sketch,
    pub target: ConstraintId,
}

/// Deterministic noise (no `rand`: the harness has no dependencies and must
/// give the same numbers natively and in wasm).
struct Lcg(u64);

impl Lcg {
    /// Uniform in `[-1, 1)`.
    fn next(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 11) as f64 / (1u64 << 53) as f64) * 2.0 - 1.0
    }
}

/// Nudge every point not held by a `Fix`, and every circle radius.
fn perturb(exact: &Sketch, amp: f64, seed: u64) -> Sketch {
    let mut rng = Lcg(seed);
    let mut s = exact.clone();
    let fixed: Vec<PointId> = s
        .constraints
        .iter()
        .filter_map(|k| match k.kind {
            Fix { p, .. } => Some(p),
            _ => None,
        })
        .collect();
    for (i, p) in s.points.iter_mut().enumerate() {
        if !fixed.contains(&PointId(i as u32)) {
            p.at.x += amp * rng.next();
            p.at.y += amp * rng.next();
        }
    }
    for c in &mut s.curves {
        if let Curve::Circle { r, .. } = &mut c.kind {
            *r += 0.2 * amp * rng.next();
        }
    }
    s
}

fn fixture(name: &'static str, exact: Sketch, expected_dof: usize) -> Fixture {
    Fixture {
        name,
        start: perturb(&exact, 0.5, 0x5eed),
        exact,
        expected_dof,
        conflict: Vec::new(),
        drags: Vec::new(),
    }
}

/// Handles into the base M0 sketch, for the variants.
struct Rect {
    sk: Sketch,
    bottom: CurveId,
    top: CurveId,
    c0: PointId,
    c1: PointId,
    radius: ConstraintId,
    width: ConstraintId,
}

/// The SPEC §13 M0 sketch: a 60 x 40 mm rectangle with 5 mm filleted corners
/// (lines tangent to arcs, sharing their end points), a 5 mm circle concentric to
/// the bottom-left fillet, and a distance dimension between the fillet centres.
///
/// Unknowns: 13 points x 2 + 1 circle radius = 27. Equations: 4 arcs (implicit),
/// 8 tangent, 4 horizontal/vertical, 3 equal radius, 1 radius, 2 fix, 1 distance,
/// 1 line-line distance, 2 concentric and 1 diameter = 27, all independent, so
/// DOF = 0.
fn rect() -> Rect {
    let mut sk = Sketch::default();
    let r = 5.0;
    let (w, h) = (60.0, 40.0);
    // Fillet centres: bottom-left, bottom-right, top-right, top-left.
    let c0 = sk.point(r, r);
    let c1 = sk.point(w - r, r);
    let c2 = sk.point(w - r, h - r);
    let c3 = sk.point(r, h - r);
    // Tangent points, two per side, counter-clockwise from the bottom.
    let p4 = sk.point(r, 0.0);
    let p5 = sk.point(w - r, 0.0);
    let p6 = sk.point(w, r);
    let p7 = sk.point(w, h - r);
    let p8 = sk.point(w - r, h);
    let p9 = sk.point(r, h);
    let p10 = sk.point(0.0, h - r);
    let p11 = sk.point(0.0, r);
    let bottom = sk.line(p4, p5);
    let right = sk.line(p6, p7);
    let top = sk.line(p8, p9);
    let left = sk.line(p10, p11);
    let a_br = sk.curve(Curve::Arc {
        centre: c1,
        a: p5,
        b: p6,
    });
    let a_tr = sk.curve(Curve::Arc {
        centre: c2,
        a: p7,
        b: p8,
    });
    let a_tl = sk.curve(Curve::Arc {
        centre: c3,
        a: p9,
        b: p10,
    });
    let a_bl = sk.curve(Curve::Arc {
        centre: c0,
        a: p11,
        b: p4,
    });
    let cc = sk.point(r, r);
    let circle = sk.curve(Curve::Circle { centre: cc, r: 2.5 });

    for (l, a) in [
        (bottom, a_bl),
        (bottom, a_br),
        (right, a_br),
        (right, a_tr),
        (top, a_tr),
        (top, a_tl),
        (left, a_tl),
        (left, a_bl),
    ] {
        sk.constrain(Tangent(l, a));
    }
    sk.constrain(Horizontal(bottom));
    sk.constrain(Horizontal(top));
    sk.constrain(Vertical(right));
    sk.constrain(Vertical(left));
    for a in [a_br, a_tr, a_tl] {
        sk.constrain(EqualRadius(a_bl, a));
    }
    let radius = sk.constrain(Radius { c: a_bl, r });
    sk.constrain(Fix {
        p: c0,
        at: Vec2::new(r, r),
    });
    let width = sk.constrain(Distance {
        p: c0,
        q: c1,
        d: w - 2.0 * r,
    });
    sk.constrain(LineLineDistance {
        l: bottom,
        m: top,
        d: h,
    });
    sk.constrain(Concentric(circle, a_bl));
    sk.constrain(Diameter { c: circle, d: 5.0 });
    Rect {
        sk,
        bottom,
        top,
        c0,
        c1,
        radius,
        width,
    }
}

/// The M0 sketch, fully constrained.
pub fn filleted_rect() -> Fixture {
    fixture("fillet-rect", rect().sk, 0)
}

/// Without the fillet radius: one DOF (the fillets can grow together).
pub fn filleted_rect_under() -> Fixture {
    let mut r = rect();
    r.sk.constraints.remove(r.radius.index());
    fixture("fillet-rect-under", r.sk, 1)
}

/// Plus `parallel(bottom, top)`: already implied by horizontal + horizontal, so
/// redundant but consistent. Should solve with DOF 0.
pub fn filleted_rect_redundant() -> Fixture {
    let mut r = rect();
    r.sk.constrain(Parallel(r.bottom, r.top));
    fixture("fillet-rect-redundant", r.sk, 0)
}

/// Plus a 45 mm horizontal distance between the fillet centres that the 50 mm
/// distance dimension already fixes: a deliberate conflict. A report should name
/// the new constraint and the distance it fights.
pub fn filleted_rect_conflict() -> Fixture {
    let mut r = rect();
    let extra = r.sk.constrain(HorizontalDistance {
        p: r.c0,
        q: r.c1,
        d: 45.0,
    });
    let mut f = fixture("fillet-rect-conflict", r.sk, 0);
    f.conflict = vec![r.width, extra];
    f
}

const CELL_W: f64 = 20.0;
const CELL_H: f64 = 10.0;
const CELL_GAP: f64 = 5.0;
const ARC_R: f64 = 3.0;
const SPOKE_DEG: f64 = 40.0;

/// Points and curves of one generated cell.
struct Cell {
    b: PointId,
    bottom: CurveId,
}

/// A generated sketch of `cells` cells in one connected chain, 7 curves each
/// (`cells = 72` gives 504 curves and 720 points: the §3 "500 entities").
///
/// Each cell: a 20 x 10 rectangle (horizontal, vertical, parallel, perpendicular,
/// line-line distance), a midpoint on its base, a circle centred above it and
/// tangent to the top, a quarter arc (coincident centre on even cells, concentric
/// on odd; radius on even, diameter on odd) with its ends set by horizontal and
/// vertical distances, and a spoke from the circle centre to a point on the circle,
/// at a fixed angle on odd cells and free on even ones. Cell 0 is fixed with a
/// width dimension; every other cell sits on the previous base line, with an equal
/// base length and a 5 mm horizontal gap, except the middle cell, whose gap is left
/// free.
///
/// Per cell: 21 unknowns, 20 equations on odd cells and 21 on even ones, so
/// DOF = (even cells) + 1 (the free gap).
pub fn generated(cells: usize) -> Fixture {
    let mut sk = Sketch::default();
    let free_link = cells / 2;
    let mut prev: Option<Cell> = None;
    let mut chain_point = PointId(0);
    let mut local_point = PointId(0);
    let mut local_centre = Vec2::default();
    for k in 0..cells {
        let ox = k as f64 * (CELL_W + CELL_GAP);
        let r = CELL_H / 2.0;
        let a = sk.point(ox, 0.0);
        let b = sk.point(ox + CELL_W, 0.0);
        let c = sk.point(ox + CELL_W, CELL_H);
        let d = sk.point(ox, CELL_H);
        let m = sk.point(ox + CELL_W / 2.0, 0.0);
        let ec = Vec2::new(ox + CELL_W / 2.0, r);
        let e = sk.point(ec.x, ec.y);
        let f = sk.point(ec.x, ec.y);
        let g = sk.point(ec.x + ARC_R, ec.y);
        let hp = sk.point(ec.x, ec.y + ARC_R);
        let (s, co) = SPOKE_DEG.to_radians().sin_cos();
        let p = sk.point(ec.x + r * co, ec.y + r * s);

        let bottom = sk.line(a, b);
        let right = sk.line(b, c);
        let top = sk.line(c, d);
        let left = sk.line(d, a);
        let circle = sk.curve(Curve::Circle { centre: e, r });
        let arc = sk.curve(Curve::Arc {
            centre: f,
            a: g,
            b: hp,
        });
        let spoke = sk.line(e, p);
        let odd = k % 2 == 1;

        sk.constrain(Horizontal(bottom));
        sk.constrain(Vertical(right));
        sk.constrain(Parallel(top, bottom));
        sk.constrain(Perpendicular(left, bottom));
        sk.constrain(LineLineDistance {
            l: bottom,
            m: top,
            d: CELL_H,
        });
        match &prev {
            None => {
                sk.constrain(Fix {
                    p: a,
                    at: Vec2::new(ox, 0.0),
                });
                sk.constrain(Distance {
                    p: a,
                    q: b,
                    d: CELL_W,
                });
            }
            Some(pc) => {
                sk.constrain(PointOnCurve(a, pc.bottom));
                sk.constrain(EqualLength(bottom, pc.bottom));
                if k != free_link {
                    sk.constrain(HorizontalDistance {
                        p: pc.b,
                        q: a,
                        d: CELL_GAP,
                    });
                }
            }
        }
        sk.constrain(Midpoint { p: m, line: bottom });
        sk.constrain(HorizontalDistance { p: m, q: e, d: 0.0 });
        sk.constrain(VerticalDistance { p: m, q: e, d: r });
        sk.constrain(Tangent(circle, top));
        if odd {
            sk.constrain(Concentric(arc, circle));
            sk.constrain(Diameter {
                c: arc,
                d: 2.0 * ARC_R,
            });
        } else {
            sk.constrain(Coincident(f, e));
            sk.constrain(Radius { c: arc, r: ARC_R });
        }
        sk.constrain(HorizontalDistance {
            p: f,
            q: hp,
            d: 0.0,
        });
        sk.constrain(VerticalDistance { p: f, q: g, d: 0.0 });
        sk.constrain(PointOnCurve(p, circle));
        if odd {
            sk.constrain(Angle {
                l: bottom,
                m: spoke,
                rad: SPOKE_DEG.to_radians(),
            });
        }
        if k == free_link {
            chain_point = a;
        }
        // A free spoke near the middle, for the local drag.
        if !odd && k + 2 >= free_link && local_point == PointId(0) {
            local_point = p;
            local_centre = ec;
        }
        prev = Some(Cell { b, bottom });
    }
    let even = cells.div_ceil(2);
    let mut f = fixture("generated", sk, even + usize::from(cells > 1));
    f.start = perturb(&f.exact, 0.3, 0x0500);
    let frames = 30;
    let start = f.exact.point_at(chain_point).unwrap_or_default();
    f.drags.push(Drag {
        name: "drag-chain",
        point: chain_point,
        path: (1..=frames)
            .map(|i| start + Vec2::new(15.0 * i as f64 / frames as f64, 0.0))
            .collect(),
    });
    f.drags.push(Drag {
        name: "drag-local",
        point: local_point,
        path: (1..=frames)
            .map(|i| {
                let t = (SPOKE_DEG + 60.0 * i as f64 / frames as f64).to_radians();
                local_centre + Vec2::new(t.cos(), t.sin()) * (CELL_H / 2.0)
            })
            .collect(),
    });
    f
}

/// [`generated`] plus a second gap dimension on the first link that fights the
/// first (6 mm against 5 mm): conflict detection at scale.
pub fn generated_conflict(cells: usize) -> Fixture {
    let mut f = generated(cells);
    f.name = "generated-conflict";
    // The first link's gap: the HorizontalDistance from cell 0's B (point 1) to
    // cell 1's A (point 10).
    let link = f
        .exact
        .constraints
        .iter()
        .position(|k| matches!(k.kind, HorizontalDistance { p: PointId(1), q: PointId(10), d } if d == CELL_GAP));
    let extra = f.exact.constrain(HorizontalDistance {
        p: PointId(1),
        q: PointId(10),
        d: CELL_GAP + 1.0,
    });
    f.start.constraints = f.exact.constraints.clone();
    f.conflict = link
        .map(|i| ConstraintId(i as u32))
        .into_iter()
        .chain([extra])
        .collect();
    f.drags.clear();
    f
}

/// One small sketch per SPEC §5 v1 constraint (geometric and dimensional), each
/// starting off by a millimetre or more. Tangent, point-on-curve, equal and
/// radius have one case per curve pairing.
pub fn coverage() -> Vec<CoverageCase> {
    let mut out = Vec::new();
    let mut case = |name: &'static str, build: &dyn Fn(&mut Sketch) -> Constraint| {
        let mut start = Sketch::default();
        let k = build(&mut start);
        let target = start.constrain(k);
        out.push(CoverageCase {
            name,
            start,
            target,
        });
    };
    fn ln(s: &mut Sketch, a: (f64, f64), b: (f64, f64)) -> CurveId {
        let a = s.point(a.0, a.1);
        let b = s.point(b.0, b.1);
        s.line(a, b)
    }
    fn circ(s: &mut Sketch, c: (f64, f64), r: f64) -> CurveId {
        let c = s.point(c.0, c.1);
        s.curve(Curve::Circle { centre: c, r })
    }
    fn arc(s: &mut Sketch, c: (f64, f64), a: (f64, f64), b: (f64, f64)) -> CurveId {
        let c = s.point(c.0, c.1);
        let a = s.point(a.0, a.1);
        let b = s.point(b.0, b.1);
        s.curve(Curve::Arc { centre: c, a, b })
    }
    case("coincident", &|s| {
        let p = s.point(0.0, 0.0);
        let q = s.point(1.0, 1.0);
        Coincident(p, q)
    });
    case("point-on-line", &|s| {
        let l = ln(s, (0.0, 0.0), (10.0, 0.0));
        let p = s.point(5.0, 2.0);
        PointOnCurve(p, l)
    });
    case("point-on-circle", &|s| {
        let c = circ(s, (0.0, 0.0), 5.0);
        let p = s.point(7.0, 1.0);
        PointOnCurve(p, c)
    });
    case("point-on-arc", &|s| {
        let c = arc(s, (0.0, 0.0), (5.0, 0.0), (0.0, 5.0));
        let p = s.point(4.5, 4.5);
        PointOnCurve(p, c)
    });
    case("horizontal", &|s| {
        Horizontal(ln(s, (0.0, 0.0), (10.0, 1.0)))
    });
    case("vertical", &|s| Vertical(ln(s, (0.0, 0.0), (1.0, 10.0))));
    case("parallel", &|s| {
        let a = ln(s, (0.0, 0.0), (10.0, 0.0));
        let b = ln(s, (0.0, 5.0), (10.0, 7.0));
        Parallel(a, b)
    });
    case("perpendicular", &|s| {
        let a = ln(s, (0.0, 0.0), (10.0, 0.0));
        let b = ln(s, (2.0, 1.0), (4.0, 9.0));
        Perpendicular(a, b)
    });
    case("tangent line-circle", &|s| {
        let l = ln(s, (0.0, 0.0), (10.0, 0.0));
        let c = circ(s, (5.0, 6.0), 5.0);
        Tangent(l, c)
    });
    case("tangent circle-circle", &|s| {
        let a = circ(s, (0.0, 0.0), 5.0);
        let b = circ(s, (12.0, 0.0), 5.0);
        Tangent(a, b)
    });
    case("tangent line-arc", &|s| {
        // A fillet: the line ends where the arc starts.
        let q = s.point(-10.0, 0.0);
        let p = s.point(0.0, 0.0);
        let l = s.line(q, p);
        let c = s.point(1.0, 5.0);
        let b = s.point(6.0, 5.0);
        let a = s.curve(Curve::Arc { centre: c, a: p, b });
        Tangent(l, a)
    });
    case("tangent arc-arc", &|s| {
        let a = arc(s, (0.0, 0.0), (5.0, 0.0), (0.0, 5.0));
        let b = arc(s, (12.0, 1.0), (7.0, 1.0), (12.0, 6.0));
        Tangent(a, b)
    });
    case("tangent line-arc apart", &|s| {
        let l = ln(s, (-10.0, 0.0), (10.0, 0.0));
        let a = arc(s, (0.0, 7.0), (5.0, 7.0), (0.0, 12.0));
        Tangent(l, a)
    });
    case("tangent circle-arc", &|s| {
        let a = circ(s, (0.0, 0.0), 5.0);
        let b = arc(s, (12.0, 0.0), (17.0, 0.0), (12.0, 5.0));
        Tangent(a, b)
    });
    case("collinear", &|s| {
        let a = ln(s, (0.0, 0.0), (10.0, 0.0));
        let b = ln(s, (12.0, 1.0), (20.0, 2.0));
        Collinear(a, b)
    });
    case("concentric", &|s| {
        let a = circ(s, (0.0, 0.0), 5.0);
        let b = circ(s, (1.0, 1.0), 3.0);
        Concentric(a, b)
    });
    case("equal length", &|s| {
        let a = ln(s, (0.0, 0.0), (10.0, 0.0));
        let b = ln(s, (0.0, 5.0), (7.0, 5.0));
        EqualLength(a, b)
    });
    case("equal radius", &|s| {
        let a = circ(s, (0.0, 0.0), 5.0);
        let b = circ(s, (20.0, 0.0), 3.0);
        EqualRadius(a, b)
    });
    case("equal radius arc", &|s| {
        let a = circ(s, (0.0, 0.0), 5.0);
        let b = arc(s, (20.0, 0.0), (23.0, 0.0), (20.0, 3.0));
        EqualRadius(a, b)
    });
    case("concentric arc", &|s| {
        let a = arc(s, (0.0, 0.0), (5.0, 0.0), (0.0, 5.0));
        let b = circ(s, (1.0, 1.0), 3.0);
        Concentric(a, b)
    });
    case("symmetric", &|s| {
        let l = ln(s, (0.0, -10.0), (0.0, 10.0));
        let p = s.point(-3.0, 1.0);
        let q = s.point(4.0, 2.0);
        Symmetric { p, q, about: l }
    });
    case("midpoint", &|s| {
        let l = ln(s, (0.0, 0.0), (10.0, 0.0));
        let p = s.point(4.0, 1.0);
        Midpoint { p, line: l }
    });
    case("fix", &|s| {
        let p = s.point(1.0, 1.0);
        Fix {
            p,
            at: Vec2::new(0.0, 0.0),
        }
    });
    case("distance p-p", &|s| {
        let p = s.point(0.0, 0.0);
        let q = s.point(3.0, 4.0);
        Distance { p, q, d: 10.0 }
    });
    case("distance p-l", &|s| {
        let l = ln(s, (0.0, 0.0), (10.0, 0.0));
        let p = s.point(5.0, 3.0);
        PointLineDistance { p, line: l, d: 5.0 }
    });
    case("distance l-l", &|s| {
        let l = ln(s, (0.0, 0.0), (10.0, 0.0));
        let m = ln(s, (0.0, 3.0), (10.0, 3.0));
        LineLineDistance { l, m, d: 5.0 }
    });
    case("horizontal distance", &|s| {
        let p = s.point(0.0, 0.0);
        let q = s.point(3.0, 1.0);
        HorizontalDistance { p, q, d: 10.0 }
    });
    case("vertical distance", &|s| {
        let p = s.point(0.0, 0.0);
        let q = s.point(1.0, 3.0);
        VerticalDistance { p, q, d: 10.0 }
    });
    case("angle", &|s| {
        let l = ln(s, (0.0, 0.0), (10.0, 0.0));
        let m = ln(s, (0.0, 1.0), (10.0, 3.0));
        Angle {
            l,
            m,
            rad: 45f64.to_radians(),
        }
    });
    case("radius circle", &|s| Radius {
        c: circ(s, (0.0, 0.0), 5.0),
        r: 8.0,
    });
    case("radius arc", &|s| Radius {
        c: arc(s, (0.0, 0.0), (5.0, 0.0), (0.0, 5.0)),
        r: 8.0,
    });
    case("diameter", &|s| Diameter {
        c: circ(s, (0.0, 0.0), 5.0),
        d: 12.0,
    });
    out
}

#[cfg(test)]
mod fixture_tests {
    use super::*;

    #[test]
    fn generated_sketch_is_the_section_3_size() {
        let f = generated(72);
        assert_eq!(f.exact.curves.len(), 504);
        assert_eq!(f.exact.points.len(), 720);
        assert_eq!(f.expected_dof, 37);
        assert!(f.drags.iter().all(|d| d.point != PointId(0)));
    }

    #[test]
    fn hand_counted_dof_matches_unknowns_minus_equations() {
        // Every constraint here removes as many DOF as it has scalar equations.
        fn rows(k: &locus_solver::Constraint) -> usize {
            match k.kind {
                Coincident(..) | Concentric(..) | Midpoint { .. } | Fix { .. } => 2,
                Symmetric { .. } | Collinear(..) => 2,
                _ => 1,
            }
        }
        for f in [filleted_rect(), filleted_rect_under(), generated(72)] {
            let arcs = f
                .exact
                .curves
                .iter()
                .filter(|c| matches!(c.kind, Curve::Arc { .. }))
                .count();
            let eqs: usize = f.exact.constraints.iter().map(rows).sum::<usize>() + arcs;
            assert_eq!(f.exact.unknowns() - eqs, f.expected_dof, "{}", f.name);
        }
    }

    #[test]
    fn coverage_lists_every_section_5_v1_constraint() {
        let names: Vec<_> = coverage()
            .iter()
            .map(|c| c.start.constraints[c.target.index()].kind.name())
            .collect();
        for n in [
            "coincident",
            "point-on-curve",
            "horizontal",
            "vertical",
            "parallel",
            "perpendicular",
            "tangent",
            "collinear",
            "concentric",
            "equal-length",
            "equal-radius",
            "symmetric",
            "midpoint",
            "fix",
            "distance",
            "point-line-distance",
            "line-line-distance",
            "horizontal-distance",
            "vertical-distance",
            "angle",
            "radius",
            "diameter",
        ] {
            assert!(names.contains(&n), "{n}");
        }
    }

    #[test]
    fn conflict_fixtures_name_two_constraints() {
        assert_eq!(filleted_rect_conflict().conflict.len(), 2);
        assert_eq!(generated_conflict(72).conflict.len(), 2);
    }
}
