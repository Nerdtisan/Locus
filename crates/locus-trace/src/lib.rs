// SPDX-License-Identifier: AGPL-3.0-or-later
//! Auto trace (#461, ADR 0019 decision 8): a raster reference into lines
//! and cubic Béziers.
//!
//! Locus's own implementation, written from published methods only:
//! Selinger's *Potrace: a polygon-based tracing algorithm* (2003) for
//! outlines, Otsu (1979) for the automatic threshold, Sobel for edges,
//! k-means for colours and Zhang–Suen (1984) for centrelines. No Potrace
//! code is used (it is GPL-2.0-or-later, which Locus's licence policy
//! doesn't allow, SPEC §2).
//!
//! Core crate (SPEC §3): no GUI, no I/O, no clock and no threads. Work is
//! bounded by a step [`Budget`] instead of a timer, the image is
//! downsampled to at most [`Limits::max_pixels`], and the output to at most
//! [`Limits::max_segments`] pieces. Malformed input returns an error; it
//! never panics.

mod bitmap;
mod centreline;
mod curve;
mod decompose;

pub use bitmap::{Bitmap, otsu};
pub use curve::CurveParams;
pub use decompose::{LatticePath, decompose};

use locus_geom::Vec2;

/// Most colours in Colours mode.
pub const MAX_COLOURS: usize = 16;

/// The default step budget (spike S5, ADR 0019): enough for a centreline
/// of a noisy 4 MP photo (616 M steps), the heaviest fixture.
pub const DEFAULT_STEPS: u64 = 1_000_000_000;

/// Why a trace stopped.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TraceError {
    #[error("the image's size and pixel data don't match")]
    BadImage,
    #[error("the trace needs more work than allowed; raise smoothing or speckle size")]
    Budget,
    #[error("too many curves ({count} > {limit}): raise smoothing or speckle size")]
    TooMany { count: usize, limit: usize },
    #[error("cancelled")]
    Cancelled,
}

/// Work allowed, in steps (about one per pixel or path point visited).
/// It replaces a time limit, which a core crate can't read (SPEC §3).
pub struct Budget<'a> {
    left: u64,
    since_check: u64,
    cancel: Option<&'a mut dyn FnMut() -> bool>,
}

impl std::fmt::Debug for Budget<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Budget")
            .field("left", &self.left)
            .finish_non_exhaustive()
    }
}

impl<'a> Budget<'a> {
    #[must_use]
    pub fn new(steps: u64) -> Self {
        Self {
            left: steps,
            since_check: 0,
            cancel: None,
        }
    }

    /// Asks `cancel` every so often; `true` stops the trace.
    #[must_use]
    pub fn with_cancel(mut self, cancel: &'a mut dyn FnMut() -> bool) -> Self {
        self.cancel = Some(cancel);
        self
    }

    pub(crate) fn spend(&mut self, steps: usize) -> Result<(), TraceError> {
        let steps = steps as u64;
        if steps > self.left {
            self.left = 0;
            return Err(TraceError::Budget);
        }
        self.left -= steps;
        self.since_check += steps;
        if self.since_check >= 65_536 {
            self.since_check = 0;
            if let Some(cancel) = self.cancel.as_mut()
                && cancel()
            {
                return Err(TraceError::Cancelled);
            }
        }
        Ok(())
    }

    /// Steps left.
    #[must_use]
    pub fn left(&self) -> u64 {
        self.left
    }
}

/// Bounds on the work and the result (§6.2.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Larger images are box-downsampled by a whole factor first.
    pub max_pixels: usize,
    /// Most pieces (lines and curves) in the result: the entity limit.
    pub max_segments: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_pixels: 4_000_000,
            max_segments: 200_000,
        }
    }
}

/// What to trace.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mode {
    /// Dark pixels, at or below `level` (Otsu's level when `None`).
    Threshold { level: Option<u8> },
    /// Where the brightness changes (Sobel), above `level` strength.
    Edges { level: Option<u8> },
    /// `count` colours, each its own group.
    Colours { count: u8 },
    /// Single-line paths through the dark parts, for pens and engraving.
    Centreline { level: Option<u8> },
}

/// The dialog's settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Params {
    pub mode: Mode,
    /// Shapes of at most this many pixels are dropped.
    pub speckle: u32,
    pub curve: CurveParams,
    /// Centreline simplification, pixels.
    pub tolerance: f64,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            mode: Mode::Threshold { level: None },
            speckle: 4,
            curve: CurveParams::default(),
            tolerance: 0.5,
        }
    }
}

