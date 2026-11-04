// SPDX-License-Identifier: AGPL-3.0-or-later

//! How constrained a sketch is (#60) and which constraints are redundant or
//! conflicting (#61), as structured data for the UI and, later, MCP (§5 Solver
//! UX, §16.3). Everything names the caller's [`Key`]s.

use crate::rank::{Echelon, small_rank};
use crate::sketch::{CurveKind, Key, Sketch};
use crate::system::{Eq, System};
use locus_geom::SOLVER_TOL;

/// §5 sketch status.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// Some geometry can still move: [`Analysis::dof`] > 0.
    UnderConstrained,
    /// Nothing can move and no constraint is redundant or conflicting.
    FullyConstrained,
    /// At least one constraint is redundant or conflicting.
    OverConstrained,
}

/// A minimal set of constraints that depend on each other: removing any one
/// of them breaks the dependency.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConstraintSet {
    /// The constraint found to depend on the others (the last of the set in
    /// sketch order, or the one being checked).
    pub constraint: Key,
    /// Every constraint in the set, `constraint` included, in sketch order.
    pub constraints: Vec<Key>,
    /// Points and curves those constraints refer to (curves include arcs whose
    /// implicit equal-radius condition is part of the set).
    pub points: Vec<Key>,
    pub curves: Vec<Key>,
}

/// Remaining freedom of one point or curve.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Freedom {
    /// Independent ways it can still move (0 when fully constrained).
    pub dof: u8,
}

