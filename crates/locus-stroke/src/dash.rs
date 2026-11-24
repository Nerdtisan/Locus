// SPDX-License-Identifier: AGPL-3.0-or-later

//! Dash patterns (#427, ADR 0019 decision 3): the path is split by arc
//! length, exactly on lines and arcs, into one pen path per dash.
//!
//! * [`DashAlign::Exact`] repeats the pattern from the path's start, shifted
//!   by the phase, as SVG's `stroke-dasharray` does.
//! * [`DashAlign::Corners`] (Illustrator's "align dashes to corners and path
//!   ends") scales the pattern on each corner-to-corner span so a whole
//!   number of repeats fits, with a dash centred on every corner and on both
//!   path ends.

use crate::{Chain, Limits, StrokeError};
use core::f64::consts::PI;
use locus_region::Seg;
use serde::{Deserialize, Serialize};

/// Most dash/gap pairs in a pattern (design §3).
pub const MAX_DASH_PAIRS: usize = 8;
/// Longest dash, gap or phase, mm.
pub const MAX_DASH_LEN: f64 = 1000.0;
/// A turn sharper than this (radians) is a corner for [`DashAlign::Corners`].
pub const CORNER_TURN: f64 = 15.0 * PI / 180.0;

/// How the pattern meets the path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum DashAlign {
    /// From the start, shifted by the phase (SVG and PDF dashes).
    #[default]
    Exact,
    /// Scaled per corner-to-corner span, centred on corners and ends.
    Corners,
}

/// A dash pattern: dash, gap, dash, gap… in mm, at most
/// [`MAX_DASH_PAIRS`] pairs. Fixed-size, so styles stay `Copy` and a file
/// can't make it grow. Stored in `.locus` as `pattern: [..]`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "DashFile", into = "DashFile")]
pub struct Dash {
    values: [f64; 2 * MAX_DASH_PAIRS],
    len: u8,
    /// Shift of the pattern along the path, mm (Exact only).
    pub phase: f64,
    pub align: DashAlign,
}

/// The on-disk shape of a [`Dash`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct DashFile {
    pattern: Vec<f64>,
    #[serde(default, skip_serializing_if = "is_zero")]
    phase: f64,
    #[serde(default, skip_serializing_if = "is_exact")]
    align: DashAlign,
}

fn is_zero(v: &f64) -> bool {
    *v == 0.0
}

fn is_exact(a: &DashAlign) -> bool {
    *a == DashAlign::Exact
}

impl TryFrom<DashFile> for Dash {
    type Error = StrokeError;
    fn try_from(f: DashFile) -> Result<Self, StrokeError> {
        Dash::new(&f.pattern, f.phase, f.align)
    }
}

impl From<Dash> for DashFile {
    fn from(d: Dash) -> Self {
        DashFile {
            pattern: d.pattern().to_vec(),
            phase: d.phase,
            align: d.align,
        }
    }
}

impl Dash {
    /// # Errors
    /// [`StrokeError::BadValue`] unless `pattern` is 1 to
    /// [`MAX_DASH_PAIRS`] dash/gap pairs, each finite and in
    /// `0..=MAX_DASH_LEN`, with a positive total, and `phase` is finite and
    /// in range.
    pub fn new(pattern: &[f64], phase: f64, align: DashAlign) -> Result<Self, StrokeError> {
        let bad = |what, value| Err(StrokeError::BadValue { what, value });
        let n = pattern.len();
        if n == 0 || !n.is_multiple_of(2) || n > 2 * MAX_DASH_PAIRS {
            return bad("dash pattern length", n as f64);
        }
        if let Some(v) = pattern
            .iter()
            .find(|v| !(v.is_finite() && (0.0..=MAX_DASH_LEN).contains(*v)))
        {
            return bad("dash length", *v);
        }
        let total: f64 = pattern.iter().sum();
        if total <= 0.0 {
            return bad("dash pattern total", total);
        }
        if !(phase.is_finite() && phase.abs() <= MAX_DASH_LEN) {
            return bad("dash phase", phase);
        }
        let mut values = [0.0; 2 * MAX_DASH_PAIRS];
        values[..n].copy_from_slice(pattern);
        Ok(Self {
            values,
            #[allow(clippy::cast_possible_truncation)]
            len: n as u8,
            phase,
            align,
        })
    }

    /// Dash, gap, dash, gap…, mm.
    #[must_use]
    pub fn pattern(&self) -> &[f64] {
        self.values.get(..usize::from(self.len)).unwrap_or(&[])
    }

    fn period(&self) -> f64 {
        self.pattern().iter().sum()
    }

