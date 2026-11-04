// SPDX-License-Identifier: AGPL-3.0-or-later

//! A Locus-owned sparse Levenberg–Marquardt solver (ADR 0007 candidates).
//!
//! ADR 0002 picked ezpz, whose `faer → gemm/pulp` tree pulls in `paste`
//! (RUSTSEC-2024-0436, unmaintained). These candidates share one solver: exact
//! gradients from dual numbers ([`dual`]), one residual definition for solving
//! and for the rank analysis ([`model`], [`rank`]), LM with `λI` damping
//! ([`lm`]). Only the linear solve differs, behind [`ldl::LinSolve`]:
//! [`ldl::Ldl`] (dependency-free) and, with the `nalgebra` feature,
//! [`nalg::NalgebraCholesky`] (`nalgebra-sparse`).

pub mod dual;
pub mod ldl;
pub mod lm;
pub mod model;
#[cfg(feature = "nalgebra")]
pub mod nalg;
pub mod rank;

use ldl::LinSolve;
use model::Model;
use solver_harness::{Backend, Diagnosis, PointId, Sketch, Vec2};
use std::marker::PhantomData;

/// Residuals above this after a failed solve name a constraint as conflicting.
/// At a least-squares minimum `Jᵀr = 0` with `r ≠ 0`, so `r` itself is a
/// dependency among rows: its support is the conflicting set.
const CONFLICT_TOL: f64 = 1e-6;

pub struct Own<L>(&'static str, PhantomData<L>);

impl Own<ldl::Ldl> {
    pub const LDL: Self = Own("own LM + own LDLᵀ", PhantomData);
}

#[cfg(feature = "nalgebra")]
impl Own<nalg::NalgebraCholesky> {
    pub const NALGEBRA: Self = Own("own LM + nalgebra-sparse", PhantomData);
}

pub struct State<L> {
    model: Model,
    lin: L,
    x: Vec<f64>,
    /// The drag model (base plus a hold on one point) and its factorisation,
    /// reused across the frames of one drag.
    drag: Option<(PointId, Model, L)>,
    last: lm::Outcome,
}

impl<L: LinSolve> Backend for Own<L> {
    type State = State<L>;

    fn name(&self) -> &'static str {
        self.0
    }

    fn load(&self, sk: &Sketch) -> Result<State<L>, String> {
        let model = Model::new(sk, None)?;
        Ok(State {
            lin: L::analyse(&model),
            x: model.x0(),
            model,
            drag: None,
            last: lm::Outcome::default(),
        })
    }

    fn solve(&self, st: &mut State<L>) -> bool {
        st.last = lm::solve(&st.model, &mut st.lin, &mut st.x);
        st.last.converged
    }

    fn drag(&self, st: &mut State<L>, p: PointId, to: Vec2) -> bool {
        let fresh = !matches!(&st.drag, Some((q, ..)) if *q == p);
        if fresh {
            let Ok(m) = Model::new(&st.model.sk, Some((p, Vec2::default()))) else {
                return false;
            };
            let lin = L::analyse(&m);
            st.drag = Some((p, m, lin));
        }
        let Some((_, m, lin)) = st.drag.as_mut() else {
            return false;
        };
        // Retarget the hold block (always last); the pattern is unchanged.
        if let Some(b) = m.blocks.last_mut() {
            b.eq = model::Eq::Hold(p, to);
        }
        // Start the frame with the point already at the target.
        for (i, v) in [(2 * p, to.x), (2 * p + 1, to.y)] {
            if let Some(slot) = st.x.get_mut(i) {
                *slot = v;
            }
        }
        let out = lm::solve(m, lin, &mut st.x);
        let ok = out.converged;
        st.last = out;
        ok
    }

    fn diagnose(&self, st: &mut State<L>) -> Diagnosis {
        if !st.last.converged {
            let mut conflicting = Vec::new();
            for b in &st.model.blocks {
                if let model::Eq::User(i) = b.eq {
                    let worst = st
                        .last
                        .r
                        .get(b.r0..b.r0 + b.rows)
                        .map_or(0.0, |r| r.iter().fold(0.0f64, |m, v| m.max(v.abs())));
                    if worst > CONFLICT_TOL {
                        conflicting.push(i);
                    }
                }
            }
            return Diagnosis {
                dof: None,
                conflicting: Some(conflicting),
                redundant: None,
                note: None,
            };
        }
        let a = rank::analyse(&st.model, &st.x);
        Diagnosis {
            dof: Some(a.dof),
            conflicting: Some(Vec::new()),
            redundant: Some(a.dependent),
            note: None,
        }
    }

    fn read(&self, st: &State<L>, into: &mut Sketch) {
        st.model.write(&st.x, into);
    }
}
