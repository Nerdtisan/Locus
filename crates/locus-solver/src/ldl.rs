// SPDX-License-Identifier: AGPL-3.0-or-later

//! Sparse symmetric solves of the Levenberg–Marquardt normal equations
//! `(JᵀJ + λI) δ = -Jᵀr`, behind [`LinSolve`] so the linear-algebra backend can
//! be swapped and measured.
//!
//! [`Ldl`] is dependency-free: a minimum-degree ordering on the explicit
//! elimination graph, then an up-looking LDLᵀ after T. A. Davis's LDL package
//! (elimination tree, row patterns by tree reach). The sparsity pattern depends
//! only on which unknowns each block touches, so ordering and symbolic analysis
//! run once per constraint set; every LM iteration and drag frame only refactors
//! numerically.

use crate::system::System;
use std::cmp::Reverse;
use std::collections::BinaryHeap;

pub(crate) trait LinSolve: Sized {
    /// Symbolic analysis for the model's sparsity pattern.
    fn analyse(m: &System) -> Self;
    /// Assemble `JᵀJ + λI` from Jacobian values and factor. False if a pivot is
    /// not positive (the caller raises λ).
    fn factor(&mut self, m: &System, j: &[f64], lambda: f64) -> bool;
    /// Solve in place with the last factorisation.
    fn solve(&mut self, b: &mut [f64]);
}

/// Adjacency of `JᵀJ`: unknowns sharing a block.
pub(crate) fn graph(m: &System) -> Vec<Vec<usize>> {
    let mut adj = vec![Vec::new(); m.n];
    for b in &m.blocks {
        for &u in &b.vars {
            if let Some(a) = adj.get_mut(u) {
                a.extend(b.vars.iter().copied().filter(|&v| v != u));
            }
        }
    }
    for a in &mut adj {
        a.sort_unstable();
        a.dedup();
    }
    adj
}

/// Minimum-degree elimination order (new → old). Ties go to the lower index so
/// the order is deterministic.
pub(crate) fn min_degree(mut adj: Vec<Vec<usize>>) -> Vec<usize> {
    let n = adj.len();
    let mut done = vec![false; n];
    let mut heap: BinaryHeap<Reverse<(usize, usize)>> =
        (0..n).map(|v| Reverse((adj[v].len(), v))).collect();
    let mut order = Vec::with_capacity(n);
    let mut merged = Vec::new();
    while let Some(Reverse((d, v))) = heap.pop() {
        if done[v] || adj[v].len() != d {
            continue;
        }
        done[v] = true;
        order.push(v);
        let nb = std::mem::take(&mut adj[v]);
        // Eliminating v makes its neighbours a clique.
        for &u in &nb {
            merged.clear();
            let au = &adj[u];
            let (mut i, mut k) = (0, 0);
            while i < au.len() || k < nb.len() {
                let (x, y) = (
                    au.get(i).copied().unwrap_or(usize::MAX),
                    nb.get(k).copied().unwrap_or(usize::MAX),
                );
                let w = x.min(y);
                i += usize::from(x == w);
                k += usize::from(y == w);
                if w != u && w != v {
                    merged.push(w);
                }
            }
            std::mem::swap(&mut adj[u], &mut merged);
            heap.push(Reverse((adj[u].len(), u)));
        }
    }
    order
}

#[derive(Debug)]
pub(crate) struct Ldl {
    n: usize,
    /// New → old and old → new.
    perm: Vec<usize>,
    iperm: Vec<usize>,
    /// Upper triangle of the permuted `JᵀJ`, compressed by column.
    ap: Vec<usize>,
    ai: Vec<usize>,
    ax: Vec<f64>,
    /// Position of the diagonal in each column of `ax`.
    diag: Vec<usize>,
    /// Per block, per unknown pair `(a, b)` with `a <= b`: the slot in `ax`.
    pairs: Vec<usize>,
    parent: Vec<usize>,
    lp: Vec<usize>,
    li: Vec<usize>,
    lx: Vec<f64>,
    d: Vec<f64>,
    // Workspace.
    lnz: Vec<usize>,
    flag: Vec<usize>,
    pattern: Vec<usize>,
    y: Vec<f64>,
}

const NONE: usize = usize::MAX;

impl Ldl {
    fn slot(&self, a: usize, b: usize) -> usize {
        let (r, c) = {
            let (pa, pb) = (self.iperm[a], self.iperm[b]);
            (pa.min(pb), pa.max(pb))
        };
        let col = &self.ai[self.ap[c]..self.ap[c + 1]];
        // The pattern holds every pair that shares a block, by construction.
        self.ap[c] + col.binary_search(&r).unwrap_or(0)
    }
}

