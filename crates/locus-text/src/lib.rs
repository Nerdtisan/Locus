// SPDX-License-Identifier: AGPL-3.0-or-later

//! Single-line (Hershey/EMS) and outline font to geometry (SPEC §9).
//!
//! Core crate: no GUI deps, no filesystem access (font bytes come from the
//! caller), builds for wasm32. Coordinates are mm, y up, with the origin at the
//! first line's left baseline *before* alignment (SPEC §4).

// Tests may unwrap; library code may not (CLAUDE.md, Code Style).
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod bundled;
pub mod error;
pub mod geometry;
pub mod outline;
pub mod stroke;

use locus_geom::Vec2;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub use bundled::BundledFont;
pub use error::TextError;
pub use geometry::{Contour, Seg, TextGeometry};
pub use outline::OutlineFont;
pub use stroke::{StrokeFont, StrokeGlyph};

/// Most characters laid out in one text entity; bounds work on untrusted
/// `.locus` content (CLAUDE.md, Secure Coding).
pub const MAX_TEXT_CHARS: usize = 10_000;
/// Most lines laid out in one text entity.
pub const MAX_TEXT_LINES: usize = 500;

/// Horizontal alignment of each line about the anchor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
}

/// Layout parameters (SPEC §9).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TextStyle {
    /// The font's em in mm: the nominal font size. A capital is about
    /// 0.65–0.75 of it.
    pub size: f64,
    pub align: Align,
    /// Multiplier on the font's natural line pitch.
    pub line_spacing: f64,
}

impl Default for TextStyle {
    fn default() -> Self {
        Self {
            size: 10.0,
            align: Align::Left,
            line_spacing: 1.0,
        }
    }
}

/// A font ready to lay text out with.
#[derive(Debug, Clone)]
pub enum Font {
    /// Single-line: open strokes for pens and engravers.
    Stroke(Arc<StrokeFont>),
    /// Outline: closed contours for cutting.
    Outline(OutlineFont),
}

impl Font {
    /// Single-line fonts trace strokes; outline fonts trace shapes.
    #[must_use]
    pub fn is_single_line(&self) -> bool {
        matches!(self, Font::Stroke(_))
    }

    /// Lays `text` out. `\n` starts a new line; `\r` is ignored; other control
    /// characters are dropped.
    ///
    /// # Errors
    /// [`TextError::TextTooLong`] past [`MAX_TEXT_CHARS`]/[`MAX_TEXT_LINES`];
    /// [`TextError::BadOutlineFont`] if shaping fails.
    pub fn layout(&self, text: &str, style: &TextStyle) -> Result<TextGeometry, TextError> {
        if text.chars().count() > MAX_TEXT_CHARS {
            return Err(TextError::TextTooLong(MAX_TEXT_CHARS));
        }
        let lines: Vec<String> = text
            .split('\n')
            .map(|l| l.chars().filter(|c| !c.is_control()).collect())
            .collect();
        if lines.len() > MAX_TEXT_LINES {
            return Err(TextError::TextTooLong(MAX_TEXT_LINES));
        }
        // Reject non-finite/non-positive sizes as empty text rather than
        // emitting NaN geometry.
        let size = if style.size.is_finite() && style.size > 0.0 {
            style.size
        } else {
            0.0
        };
        let spacing = if style.line_spacing.is_finite() && style.line_spacing > 0.0 {
            style.line_spacing
        } else {
            1.0
        };
        let (scale, ascent, descent, pitch) = match self {
            Font::Stroke(f) => {
                let s = size / f.units_per_em;
                (s, f.ascent * s, f.descent * s, (f.ascent + f.descent) * s)
            }
            Font::Outline(f) => {
                let s = size / f.units_per_em();
                let (a, d) = f.vertical_extents();
                (s, a * s, d * s, (a + d + f.line_gap()) * s)
            }
        };
        let mut geom = TextGeometry {
            ascent,
            descent,
            lines: lines.len(),
            ..TextGeometry::default()
        };
        if size == 0.0 {
            return Ok(geom);
        }
        for (i, line) in lines.iter().enumerate() {
            let baseline = -(i as f64) * pitch * spacing;
            let width = match self {
                Font::Stroke(f) => stroke_width(f, line, scale),
                Font::Outline(f) => f.line_width(line, scale)?,
            };
            let x0 = match style.align {
                Align::Left => 0.0,
                Align::Center => -width / 2.0,
                Align::Right => -width,
            };
            let origin = Vec2::new(x0, baseline);
            match self {
                Font::Stroke(f) => stroke_line(f, line, scale, origin, &mut geom.contours),
                Font::Outline(f) => {
                    f.layout_line(line, scale, origin, &mut geom.contours)?;
                }
            }
            geom.width = geom.width.max(width);
        }
        geom.descent = descent + (lines.len() - 1) as f64 * pitch * spacing;
        Ok(geom)
    }
}

fn stroke_width(f: &StrokeFont, line: &str, scale: f64) -> f64 {
    line.chars()
        .map(|c| {
            f.glyph_or_missing(c)
                .map_or(f.space_advance(), |g| g.advance)
        })
        .sum::<f64>()
        * scale
}

fn stroke_line(f: &StrokeFont, line: &str, scale: f64, origin: Vec2, out: &mut Vec<Contour>) {
    let mut x = 0.0;
    for c in line.chars() {
        let Some(g) = f.glyph_or_missing(c) else {
            x += f.space_advance();
            continue;
        };
        for stroke in &g.strokes {
            let at = |p: &Vec2| Vec2::new(origin.x + (x + p.x) * scale, origin.y + p.y * scale);
            let Some(first) = stroke.first() else {
                continue;
            };
            let mut contour = Contour::new(at(first), false);
            for p in &stroke[1..] {
                contour.segs.push(Seg::Line { end: at(p) });
            }
            out.push(contour);
        }
        x += g.advance;
    }
}

#[cfg(test)]
mod tests;
