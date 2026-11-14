// SPDX-License-Identifier: AGPL-3.0-or-later

//! The text entity (SPEC §9, §10; #179).
//!
//! A text entity is a *container* that generates geometry: its content, font
//! reference and style are stored, and so is the laid-out geometry (the
//! cache). Font files are never embedded (licences often forbid it), so a file
//! opens and exports identically on a machine without the font; the font
//! simply reports as missing until the user picks another, which re-lays the
//! text out.
//!
//! Placement is constrainable like any sketch geometry: the text refers to two
//! point entities, the `anchor` (left/centre/right baseline origin, depending
//! on alignment) and `dir`, whose offset from the anchor sets the baseline
//! direction. Both are ordinary points, so coincident/fix/distance constraints
//! apply to them directly, and a construction line drawn between them lets
//! horizontal/vertical/angle constraints control the baseline.
//!
//! The cache is in the text's local frame (origin at the anchor, baseline
//! along +x, mm), so moving or rotating the points never needs a re-layout.

use crate::document::Document;
use crate::error::DocError;
use crate::ids::EntityId;
use locus_geom::{Affine2, Vec2};
use locus_text::{
    Align, Font, FontProvider, FontRef, MAX_TEXT_CHARS, MAX_TEXT_LINES, TextGeometry, TextStyle,
};
use serde::{Deserialize, Serialize};

/// Most contour segments one text entity may cache (bounds hostile files).
pub const MAX_CACHE_SEGS: usize = 400_000;
/// Most contours one text entity may cache.
pub const MAX_CACHE_CONTOURS: usize = 50_000;
/// Largest accepted font size, mm.
pub const MAX_TEXT_SIZE: f64 = 100_000.0;
/// Largest accepted line-spacing multiplier.
pub const MAX_LINE_SPACING: f64 = 100.0;

// serde's `skip_serializing_if` passes a reference.
#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_zero(v: &f64) -> bool {
    *v == 0.0
}

/// Content, font and style of a text entity, plus the cached layout.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextData {
    pub content: String,
    pub font: FontRef,
    /// Font size (the em), mm.
    pub size: f64,
    pub align: Align,
    pub line_spacing: f64,
    /// Letter spacing (tracking) in mm (#222, format v14). Omitted from
    /// files while zero, so older files load unchanged.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub tracking: f64,
    /// Laid-out geometry in the local frame; derived from the fields above
    /// by [`TextData::new`] and stored so the file needs no font to open.
    pub layout: TextGeometry,
}

impl TextData {
    /// Lays `content` out with the font `provider` finds for `font`.
    ///
    /// # Errors
    /// [`DocError::FontUnavailable`] if the font cannot be found,
    /// [`DocError::BadText`] for invalid style values or text that is too long.
    pub fn new(
        content: &str,
        font: FontRef,
        size: f64,
        align: Align,
        line_spacing: f64,
        provider: &dyn FontProvider,
    ) -> Result<Self, DocError> {
        let mut t = Self {
            content: content.to_owned(),
            font,
            size,
            align,
            line_spacing,
            tracking: 0.0,
            layout: TextGeometry::default(),
        };
        t.check_style()?;
        t.layout = t.lay_out(provider)?;
        t.validate()?;
        Ok(t)
    }

    /// The same text laid out again, for an edit to the content or style made
    /// through `with`. Fails (leaving `self` as it was) if the font is missing.
    ///
    /// # Errors
    /// As [`TextData::new`].
    pub fn edited(
        &self,
        provider: &dyn FontProvider,
        with: impl FnOnce(&mut TextData),
    ) -> Result<Self, DocError> {
        let mut t = self.clone();
        with(&mut t);
        t.check_style()?;
        t.layout = t.lay_out(provider)?;
        t.validate()?;
        Ok(t)
    }

    #[must_use]
    pub fn style(&self) -> TextStyle {
        TextStyle {
            size: self.size,
            align: self.align,
            line_spacing: self.line_spacing,
            tracking: self.tracking,
        }
    }

    /// Is the font absent from `provider`? The cached geometry still exports;
    /// only editing needs the font (#179: "font missing" until edited).
    #[must_use]
    pub fn font_missing(&self, provider: &dyn FontProvider) -> bool {
        provider.resolve(&self.font).is_none()
    }

    fn lay_out(&self, provider: &dyn FontProvider) -> Result<TextGeometry, DocError> {
        let font: Font = provider
            .resolve(&self.font)
            .ok_or_else(|| DocError::FontUnavailable(self.font.display_name()))?;
        font.layout(&self.content, &self.style())
            .map_err(|e| DocError::BadText(e.to_string()))
    }

    fn check_style(&self) -> Result<(), DocError> {
        if !self.font.is_valid() {
            return Err(DocError::BadText("invalid font reference".to_owned()));
        }
        if !(self.size.is_finite() && self.size > 0.0 && self.size <= MAX_TEXT_SIZE) {
            return Err(DocError::BadValue {
                what: "text size",
                value: self.size,
            });
        }
        if !(self.tracking.is_finite() && self.tracking.abs() <= MAX_TEXT_SIZE) {
            return Err(DocError::BadValue {
                what: "letter spacing",
                value: self.tracking,
            });
        }
        if !(self.line_spacing.is_finite()
            && self.line_spacing > 0.0
            && self.line_spacing <= MAX_LINE_SPACING)
        {
            return Err(DocError::BadValue {
                what: "line spacing",
                value: self.line_spacing,
            });
        }
        Ok(())
    }