impl LinSolve for Ldl {
    fn analyse(m: &System) -> Self {
        let n = m.n;
        let adj = graph(m);
        let perm = min_degree(adj.clone());
        let mut iperm = vec![0; n];
        for (k, &v) in perm.iter().enumerate() {
            iperm[v] = k;
        }
        // Upper triangle of the permuted matrix, diagonal included.
        let mut ap = vec![0; n + 1];
        let mut ai = Vec::new();
        let mut diag = vec![0; n];
        for k in 0..n {
            let mut col: Vec<usize> = adj[perm[k]]
                .iter()
                .map(|&u| iperm[u])
                .filter(|&i| i < k)
                .collect();
            col.push(k);
            col.sort_unstable();
            diag[k] = ai.len() + col.len() - 1;
            ai.extend(col);
            ap[k + 1] = ai.len();
        }
        // Elimination tree and column counts of L (ldl_symbolic).
        let mut parent = vec![NONE; n];
        let mut lnz = vec![0; n];
        let mut flag = vec![NONE; n];
        for k in 0..n {
            flag[k] = k;
            for &i0 in &ai[ap[k]..ap[k + 1]] {
                let mut i = i0;
                while i < k && flag[i] != k {
                    if parent[i] == NONE {
                        parent[i] = k;
                    }
                    lnz[i] += 1;
                    flag[i] = k;
                    i = parent[i];
                }
            }
        }
        let mut lp = vec![0; n + 1];
        for k in 0..n {
            lp[k + 1] = lp[k] + lnz[k];
        }
        let nl = lp[n];
        let mut s = Ldl {
            n,
            perm,
            iperm,
            ax: vec![0.0; ai.len()],
            ap,
            ai,
            diag,
            pairs: Vec::new(),
            parent,
            lp,
            li: vec![0; nl],
            lx: vec![0.0; nl],
            d: vec![0.0; n],
            lnz,
            flag,
            pattern: vec![0; n],
            y: vec![0.0; n],
        };
        let mut pairs = Vec::new();
        for b in &m.blocks {
            for a in 0..b.vars.len() {
                for c in a..b.vars.len() {
                    pairs.push(s.slot(b.vars[a], b.vars[c]));
                }
            }
        }
        s.pairs = pairs;
        s
    }

    fn factor(&mut self, m: &System, j: &[f64], lambda: f64) -> bool {
        self.ax.iter_mut().for_each(|v| *v = 0.0);
        let mut it = self.pairs.iter();
        for b in &m.blocks {
            let nv = b.vars.len();
            let g = &j[b.j0..b.j0 + b.rows * nv];
            for a in 0..nv {
                for c in a..nv {
                    let mut s = 0.0;
                    for r in 0..b.rows {
                        s += g[r * nv + a] * g[r * nv + c];
                    }
                    if let Some(&p) = it.next() {
                        // Off-diagonal pairs within one block: (a, c) and (c, a)
                        // both land on one stored upper entry, counted once.
                        self.ax[p] += s;
                    }
                }
            }
        }
        for &p in &self.diag {
            self.ax[p] += lambda;
        }
        // ldl_numeric.
        let n = self.n;
        for k in 0..n {
            self.y[k] = 0.0;
            let mut top = n;
            self.flag[k] = k;
            self.lnz[k] = 0;
            for p in self.ap[k]..self.ap[k + 1] {
                let mut i = self.ai[p];
                self.y[i] += self.ax[p];
                let mut len = 0;
                while self.flag[i] != k {
                    self.pattern[len] = i;
                    len += 1;
                    self.flag[i] = k;
                    i = self.parent[i];
                }
                while len > 0 {
                    top -= 1;
                    len -= 1;
                    self.pattern[top] = self.pattern[len];
                }
            }
            let mut dk = self.y[k];
            self.y[k] = 0.0;
            for t in top..n {
                let i = self.pattern[t];
                let yi = self.y[i];
                self.y[i] = 0.0;
                let p2 = self.lp[i] + self.lnz[i];
                for p in self.lp[i]..p2 {
                    self.y[self.li[p]] -= self.lx[p] * yi;
                }
                let l = yi / self.d[i];
                dk -= l * yi;
                self.li[p2] = k;
                self.lx[p2] = l;
                self.lnz[i] += 1;
            }
            // λ > 0 makes the matrix positive definite; a non-positive pivot is
            // round-off swamping a tiny λ.
            if dk.is_nan() || dk <= 0.0 {
                return false;
            }
            self.d[k] = dk;
        }
        true
    }

