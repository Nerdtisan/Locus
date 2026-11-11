// SPDX-License-Identifier: AGPL-3.0-or-later

//! Outline fonts (SPEC §9, §3): `harfrust` shapes (kerning, ligatures,
//! contextual forms), `skrifa`/`read-fonts` reads the glyph outlines. The font
//! bytes come from the caller, so this module never touches the filesystem and
//! builds for wasm32.

use crate::error::TextError;
use crate::geometry::{Contour, Seg};
use harfrust::{Buffer, Font, GlyphId, ShapeOptions, ShaperFont};
use locus_geom::Vec2;
use skrifa::font::glyph::outline::{DrawMemory, DrawOptions, PathSink};

/// Largest user font file accepted, bytes. Large CJK fonts are tens of MB;
/// this bounds memory for a hostile file without excluding normal fonts.
pub const MAX_OUTLINE_FONT_BYTES: usize = 64 * 1024 * 1024;

/// A parsed TrueType/OpenType font, cheap to clone (the font data is shared).
#[derive(Clone)]
pub struct OutlineFont {
    font: Font,
    name: String,
}

impl std::fmt::Debug for OutlineFont {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OutlineFont")
            .field("name", &self.name)
            .field("units_per_em", &self.font.units_per_em())
            .finish_non_exhaustive()
    }
}

/// One shaped glyph in font units, relative to the line's start.
struct Placed {
    gid: u32,
    x: f64,
    y: f64,
}

impl OutlineFont {
    /// Loads a font from owned bytes (a user `.ttf`/`.otf`; the app does the
    /// I/O). Variable fonts use their default instance.
    ///
    /// # Errors
    /// [`TextError`] if the data is too large or is not a font with outlines.
    pub fn from_bytes(name: &str, data: Vec<u8>) -> Result<Self, TextError> {
        if data.len() > MAX_OUTLINE_FONT_BYTES {
            return Err(TextError::FontTooLarge(MAX_OUTLINE_FONT_BYTES));
        }
        Self::build(name, Font::new(data, 0))
    }

    /// Loads a font from data baked into the binary, without copying it.
    ///
    /// # Errors
    /// As [`OutlineFont::from_bytes`].
    pub fn from_static(name: &str, data: &'static [u8]) -> Result<Self, TextError> {
        Self::build(name, Font::new(data, 0))
    }

