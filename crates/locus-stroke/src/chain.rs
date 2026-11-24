// SPDX-License-Identifier: AGPL-3.0-or-later

//! Joining curves into the paths a stroke follows: where two curves meet end
//! to end the stroke joins (a corner), elsewhere it caps (SPEC §4, #428).

use crate::same;
use locus_geom::Vec2;
use locus_region::Seg;
use std::collections::HashMap;

/// Pieces joined end to end, in travel order.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Chain {
    pub segs: Vec<Seg>,
    pub closed: bool,
}

impl Chain {
    fn start(&self) -> Option<Vec2> {
        self.segs.first().map(Seg::start)
    }
    fn end(&self) -> Option<Vec2> {
        self.segs.last().map(Seg::end)
    }
    fn reversed(mut self) -> Self {
        self.segs.reverse();
        for s in &mut self.segs {
            *s = s.reversed();
        }
        self
    }
}

/// Grid key of a point, at a cell far above the snap distance.
fn key(p: Vec2) -> (i64, i64) {
    const CELL: f64 = 1e-3;
    // Saturating casts: callers pass finite coordinates.
    ((p.x / CELL).round() as i64, (p.y / CELL).round() as i64)
}

/// Joins `items` (each a curve's pieces in travel order) into chains. Two
/// curves join where an end of one meets an end of the other and no third
/// curve ends there; at a branch every curve ends with a cap. A closed curve
/// (a circle) is its own closed chain. The output order follows the input,
/// so it is deterministic.
#[must_use]
pub fn chains(items: Vec<Vec<Seg>>) -> Vec<Chain> {
    let items: Vec<Chain> = items
        .into_iter()
        .filter(|s| !s.is_empty())
        .map(|segs| {
            let closed = matches!((segs.first(), segs.last()), (Some(a), Some(b)) if same(a.start(), b.end()));
            Chain { segs, closed }
        })
        .collect();
    // How many open curve ends sit at each point.
    let mut degree: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (i, c) in items.iter().enumerate() {
        if c.closed {
            continue;
        }
        for p in [c.start(), c.end()].into_iter().flatten() {
            degree.entry(key(p)).or_default().push(i);
        }
    }
    let partner = |p: Vec2, me: usize, used: &[bool]| -> Option<usize> {
        let at: Vec<usize> = degree
            .get(&key(p))
            .map(|v| v.iter().copied().filter(|&j| j != me).collect())
            .unwrap_or_default();
        // Exactly one other curve ends here (a self-loop counts twice).
        match at.as_slice() {
            [j] if !used.get(*j).copied().unwrap_or(true) => Some(*j),
            _ => None,
        }
    };
    let mut used = vec![false; items.len()];
    let mut out = Vec::new();
    for i in 0..items.len() {
        if used[i] {
            continue;
        }
        used[i] = true;
        let Some(first) = items.get(i).cloned() else {
            continue;
        };
        if first.closed {
            out.push(first);
            continue;
        }
        let mut chain = first;
        // Grow forward, then backward.
        for _ in 0..2 {
            // The curve whose end the chain currently ends at.
            let mut cur = i;
            while let Some(end) = chain.end() {
                let Some(j) = partner(end, cur, &used) else {
                    break;
                };
                used[j] = true;
                cur = j;
                let Some(next) = items.get(j).cloned() else {
                    break;
                };
                let next = if next.start().is_some_and(|s| same(s, end)) {
                    next
                } else {
                    next.reversed()
                };
                chain.segs.extend(next.segs);
                if let (Some(a), Some(b)) = (chain.start(), chain.end())
                    && same(a, b)
                {
                    chain.closed = true;
                    break;
                }
            }
            if chain.closed {
                break;
            }
            chain = chain.reversed();
        }
        out.push(chain);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn l(a: (f64, f64), b: (f64, f64)) -> Vec<Seg> {
        vec![Seg::Line {
            a: Vec2::new(a.0, a.1),
            b: Vec2::new(b.0, b.1),
        }]
    }

    #[test]
    fn a_rectangle_of_four_lines_is_one_closed_chain() {
        let c = chains(vec![
            l((0.0, 0.0), (1.0, 0.0)),
            l((1.0, 1.0), (1.0, 0.0)),
            l((1.0, 1.0), (0.0, 1.0)),
            l((0.0, 0.0), (0.0, 1.0)),
        ]);
        assert_eq!(c.len(), 1);
        assert!(c[0].closed);
        assert_eq!(c[0].segs.len(), 4);
        for w in c[0].segs.windows(2) {
            assert!(same(w[0].end(), w[1].start()));
        }
    }

    #[test]
    fn a_branch_ends_every_chain() {
        // Three lines meeting at the origin: no joins there.
        let c = chains(vec![
            l((0.0, 0.0), (1.0, 0.0)),
            l((0.0, 0.0), (0.0, 1.0)),
            l((0.0, 0.0), (-1.0, 0.0)),
        ]);
        assert_eq!(c.len(), 3);
        assert!(c.iter().all(|c| !c.closed && c.segs.len() == 1));
    }

    #[test]
    fn an_open_polyline_joins_in_order() {
        let c = chains(vec![
            l((1.0, 0.0), (2.0, 0.0)),
            l((0.0, 0.0), (1.0, 0.0)),
            l((2.0, 0.0), (2.0, 1.0)),
        ]);
        assert_eq!(c.len(), 1);
        assert!(!c[0].closed);
        assert_eq!(c[0].segs.len(), 3);
    }
}
