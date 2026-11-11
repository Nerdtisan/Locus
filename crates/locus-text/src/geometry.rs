// SPDX-License-Identifier: AGPL-3.0-or-later

//! Output geometry of text layout: contours of lines and cubic Béziers in mm,
//! y up (SPEC §4, §9). Quadratic TrueType segments are elevated exactly to
//! cubics, so one curve type reaches the document (the `Bezier` entity, #159).

use locus_geom::{Aabb, Affine2, Cubic, Vec2};
use serde::{Deserialize, Serialize};

/// One segment of a [`Contour`], ending at `end`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Seg {
    Line { end: Vec2 },
    Cubic { c0: Vec2, c1: Vec2, end: Vec2 },
}

impl Seg {
    #[must_use]
    pub const fn end(&self) -> Vec2 {
        match self {
            Seg::Line { end } | Seg::Cubic { end, .. } => *end,
        }
    }

    /// The segment as a cubic starting at `from` (a line becomes a straight
    /// cubic), for callers that want one curve type.
    #[must_use]
    pub fn as_cubic(&self, from: Vec2) -> Cubic {
        match *self {
            Seg::Line { end } => Cubic::new(
                from,
                from + (end - from) * (1.0 / 3.0),
                from + (end - from) * (2.0 / 3.0),
                end,
            ),
            Seg::Cubic { c0, c1, end } => Cubic::new(from, c0, c1, end),
        }
    }

    fn map(self, f: impl Fn(Vec2) -> Vec2) -> Self {
        match self {
            Seg::Line { end } => Seg::Line { end: f(end) },
            Seg::Cubic { c0, c1, end } => Seg::Cubic {
                c0: f(c0),
                c1: f(c1),
                end: f(end),
            },
        }
    }
}

/// A connected run of segments. Outline glyphs give closed contours (the last
/// segment ends at `start`); single-line glyphs give open strokes (SPEC §9).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Contour {
    pub start: Vec2,
    pub segs: Vec<Seg>,
    pub closed: bool,
}

impl Contour {
    #[must_use]
    pub fn new(start: Vec2, closed: bool) -> Self {
        Self {
            start,
            segs: Vec::new(),
            closed,
        }
    }

    /// Every segment as a cubic with its start point.
    pub fn cubics(&self) -> impl Iterator<Item = Cubic> + '_ {
        let mut from = self.start;
        self.segs.iter().map(move |s| {
            let c = s.as_cubic(from);
            from = s.end();
            c
        })
    }

    #[must_use]
    pub fn bbox(&self) -> Aabb {
        let mut b = Aabb::from_point(self.start);
        let mut from = self.start;
        for s in &self.segs {
            b = b.union(s.as_cubic(from).bbox());
            from = s.end();
        }
        b
    }

    fn map(&self, f: impl Fn(Vec2) -> Vec2 + Copy) -> Self {
        Self {
            start: f(self.start),
            segs: self.segs.iter().map(|s| s.map(f)).collect(),
            closed: self.closed,
        }
    }
}

/// Laid-out text: contours plus the line metrics callers need to constrain it.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct TextGeometry {
    pub contours: Vec<Contour>,
    /// Widest line, mm.
    pub width: f64,
    /// Distance from the first baseline up to the font's ascent, mm.
    pub ascent: f64,
    /// Distance from the last baseline down to the font's descent, mm (positive).
    pub descent: f64,
    /// Number of lines laid out.
    pub lines: usize,
}

impl TextGeometry {
    /// Bounding box of the geometry, or `None` when there is none (blank text).
    #[must_use]
    pub fn bbox(&self) -> Option<Aabb> {
        self.contours
            .iter()
            .map(Contour::bbox)
            .reduce(|a, b| a.union(b))
    }

    /// The geometry moved/rotated/scaled by `t`.
    #[must_use]
    pub fn transformed(&self, t: &Affine2) -> Self {
        let f = |p: Vec2| t.apply(p);
        Self {
            contours: self.contours.iter().map(|c| c.map(f)).collect(),
            ..self.clone()
        }
    }
}