    fn solve(&mut self, b: &mut [f64]) {
        let n = self.n;
        for k in 0..n {
            self.y[k] = b[self.perm[k]];
        }
        for j in 0..n {
            let yj = self.y[j];
            for p in self.lp[j]..self.lp[j + 1] {
                self.y[self.li[p]] -= self.lx[p] * yj;
            }
        }
        for j in 0..n {
            self.y[j] /= self.d[j];
        }
        for j in (0..n).rev() {
            let mut yj = self.y[j];
            for p in self.lp[j]..self.lp[j + 1] {
                yj -= self.lx[p] * self.y[self.li[p]];
            }
            self.y[j] = yj;
        }
        for k in 0..n {
            b[self.perm[k]] = self.y[k];
            self.y[k] = 0.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sketch::{ConstraintKind, Sketch};

    /// A random-ish chain of points with distance and angle constraints: a
    /// sparse, rank-deficient `JᵀJ`, as the solver sees it.
    fn sketch(n: usize) -> Sketch {
        let mut sk = Sketch::default();
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut rnd = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % 1000) as f64 / 100.0
        };
        let pts: Vec<_> = (0..n).map(|_| sk.point(rnd(), rnd())).collect();
        for w in pts.windows(3) {
            sk.constrain(ConstraintKind::Distance {
                p: w[0],
                q: w[2],
                d: 1.0,
            });
            let l = sk.line(w[0], w[1]);
            let m = sk.line(w[1], w[2]);
            sk.constrain(ConstraintKind::Angle { l, m, rad: 0.3 });
        }
        sk
    }

    /// Dense `(JᵀJ + λI) x = b` by Gaussian elimination, as the reference.
    fn dense(sys: &System, j: &[f64], lambda: f64, b: &[f64]) -> Vec<f64> {
        let n = sys.n;
        let mut a = vec![vec![0.0; n + 1]; n];
        for blk in &sys.blocks {
            let nv = blk.vars.len();
            for r in 0..blk.rows {
                for (p, &u) in blk.vars.iter().enumerate() {
                    for (q, &v) in blk.vars.iter().enumerate() {
                        a[u][v] += j[blk.j0 + r * nv + p] * j[blk.j0 + r * nv + q];
                    }
                }
            }
        }
        for (i, row) in a.iter_mut().enumerate() {
            row[i] += lambda;
            row[n] = b[i];
        }
        for c in 0..n {
            let piv = (c..n)
                .max_by(|&x, &y| a[x][c].abs().total_cmp(&a[y][c].abs()))
                .unwrap();
            a.swap(c, piv);
            let pivot = a[c].clone();
            for row in a.iter_mut().skip(c + 1) {
                let k = row[c] / pivot[c];
                for (v, p) in row.iter_mut().zip(&pivot).skip(c) {
                    *v -= k * p;
                }
            }
        }
        let mut x = vec![0.0; n];
        for c in (0..n).rev() {
            let s: f64 = (c + 1..n).map(|k| a[c][k] * x[k]).sum();
            x[c] = (a[c][n] - s) / a[c][c];
        }
        x
    }

    #[test]
    fn ldl_matches_a_dense_solve() {
        let sk = sketch(40);
        let sys = System::new(&sk, None).unwrap();
        let x0 = sys.x0(&sk);
        let (mut r, mut j) = (vec![0.0; sys.m], vec![0.0; sys.nj]);
        sys.eval(&sk, &x0, &mut r, Some(&mut j));
        let b: Vec<f64> = (0..sys.n).map(|i| (i as f64 * 0.37).sin()).collect();
        let mut ldl = Ldl::analyse(&sys);
        for lambda in [1.0, 1e-3, 1e-7] {
            assert!(ldl.factor(&sys, &j, lambda));
            let mut x = b.clone();
            ldl.solve(&mut x);
            let want = dense(&sys, &j, lambda, &b);
            let scale = want.iter().fold(1.0f64, |m, v| m.max(v.abs()));
            for (got, want) in x.iter().zip(&want) {
                assert!(
                    (got - want).abs() < 1e-8 * scale,
                    "λ {lambda}: {got} vs {want}"
                );
            }
        }
    }

    #[test]
    fn ordering_is_a_permutation() {
        let sk = sketch(30);
        let sys = System::new(&sk, None).unwrap();
        let mut order = min_degree(graph(&sys));
        order.sort_unstable();
        assert_eq!(order, (0..sys.n).collect::<Vec<_>>());
    }
}