/// One traced piece, in source-image pixels (x right, y down).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Seg {
    Line(Vec2, Vec2),
    Cubic(Vec2, Vec2, Vec2, Vec2),
}

impl Seg {
    /// The point at `t` in 0..=1.
    #[must_use]
    pub fn at(&self, t: f64) -> Vec2 {
        match *self {
            Seg::Line(a, b) => a.lerp(b, t),
            Seg::Cubic(a, b, c, d) => {
                let u = 1.0 - t;
                a * (u * u * u) + b * (3.0 * u * u * t) + c * (3.0 * u * t * t) + d * (t * t * t)
            }
        }
    }
}

/// One path: its pieces in order, and whether it closes.
#[derive(Clone, Debug, PartialEq)]
pub struct TracedPath {
    pub segments: Vec<Seg>,
    pub closed: bool,
}

/// Paths that share a colour: one per colour in Colours mode, else one.
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub colour: [u8; 3],
    pub paths: Vec<TracedPath>,
}

/// What a trace made.
#[derive(Clone, Debug, PartialEq)]
pub struct Traced {
    pub groups: Vec<Group>,
    /// Source pixels per traced pixel (1 unless the image was downsampled).
    pub factor: usize,
}

impl Traced {
    /// All pieces, for the dialog's count and the entity limit.
    #[must_use]
    pub fn segments(&self) -> usize {
        self.groups
            .iter()
            .flat_map(|g| &g.paths)
            .map(|p| p.segments.len())
            .sum()
    }
}

/// Trace an RGBA8 image, `width` × `height`, row 0 at the top.
///
/// # Errors
/// [`TraceError`]: bad input, the budget or the output limit exceeded, or
/// cancelled.
pub fn trace(
    width: u32,
    height: u32,
    rgba: &[u8],
    params: &Params,
    limits: &Limits,
    budget: &mut Budget,
) -> Result<Traced, TraceError> {
    let px = bitmap::Pixels::from_rgba(width, height, rgba, limits.max_pixels, budget)?;
    let factor = px.factor as f64;
    let speckle = u64::from(params.speckle);
    let (w, h) = (px.width, px.height);
    let mut groups = Vec::new();
    let outlines =
        |b: &bitmap::Bitmap, budget: &mut Budget| -> Result<Vec<TracedPath>, TraceError> {
            let mut paths = Vec::new();
            for p in decompose(b, speckle, budget)? {
                let segments = curve::outline(&p, &params.curve, factor, budget)?;
                if !segments.is_empty() {
                    paths.push(TracedPath {
                        segments,
                        closed: true,
                    });
                }
            }
            Ok(paths)
        };
    match params.mode {
        Mode::Threshold { level } => {
            let lum = px.luminance();
            let level = level.unwrap_or_else(|| otsu(&lum));
            let b = bitmap::threshold(w, h, &lum, level);
            groups.push(Group {
                colour: [0, 0, 0],
                paths: outlines(&b, budget)?,
            });
        }
        Mode::Edges { level } => {
            let edges = bitmap::sobel(w, h, &px.luminance(), budget)?;
            let level = level.unwrap_or_else(|| otsu(&edges));
            let b = bitmap::threshold(w, h, &edges, level);
            groups.push(Group {
                colour: [0, 0, 0],
                paths: outlines(&b, budget)?,
            });
        }
        Mode::Colours { count } => {
            let (colours, labels) = bitmap::quantise(&px, usize::from(count), budget)?;
            for (c, colour) in colours.iter().enumerate() {
                let mut b = bitmap::Bitmap::new(w, h);
                for (i, l) in labels.iter().enumerate() {
                    if usize::from(*l) == c {
                        b.set(i % w, i / w, true);
                    }
                }
                if b.count() == 0 {
                    continue;
                }
                groups.push(Group {
                    colour: *colour,
                    paths: outlines(&b, budget)?,
                });
            }
        }
        Mode::Centreline { level } => {
            let lum = px.luminance();
            let level = level.unwrap_or_else(|| otsu(&lum));
            let b = bitmap::threshold(w, h, &lum, level);
            let lines = centreline::centrelines(
                &b,
                params.tolerance,
                params.speckle as usize,
                factor,
                budget,
            )?;
            groups.push(Group {
                colour: [0, 0, 0],
                paths: lines
                    .into_iter()
                    .map(|(segments, closed)| TracedPath { segments, closed })
                    .collect(),
            });
        }
    }
    let traced = Traced {
        groups,
        factor: px.factor,
    };
    let count = traced.segments();
    if count > limits.max_segments {
        return Err(TraceError::TooMany {
            count,
            limit: limits.max_segments,
        });
    }
    Ok(traced)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests;