impl Freedom {
    /// Drawn in the fully-constrained colour (§5).
    pub fn is_fixed(self) -> bool {
        self.dof == 0
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Analysis {
    /// Whether the solve behind this analysis converged. When it didn't, the
    /// analysis is at the closest geometry found and `conflicts` says why.
    pub converged: bool,
    /// Degrees of freedom left in the whole sketch.
    pub dof: usize,
    pub status: Status,
    /// Per point and per curve, in sketch order.
    pub points: Vec<Freedom>,
    pub curves: Vec<Freedom>,
    /// Consistent constraints implied by others (removable without change).
    pub redundant: Vec<ConstraintSet>,
    /// Constraints that cannot all hold.
    pub conflicts: Vec<ConstraintSet>,
}

/// Analyse at `x` (a solution, or the least-squares point of a failed solve).
pub(crate) fn analyse(sys: &System, sk: &Sketch, x: &[f64], converged: bool) -> Analysis {
    let (mut r, mut j) = (vec![0.0; sys.m], vec![0.0; sys.nj]);
    sys.eval(sk, x, &mut r, Some(&mut j));
    // Which block each row belongs to.
    let mut owner = vec![usize::MAX; sys.m];
    for (bi, b) in sys.blocks.iter().enumerate() {
        for k in 0..b.rows {
            owner[b.r0 + k] = bi;
        }
    }
    let mut ech = Echelon::new(sys.n);
    let (mut redundant, mut conflicts) = (Vec::new(), Vec::new());
    let mut grad = Vec::new();
    for b in &sys.blocks {
        if matches!(b.eq, Eq::Hold { .. }) {
            continue;
        }
        let nv = b.vars.len();
        let mut found: Option<Vec<usize>> = None;
        for k in 0..b.rows {
            let g = &j[b.j0 + k * nv..b.j0 + (k + 1) * nv];
            grad.clear();
            grad.extend(b.vars.iter().copied().zip(g.iter().copied()));
            if let Some(c) = ech.insert(b.r0 + k, &grad) {
                found.get_or_insert(c);
            }
        }
        let (Some(rows), Eq::User(ci)) = (found, b.eq) else {
            continue;
        };
        let violated = rows.iter().any(|&row| r[row].abs() > SOLVER_TOL);
        let set = set_of(sys, sk, &owner, &rows, ci.index());
        if violated {
            conflicts.push(set);
        } else {
            redundant.push(set);
        }
    }
    let dof = sys.n - ech.rank();
    let (points, curves) = freedom(sys, sk, &ech);
    let status = if !redundant.is_empty() || !conflicts.is_empty() {
        Status::OverConstrained
    } else if dof == 0 {
        Status::FullyConstrained
    } else {
        Status::UnderConstrained
    };
    Analysis {
        converged,
        dof,
        status,
        points,
        curves,
        redundant,
        conflicts,
    }
}

/// The constraints and entities behind a circuit of rows.
fn set_of(
    sys: &System,
    sk: &Sketch,
    owner: &[usize],
    rows: &[usize],
    found: usize,
) -> ConstraintSet {
    let (mut cons, mut pts, mut curves) = (Vec::new(), Vec::new(), Vec::new());
    for &row in rows {
        match sys.blocks.get(owner[row]).map(|b| b.eq) {
            Some(Eq::User(ci)) => {
                if !cons.contains(&ci.index()) {
                    cons.push(ci.index());
                }
            }
            Some(Eq::Arc(c)) => curves.push(c.index()),
            _ => {}
        }
    }
    cons.sort_unstable();
    for &ci in &cons {
        if let Some(c) = sk.constraints.get(ci) {
            let (ps, cs) = c.kind.refs();
            pts.extend(ps.iter().map(|p| p.index()));
            curves.extend(cs.iter().map(|c| c.index()));
        }
    }
    pts.sort_unstable();
    pts.dedup();
    curves.sort_unstable();
    curves.dedup();
    let key_c = |i: usize| sk.constraints.get(i).map(|c| c.key);
    ConstraintSet {
        constraint: key_c(found).unwrap_or(Key(0)),
        constraints: cons.into_iter().filter_map(key_c).collect(),
        points: pts
            .into_iter()
            .filter_map(|i| sk.points.get(i).map(|p| p.key))
            .collect(),
        curves: curves
            .into_iter()
            .filter_map(|i| sk.curves.get(i).map(|c| c.key))
            .collect(),
    }
}

/// Unknowns of each point and curve.
fn params(sys: &System, sk: &Sketch) -> (Vec<Vec<usize>>, Vec<Vec<usize>>) {
    let pt = |p: usize| [2 * p, 2 * p + 1];
    let points = (0..sk.points.len()).map(|p| pt(p).to_vec()).collect();
    let curves = sk
        .curves
        .iter()
        .enumerate()
        .map(|(i, c)| match c.kind {
            CurveKind::Line { a, b } => [pt(a.index()), pt(b.index())].concat(),
            CurveKind::Circle { centre, .. } => {
                let mut v = pt(centre.index()).to_vec();
                v.extend(sys.radius.get(i).copied().flatten());
                v
            }
            CurveKind::Arc { centre, a, b } => {
                [pt(centre.index()), pt(a.index()), pt(b.index())].concat()
            }
        })
        .collect();
    (points, curves)
}

/// Per-entity freedom: the rank of the null space projected onto the entity's
/// unknowns. Only non-zero projections are kept, so a sketch of many free
/// points stays cheap.
fn freedom(sys: &System, sk: &Sketch, ech: &Echelon) -> (Vec<Freedom>, Vec<Freedom>) {
    let (pp, cp) = params(sys, sk);
    let null = ech.null_space();
    let rank_of = |vars: &Vec<usize>| {
        let proj: Vec<Vec<f64>> = null
            .iter()
            .map(|v| {
                vars.iter()
                    .map(|&i| v.binary_search_by_key(&i, |e| e.0).map_or(0.0, |k| v[k].1))
                    .collect::<Vec<f64>>()
            })
            .filter(|p| p.iter().any(|x| *x != 0.0))
            .collect();
        Freedom {
            dof: small_rank(proj).min(u8::MAX as usize) as u8,
        }
    };
    (
        pp.iter().map(rank_of).collect(),
        cp.iter().map(rank_of).collect(),
    )
}