    /// # Errors
    /// As [`Dash::new`], for a value changed in place or read from a file.
    pub fn validate(&self) -> Result<(), StrokeError> {
        Dash::new(self.pattern(), self.phase, self.align).map(|_| ())
    }
}

/// Arc-length intervals of the dashes along a path of `total` mm, made from
/// the pattern starting at `start` (≤ 0) scaled by `k`, clipped to `[a, b]`.
fn fill(
    dash: &Dash,
    k: f64,
    start: f64,
    a: f64,
    b: f64,
    limit: usize,
    out: &mut Vec<(f64, f64)>,
) -> Result<(), StrokeError> {
    let p = dash.pattern();
    let mut s = start;
    let mut i = 0usize;
    while s < b {
        let d = p.get(i % p.len()).copied().unwrap_or(0.0) * k;
        let g = p.get(i % p.len() + 1).copied().unwrap_or(0.0) * k;
        let (lo, hi) = (s.max(a), (s + d).min(b));
        if hi >= lo && s + d >= a {
            out.push((lo, hi));
            if out.len() > limit {
                return Err(StrokeError::Limit {
                    what: "dashes",
                    count: out.len(),
                });
            }
        }
        s += d + g;
        i += 2;
    }
    Ok(())
}

/// Joins intervals that touch (a dash running through a corner).
fn merge(mut v: Vec<(f64, f64)>) -> Vec<(f64, f64)> {
    v.sort_by(|x, y| x.0.total_cmp(&y.0));
    let mut out: Vec<(f64, f64)> = Vec::with_capacity(v.len());
    for (a, b) in v {
        match out.last_mut() {
            Some(last) if a <= last.1 + 1e-9 => last.1 = last.1.max(b),
            _ => out.push((a, b)),
        }
    }
    out
}

/// The part of `chain` between arc lengths `a` and `b`, exact for lines and
/// arcs. A zero-length dash (a dot) is a zero-length line at that point.
fn slice(chain: &Chain, lens: &[f64], a: f64, b: f64) -> Chain {
    let mut segs = Vec::new();
    let mut s0 = 0.0;
    for (seg, &l) in chain.segs.iter().zip(lens) {
        let s1 = s0 + l;
        if l > 0.0 && s1 >= a && s0 <= b {
            let (t0, t1) = (
                ((a - s0) / l).clamp(0.0, 1.0),
                ((b - s0) / l).clamp(0.0, 1.0),
            );
            if t1 > t0 || (a == b && segs.is_empty()) {
                segs.push(seg.sub(t0, t1.max(t0)));
            }
        }
        s0 = s1;
    }
    Chain {
        segs,
        closed: false,
    }
}

