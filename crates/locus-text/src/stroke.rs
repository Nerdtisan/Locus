// SPDX-License-Identifier: AGPL-3.0-or-later

//! Single-line fonts (SPEC §9): glyphs are open strokes a pen or engraver
//! traces once. Two sources feed one [`StrokeFont`]: Hershey `.jhf` data
//! (bundled, public domain) and EMS-style SVG fonts supplied by the user.

use crate::error::TextError;
use locus_geom::Vec2;
use std::collections::BTreeMap;

/// Largest user font file accepted, bytes. SVG fonts are small text files.
pub const MAX_STROKE_FONT_BYTES: usize = 8 * 1024 * 1024;
/// Most glyphs read from one user font.
const MAX_GLYPHS: usize = 20_000;
/// Most points read from one glyph.
const MAX_GLYPH_POINTS: usize = 10_000;

/// One glyph: strokes in font units (y up, baseline at 0) and its advance.
#[derive(Debug, Clone, PartialEq)]
pub struct StrokeGlyph {
    pub advance: f64,
    pub strokes: Vec<Vec<Vec2>>,
}

/// A single-line font. Measurements are in font units; `units_per_em` is what
/// `TextStyle::size` maps to.
#[derive(Debug, Clone, PartialEq)]
pub struct StrokeFont {
    pub name: String,
    pub units_per_em: f64,
    pub ascent: f64,
    /// Positive distance below the baseline.
    pub descent: f64,
    pub cap_height: f64,
    glyphs: BTreeMap<char, StrokeGlyph>,
    /// Used for characters the font lacks.
    missing: Option<StrokeGlyph>,
}

impl StrokeFont {
    /// The glyph for `c`, else the font's own "missing" glyph, else `?`.
    #[must_use]
    pub fn glyph_or_missing(&self, c: char) -> Option<&StrokeGlyph> {
        self.glyphs
            .get(&c)
            .or(self.missing.as_ref())
            .or_else(|| self.glyphs.get(&'?'))
    }

    /// The glyph for exactly `c`.
    #[must_use]
    pub fn glyph(&self, c: char) -> Option<&StrokeGlyph> {
        self.glyphs.get(&c)
    }

    #[must_use]
    pub fn glyph_count(&self) -> usize {
        self.glyphs.len()
    }

    /// Advance for a space, falling back to a quarter em.
    #[must_use]
    pub fn space_advance(&self) -> f64 {
        self.glyphs
            .get(&' ')
            .map_or(self.units_per_em * 0.25, |g| g.advance)
    }

    /// Parses Hershey `.jhf` data covering ASCII 32..=126 in order, the layout
    /// of the bundled files. Hershey y grows downward with the baseline at
    /// y = 9; both are normalised here (y up, baseline 0).
    ///
    /// # Errors
    /// [`TextError::BadStrokeFont`] on a malformed record.
    pub fn from_jhf(name: &str, data: &str) -> Result<Self, TextError> {
        const BASELINE: f64 = 9.0;
        let bad = |m: &str| TextError::BadStrokeFont(m.to_owned());
        let mut glyphs = BTreeMap::new();
        for (i, line) in data.lines().filter(|l| !l.trim().is_empty()).enumerate() {
            let code = 32 + u32::try_from(i).map_err(|_| bad("too many glyphs"))?;
            if code > 126 {
                break;
            }
            let ch = char::from_u32(code).ok_or_else(|| bad("bad code point"))?;
            // 5-char id, 3-char vertex count (includes the bearing pair), then pairs.
            let bytes = line.as_bytes();
            if bytes.len() < 10 || !line.is_ascii() {
                return Err(bad("short or non-ASCII record"));
            }
            let count: usize = line[5..8]
                .trim()
                .parse()
                .map_err(|_| bad("bad vertex count"))?;
            let body = &bytes[8..];
            if count == 0 || body.len() < count * 2 {
                return Err(bad("vertex count exceeds record"));
            }
            let coord = |b: u8| f64::from(b) - f64::from(b'R');
            let left = coord(body[0]);
            let right = coord(body[1]);
            let mut strokes: Vec<Vec<Vec2>> = Vec::new();
            let mut cur: Vec<Vec2> = Vec::new();
            for pair in body[2..count * 2].as_chunks::<2>().0 {
                if pair == b" R" {
                    flush(&mut strokes, &mut cur);
                } else {
                    cur.push(Vec2::new(coord(pair[0]) - left, BASELINE - coord(pair[1])));
                }
            }
            flush(&mut strokes, &mut cur);
            glyphs.insert(
                ch,
                StrokeGlyph {
                    advance: right - left,
                    strokes,
                },
            );
        }
        if glyphs.len() < 95 {
            return Err(bad("expected 95 ASCII glyphs"));
        }
        Ok(Self {
            name: name.to_owned(),
            // Hershey line pitch is ~32 units; capitals are 21 tall.
            units_per_em: 32.0,
            ascent: 25.0,
            descent: 7.0,
            cap_height: 21.0,
            glyphs,
            missing: None,
        })
    }

