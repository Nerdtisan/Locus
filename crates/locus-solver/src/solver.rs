// SPDX-License-Identifier: AGPL-3.0-or-later

//! The [`Solver`] trait (SPEC §3: backends are swappable) and Locus's own
//! backend, [`LmSolver`] (ADR 0007).

use crate::ldl::{Ldl, LinSolve};
use crate::lm;
use crate::sketch::{Key, PointId, Sketch, SketchError};
use crate::system::{self, Eq, System};
use locus_geom::{SOLVER_TOL, Vec2};

/// What a solve did. On failure the sketch is left as it was.
#[derive(Clone, Debug, PartialEq)]
pub struct SolveReport {
    /// Every driving constraint holds (the geometry was updated).
    pub converged: bool,
    pub iterations: usize,
    /// Largest constraint residual reached, in mm.
    pub max_residual: f64,
    /// Constraints still violated by more than `SOLVER_TOL` at the closest
    /// geometry the solver found (empty on success).
    pub unsatisfied: Vec<Key>,
    /// Current value of every driven dimension (§5), in sketch order.
    pub driven: Vec<(Key, f64)>,
}

/// A constraint solver over a [`Sketch`].
pub trait Solver {
    /// Solve from the sketch's current geometry and write the result back.
    fn solve(&mut self, sk: &mut Sketch) -> Result<SolveReport, SketchError>;
    /// One drag frame: move point `p` to `to` and re-solve from the current
    /// geometry, keeping everything else as still as the constraints allow.
    fn drag(&mut self, sk: &mut Sketch, p: PointId, to: Vec2) -> Result<SolveReport, SketchError>;
}

/// A system and its symbolic factorisation, reused while the pattern holds.
#[derive(Debug)]
struct Cached {
    sys: System,
    lin: Ldl,
}

/// Sparse Levenberg–Marquardt with Locus's own LDLᵀ (ADR 0007).
///
/// It keeps the ordering and symbolic analysis of the last solve and of the
/// last drag: as long as the constraint set is unchanged, later solves and
/// drag frames only refactor numerically.
#[derive(Debug, Default)]
pub struct LmSolver {
    base: Option<Cached>,
    drag: Option<Cached>,
}

impl LmSolver {
    pub fn new() -> Self {
        Self::default()
    }

    /// Solve `sys` with the factorisation cached in `slot`, re-analysed only
    /// if the pattern changed.
    fn run(slot: &mut Option<Cached>, sys: System, sk: &mut Sketch, x: &mut [f64]) -> SolveReport {
        let mut c = match slot.take() {
            Some(mut c) if c.sys.same_pattern(&sys) => {
                // Same pattern; take the new values (hold target, etc.).
                c.sys = sys;
                c
            }
            _ => Cached {
                lin: Ldl::analyse(&sys),
                sys,
            },
        };
        let report = Self::finish(&mut c, sk, x);
        *slot = Some(c);
        report
    }

    fn finish(c: &mut Cached, sk: &mut Sketch, x: &mut [f64]) -> SolveReport {
        let out = lm::solve(&c.sys, sk, &mut c.lin, x);
        let max_residual = out.r.iter().fold(0.0f64, |m, v| m.max(v.abs()));
        let mut unsatisfied = Vec::new();
        if out.converged {
            c.sys.write(x, sk);
        } else {
            for b in &c.sys.blocks {
                if let Eq::User(i) = b.eq {
                    let worst = out
                        .r
                        .get(b.r0..b.r0 + b.rows)
                        .map_or(0.0, |r| r.iter().fold(0.0f64, |m, v| m.max(v.abs())));
                    if worst > SOLVER_TOL {
                        unsatisfied.extend(sk.constraints.get(i.index()).map(|k| k.key));
                    }
                }
            }
        }
        SolveReport {
            converged: out.converged,
            iterations: out.iterations,
            max_residual,
            unsatisfied,
            driven: driven(sk),
        }
    }
}

/// Measured values of the driven dimensions.
fn driven(sk: &Sketch) -> Vec<(Key, f64)> {
    sk.constraints
        .iter()
        .filter(|c| c.driven)
        .filter_map(|c| system::measure(sk, c.kind).map(|v| (c.key, v)))
        .collect()
}

impl Solver for LmSolver {
    fn solve(&mut self, sk: &mut Sketch) -> Result<SolveReport, SketchError> {
        let sys = System::new(sk, None)?;
        let mut x = sys.x0(sk);
        Ok(Self::run(&mut self.base, sys, sk, &mut x))
    }

    fn drag(&mut self, sk: &mut Sketch, p: PointId, to: Vec2) -> Result<SolveReport, SketchError> {
        if p.index() >= sk.points.len() || !to.is_finite() {
            return Err(SketchError::BadDrag);
        }
        let sys = System::new(sk, Some(Eq::Hold { p, at: to, w: 1.0 }))?;
        let mut x = sys.x0(sk);
        // Start the frame with the point already at the target.
        x[2 * p.index()] = to.x;
        x[2 * p.index() + 1] = to.y;
        Ok(Self::run(&mut self.drag, sys, sk, &mut x))
    }
}
