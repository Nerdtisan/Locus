// SPDX-License-Identifier: AGPL-3.0-or-later

//! The [`Solver`] trait (SPEC §3: backends are swappable) and Locus's own
//! backend, [`LmSolver`] (ADR 0007).

use crate::analysis::{self, Analysis, ConstraintSet};
use crate::ldl::{Ldl, LinSolve};
use crate::lm;
use crate::sketch::{Constraint, ConstraintId, Key, PointId, Sketch, SketchError};
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
    /// The dragged point reached its target (always true for a plain solve).
    /// False when the target is out of reach: the point then goes as near to
    /// it as the constraints allow (#63).
    pub at_target: bool,
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
    /// DOF, status, per-entity freedom, and redundant and conflicting sets
    /// (#60, #61). Solves a copy first, so it works on unsolved and
    /// conflicting sketches too; `sk` is not changed.
    fn analyse(&mut self, sk: &Sketch) -> Result<Analysis, SketchError>;
    /// Would adding `c` (as a driving constraint) conflict, or be redundant?
    /// Lets the UI refuse a conflicting constraint before it is applied (§5).
    fn check_add(&mut self, sk: &Sketch, c: Constraint) -> Result<Check, SketchError>;
    /// The same check for turning driven dimension `id` into a driving one
    /// (#62): refused with a conflict report if it would over-constrain.
    fn check_driving(&mut self, sk: &Sketch, id: ConstraintId) -> Result<Check, SketchError>;
}

/// Outcome of a pre-check.
#[derive(Clone, Debug, PartialEq)]
pub enum Check {
    /// Adds a real restriction and can hold.
    Ok,
    /// Implied by the constraints in the set (consistent, but removes no
    /// freedom). The UI may still offer it as a driven dimension.
    Redundant(ConstraintSet),
    /// Cannot hold together with the constraints in the set.
    Conflict(ConstraintSet),
}

/// Weight of a soft drag target against the constraints (all rows are in
/// mm): small enough that the constraints win, large enough to pull.
const SOFT_HOLD: f64 = 1e-2;

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
    /// Pre-checks: the sketch plus one constraint, kept apart so they don't
    /// evict the main solve's factorisation.
    check: Option<Cached>,
}

impl LmSolver {
    pub fn new() -> Self {
        Self::default()
    }

    /// Solve `sys` with the factorisation cached in `slot`, re-analysed only
    /// if the pattern changed.
    fn run(
        slot: &mut Option<Cached>,
        sys: System,
        sk: &mut Sketch,
        x: &mut [f64],
        max_iter: usize,
    ) -> SolveReport {
        let c = Self::cached(slot, sys);
        Self::finish(c, sk, x, max_iter)
    }

    /// The cache entry for `sys`, re-analysed only if the pattern changed.
    fn cached(slot: &mut Option<Cached>, sys: System) -> &mut Cached {
        let c = match slot.take() {
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
        slot.insert(c)
    }

    /// Solve a copy and analyse at the result (or the least-squares point).
    fn analyse_with(slot: &mut Option<Cached>, sk: &Sketch) -> Result<Analysis, SketchError> {
        let sys = System::new(sk, None)?;
        let mut x = sys.x0(sk);
        let c = Self::cached(slot, sys);
        let out = lm::solve(&c.sys, sk, &mut c.lin, &mut x, lm::MAX_ITER);
        Ok(analysis::analyse(&c.sys, sk, &x, out.converged))
    }

    /// Analyse `sk` with `c` appended last (so a dependency names it), and
    /// classify `c`.
    fn check_with(&mut self, sk: &Sketch, mut c: Constraint) -> Result<Check, SketchError> {
        c.driven = false;
        let mut sk = sk.clone();
        sk.constraints.push(c);
        let a = Self::analyse_with(&mut self.check, &sk)?;
        let find = |sets: Vec<ConstraintSet>| sets.into_iter().find(|s| s.constraint == c.key);
        Ok(match (find(a.conflicts), find(a.redundant)) {
            (Some(s), _) => Check::Conflict(s),
            (None, Some(s)) => Check::Redundant(s),
            (None, None) => Check::Ok,
        })
    }

    fn finish(c: &mut Cached, sk: &mut Sketch, x: &mut [f64], max_iter: usize) -> SolveReport {
        let out = lm::solve(&c.sys, sk, &mut c.lin, x, max_iter);
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
            at_target: true,
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
        Ok(Self::run(&mut self.base, sys, sk, &mut x, lm::MAX_ITER))
    }

    fn drag(&mut self, sk: &mut Sketch, p: PointId, to: Vec2) -> Result<SolveReport, SketchError> {
        if p.index() >= sk.points.len() || !to.is_finite() {
            return Err(SketchError::BadDrag);
        }
        // Warm start: from the previous frame's geometry (the sketch), with the
        // point already at the target, held there by two hard rows.
        let sys = System::new(sk, Some(Eq::Hold { p, at: to, w: 1.0 }))?;
        let mut x = sys.x0(sk);
        x[2 * p.index()] = to.x;
        x[2 * p.index() + 1] = to.y;
        let hard = Self::run(&mut self.drag, sys, sk, &mut x, lm::DRAG_ITER);
        if hard.converged {
            return Ok(hard);
        }
        // Out of reach: the target becomes soft (weighted rows, same pattern,
        // so the drag factorisation is reused) and pulls the point as near as
        // the constraints allow; then a plain solve from there restores every
        // constraint exactly.
        let sys = System::new(sk, Some(Eq::Hold { p, at: to, w: SOFT_HOLD }))?;
        let mut x = sys.x0(sk);
        let c = Self::cached(&mut self.drag, sys);
        let soft = lm::solve(&c.sys, sk, &mut c.lin, &mut x, lm::DRAG_ITER);
        let sys = System::new(sk, None)?;
        let mut report = Self::run(&mut self.base, sys, sk, &mut x, lm::DRAG_ITER);
        report.iterations += hard.iterations + soft.iterations;
        report.at_target = false;
        Ok(report)
    }

    fn analyse(&mut self, sk: &Sketch) -> Result<Analysis, SketchError> {
        Self::analyse_with(&mut self.base, sk)
    }

    fn check_add(&mut self, sk: &Sketch, c: Constraint) -> Result<Check, SketchError> {
        self.check_with(sk, c)
    }

    fn check_driving(&mut self, sk: &Sketch, id: ConstraintId) -> Result<Check, SketchError> {
        let Some(c) = sk.constraints.get(id.index()).copied() else {
            return Err(SketchError::MissingConstraint);
        };
        let mut rest = sk.clone();
        rest.constraints.remove(id.index());
        self.check_with(&rest, c)
    }
}
