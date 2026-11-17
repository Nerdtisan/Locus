// SPDX-License-Identifier: AGPL-3.0-or-later

//! Text on a path (#392, SPEC §6.2): a recipe that sets a line of text
//! along a chain of curves: a line, arc, circle, shape outline or Bézier.
//!
//! The recipe keeps the text (content, font reference, size, letter spacing)
//! and how it sits on the path (start offset, side, alignment), plus the
//! glyph outlines laid out once by the app with the font
//! ([`PathText::new`]). Its instances are those glyphs placed along the
//! path, each turned to the path's direction at its middle. They are written
//! after every solve from the cached glyphs (as an offset is), so the text
//! follows the path without the font, and it stays re-editable: editing is an
//! `EditRecipe` with a re-laid-out [`PathText`].
//!
//! The path runs left to right when open, and clockwise when closed, so
//! [`PathSide::Above`] is on top of a line and outside a circle or shape. A
//! circle starts at its bottom, so centred text sits on its top.

use std::f64::consts::{PI, TAU};

use locus_geom::Vec2;
use locus_region::Seg as Piece;
use locus_text::{Align, FontProvider, FontRef, GlyphRun, MAX_TEXT_CHARS, Seg, TextStyle};
use serde::{Deserialize, Serialize};

use crate::command::Patch;
use crate::document::Document;
use crate::entity::Geom;
use crate::error::DocError;
use crate::ids::{EntityId, RecipeId};
use crate::offset::Out;
use crate::recipe::RecipeKind;
use crate::text::{MAX_CACHE_SEGS, MAX_TEXT_SIZE};

/// Which side of the path the text stands on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum PathSide {
    /// On the path's baseline, on its left (on top of a line running left
    /// to right, outside a circle).
    #[default]
    Above,
    /// Hanging below the path: the tops of the letters touch it.
    Below,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_zero(v: &f64) -> bool {
    *v == 0.0
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_false(v: &bool) -> bool {
    !*v
}

/// The text and its placement, with the glyphs cached (format v17).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PathText {
    pub content: String,
    pub font: FontRef,
    /// Letter height, mm.
    pub size: f64,
    /// Letter spacing, mm.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub tracking: f64,
    /// How far along the path the text moves from where `align` puts it,
    /// mm (towards the path's end; may be negative).
    pub offset: f64,
    pub side: PathSide,
    /// Left: starts at the path's start; Center: centred on its middle;
    /// Right: ends at its end.
    pub align: Align,
    /// Run the path the other way (the text flips over).
    #[serde(default, skip_serializing_if = "is_false")]
    pub flip: bool,
    /// The glyphs, laid out with the font (a cache: the file opens and the
    /// text follows its path without the font).
    pub run: GlyphRun,
}