    fn build(name: &str, font: Option<Font>) -> Result<Self, TextError> {
        let font = font.ok_or(TextError::BadOutlineFont)?;
        // Reject fonts without a usable em or any outlines up front.
        if font.units_per_em() == 0 || font.num_glyphs() == 0 {
            return Err(TextError::BadOutlineFont);
        }
        Ok(Self {
            font,
            name: name.to_owned(),
        })
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn units_per_em(&self) -> f64 {
        f64::from(self.font.units_per_em())
    }

    /// `(ascent, descent)` in font units, both positive.
    #[must_use]
    pub fn vertical_extents(&self) -> (f64, f64) {
        let m = self.font.metrics();
        let line = m.hhea_line.or(m.typo_line);
        match line {
            Some(l) => (
                f64::from(l.ascender.to_f32()),
                f64::from(l.descender.to_f32()).abs(),
            ),
            None => (self.units_per_em() * 0.8, self.units_per_em() * 0.2),
        }
    }

    /// Line gap in font units.
    #[must_use]
    pub fn line_gap(&self) -> f64 {
        let m = self.font.metrics();
        m.hhea_line
            .or(m.typo_line)
            .map_or(0.0, |l| f64::from(l.line_gap.to_f32()))
    }

    /// Height of a capital letter in font units, if the font says.
    #[must_use]
    pub fn cap_height(&self) -> Option<f64> {
        self.font
            .metrics()
            .cap_height
            .map(|c| f64::from(c.to_f32()))
    }

    /// Shapes one line (no newlines): returns glyph placements and the
    /// line's advance, in font units.
    fn shape_line(&self, line: &str) -> Result<(Vec<Placed>, f64), TextError> {
        let shaper = ShaperFont::new(&self.font);
        let mut buf = Buffer::new();
        buf.push_str(line);
        buf.guess_segment_properties();
        harfrust::shape(&shaper, &mut buf, ShapeOptions::new())
            .map_err(|_| TextError::BadOutlineFont)?;
        let mut pen = 0.0_f64;
        let mut out = Vec::with_capacity(buf.len());
        for (info, pos) in buf.glyph_infos().iter().zip(buf.glyph_positions()) {
            out.push(Placed {
                gid: info.glyph_id,
                x: pen + f64::from(pos.x_offset),
                y: f64::from(pos.y_offset),
            });
            pen += f64::from(pos.x_advance);
        }
        Ok((out, pen))
    }

    /// Lays out one line at `(origin_x, baseline_y)` mm with `scale` mm per
    /// font unit, appending closed contours. Returns the line's advance in mm.
    pub(crate) fn layout_line(
        &self,
        line: &str,
        scale: f64,
        origin: Vec2,
        out: &mut Vec<Contour>,
    ) -> Result<f64, TextError> {
        let (placed, advance) = self.shape_line(line)?;
        let glyphs = self.font.outline_glyphs();
        for p in &placed {
            let Some(glyph) = glyphs.get(GlyphId::new(p.gid)) else {
                continue;
            };
            let mut sink = ContourSink {
                out,
                scale,
                dx: origin.x + p.x * scale,
                dy: origin.y + p.y * scale,
                cur: None,
            };
            // A glyph whose outline fails to decode is skipped, not fatal:
            // a corrupt glyph must not stop the rest of the text.
            if glyph
                .draw(&DrawOptions::new(), DrawMemory::Scratch, &mut sink)
                .is_ok()
            {
                sink.finish();
            } else {
                sink.discard();
            }
        }
        Ok(advance * scale)
    }

    /// Advance of `line` in mm at `scale` mm per font unit.
    pub(crate) fn line_width(&self, line: &str, scale: f64) -> Result<f64, TextError> {
        Ok(self.shape_line(line)?.1 * scale)
    }
}

/// Collects a glyph's path as closed [`Contour`]s, elevating quadratics to
/// cubics so only lines and cubics reach the document.
struct ContourSink<'a> {
    out: &'a mut Vec<Contour>,
    scale: f64,
    dx: f64,
    dy: f64,
    cur: Option<Contour>,
}

impl ContourSink<'_> {
    fn pt(&self, x: f32, y: f32) -> Vec2 {
        Vec2::new(
            self.dx + f64::from(x) * self.scale,
            self.dy + f64::from(y) * self.scale,
        )
    }

    fn last(&self) -> Vec2 {
        self.cur.as_ref().map_or(Vec2::new(0.0, 0.0), |c| {
            c.segs.last().map_or(c.start, Seg::end)
        })
    }

    fn push(&mut self, s: Seg) {
        if let Some(c) = &mut self.cur {
            c.segs.push(s);
        }
    }

    fn finish(&mut self) {
        self.close();
    }

    /// Drops a half-drawn contour left by a failed decode.
    fn discard(&mut self) {
        self.cur = None;
    }
}

impl PathSink for ContourSink<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        self.close();
        self.cur = Some(Contour::new(self.pt(x, y), true));
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let end = self.pt(x, y);
        self.push(Seg::Line { end });
    }

    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        let p0 = self.last();
        let q = self.pt(cx0, cy0);
        let end = self.pt(x, y);
        // Exact degree elevation.
        self.push(Seg::Cubic {
            c0: p0 + (q - p0) * (2.0 / 3.0),
            c1: end + (q - end) * (2.0 / 3.0),
            end,
        });
    }

    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        let (c0, c1, end) = (self.pt(cx0, cy0), self.pt(cx1, cy1), self.pt(x, y));
        self.push(Seg::Cubic { c0, c1, end });
    }

    fn close(&mut self) {
        let Some(mut c) = self.cur.take() else { return };
        // Make the closing edge explicit so every closed contour ends where it
        // starts; drop the contour if it encloses nothing.
        let last = c.segs.last().map_or(c.start, Seg::end);
        if (last - c.start).length() > 1e-9 {
            c.segs.push(Seg::Line { end: c.start });
        }
        if !c.segs.is_empty() {
            self.out.push(c);
        }
    }
}
