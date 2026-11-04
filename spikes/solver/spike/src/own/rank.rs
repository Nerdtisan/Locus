// SPDX-License-Identifier: AGPL-3.0-or-later

//! DOF and redundancy by Jacobian rank, on the solver's own exact gradients
//! (the #42 prototype in `analysis.rs` used central differences). Rows go in
//! block order (arcs' implicit rows first, so a user constraint is the one
//! named) into a sparse row-echelon form with partial pivoting; a row that
//! reduces to zero depends on earlier ones. DOF = unknowns − rank.

use super::model::{Eq, Model};

const RANK_TOL: f64 = 1e-8;
const MIN_ROW_NORM: f64 = 1e-6;

#[derive(Clone, Debug, PartialEq)]
pub struct Freedom {
    pub dof: usize,
    /// Harness constraints with at least one dependent row, in order.
    pub dependent: Vec<usize>,
}

fn axpy(a: &[(usize, f64)], k: f64, b: &[(usize, f64)], out: &mut Vec<(usize, f64)>) {
    out.clear();
    let (mut i, mut j) = (0, 0);
    while i < a.len() || j < b.len() {
        let (ca, cb) = (
            a.get(i).map_or(usize::MAX, |x| x.0),
            b.get(j).map_or(usize::MAX, |x| x.0),
        );
        let (c, v) = if ca < cb {
            i += 1;
            (ca, a[i - 1].1)
        } else if cb < ca {
            j += 1;
            (cb, -k * b[j - 1].1)
        } else {
            i += 1;
            j += 1;
            (ca, a[i - 1].1 - k * b[j - 1].1)
        };
        if v.abs() > RANK_TOL * 1e-3 {
            out.push((c, v));
        }
    }
}

struct Echelon {
    pivot: Vec<Option<Vec<(usize, f64)>>>,
    tmp: Vec<(usize, f64)>,
}

impl Echelon {
    fn insert(&mut self, mut row: Vec<(usize, f64)>) -> bool {
        loop {
            let Some(lead) = row.iter().position(|(_, v)| v.abs() > RANK_TOL) else {
                return false;
            };
            row.drain(..lead);
            let (c, v) = row[0];
            let Some(slot) = self.pivot.get_mut(c) else {
                return false;
            };
            match slot.take() {
                None => {
                    *slot = Some(row);
                    return true;
                }
                Some(p) => {
                    let (keep, other) = if v.abs() > p[0].1.abs() {
                        (row, p)
                    } else {
                        (p, row)
                    };
                    let k = other[0].1 / keep[0].1;
                    axpy(&other, k, &keep, &mut self.tmp);
                    *slot = Some(keep);
                    row = std::mem::take(&mut self.tmp);
                }
            }
        }
    }
}

/// DOF and dependent constraints at `x` (hold rows are ignored).
pub fn analyse(m: &Model, x: &[f64]) -> Freedom {
    let mut r = vec![0.0; m.m];
    let mut j = vec![0.0; m.nj];
    m.eval(x, &mut r, Some(&mut j));
    let mut ech = Echelon {
        pivot: vec![None; m.n],
        tmp: Vec::new(),
    };
    let mut rank = 0;
    let mut dependent = Vec::new();
    for b in &m.blocks {
        if matches!(b.eq, Eq::Hold(..)) {
            continue;
        }
        let nv = b.vars.len();
        let mut dep = false;
        for k in 0..b.rows {
            let g = &j[b.j0 + k * nv..b.j0 + (k + 1) * nv];
            let norm = g.iter().map(|v| v * v).sum::<f64>().sqrt();
            // A row that is numerically zero says nothing at first order: it
            // counts as dependent rather than being blown up to unit length.
            let row: Vec<(usize, f64)> = if norm < MIN_ROW_NORM {
                Vec::new()
            } else {
                b.vars
                    .iter()
                    .zip(g)
                    .filter(|(_, v)| **v != 0.0)
                    .map(|(u, v)| (*u, v / norm))
                    .collect()
            };
            if ech.insert(row) {
                rank += 1;
            } else {
                dep = true;
            }
        }
        if let (true, Eq::User(i)) = (dep, b.eq) {
            dependent.push(i);
        }
    }
    Freedom {
        dof: m.n - rank,
        dependent,
    }
}