impl PathText {
    /// Lay out `content` with `font` from `provider`.
    ///
    /// # Errors
    /// [`DocError::FontUnavailable`] for a font the provider lacks, and
    /// [`DocError::BadValue`] / [`DocError::BadText`] for values out of range.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        content: &str,
        font: FontRef,
        size: f64,
        tracking: f64,
        offset: f64,
        side: PathSide,
        align: Align,
        provider: &dyn FontProvider,
    ) -> Result<Self, DocError> {
        let mut t = Self {
            content: content.to_owned(),
            font,
            size,
            tracking,
            offset,
            side,
            align,
            flip: false,
            run: GlyphRun::default(),
        };
        t.lay_out(provider)?;
        Ok(t)
    }

    /// A copy with `with` applied and the glyphs laid out again.
    ///
    /// # Errors
    /// As [`PathText::new`].
    pub fn edited(
        &self,
        provider: &dyn FontProvider,
        with: impl FnOnce(&mut PathText),
    ) -> Result<Self, DocError> {
        let mut t = self.clone();
        with(&mut t);
        t.lay_out(provider)?;
        Ok(t)
    }

    fn lay_out(&mut self, provider: &dyn FontProvider) -> Result<(), DocError> {
        self.check_values()?;
        let font = provider
            .resolve(&self.font)
            .ok_or_else(|| DocError::FontUnavailable(self.font.display_name()))?;
        let style = TextStyle {
            size: self.size,
            align: Align::Left,
            line_spacing: 1.0,
            tracking: self.tracking,
        };
        self.run = font
            .glyph_run(&self.content, &style)
            .map_err(|e| DocError::BadText(e.to_string()))?;
        self.validate()
    }

    fn check_values(&self) -> Result<(), DocError> {
        if !self.font.is_valid() {
            return Err(DocError::BadText("invalid font reference".to_owned()));
        }
        if self.content.trim().is_empty() {
            return Err(DocError::BadText("the text is empty".to_owned()));
        }
        if self.content.chars().count() > MAX_TEXT_CHARS {
            return Err(DocError::BadText(format!(
                "text on a path is at most {MAX_TEXT_CHARS} characters"
            )));
        }
        for (what, v, ok) in [
            (
                "text size",
                self.size,
                self.size > 0.0 && self.size <= MAX_TEXT_SIZE,
            ),
            (
                "letter spacing",
                self.tracking,
                self.tracking.abs() <= MAX_TEXT_SIZE,
            ),
            (
                "start offset",
                self.offset,
                self.offset.abs() <= crate::operation::MAX_LENGTH * 1000.0,
            ),
        ] {
            if !(v.is_finite() && ok) {
                return Err(DocError::BadValue { what, value: v });
            }
        }
        Ok(())
    }

    /// The whole value, cache included (it is untrusted when loaded).
    pub(crate) fn validate(&self) -> Result<(), DocError> {
        self.check_values()?;
        let r = &self.run;
        let finite = |p: Vec2| p.x.is_finite() && p.y.is_finite();
        let mut segs = 0usize;
        for g in &r.glyphs {
            if !(g.x.is_finite() && g.advance.is_finite()) {
                return Err(DocError::BadText("bad glyph cache".to_owned()));
            }
            for c in &g.contours {
                segs += c.segs.len() + 1;
                let pts_ok = finite(c.start)
                    && c.segs.iter().all(|s| match s {
                        Seg::Line { end } => finite(*end),
                        Seg::Cubic { c0, c1, end } => finite(*c0) && finite(*c1) && finite(*end),
                    });
                if !pts_ok {
                    return Err(DocError::BadText("bad glyph cache".to_owned()));
                }
            }
        }
        if segs > MAX_CACHE_SEGS
            || r.glyphs.len() > MAX_TEXT_CHARS
            || !(r.width.is_finite() && r.ascent.is_finite() && r.descent.is_finite())
        {
            return Err(DocError::BadText("the glyph cache is too large".to_owned()));
        }
        Ok(())
    }
}

