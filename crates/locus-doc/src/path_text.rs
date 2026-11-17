// SPDX-License-Identifier: AGPL-3.0-or-later

//! Text on a path (#392, SPEC §6.2, §9): an existing text bound to a chain
//! of curves (a line, arc, circle, shape outline or Bézier).
//!
//! Binding is part of the text itself ([`TextData::path`]), so the text
//! stays one entity: re-editable in the text dialog (#222), exported,
//! offset and patterned as text. Its glyphs are laid out once with the font
//! ([`TextPath::run`], a cache like [`TextData::layout`]) and bent along the
//! path whenever the geometry is asked for ([`Document::text_geometry`]),
//! each glyph turned to the path's direction at its middle. So the text
//! follows the path when the path changes, without the font. Detaching sets
//! `path` back to `None`, and the text returns to its anchor.
//!
//! The path runs left to right when open, and clockwise when closed, so
//! [`PathSide::Above`] is on top of a line and outside a circle or shape. A
//! circle starts at its bottom, so centred text sits on its top. The text's
//! own alignment places it: Left at the path's start, Center on its middle,
//! Right at its end; [`TextPath::offset`] slides it along from there.
//!
//! [`TextData::path`]: crate::text::TextData::path
//! [`TextData::layout`]: crate::text::TextData::layout

use std::f64::consts::{PI, TAU};

use locus_geom::Vec2;
use locus_region::Seg as Piece;
use locus_text::{Align, Contour, GlyphRun, Seg};
use serde::{Deserialize, Serialize};

use crate::document::Document;
use crate::error::DocError;
use crate::ids::EntityId;

/// Most curves a text path may run over.
pub const MAX_PATH_CURVES: usize = 10_000;

/// Which side of the path the text stands on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum PathSide {
    /// On the path, on its left (on top of a line running left to right,
    /// outside a circle).
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

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_above(v: &PathSide) -> bool {
    *v == PathSide::Above
}

/// Where a text runs along a path (format v17).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextPath {
    /// The path: one chain of curves.
    pub curves: Vec<EntityId>,
    /// How far along the path the text slides from where its alignment puts
    /// it, mm (towards the path's end; may be negative).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub offset: f64,
    #[serde(default, skip_serializing_if = "is_above")]
    pub side: PathSide,
    /// Run the path the other way (the text flips over).
    #[serde(default, skip_serializing_if = "is_false")]
    pub flip: bool,
    /// The glyphs one by one (a cache, laid out with the font).
    pub run: GlyphRun,
}

impl TextPath {
    /// A path over `curves` with default placement; the glyphs are laid out
    /// when the text is ([`crate::text::TextData::edited`]).
    #[must_use]
    pub fn new(curves: Vec<EntityId>) -> Self {
        Self {
            curves,
            offset: 0.0,
            side: PathSide::Above,
            flip: false,
            run: GlyphRun::default(),
        }
    }

    /// The values and the cache are untrusted when loaded.
    pub(crate) fn validate(&self) -> Result<(), DocError> {
        if self.curves.is_empty() || self.curves.len() > MAX_PATH_CURVES {
            return Err(DocError::BadText(
                "a text path needs 1 to 10 000 curves".to_owned(),
            ));
        }
        if !(self.offset.is_finite() && self.offset.abs() <= crate::operation::MAX_LENGTH * 1000.0)
        {
            return Err(DocError::BadValue {
                what: "text path offset",
                value: self.offset,
            });
        }
        let r = &self.run;
        let finite = |p: Vec2| p.x.is_finite() && p.y.is_finite();
        let mut segs = 0usize;
        for g in &r.glyphs {
            if !(g.x.is_finite() && g.advance.is_finite()) {
                return Err(DocError::BadText("bad glyph cache".to_owned()));
            }
            for c in &g.contours {
                segs += c.segs.len() + 1;
                let ok = finite(c.start)
                    && c.segs.iter().all(|s| match s {
                        Seg::Line { end } => finite(*end),
                        Seg::Cubic { c0, c1, end } => finite(*c0) && finite(*c1) && finite(*end),
                    });
                if !ok {
                    return Err(DocError::BadText("bad glyph cache".to_owned()));
                }
            }
        }
        if segs > crate::text::MAX_CACHE_SEGS
            || r.glyphs.len() > locus_text::MAX_TEXT_CHARS
            || !(r.width.is_finite() && r.ascent.is_finite() && r.descent.is_finite())
        {
            return Err(DocError::BadText("the glyph cache is too large".to_owned()));
        }
        Ok(())
    }
}