    /// Everything a stored or loaded text must satisfy (untrusted input).
    pub(crate) fn validate(&self) -> Result<(), DocError> {
        self.check_style()?;
        let bad = |m: &str| Err(DocError::BadText(m.to_owned()));
        if self.content.chars().count() > MAX_TEXT_CHARS
            || self.content.matches('\n').count() >= MAX_TEXT_LINES
        {
            return bad("text is too long");
        }
        let l = &self.layout;
        if l.contours.len() > MAX_CACHE_CONTOURS
            || l.contours.iter().map(|c| c.segs.len()).sum::<usize>() > MAX_CACHE_SEGS
        {
            return bad("cached geometry is too large");
        }
        let finite = |v: Vec2| v.x.is_finite() && v.y.is_finite();
        let ok = [l.width, l.ascent, l.descent].iter().all(|v| v.is_finite())
            && l.contours.iter().all(|c| {
                finite(c.start)
                    && c.segs.iter().all(|s| match s {
                        locus_text::Seg::Line { end } => finite(*end),
                        locus_text::Seg::Cubic { c0, c1, end } => {
                            finite(*c0) && finite(*c1) && finite(*end)
                        }
                    })
            });
        if !ok {
            return bad("cached geometry is not finite");
        }
        Ok(())
    }
}

/// Local frame → document: origin at `anchor`, +x along `anchor → dir`.
/// A `dir` on top of `anchor` falls back to the document x axis rather than
/// producing NaN geometry.
#[must_use]
pub fn frame(anchor: Vec2, dir: Vec2) -> Affine2 {
    let d = dir - anchor;
    let angle = if d.length() > 1e-12 {
        d.y.atan2(d.x)
    } else {
        0.0
    };
    Affine2::rotate(angle).then(&Affine2::translate(anchor))
}

impl Document {
    /// A text entity's data and its anchor/direction points.
    #[must_use]
    pub fn text(&self, id: EntityId) -> Option<(&TextData, EntityId, EntityId)> {
        match &self.entity(id)?.geom {
            crate::entity::Geom::Text { anchor, dir, text } => Some((text, *anchor, *dir)),
            _ => None,
        }
    }

    /// The text's geometry in document millimetres, placed by its points.
    #[must_use]
    pub fn text_geometry(&self, id: EntityId) -> Option<TextGeometry> {
        let (t, anchor, dir) = self.text(id)?;
        let f = frame(self.point_pos(anchor)?, self.point_pos(dir)?);
        Some(t.layout.transformed(&f))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use locus_text::{BundledFont, BundledProvider};

    fn data(content: &str) -> TextData {
        TextData::new(
            content,
            FontRef::bundled(BundledFont::Figtree),
            10.0,
            Align::Left,
            1.0,
            &BundledProvider,
        )
        .unwrap()
    }

    #[test]
    fn layout_is_cached_and_edit_relays_out() {
        let t = data("Hi");
        assert!(!t.layout.contours.is_empty());
        let longer = t
            .edited(&BundledProvider, |t| t.content = "Hello".into())
            .unwrap();
        assert!(longer.layout.width > t.layout.width);
    }

    #[test]
    fn missing_font_reports_and_blocks_edit_but_keeps_cache() {
        let mut t = data("Hi");
        t.font = FontRef::File("Gone.ttf".into());
        assert!(t.font_missing(&BundledProvider));
        assert!(!t.layout.contours.is_empty(), "cache survives");
        assert!(matches!(
            t.edited(&BundledProvider, |t| t.content = "x".into()),
            Err(DocError::FontUnavailable(_))
        ));
        // Picking an available font fixes it.
        let fixed = t
            .edited(&BundledProvider, |t| {
                t.font = FontRef::bundled(BundledFont::HersheySans);
            })
            .unwrap();
        assert!(!fixed.font_missing(&BundledProvider));
    }

    #[test]
    fn bad_styles_are_rejected() {
        for size in [0.0, -1.0, f64::NAN, MAX_TEXT_SIZE * 2.0] {
            assert!(
                TextData::new(
                    "a",
                    FontRef::bundled(BundledFont::Figtree),
                    size,
                    Align::Left,
                    1.0,
                    &BundledProvider
                )
                .is_err()
            );
        }
    }

    #[test]
    fn frame_places_and_rotates() {
        let f = frame(Vec2::new(10.0, 5.0), Vec2::new(10.0, 9.0));
        let p = f.apply(Vec2::new(2.0, 0.0));
        assert!(
            (p.x - 10.0).abs() < 1e-9 && (p.y - 7.0).abs() < 1e-9,
            "{p:?}"
        );
        // Degenerate direction falls back to +x.
        let g = frame(Vec2::new(1.0, 1.0), Vec2::new(1.0, 1.0));
        assert!((g.apply(Vec2::new(2.0, 0.0)).x - 3.0).abs() < 1e-9);
    }
}