/// The path through `sources` as a polyline in travel order (see the module
/// notes for its direction), and whether it is closed.
pub(crate) fn path(
    doc: &Document,
    sources: &[EntityId],
    flip: bool,
) -> Result<(Vec<Vec2>, bool), DocError> {
    let rename = |e: DocError| match e {
        DocError::Invalid(m) => DocError::Invalid(m.replacen("offset:", "text on a path:", 1)),
        e => e,
    };
    let (order, closed) = crate::offset::chain(doc, sources).map_err(rename)?;
    let mut pieces: Vec<Piece> = Vec::new();
    for (id, backwards) in &order {
        if let Some(c) = doc.circle(*id).filter(|_| order.len() == 1) {
            // From the bottom, clockwise.
            pieces.push(Piece::Arc {
                center: c.center,
                radius: c.radius,
                start: -PI / 2.0,
                sweep: -TAU,
            });
            continue;
        }
        pieces.extend(crate::offset::pieces(doc, *id, *backwards));
    }
    let mut pts: Vec<Vec2> = Vec::new();
    for p in &pieces {
        match *p {
            Piece::Line { a, b } => {
                if pts.is_empty() {
                    pts.push(a);
                }
                pts.push(b);
            }
            Piece::Arc {
                center,
                radius,
                start,
                sweep,
            } => {
                // Steps short enough that the chord is within 0.005 mm.
                let step = if radius > 0.01 {
                    2.0 * (1.0 - 0.005 / radius).clamp(-1.0, 1.0).acos()
                } else {
                    sweep.abs()
                };
                let n = ((sweep.abs() / step.max(1e-3)).ceil() as usize).clamp(1, 4096);
                let at = |t: f64| center + Vec2::new(t.cos(), t.sin()) * radius;
                if pts.is_empty() {
                    pts.push(at(start));
                }
                for i in 1..=n {
                    pts.push(at(start + sweep * i as f64 / n as f64));
                }
            }
        }
    }
    if pts.len() < 2 {
        return Err(DocError::Invalid(
            "text on a path: the path is empty".to_owned(),
        ));
    }
    let circle = order.len() == 1 && doc.circle(order[0].0).is_some();
    // Open: left to right. Closed shapes: clockwise (negative area).
    let backwards = if circle {
        false
    } else if closed {
        let area: f64 = pts.windows(2).map(|w| w[0].cross(w[1])).sum::<f64>()
            + pts[pts.len() - 1].cross(pts[0]);
        area > 0.0
    } else {
        pts[pts.len() - 1].x < pts[0].x
    };
    if backwards != flip {
        pts.reverse();
    }
    Ok((pts, closed))
}

/// The glyphs placed along `pts`: points, lines and Béziers in
/// [`crate::offset::write_flat`] order. A glyph whose middle falls off an
/// open path is left out.
pub(crate) fn place(t: &PathText, pts: &[Vec2], closed: bool) -> Vec<Out> {
    let mut cum = Vec::with_capacity(pts.len());
    let mut total = 0.0;
    cum.push(0.0);
    for w in pts.windows(2) {
        total += w[0].distance(w[1]);
        cum.push(total);
    }
    if total <= f64::EPSILON {
        return Vec::new();
    }
    let width = t.run.width;
    let start = match t.align {
        Align::Left => 0.0,
        Align::Center => (total - width) / 2.0,
        Align::Right => total - width,
    } + t.offset;
    // The point and unit direction `s` along the path.
    let at = |s: f64| -> (Vec2, Vec2) {
        let i = match cum.binary_search_by(|c: &f64| c.total_cmp(&s)) {
            Ok(i) | Err(i) => i.clamp(1, cum.len() - 1),
        };
        let (a, b) = (pts[i - 1], pts[i]);
        let len = cum[i] - cum[i - 1];
        let dir = if len > 0.0 {
            (b - a) * (1.0 / len)
        } else {
            Vec2::new(1.0, 0.0)
        };
        (a + dir * (s - cum[i - 1]), dir)
    };
    let lift = match t.side {
        PathSide::Above => 0.0,
        PathSide::Below => -t.run.ascent,
    };
    let mut out = Vec::new();
    for g in &t.run.glyphs {
        let half = (g.advance - t.tracking) / 2.0;
        let mut mid = start + g.x + half;
        if closed {
            mid = mid.rem_euclid(total);
        } else if !(0.0..=total).contains(&mid) {
            continue;
        }
        let (p, u) = at(mid);
        let n = Vec2::new(-u.y, u.x);
        let map = |q: Vec2| p + u * (q.x - half) + n * (q.y + lift);
        for c in &g.contours {
            let first = out.len();
            out.push(Out::Point(map(c.start)));
            let mut prev = first;
            let last = c.segs.len().saturating_sub(1);
            for (k, s) in c.segs.iter().enumerate() {
                // A closed contour ends on its start point.
                let closes = c.closed && k == last && s.end().distance(c.start) < 1e-9;
                let end = |out: &mut Vec<Out>| {
                    if closes {
                        first
                    } else {
                        out.push(Out::Point(map(s.end())));
                        out.len() - 1
                    }
                };
                match *s {
                    Seg::Line { .. } => {
                        let e = end(&mut out);
                        out.push(Out::Line(prev, e));
                        prev = e;
                    }
                    Seg::Cubic { c0, c1, .. } => {
                        out.push(Out::Point(map(c0)));
                        let k0 = out.len() - 1;
                        out.push(Out::Point(map(c1)));
                        let k1 = out.len() - 1;
                        let e = end(&mut out);
                        out.push(Out::Bezier(prev, k0, k1, e));
                        prev = e;
                    }
                }
            }
        }
    }
    out
}