/// One pen path per dash of `chain` under `dash`, in path order.
///
/// # Errors
/// [`StrokeError::BadValue`] for a bad pattern; [`StrokeError::Limit`]
/// past `limits.dashes` (§6.2.1).
pub fn dashes(chain: &Chain, dash: &Dash, limits: &Limits) -> Result<Vec<Chain>, StrokeError> {
    dash.validate()?;
    let lens: Vec<f64> = chain.segs.iter().map(Seg::length).collect();
    let total: f64 = lens.iter().sum();
    if !(total.is_finite() && total > 0.0) {
        return Ok(Vec::new());
    }
    let period = dash.period();
    let mut iv = Vec::new();
    match dash.align {
        DashAlign::Exact => {
            let start = -dash.phase.rem_euclid(period);
            // A whole pattern repeat per period: bounded by the limit check.
            if total / period > limits.dashes as f64 * 2.0 + 2.0 {
                return Err(StrokeError::Limit {
                    what: "dashes",
                    count: (total / period) as usize,
                });
            }
            fill(dash, 1.0, start, 0.0, total, limits.dashes, &mut iv)?;
        }
        DashAlign::Corners => {
            // Corner-to-corner spans by arc length.
            let mut cuts = vec![0.0];
            let mut s = 0.0;
            for (w, &l) in chain.segs.windows(2).zip(&lens) {
                s += l;
                if let [x, y] = w {
                    let (t1, t2) = (x.tangent(1.0), y.tangent(0.0));
                    if t1.cross(t2).atan2(t1.dot(t2)).abs() > CORNER_TURN {
                        cuts.push(s);
                    }
                }
            }
            cuts.push(total);
            let first = dash.pattern().first().copied().unwrap_or(0.0);
            for w in cuts.windows(2) {
                let (a, b) = (w[0], w[1]);
                let span = b - a;
                if span <= 0.0 {
                    continue;
                }
                if span / period > limits.dashes as f64 + 1.0 {
                    return Err(StrokeError::Limit {
                        what: "dashes",
                        count: (span / period) as usize,
                    });
                }
                // A whole number of repeats, each starting mid-dash, so the
                // span begins and ends on half a dash.
                let n = (span / period).round().max(1.0);
                let k = span / (n * period);
                fill(dash, k, a - first * k * 0.5, a, b, limits.dashes, &mut iv)?;
            }
            iv = merge(iv);
        }
    }
    Ok(iv
        .into_iter()
        .map(|(a, b)| slice(chain, &lens, a, b))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use locus_geom::Vec2;

    fn line(x0: f64, y0: f64, x1: f64, y1: f64) -> Seg {
        Seg::Line {
            a: Vec2::new(x0, y0),
            b: Vec2::new(x1, y1),
        }
    }

    fn len(c: &Chain) -> f64 {
        c.segs.iter().map(Seg::length).sum()
    }

    #[test]
    fn exact_dashes_follow_the_pattern_and_phase() {
        let c = Chain {
            segs: vec![line(0.0, 0.0, 10.0, 0.0)],
            closed: false,
        };
        let d = Dash::new(&[3.0, 2.0], 0.0, DashAlign::Exact).unwrap();
        let ds = dashes(&c, &d, &Limits::default()).unwrap();
        assert_eq!(ds.len(), 2);
        assert!(ds.iter().all(|x| (len(x) - 3.0).abs() < 1e-12));
        let shifted = Dash::new(&[3.0, 2.0], 1.0, DashAlign::Exact).unwrap();
        let ds = dashes(&c, &shifted, &Limits::default()).unwrap();
        // 0..2, 4..7, 9..10
        let lens: Vec<f64> = ds.iter().map(len).collect();
        assert_eq!(lens.len(), 3);
        assert!((lens[0] - 2.0).abs() < 1e-12 && (lens[2] - 1.0).abs() < 1e-12);
    }

    #[test]
    fn corners_centre_a_dash_on_every_corner() {
        // An L of two 10 mm arms: dashes 2, gaps 2; a dash runs through the
        // corner, so it is one chain of two pieces.
        let c = Chain {
            segs: vec![line(0.0, 0.0, 10.0, 0.0), line(10.0, 0.0, 10.0, 10.0)],
            closed: false,
        };
        let d = Dash::new(&[2.0, 2.0], 0.0, DashAlign::Corners).unwrap();
        let ds = dashes(&c, &d, &Limits::default()).unwrap();
        let corner = ds.iter().find(|x| x.segs.len() == 2).unwrap();
        assert!((len(corner) - 2.0 * 10.0 / 12.0).abs() < 1e-9);
        // Both path ends start and finish on half a dash.
        assert!(ds[0].segs[0].start().distance(Vec2::new(0.0, 0.0)) < 1e-12);
        assert!(
            ds.last()
                .unwrap()
                .segs
                .last()
                .unwrap()
                .end()
                .distance(Vec2::new(10.0, 10.0))
                < 1e-12
        );
    }

    #[test]
    fn dashes_on_an_arc_stay_arcs() {
        let c = Chain {
            segs: vec![Seg::Arc {
                center: Vec2::ZERO,
                radius: 5.0,
                start: 0.0,
                sweep: PI,
            }],
            closed: false,
        };
        let d = Dash::new(&[1.0, 1.0], 0.0, DashAlign::Exact).unwrap();
        let ds = dashes(&c, &d, &Limits::default()).unwrap();
        assert!(ds.iter().all(|x| x.segs.iter().all(Seg::is_arc)));
        assert_eq!(ds.len(), 8);
    }

    #[test]
    fn bad_patterns_and_floods_are_refused() {
        for p in [
            &[][..],
            &[1.0][..],
            &[0.0, 0.0][..],
            &[f64::NAN, 1.0][..],
            &[1.0; 18][..],
        ] {
            assert!(Dash::new(p, 0.0, DashAlign::Exact).is_err(), "{p:?}");
        }
        let c = Chain {
            segs: vec![line(0.0, 0.0, 1000.0, 0.0)],
            closed: false,
        };
        let d = Dash::new(&[0.001, 0.001], 0.0, DashAlign::Exact).unwrap();
        assert!(matches!(
            dashes(&c, &d, &Limits::default()),
            Err(StrokeError::Limit { .. })
        ));
    }

    #[test]
    fn round_trips_as_a_pattern_list() {
        let d = Dash::new(&[3.0, 2.0, 0.0, 2.0], 0.5, DashAlign::Corners).unwrap();
        let s = ron::to_string(&d).unwrap();
        assert!(s.contains("pattern:"), "{s}");
        assert_eq!(ron::from_str::<Dash>(&s).unwrap(), d);
        assert!(ron::from_str::<Dash>("(pattern: [1.0])").is_err());
    }
}