    /// Parses an EMS-style SVG single-stroke font (`<font>` with `<glyph
    /// unicode=… d=…>` children, as shipped with Hershey Text for Inkscape).
    /// Path data may use M/L/H/V/Z (either case); anything else is rejected,
    /// not guessed at.
    ///
    /// # Errors
    /// [`TextError`] for oversized or malformed input.
    pub fn from_svg_font(name: &str, data: &str) -> Result<Self, TextError> {
        let bad = |m: &str| TextError::BadStrokeFont(m.to_owned());
        if data.len() > MAX_STROKE_FONT_BYTES {
            return Err(TextError::FontTooLarge(MAX_STROKE_FONT_BYTES));
        }
        let mut upem = 1000.0;
        let mut ascent = None;
        let mut descent = None;
        let mut cap_height = None;
        let mut default_adv = None;
        let mut glyphs = BTreeMap::new();
        let mut missing = None;
        for tag in tags(data) {
            let attrs = Attrs::parse(tag.body);
            match tag.name {
                "font" => default_adv = attrs.num("horiz-adv-x"),
                "font-face" => {
                    upem = attrs.num("units-per-em").unwrap_or(upem);
                    ascent = attrs.num("ascent");
                    descent = attrs.num("descent");
                    cap_height = attrs.num("cap-height");
                }
                "glyph" | "missing-glyph" => {
                    let adv = attrs
                        .num("horiz-adv-x")
                        .or(default_adv)
                        .ok_or_else(|| bad("glyph without advance"))?;
                    let strokes = match attrs.get("d") {
                        Some(d) => path_strokes(d)?,
                        None => Vec::new(),
                    };
                    let glyph = StrokeGlyph {
                        advance: adv,
                        strokes,
                    };
                    if tag.name == "missing-glyph" {
                        missing = Some(glyph);
                    } else if let Some(u) = attrs.get("unicode") {
                        let mut it = u.chars();
                        // Ligature glyphs (multi-char unicode) are skipped.
                        if let (Some(c), None) = (it.next(), it.next()) {
                            if glyphs.len() >= MAX_GLYPHS {
                                return Err(bad("too many glyphs"));
                            }
                            glyphs.insert(c, glyph);
                        }
                    }
                }
                _ => {}
            }
        }
        if glyphs.is_empty() {
            return Err(bad("no glyphs"));
        }
        if !(upem.is_finite() && upem > 0.0) {
            return Err(bad("bad units-per-em"));
        }
        let cap = cap_height
            .or_else(|| {
                glyphs
                    .get(&'H')
                    .and_then(|g| g.strokes.iter().flatten().map(|p| p.y).reduce(f64::max))
            })
            .unwrap_or(upem * 0.7);
        Ok(Self {
            name: name.to_owned(),
            units_per_em: upem,
            ascent: ascent.unwrap_or(upem * 0.8),
            descent: descent.map_or(upem * 0.2, f64::abs),
            cap_height: cap,
            glyphs,
            missing,
        })
    }
}

fn flush(strokes: &mut Vec<Vec<Vec2>>, cur: &mut Vec<Vec2>) {
    // A lone point is a dot; keep it as a zero-length stroke so a plotter
    // still marks it.
    if cur.len() == 1 {
        cur.push(cur[0]);
    }
    if !cur.is_empty() {
        strokes.push(std::mem::take(cur));
    }
}

struct Tag<'a> {
    name: &'a str,
    body: &'a str,
}