/// The path through `curves` as a polyline in travel order (see the module
/// notes for its direction), and whether it is closed.
///
/// # Errors
/// [`DocError`] when the curves are missing or are not one chain.
pub fn path(
    doc: &Document,
    curves: &[EntityId],
    flip: bool,
) -> Result<(Vec<Vec2>, bool), DocError> {
    let rename = |e: DocError| match e {
        DocError::Invalid(m) => DocError::Invalid(m.replacen("offset:", "text on a path:", 1)),
        e => e,
    };
    let (order, closed) = crate::offset::chain(doc, curves).map_err(rename)?;
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

/// Arc length along a polyline: running totals.
fn lengths(pts: &[Vec2]) -> Vec<f64> {
    let mut cum = Vec::with_capacity(pts.len());
    let mut total = 0.0;
    cum.push(0.0);
    for w in pts.windows(2) {
        total += w[0].distance(w[1]);
        cum.push(total);
    }
    cum
}

/// How far along `pts` the point nearest `p` is, mm: for sliding text
/// along its path.
#[must_use]
pub fn along(pts: &[Vec2], p: Vec2) -> f64 {
    let cum = lengths(pts);
    let mut best = (f64::MAX, 0.0);
    for (i, w) in pts.windows(2).enumerate() {
        let d = w[1] - w[0];
        let len2 = d.dot(d);
        let t = if len2 > 0.0 {
            ((p - w[0]).dot(d) / len2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let q = w[0] + d * t;
        let dist = q.distance(p);
        if dist < best.0 {
            best = (dist, cum[i] + (cum[i + 1] - cum[i]) * t);
        }
    }
    best.1
}

/// The point and unit direction `s` mm along `pts`, whose running lengths
/// are `cum` (`pts` has at least two points).
fn point_at(pts: &[Vec2], cum: &[f64], s: f64) -> (Vec2, Vec2) {
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
}

/// The text's alignment point on the path (#416): where its left edge,
/// middle or right edge sits for Left, Center or Right, as [`bend`] places
/// it, and the path's unit direction there. Off the end of an open path it
/// stays at that end. `None` for an empty path.
#[must_use]
pub fn alignment_point(
    tp: &TextPath,
    align: Align,
    pts: &[Vec2],
    closed: bool,
) -> Option<(Vec2, Vec2)> {
    let cum = lengths(pts);
    let total = cum.last().copied().unwrap_or(0.0);
    if total <= f64::EPSILON || pts.len() < 2 {
        return None;
    }
    let s = match align {
        Align::Left => 0.0,
        Align::Center => total / 2.0,
        Align::Right => total,
    } + tp.offset;
    let s = if closed {
        s.rem_euclid(total)
    } else {
        s.clamp(0.0, total)
    };
    Some(point_at(pts, &cum, s))
}

/// The dashed alignment guide of text on a path (#416), the counterpart of
/// a plain text's baseline from its anchor: a tick across the path at the
/// [`alignment_point`], spanning the letters' height on the text's side.
/// `size` is the font size, used when the run has no height (no glyphs).
#[must_use]
pub fn guide(
    tp: &TextPath,
    align: Align,
    size: f64,
    pts: &[Vec2],
    closed: bool,
) -> Option<(Vec2, Vec2)> {
    let (p, u) = alignment_point(tp, align, pts, closed)?;
    let n = Vec2::new(-u.y, u.x);
    let h = if tp.run.ascent > 0.0 {
        tp.run.ascent
    } else {
        size
    };
    let lift = match tp.side {
        PathSide::Above => 0.0,
        PathSide::Below => -h,
    };
    Some((p + n * (lift - h / 4.0), p + n * (lift + h * 1.25)))
}

/// The glyphs of `tp.run` set along `pts`, as contours in document mm. A
/// glyph whose middle falls off an open path is left out.
#[must_use]
pub fn bend(
    tp: &TextPath,
    align: Align,
    tracking: f64,
    pts: &[Vec2],
    closed: bool,
) -> Vec<Contour> {
    let cum = lengths(pts);
    let total = cum.last().copied().unwrap_or(0.0);
    if total <= f64::EPSILON || pts.len() < 2 {
        return Vec::new();
    }
    let width = tp.run.width;
    let start = match align {
        Align::Left => 0.0,
        Align::Center => (total - width) / 2.0,
        Align::Right => total - width,
    } + tp.offset;
    let at = |s: f64| point_at(pts, &cum, s);
    let lift = match tp.side {
        PathSide::Above => 0.0,
        PathSide::Below => -tp.run.ascent,
    };
    let mut out = Vec::new();
    for g in &tp.run.glyphs {
        let half = (g.advance - tracking) / 2.0;
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
            let mut m = Contour::new(map(c.start), c.closed);
            for s in &c.segs {
                m.segs.push(match *s {
                    Seg::Line { end } => Seg::Line { end: map(end) },
                    Seg::Cubic { c0, c1, end } => Seg::Cubic {
                        c0: map(c0),
                        c1: map(c1),
                        end: map(end),
                    },
                });
            }
            out.push(m);
        }
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // fixtures built in the test
mod tests {
    use super::*;
    use crate::command::{Command, Editor};
    use crate::entity::Geom;
    use crate::text::TextData;
    use locus_text::{BundledFont, BundledProvider, FontRef};

    const L: crate::LayerId = Document::default_layer();

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

    fn add(ed: &mut Editor, geom: Geom) -> EntityId {
        ed.apply(&Command::AddEntity {
            layer: L,
            construction: false,
            geom,
            style: Default::default(),
        })
        .unwrap()
        .entities[0]
    }

    /// Text placed at (−50, 50), and its id.
    fn text(ed: &mut Editor, content: &str, align: Align) -> EntityId {
        let (a, d) = (point(ed, -50.0, 50.0), point(ed, -40.0, 50.0));
        let t = TextData::new(
            content,
            FontRef::bundled(BundledFont::HersheySans),
            5.0,
            align,
            1.0,
            &BundledProvider,
        )
        .unwrap();
        add(
            ed,
            Geom::Text {
                anchor: a,
                dir: d,
                text: Box::new(t),
            },
        )
    }

    /// Bind text `t` to `curves` with `edit` applied, as the tool does.
    fn bind(ed: &mut Editor, t: EntityId, curves: Vec<EntityId>, edit: impl FnOnce(&mut TextPath)) {
        let (data, anchor, dir) = ed.document().text(t).unwrap();
        let (anchor, dir) = (anchor, dir);
        let bound = data
            .edited(&BundledProvider, |d| {
                let mut p = TextPath::new(curves);
                edit(&mut p);
                d.path = Some(Box::new(p));
            })
            .unwrap();
        ed.apply(&Command::ModifyEntity {
            id: t,
            geom: Geom::Text {
                anchor,
                dir,
                text: Box::new(bound),
            },
        })
        .unwrap();
    }

    fn pts(ed: &Editor, t: EntityId) -> Vec<Vec2> {
        ed.document()
            .text_geometry(t)
            .unwrap()
            .contours
            .iter()
            .flat_map(|c| std::iter::once(c.start).chain(c.segs.iter().map(Seg::end)))
            .collect()
    }

    /// #392: text bound to a line sits on it, centred, as one undo step;
    /// it follows the line when the line moves; detaching puts it back.
    #[test]
    fn bound_text_follows_its_line_and_detaches() {
        let mut ed = Editor::new(Document::new());
        let t = text(&mut ed, "LOCUS", Align::Center);
        let straight = pts(&ed, t);
        let (a, q) = (point(&mut ed, 0.0, 0.0), point(&mut ed, 100.0, 0.0));
        let line = add(&mut ed, Geom::Line { start: a, end: q });
        bind(&mut ed, t, vec![line], |_| {});
        let p = pts(&ed, t);
        let (lo, hi) = p.iter().fold((f64::MAX, f64::MIN), |(lo, hi), p| {
            (lo.min(p.x), hi.max(p.x))
        });
        assert!(((lo + hi) / 2.0 - 50.0).abs() < 2.0, "centred: {lo}..{hi}");
        assert!(p.iter().all(|p| p.y >= -1e-9 && p.y <= 6.0), "on the line");
        ed.apply(&Command::ModifyEntity {
            id: q,
            geom: Geom::Point {
                pos: Vec2::new(0.0, 100.0),
            },
        })
        .unwrap();
        let p = pts(&ed, t);
        assert!(p.iter().all(|p| p.x <= 1e-9), "left of a line going up");
        assert!(p.iter().any(|p| p.y > 40.0));
        ed.undo().unwrap();
        ed.undo().unwrap();
        assert_eq!(pts(&ed, t), straight, "unbound by undo");
    }

    /// Below hangs the letters under the path; a circle's centred text sits
    /// on its top, outside; the offset slides it.
    #[test]
    fn side_circle_and_offset() {
        let mut ed = Editor::new(Document::new());
        let t = text(&mut ed, "TOP", Align::Center);
        let c = point(&mut ed, 0.0, 0.0);
        let circle = add(
            &mut ed,
            Geom::Circle {
                center: c,
                radius: 20.0,
            },
        );
        bind(&mut ed, t, vec![circle], |_| {});
        let p = pts(&ed, t);
        assert!(p.iter().all(|p| p.y > 15.0), "on top: {p:?}");
        assert!(p.iter().all(|p| p.length() >= 20.0 - 0.01), "outside");
        let t2 = text(&mut ed, "AB", Align::Left);
        let (a, q) = (point(&mut ed, 0.0, -50.0), point(&mut ed, 100.0, -50.0));
        let line = add(&mut ed, Geom::Line { start: a, end: q });
        bind(&mut ed, t2, vec![line], |p| {
            p.side = PathSide::Below;
            p.offset = 30.0;
        });
        let p = pts(&ed, t2);
        assert!(p.iter().all(|p| p.y <= -50.0 + 1e-9), "below");
        assert!(p.iter().all(|p| p.x >= 30.0 - 1e-9), "slid 30 mm along");
    }

    #[test]
    fn along_finds_the_nearest_distance() {
        let pts = [
            Vec2::new(0.0, 0.0),
            Vec2::new(10.0, 0.0),
            Vec2::new(10.0, 10.0),
        ];
        assert!((along(&pts, Vec2::new(4.0, 3.0)) - 4.0).abs() < 1e-12);
        assert!((along(&pts, Vec2::new(13.0, 6.0)) - 16.0).abs() < 1e-12);
    }

    /// #416: the guide marks the alignment point on the path (left edge,
    /// middle or right edge) and moves with the offset.
    #[test]
    fn the_guide_marks_the_alignment_point_on_the_path() {
        for (align, x) in [
            (Align::Left, 0.0),
            (Align::Center, 50.0),
            (Align::Right, 100.0),
        ] {
            let mut ed = Editor::new(Document::new());
            let t = text(&mut ed, "LOCUS", align);
            let (a, q) = (point(&mut ed, 0.0, 0.0), point(&mut ed, 100.0, 0.0));
            let line = add(&mut ed, Geom::Line { start: a, end: q });
            assert!(ed.document().text_path_guide(t).is_none(), "plain text");
            bind(&mut ed, t, vec![line], |_| {});
            let (g0, g1) = ed.document().text_path_guide(t).unwrap();
            assert!(
                (g0.x - x).abs() < 1e-9 && (g1.x - x).abs() < 1e-9,
                "{align:?}"
            );
            assert!(g0.y < 0.0 && g1.y > 3.0, "across the path, up the letters");
            // Slide 20 mm along: the guide follows.
            let (data, anchor, dir) = ed.document().text(t).unwrap();
            let mut slid = data.clone();
            slid.path.as_mut().unwrap().offset = 20.0;
            ed.apply(&Command::ModifyEntity {
                id: t,
                geom: Geom::Text {
                    anchor,
                    dir,
                    text: Box::new(slid),
                },
            })
            .unwrap();
            let (g0, _) = ed.document().text_path_guide(t).unwrap();
            let want = (x + 20.0).min(100.0);
            assert!((g0.x - want).abs() < 1e-9, "{align:?}: {g0:?}");
        }
    }

    #[test]
    fn hostile_paths_are_refused() {
        let mut p = TextPath::new(vec![]);
        assert!(p.validate().is_err());
        p.curves = vec![EntityId::new(9)];
        p.offset = f64::NAN;
        assert!(p.validate().is_err());
    }
}