/// Rewrite a text-on-path recipe's glyphs from its path (after each solve).
pub(crate) fn regenerate(
    doc: &mut Document,
    rid: RecipeId,
    patches: &mut Vec<Patch>,
) -> Result<(), DocError> {
    let Some(r) = doc.recipe(rid).cloned() else {
        return Ok(());
    };
    let RecipeKind::TextOnPath(t) = &r.kind else {
        return Ok(());
    };
    // A broken path draws nothing rather than failing the solve.
    let out = path(doc, &r.sources, t.flip)
        .map(|(pts, closed)| place(t, &pts, closed))
        .unwrap_or_default();
    if out.len() > crate::recipe::MAX_INSTANCE_ENTITIES {
        return Err(DocError::Invalid(
            "text on a path: too much text".to_owned(),
        ));
    }
    let layer = r
        .sources
        .first()
        .and_then(|s| doc.entity(*s).map(|e| e.layer))
        .unwrap_or_else(Document::default_layer);
    crate::offset::write_flat(doc, rid, r, layer, crate::Style::default(), &out, patches)
}

/// The sources: one chain of lines, arcs, Béziers, or a single circle.
pub(crate) fn validate(doc: &Document, t: &PathText, sources: &[EntityId]) -> Result<(), DocError> {
    t.validate()?;
    for id in sources {
        if matches!(
            doc.entity(*id).map(|e| &e.geom),
            Some(Geom::Point { .. } | Geom::Text { .. })
        ) {
            return Err(DocError::Invalid(
                "text on a path: the path is lines, arcs, circles or Béziers".to_owned(),
            ));
        }
    }
    path(doc, sources, t.flip).map(|_| ())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // fixtures built in the test
mod tests {
    use super::*;
    use crate::command::{Command, Editor};
    use locus_text::{BundledFont, BundledProvider};

    const L: crate::LayerId = Document::default_layer();

    fn text(content: &str) -> PathText {
        PathText::new(
            content,
            FontRef::bundled(BundledFont::HersheySans),
            5.0,
            0.0,
            0.0,
            PathSide::Above,
            Align::Center,
            &BundledProvider,
        )
        .unwrap()
    }

    fn point(ed: &mut Editor, x: f64, y: f64) -> EntityId {
        ed.apply(&Command::AddEntity {
            layer: L,
            construction: false,
            geom: Geom::Point {
                pos: Vec2::new(x, y),
            },
            style: Default::default(),
        })
        .unwrap()
        .entities[0]
    }

    fn line(ed: &mut Editor, a: (f64, f64), b: (f64, f64)) -> (EntityId, EntityId, EntityId) {
        let (p, q) = (point(ed, a.0, a.1), point(ed, b.0, b.1));
        let l = ed
            .apply(&Command::AddEntity {
                layer: L,
                construction: false,
                geom: Geom::Line { start: p, end: q },
                style: Default::default(),
            })
            .unwrap()
            .entities[0];
        (l, p, q)
    }

    fn glyph_points(ed: &Editor, rid: RecipeId) -> Vec<Vec2> {
        let doc = ed.document();
        doc.recipe(rid)
            .unwrap()
            .instances()
            .filter_map(|id| doc.point_pos(id))
            .collect()
    }

    /// #392: text on a line sits above it, centred, as one undo step, and
    /// follows the line when it moves.
    #[test]
    fn text_follows_a_line() {
        let mut ed = Editor::new(Document::new());
        let (l, _, q) = line(&mut ed, (0.0, 0.0), (100.0, 0.0));
        let applied = ed
            .apply(&Command::AddRecipe {
                kind: RecipeKind::TextOnPath(Box::new(text("LOCUS"))),
                sources: vec![l],
            })
            .unwrap();
        let rid = applied.recipes[0];
        assert_eq!(ed.undo_label(), Some("Add text on path"));
        let pts = glyph_points(&ed, rid);
        assert!(!pts.is_empty());
        let (lo, hi) = pts.iter().fold((f64::MAX, f64::MIN), |(lo, hi), p| {
            (lo.min(p.x), hi.max(p.x))
        });
        assert!(((lo + hi) / 2.0 - 50.0).abs() < 2.0, "centred: {lo}..{hi}");
        assert!(
            pts.iter().all(|p| p.y >= -1e-9 && p.y <= 6.0),
            "above the line"
        );
        // Move the line's end up: the text turns with it.
        ed.apply(&Command::ModifyEntity {
            id: q,
            geom: Geom::Point {
                pos: Vec2::new(0.0, 100.0),
            },
        })
        .unwrap();
        let pts = glyph_points(&ed, rid);
        assert!(
            pts.iter().all(|p| p.x <= 1e-9),
            "left of a line going up: {pts:?}"
        );
        assert!(pts.iter().any(|p| p.y > 40.0));
    }

    /// Below hangs the letters under the path; a circle's centred text sits
    /// on its top, outside.
    #[test]
    fn side_and_circle() {
        let mut ed = Editor::new(Document::new());
        let (l, _, _) = line(&mut ed, (0.0, 0.0), (100.0, 0.0));
        let below = text("AB")
            .edited(&BundledProvider, |t| t.side = PathSide::Below)
            .unwrap();
        let rid = ed
            .apply(&Command::AddRecipe {
                kind: RecipeKind::TextOnPath(Box::new(below)),
                sources: vec![l],
            })
            .unwrap()
            .recipes[0];
        assert!(glyph_points(&ed, rid).iter().all(|p| p.y <= 1e-9));
        let c = point(&mut ed, 0.0, 0.0);
        let circle = ed
            .apply(&Command::AddEntity {
                layer: L,
                construction: false,
                geom: Geom::Circle {
                    center: c,
                    radius: 20.0,
                },
                style: Default::default(),
            })
            .unwrap()
            .entities[0];
        let rid = ed
            .apply(&Command::AddRecipe {
                kind: RecipeKind::TextOnPath(Box::new(text("TOP"))),
                sources: vec![circle],
            })
            .unwrap()
            .recipes[0];
        let pts = glyph_points(&ed, rid);
        assert!(pts.iter().all(|p| p.y > 15.0), "on top: {pts:?}");
        assert!(
            pts.iter().all(|p| p.length() >= 20.0 - 0.01),
            "outside, within the 0.005 mm flattening: {pts:?}"
        );
    }

    #[test]
    fn bad_values_and_paths_are_refused() {
        let mut ed = Editor::new(Document::new());
        let p = point(&mut ed, 0.0, 0.0);
        let bad = ed.apply(&Command::AddRecipe {
            kind: RecipeKind::TextOnPath(Box::new(text("X"))),
            sources: vec![p],
        });
        assert!(bad.is_err(), "a point is no path");
        for edit in [
            |t: &mut PathText| t.size = -1.0,
            |t: &mut PathText| t.offset = f64::NAN,
            |t: &mut PathText| t.content = " ".to_owned(),
        ] {
            assert!(text("X").edited(&BundledProvider, edit).is_err());
        }
    }
}