/// Opening/self-closing tags only: all an SVG font needs. No entity
/// expansion and no DTDs, so there is nothing to bomb.
fn tags(data: &str) -> impl Iterator<Item = Tag<'_>> {
    data.split('<').skip(1).filter_map(|chunk| {
        let end = chunk.find('>')?;
        let inner = chunk[..end].trim_end_matches('/');
        let first = inner.chars().next()?;
        if first == '/' || first == '!' || first == '?' {
            return None;
        }
        let (name, body) = inner.split_once(char::is_whitespace).unwrap_or((inner, ""));
        Some(Tag { name, body })
    })
}

struct Attrs<'a>(Vec<(&'a str, &'a str)>);

impl<'a> Attrs<'a> {
    fn parse(mut s: &'a str) -> Self {
        let mut v = Vec::new();
        while let Some(eq) = s.find('=') {
            let key = s[..eq].trim();
            let rest = s[eq + 1..].trim_start();
            let Some(q) = rest.chars().next().filter(|c| *c == '"' || *c == '\'') else {
                break;
            };
            let rest = &rest[1..];
            let Some(close) = rest.find(q) else { break };
            v.push((key, &rest[..close]));
            s = &rest[close + 1..];
        }
        Self(v)
    }

    fn get(&self, k: &str) -> Option<&'a str> {
        self.0.iter().find(|(n, _)| *n == k).map(|(_, v)| *v)
    }

    fn num(&self, k: &str) -> Option<f64> {
        self.get(k)
            .and_then(|v| v.trim().parse::<f64>().ok())
            .filter(|v| v.is_finite())
    }
}

fn path_strokes(d: &str) -> Result<Vec<Vec<Vec2>>, TextError> {
    let bad = |m: &str| TextError::BadStrokeFont(m.to_owned());
    let mut strokes = Vec::new();
    let mut cur: Vec<Vec2> = Vec::new();
    let mut pos = Vec2::new(0.0, 0.0);
    let mut start = pos;
    let mut total = 0usize;
    let mut cmd = ' ';
    let mut pending: Vec<f64> = Vec::new();
    for t in d
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|t| !t.is_empty())
        .flat_map(split_cmd)
    {
        let first = t.chars().next().unwrap_or(' ');
        if first.is_ascii_alphabetic() {
            if !pending.is_empty() {
                return Err(bad("dangling path operands"));
            }
            cmd = first;
            if cmd.eq_ignore_ascii_case(&'z') {
                if !cur.is_empty() {
                    cur.push(start);
                    flush(&mut strokes, &mut cur);
                }
                pos = start;
            } else if !"MmLlHhVv".contains(cmd) {
                return Err(bad("unsupported path command"));
            }
            continue;
        }
        let v: f64 = t.parse().map_err(|_| bad("bad path number"))?;
        if !v.is_finite() {
            return Err(bad("bad path number"));
        }
        pending.push(v);
        let rel = cmd.is_ascii_lowercase();
        let need = match cmd.to_ascii_uppercase() {
            'M' | 'L' => 2,
            'H' | 'V' => 1,
            _ => return Err(bad("operands without a command")),
        };
        if pending.len() < need {
            continue;
        }
        let p = match cmd.to_ascii_uppercase() {
            'H' => Vec2::new(if rel { pos.x + pending[0] } else { pending[0] }, pos.y),
            'V' => Vec2::new(pos.x, if rel { pos.y + pending[0] } else { pending[0] }),
            _ if rel => Vec2::new(pos.x + pending[0], pos.y + pending[1]),
            _ => Vec2::new(pending[0], pending[1]),
        };
        pending.clear();
        if cmd.eq_ignore_ascii_case(&'m') {
            flush(&mut strokes, &mut cur);
            start = p;
            // Further pairs after an M are implicit lineto.
            cmd = if rel { 'l' } else { 'L' };
        }
        cur.push(p);
        pos = p;
        total += 1;
        if total > MAX_GLYPH_POINTS {
            return Err(bad("glyph has too many points"));
        }
    }
    if !pending.is_empty() {
        return Err(bad("dangling path operands"));
    }
    flush(&mut strokes, &mut cur);
    Ok(strokes)
}

