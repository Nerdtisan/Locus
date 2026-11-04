// SPDX-License-Identifier: AGPL-3.0-or-later

//! The LM normal equations on `nalgebra-sparse`'s left-looking Cholesky (ADR
//! 0007 candidate). Same minimum-degree ordering and assembly as [`super::ldl`],
//! so only the factorisation differs. nalgebra-sparse has no fill-reducing
//! ordering of its own; it takes the full symmetric pattern.

use super::ldl::{graph, min_degree, LinSolve};
use super::model::Model;
use nalgebra::DMatrixViewMut;
use nalgebra_sparse::factorization::{CscCholesky, CscSymbolicCholesky};
use nalgebra_sparse::pattern::SparsityPattern;

pub struct NalgebraCholesky {
    perm: Vec<usize>,
    symbolic: CscSymbolicCholesky,
    chol: Option<CscCholesky<f64>>,
    values: Vec<f64>,
    diag: Vec<usize>,
    /// Per block pair: one slot (diagonal) or two (both triangles).
    pairs: Vec<(usize, usize)>,
    y: Vec<f64>,
}

impl LinSolve for NalgebraCholesky {
    fn analyse(m: &Model) -> Self {
        let n = m.n;
        let adj = graph(m);
        let perm = min_degree(adj.clone());
        let mut iperm = vec![0; n];
        for (k, &v) in perm.iter().enumerate() {
            iperm[v] = k;
        }
        let mut offsets = vec![0; n + 1];
        let mut idx = Vec::new();
        let mut diag = vec![0; n];
        for k in 0..n {
            let mut col: Vec<usize> = adj[perm[k]].iter().map(|&u| iperm[u]).collect();
            col.push(k);
            col.sort_unstable();
            diag[k] = idx.len() + col.binary_search(&k).unwrap_or(0);
            idx.extend(col);
            offsets[k + 1] = idx.len();
        }
        let slot = |r: usize, c: usize| {
            offsets[c]
                + idx[offsets[c]..offsets[c + 1]]
                    .binary_search(&r)
                    .unwrap_or(0)
        };
        let mut pairs = Vec::new();
        for b in &m.blocks {
            for a in 0..b.vars.len() {
                for c in a..b.vars.len() {
                    let (pa, pc) = (iperm[b.vars[a]], iperm[b.vars[c]]);
                    pairs.push((slot(pa, pc), slot(pc, pa)));
                }
            }
        }
        let nnz = idx.len();
        // The pattern is square and sorted by construction; an error here would
        // be a bug in the lines above, so fall back to an empty system.
        let pattern = SparsityPattern::try_from_offsets_and_indices(n, n, offsets, idx)
            .unwrap_or_else(|_| SparsityPattern::zeros(n, n));
        NalgebraCholesky {
            perm,
            symbolic: CscSymbolicCholesky::factor(pattern),
            chol: None,
            values: vec![0.0; nnz],
            diag,
            pairs,
            y: vec![0.0; n],
        }
    }

    fn factor(&mut self, m: &Model, j: &[f64], lambda: f64) -> bool {
        self.values.iter_mut().for_each(|v| *v = 0.0);
        let mut it = self.pairs.iter();
        for b in &m.blocks {
            let nv = b.vars.len();
            let g = &j[b.j0..b.j0 + b.rows * nv];
            for a in 0..nv {
                for c in a..nv {
                    let s: f64 = (0..b.rows).map(|r| g[r * nv + a] * g[r * nv + c]).sum();
                    if let Some(&(p, q)) = it.next() {
                        self.values[p] += s;
                        if q != p {
                            self.values[q] += s;
                        }
                    }
                }
            }
        }
        for &p in &self.diag {
            self.values[p] += lambda;
        }
        match self.chol.as_mut() {
            Some(c) => c.refactor(&self.values).is_ok(),
            None => match CscCholesky::factor_numerical(self.symbolic.clone(), &self.values) {
                Ok(c) => {
                    self.chol = Some(c);
                    true
                }
                Err(_) => false,
            },
        }
    }

    fn solve(&mut self, b: &mut [f64]) {
        for (k, &v) in self.perm.iter().enumerate() {
            self.y[k] = b[v];
        }
        if let Some(c) = &self.chol {
            let n = self.perm.len();
            c.solve_mut(DMatrixViewMut::from_slice(&mut self.y, n, 1));
        }
        for (k, &v) in self.perm.iter().enumerate() {
            b[v] = self.y[k];
        }
    }
}
