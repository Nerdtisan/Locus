// SPDX-License-Identifier: AGPL-3.0-or-later

//! Jacobian rank analysis (ADR 0002, ADR 0007): DOF, dependent rows with the
//! exact set of rows each depends on, and the free directions of the sketch.
//!
//! Rows (scaled to unit length) go one at a time into a sparse row-echelon
//! form with partial pivoting. Each stored row remembers which original rows it
//! is a combination of. A row that reduces to zero depends on earlier rows, and
//! its combination is the *fundamental circuit*: the unique minimal set of rows
//! (itself included) that are linearly dependent, given the rows before it. DOF
//! is unknowns − rank; the null space, read off the echelon form by back
//! substitution, says which unknowns can still move.

const RANK_TOL: f64 = 1e-8;
/// Entries below this (relative to a unit row) are round-off.
const DROP: f64 = 1e-11;
/// A row whose raw gradient is this small says nothing at first order.
const MIN_ROW_NORM: f64 = 1e-6;
/// A circuit coefficient this small (relative to the largest) is round-off.
const CIRCUIT_TOL: f64 = 1e-6;
/// A null-space component this small (null vectors have a unit entry) is
/// round-off: the unknown does not move along that direction.
const MOVE_TOL: f64 = 1e-7;

type Sparse = Vec<(usize, f64)>;

/// `a − k·b` for sorted sparse vectors.
fn axpy(a: &[(usize, f64)], k: f64, b: &[(usize, f64)], out: &mut Sparse) {
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
        if v.abs() > DROP {
            out.push((c, v));
        }
    }
}

#[derive(Clone, Debug)]
struct Row {
    /// Sorted by column; the first entry is the pivot.
    vals: Sparse,
    /// Original rows this is a combination of, sorted by row id.
    combo: Sparse,
}

/// The row-echelon form under construction.
#[derive(Debug)]
pub(crate) struct Echelon {
    pivot: Vec<Option<Row>>,
    rank: usize,
    tmp: Sparse,
}

impl Echelon {
    pub fn new(n: usize) -> Self {
        Echelon {
            pivot: vec![None; n],
            rank: 0,
            tmp: Vec::new(),
        }
    }

    pub fn rank(&self) -> usize {
        self.rank
    }

    /// Insert original row `id` (gradient entries over unknowns). `None` if it
    /// is independent of the rows before it; otherwise the rows it depends on,
    /// itself included (the fundamental circuit).
    pub fn insert(&mut self, id: usize, grad: &[(usize, f64)]) -> Option<Vec<usize>> {
        let norm = grad.iter().map(|(_, v)| v * v).sum::<f64>().sqrt();
        let mut row = Row {
            vals: if norm < MIN_ROW_NORM {
                Vec::new()
            } else {
                grad.iter()
                    .filter(|(_, v)| *v != 0.0)
                    .map(|&(c, v)| (c, v / norm))
                    .collect()
            },
            combo: vec![(id, 1.0)],
        };
        loop {
            let Some(lead) = row.vals.iter().position(|(_, v)| v.abs() > RANK_TOL) else {
                return Some(circuit(&row.combo));
            };
            row.vals.drain(..lead);
            let (c, v) = row.vals[0];
            let Some(slot) = self.pivot.get_mut(c) else {
                return Some(circuit(&row.combo));
            };
            match slot.take() {
                None => {
                    *slot = Some(row);
                    self.rank += 1;
                    return None;
                }
                Some(p) => {
                    // Partial pivoting: keep the larger leading entry as the
                    // pivot and push the other row on down.
                    let (keep, other) = if v.abs() > p.vals[0].1.abs() {
                        (row, p)
                    } else {
                        (p, row)
                    };
                    let k = other.vals[0].1 / keep.vals[0].1;
                    axpy(&other.vals, k, &keep.vals, &mut self.tmp);
                    let vals = std::mem::take(&mut self.tmp);
                    axpy(&other.combo, k, &keep.combo, &mut self.tmp);
                    let combo = std::mem::take(&mut self.tmp);
                    *slot = Some(keep);
                    row = Row { vals, combo };
                }
            }
        }
    }

    /// A basis of the null space: one vector per free unknown (an unknown with
    /// no pivot), set to 1, with the pivot unknowns solved by back
    /// substitution. Sparse: (unknown, value) sorted by unknown.
    pub fn null_space(&self) -> Vec<Sparse> {
        let n = self.pivot.len();
        let mut out = Vec::new();
        for f in (0..n).filter(|&c| self.pivot[c].is_none()) {
            let mut x = vec![0.0; n];
            x[f] = 1.0;
            for c in (0..f).rev() {
                if let Some(r) = &self.pivot[c] {
                    let s: f64 = r.vals.iter().skip(1).map(|&(j, a)| a * x[j]).sum();
                    x[c] = -s / r.vals[0].1;
                }
            }
            out.push(
                x.iter()
                    .enumerate()
                    .filter(|(_, v)| v.abs() >= MOVE_TOL)
                    .map(|(i, v)| (i, *v))
                    .collect(),
            );
        }
        out
    }
}

/// Row ids with a significant coefficient.
fn circuit(combo: &[(usize, f64)]) -> Vec<usize> {
    let max = combo.iter().fold(0.0f64, |m, (_, v)| m.max(v.abs()));
    combo
        .iter()
        .filter(|(_, v)| v.abs() > CIRCUIT_TOL * max)
        .map(|(i, _)| *i)
        .collect()
}

/// Rank of a few short vectors (at most a handful of unknowns each), by
/// Gaussian elimination with tolerance `MOVE_TOL`.
pub(crate) fn small_rank(mut rows: Vec<Vec<f64>>) -> usize {
    let width = rows.first().map_or(0, Vec::len);
    let mut rank = 0;
    for c in 0..width {
        let Some(p) =
            (rank..rows.len()).max_by(|&a, &b| rows[a][c].abs().total_cmp(&rows[b][c].abs()))
        else {
            break;
        };
        if rows[p][c].abs() < MOVE_TOL {
            continue;
        }
        rows.swap(rank, p);
        let pivot = rows[rank].clone();
        for r in rows.iter_mut().skip(rank + 1) {
            let k = r[c] / pivot[c];
            for (v, q) in r.iter_mut().zip(&pivot) {
                *v -= k * q;
            }
        }
        rank += 1;
    }
    rank
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dependent_row_reports_its_minimal_circuit() {
        let mut e = Echelon::new(4);
        assert!(e.insert(0, &[(0, 1.0), (1, 1.0)]).is_none());
        assert!(e.insert(1, &[(2, 1.0)]).is_none());
        assert!(e.insert(2, &[(1, 1.0), (3, 1.0)]).is_none());
        // x0 + x1 − (x1 + x3) = x0 − x3: rows 0 and 2, not row 1.
        let c = e.insert(3, &[(0, 2.0), (3, -2.0)]).unwrap();
        assert_eq!(c, vec![0, 2, 3]);
        assert_eq!(e.rank(), 3);
    }

    #[test]
    fn null_space_moves_only_free_unknowns() {
        let mut e = Echelon::new(3);
        e.insert(0, &[(0, 1.0)]);
        e.insert(1, &[(1, 1.0), (2, -1.0)]);
        let ns = e.null_space();
        assert_eq!(ns.len(), 1);
        assert_eq!(ns[0].len(), 2, "{ns:?}");
        assert!((ns[0][0].1 - 1.0).abs() < 1e-12 && ns[0][1] == (2, 1.0));
        assert_eq!(small_rank(vec![vec![1.0, 2.0], vec![2.0, 4.0]]), 1);
    }
}