/// Splits "M10" / "L5-3" style tokens into command and number tokens.
fn split_cmd(tok: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut from = 0;
    let b = tok.as_bytes();
    let alpha = |c: u8| c.is_ascii_alphabetic() && c != b'e' && c != b'E';
    for i in 1..b.len() {
        let (c, prev) = (b[i], b[i - 1]);
        if alpha(c) || alpha(prev) || (c == b'-' && prev != b'e' && prev != b'E') {
            out.push(&tok[from..i]);
            from = i;
        }
    }
    out.push(&tok[from..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const FUTURAL: &str = include_str!("../fonts/hershey/futural.jhf");

    #[test]
    fn jhf_parses_all_ascii() {
        let f = StrokeFont::from_jhf("futural", FUTURAL).unwrap();
        assert_eq!(f.glyph_count(), 95);
        let i = f.glyph('I').unwrap();
        assert_eq!(i.strokes.len(), 1, "'I' is a single bar");
        let ys: Vec<f64> = i.strokes[0].iter().map(|p| p.y).collect();
        assert!((ys[0] - 21.0).abs() < 1e-9 && ys[1].abs() < 1e-9, "{ys:?}");
    }

    #[test]
    fn jhf_metrics_and_spacing() {
        let f = StrokeFont::from_jhf("futural", FUTURAL).unwrap();
        assert!((f.glyph(' ').unwrap().advance - 16.0).abs() < 1e-9);
        for c in ' '..='~' {
            let g = f.glyph(c).unwrap();
            assert!(g.advance > 0.0, "{c:?}");
            for p in g.strokes.iter().flatten() {
                assert!(p.y > -8.0 && p.y < 26.0, "{c:?} {p:?}");
                assert!(p.x >= -1e-9 && p.x <= g.advance + 1e-9, "{c:?} {p:?}");
            }
        }
    }

    #[test]
    fn jhf_rejects_garbage() {
        assert!(StrokeFont::from_jhf("x", "12345  1JZ").is_err());
        assert!(StrokeFont::from_jhf("x", "").is_err());
        let non_ascii = "\u{e9}".repeat(12);
        assert!(StrokeFont::from_jhf("x", &non_ascii).is_err());
    }

    const SVG: &str = r#"<svg><defs><font id="f" horiz-adv-x="500">
        <font-face units-per-em="1000" ascent="800" descent="-200" cap-height="700"/>
        <missing-glyph horiz-adv-x="400" d="M0,0L300,700"/>
        <glyph unicode="L" horiz-adv-x="600" d="M100,700 L100,0 L500,0"/>
        <glyph unicode="=" d="M0 200h400m-400 200h400"/>
        <glyph unicode="o" d="M0,0 L100,0 L100,100 z"/>
        <glyph unicode=" "/>
        </font></defs></svg>"#;

    #[test]
    fn svg_font_parses_strokes() {
        let f = StrokeFont::from_svg_font("t", SVG).unwrap();
        assert_eq!(f.glyph_count(), 4);
        assert_eq!(f.units_per_em, 1000.0);
        assert_eq!(f.cap_height, 700.0);
        assert_eq!(f.descent, 200.0);
        let l = f.glyph('L').unwrap();
        assert_eq!(l.advance, 600.0);
        assert_eq!(l.strokes[0].len(), 3);
        let eq = f.glyph('=').unwrap();
        assert_eq!(eq.advance, 500.0, "default advance from <font>");
        assert_eq!(eq.strokes.len(), 2, "relative moveto starts a new stroke");
        assert_eq!(eq.strokes[1][0], Vec2::new(0.0, 400.0));
        let o = f.glyph('o').unwrap();
        assert_eq!(o.strokes[0].len(), 4, "z closes back to the start");
        assert!(f.glyph_or_missing('Z').is_some());
    }

    #[test]
    fn svg_font_rejects_curves_and_junk() {
        let curve = r#"<glyph unicode="a" horiz-adv-x="1" d="M0,0 C1,1 2,2 3,3"/>"#;
        assert!(StrokeFont::from_svg_font("t", curve).is_err());
        assert!(StrokeFont::from_svg_font("t", "not a font").is_err());
        let dangling = r#"<glyph unicode="a" horiz-adv-x="1" d="M0"/>"#;
        assert!(StrokeFont::from_svg_font("t", dangling).is_err());
    }
}
