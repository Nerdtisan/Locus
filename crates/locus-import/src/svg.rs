// SPDX-License-Identifier: AGPL-3.0-or-later

//! SVG → reference shapes (#241, SPEC §1 non-goals, §13 M4).
//!
//! Supported: `path` (all commands), `line`, `polyline`, `polygon`, `rect`
//! (with rounded corners), `circle`, `ellipse`, `g`, nested `svg` and `use`
//! (same-document `#id` references only), `transform` lists, `viewBox` and
//! length units. Everything else (text, images, filters, styles, clip paths)
//! is ignored. The result is in mm, Y-up, with the artwork's top-left at the
//! top of the page.
//!
//! Security: XML is parsed by `roxmltree`, which resolves only entities
//! declared in the file itself (bounded depth) and never fetches anything;
//! `href`s that are not `#id` are ignored. Work, node and shape counts and
//! nesting are all bounded.

use std::collections::HashMap;

use locus_doc::reference::ImportShape;
use locus_geom::Vec2;
use roxmltree::{Document, Node, ParsingOptions};

use crate::affine::{Affine, arc_segments, circular_arc, ellipse};
use crate::{ImportError, Imported, MAX_DEPTH, MAX_INPUT_BYTES};

/// Most XML nodes accepted.
const MAX_NODES: u32 = 1_000_000;
/// Most element visits (counts every `use` expansion).
const MAX_WORK: usize = 2_000_000;
/// CSS px per mm: 1 user unit with no other information is 1 px (96 dpi).
const MM_PER_PX: f64 = 25.4 / 96.0;
/// Closing gaps below this (mm) are snapped shut.
const JOIN_MM: f64 = 1e-6;

/// Parses `bytes` as SVG into shapes in mm.
pub fn import_svg(bytes: &[u8]) -> Result<Imported, ImportError> {
    if bytes.len() > MAX_INPUT_BYTES {
        return Err(ImportError::TooLarge);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| ImportError::BadSvg)?;
    let opts = ParsingOptions {
        // Affinity and others write a DOCTYPE; roxmltree never loads the
        // external subset, only internal entities (with a depth limit).
        allow_dtd: true,
        nodes_limit: MAX_NODES,
        ..ParsingOptions::default()
    };
    let doc = Document::parse_with_options(text, opts).map_err(|_| ImportError::BadSvg)?;
    let root = doc.root_element();
    if root.tag_name().name() != "svg" {
        return Err(ImportError::BadSvg);
    }
    let mut ids: HashMap<&str, Node> = HashMap::new();
    for n in doc.descendants().filter(|n| n.is_element()) {
        if let Some(id) = n.attribute("id") {
            ids.entry(id).or_insert(n);
        }
    }
    let mut w = Walker {
        out: Imported::default(),
        ids,
        work: 0,
    };
    let base = root_transform(root);
    w.element(root, &base, 0, true)?;
    if w.out.shapes.is_empty() {
        return Err(ImportError::Empty);
    }
    Ok(w.out)
}

// ---------------------------------------------------------------- units

/// A CSS length in user units; `None` for percentages and junk.
fn length(s: &str) -> Option<f64> {
    let (v, rest) = scan_number(s.trim())?;
    let k = match rest.trim() {
        "" | "px" => 1.0,
        "mm" => 96.0 / 25.4,
        "cm" => 96.0 / 2.54,
        "in" => 96.0,
        "pt" => 96.0 / 72.0,
        "pc" => 16.0,
        "Q" => 96.0 / 101.6,
        _ => return None,
    };
    Some(v * k)
}

fn attr_len(n: Node, name: &str) -> Option<f64> {
    n.attribute(name).and_then(length)
}

fn root_transform(root: Node) -> Affine {
    let vb = root.attribute("viewBox").and_then(|s| {
        let v: Vec<f64> = s
            .split(|c: char| c == ',' || c.is_whitespace())
            .filter(|t| !t.is_empty())
            .map(|t| t.parse::<f64>().ok().filter(|v| v.is_finite()))
            .collect::<Option<_>>()?;
        (v.len() == 4 && v[2] > 0.0 && v[3] > 0.0).then(|| (v[0], v[1], v[2], v[3]))
    });
    // Width/height in mm (px counts as CSS px); percentages are ignored.
    let w_mm = attr_len(root, "width")
        .map(|v| v * MM_PER_PX)
        .filter(|v| *v > 0.0);
    let h_mm = attr_len(root, "height")
        .map(|v| v * MM_PER_PX)
        .filter(|v| *v > 0.0);
    let none = root
        .attribute("preserveAspectRatio")
        .is_some_and(|s| s.trim() == "none");
    let (user_to_mm, height_mm) = match vb {
        Some((x, y, vw, vh)) => {
            let (sx, sy) = match (w_mm, h_mm) {
                (Some(w), Some(h)) => (w / vw, h / vh),
                (Some(w), None) => (w / vw, w / vw),
                (None, Some(h)) => (h / vh, h / vh),
                (None, None) => (MM_PER_PX, MM_PER_PX),
            };
            let (sx, sy) = if none {
                (sx, sy)
            } else {
                let s = sx.min(sy);
                (s, s)
            };
            let (full_w, full_h) = (w_mm.unwrap_or(vw * sx), h_mm.unwrap_or(vh * sy));
            // xMidYMid meet.
            let ox = (full_w - vw * sx) / 2.0;
            let oy = (full_h - vh * sy) / 2.0;
            (
                Affine::new(sx, 0.0, 0.0, sy, ox - x * sx, oy - y * sy),
                full_h,
            )
        }
        None => (Affine::scale(MM_PER_PX, MM_PER_PX), h_mm.unwrap_or(0.0)),
    };
    // SVG is Y-down; the document is Y-up.
    Affine::new(1.0, 0.0, 0.0, -1.0, 0.0, height_mm).then_after(&user_to_mm)
}

// -------------------------------------------------------------- scanning

/// Reads one SVG number from the start of `s`; returns it and the rest.
fn scan_number(s: &str) -> Option<(f64, &str)> {
    let b = s.as_bytes();
    let mut i = 0;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    let int_start = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    let mut digits = i - int_start;
    if i < b.len() && b[i] == b'.' {
        i += 1;
        let f = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        digits += i - f;
    }
    if digits == 0 {
        return None;
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        let ds = j;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        // "1em" style suffixes are not exponents: they need digits.
        if j > ds {
            i = j;
        }
    }
    let v: f64 = s[..i].parse().ok()?;
    v.is_finite().then_some((v, &s[i..]))
}

struct Scanner<'a> {
    s: &'a str,
}

impl Scanner<'_> {
    fn skip_sep(&mut self) {
        self.s = self
            .s
            .trim_start_matches(|c: char| c.is_whitespace() || c == ',');
    }
    fn peek(&self) -> Option<char> {
        self.s.chars().next()
    }
    fn at_end(&mut self) -> bool {
        self.skip_sep();
        self.s.is_empty()
    }
    fn number(&mut self) -> Option<f64> {
        self.skip_sep();
        let (v, rest) = scan_number(self.s)?;
        self.s = rest;
        Some(v)
    }
    fn nums<const N: usize>(&mut self) -> Option<[f64; N]> {
        let mut out = [0.0; N];
        for o in &mut out {
            *o = self.number()?;
        }
        Some(out)
    }
    /// Arc flags may be glued to what follows ("a1 1 0 00 1 1").
    fn flag(&mut self) -> Option<bool> {
        self.skip_sep();
        let f = match self.peek()? {
            '0' => false,
            '1' => true,
            _ => return None,
        };
        self.s = &self.s[1..];
        Some(f)
    }
    fn starts_number(&mut self) -> bool {
        self.skip_sep();
        matches!(self.peek(), Some(c) if c.is_ascii_digit() || matches!(c, '+' | '-' | '.'))
    }
}

fn parse_transform(s: &str) -> Option<Affine> {
    let mut m = Affine::IDENTITY;
    let mut rest = s.trim();
    while !rest.is_empty() {
        let open = rest.find('(')?;
        let close = rest.find(')')?;
        if close < open {
            return None;
        }
        let name = rest[..open].trim();
        let mut sc = Scanner {
            s: &rest[open + 1..close],
        };
        let mut args = Vec::new();
        while !sc.at_end() {
            args.push(sc.number()?);
            if args.len() > 6 {
                return None;
            }
        }
        let t = match (name, args.as_slice()) {
            ("matrix", [a, b, c, d, e, f]) => Affine::new(*a, *b, *c, *d, *e, *f),
            ("translate", [x]) => Affine::translate(*x, 0.0),
            ("translate", [x, y]) => Affine::translate(*x, *y),
            ("scale", [x]) => Affine::scale(*x, *x),
            ("scale", [x, y]) => Affine::scale(*x, *y),
            ("rotate", [a]) => Affine::rotate(*a),
            ("rotate", [a, cx, cy]) => Affine::translate(*cx, *cy)
                .then_after(&Affine::rotate(*a))
                .then_after(&Affine::translate(-cx, -cy)),
            ("skewX", [a]) => Affine::new(1.0, 0.0, a.to_radians().tan(), 1.0, 0.0, 0.0),
            ("skewY", [a]) => Affine::new(1.0, a.to_radians().tan(), 0.0, 1.0, 0.0, 0.0),
            _ => return None,
        };
        m = m.then_after(&t);
        rest = rest[close + 1..].trim_start_matches(|c: char| c.is_whitespace() || c == ',');
    }
    m.is_finite().then_some(m)
}

// ---------------------------------------------------------------- walker

struct Walker<'a, 'i> {
    out: Imported,
    ids: HashMap<&'a str, Node<'a, 'i>>,
    work: usize,
}

fn hidden(n: Node) -> bool {
    n.attribute("display").is_some_and(|v| v.trim() == "none")
        || n.attribute("style").is_some_and(|s| {
            s.split(';').any(|d| {
                let mut kv = d.splitn(2, ':');
                matches!((kv.next(), kv.next()), (Some(k), Some(v))
                    if k.trim() == "display" && v.trim() == "none")
            })
        })
}

impl<'a, 'i> Walker<'a, 'i> {
    fn element(
        &mut self,
        n: Node<'a, 'i>,
        parent: &Affine,
        depth: usize,
        is_root: bool,
    ) -> Result<(), ImportError> {
        self.work += 1;
        if self.work > MAX_WORK {
            return Err(ImportError::TooMuchGeometry);
        }
        if depth > MAX_DEPTH * 8 || hidden(n) {
            return Ok(());
        }
        let tag = n.tag_name().name();
        // Not rendered directly (referenced, or irrelevant to geometry).
        if matches!(
            tag,
            "defs"
                | "symbol"
                | "clipPath"
                | "mask"
                | "marker"
                | "pattern"
                | "metadata"
                | "style"
                | "script"
                | "foreignObject"
                | "title"
                | "desc"
                | "linearGradient"
                | "radialGradient"
                | "filter"
                | "text"
                | "image"
                | "switch"
        ) {
            if matches!(tag, "text" | "image") {
                self.out
                    .warn(format!("skipped <{tag}>: only vector geometry is imported"));
            }
            return Ok(());
        }
        let mut t = *parent;
        if !is_root && let Some(s) = n.attribute("transform") {
            match parse_transform(s) {
                Some(m) => t = parent.then_after(&m),
                None => {
                    self.out
                        .warn("skipped an element with an invalid transform");
                    return Ok(());
                }
            }
        }
        match tag {
            "svg" | "g" | "a" => {
                for c in n.children().filter(|c| c.is_element()) {
                    self.element(c, &t, depth + 1, false)?;
                }
            }
            "use" => self.use_element(n, &t, depth)?,
            "path" => {
                if let Some(d) = n.attribute("d") {
                    self.path_data(d, &t)?;
                }
            }
            "line" => {
                let g = |k| attr_len(n, k).unwrap_or(0.0);
                let mut b = PathBuilder::new(&t);
                b.move_to(Vec2::new(g("x1"), g("y1")));
                b.line_to(&mut self.out, Vec2::new(g("x2"), g("y2")))?;
            }
            "polyline" | "polygon" => self.poly(n, &t, tag == "polygon")?,
            "rect" => self.rect(n, &t)?,
            "circle" => {
                let (cx, cy) = (
                    attr_len(n, "cx").unwrap_or(0.0),
                    attr_len(n, "cy").unwrap_or(0.0),
                );
                if let Some(r) = attr_len(n, "r").filter(|r| *r > 0.0) {
                    self.ellipse(&t, Vec2::new(cx, cy), r, r)?;
                }
            }
            "ellipse" => {
                let (cx, cy) = (
                    attr_len(n, "cx").unwrap_or(0.0),
                    attr_len(n, "cy").unwrap_or(0.0),
                );
                let rx = attr_len(n, "rx").filter(|r| *r > 0.0);
                let ry = attr_len(n, "ry").filter(|r| *r > 0.0);
                let (rx, ry) = match (rx, ry) {
                    (Some(a), Some(b)) => (a, b),
                    (Some(a), None) | (None, Some(a)) => (a, a),
                    _ => return Ok(()),
                };
                self.ellipse(&t, Vec2::new(cx, cy), rx, ry)?;
            }
            _ => {}
        }
        Ok(())
    }

    fn use_element(
        &mut self,
        n: Node<'a, 'i>,
        t: &Affine,
        depth: usize,
    ) -> Result<(), ImportError> {
        // Same-document references only: never follow a URL (untrusted input).
        let href = n
            .attributes()
            .find(|a| a.name() == "href")
            .map(|a| a.value());
        let Some(target) = href
            .and_then(|h| h.strip_prefix('#'))
            .and_then(|id| self.ids.get(id).copied())
        else {
            self.out
                .warn("skipped a <use> that does not reference this file");
            return Ok(());
        };
        let off = Affine::translate(
            attr_len(n, "x").unwrap_or(0.0),
            attr_len(n, "y").unwrap_or(0.0),
        );
        let t = t.then_after(&off);
        // `symbol` renders only through `use`; draw its children.
        if target.tag_name().name() == "symbol" {
            for c in target.children().filter(|c| c.is_element()) {
                self.element(c, &t, depth + 1, false)?;
            }
            Ok(())
        } else {
            self.element(target, &t, depth + 1, false)
        }
    }

    fn path_data(&mut self, d: &str, t: &Affine) -> Result<(), ImportError> {
        let mut sc = Scanner { s: d };
        let mut b = PathBuilder::new(t);
        let mut cmd = None::<char>;
        let mut first = true;
        loop {
            sc.skip_sep();
            let Some(c) = sc.peek() else { break };
            if c.is_ascii_alphabetic() {
                cmd = Some(c);
                sc.s = &sc.s[c.len_utf8()..];
                if matches!(c, 'Z' | 'z') {
                    if first {
                        break;
                    }
                    b.close(&mut self.out)?;
                    continue;
                }
            } else if !sc.starts_number() {
                break;
            }
            let Some(c) = cmd else { break };
            // The first command must be a moveto; numbers after Z need a
            // command of their own.
            if (first && !matches!(c, 'M' | 'm')) || matches!(c, 'Z' | 'z') {
                break;
            }
            first = false;
            match run_command(&mut self.out, &mut b, &mut sc, c) {
                Some(r) => r?,
                None => {
                    self.out.warn("stopped reading a path at invalid data");
                    break;
                }
            }
            if matches!(c, 'M' | 'm') {
                // Further coordinate pairs are implicit linetos.
                cmd = Some(if c == 'm' { 'l' } else { 'L' });
            }
        }
        Ok(())
    }

    fn poly(&mut self, n: Node, t: &Affine, close: bool) -> Result<(), ImportError> {
        let Some(pts) = n.attribute("points") else {
            return Ok(());
        };
        let mut sc = Scanner { s: pts };
        let mut b = PathBuilder::new(t);
        let mut first = true;
        while !sc.at_end() {
            let Some([x, y]) = sc.nums::<2>() else {
                break;
            };
            let p = Vec2::new(x, y);
            if first {
                b.move_to(p);
                first = false;
            } else {
                b.line_to(&mut self.out, p)?;
            }
        }
        if close {
            b.close(&mut self.out)?;
        }
        Ok(())
    }

    fn rect(&mut self, n: Node, t: &Affine) -> Result<(), ImportError> {
        let g = |k| attr_len(n, k).unwrap_or(0.0);
        let (x, y) = (g("x"), g("y"));
        let (Some(w), Some(h)) = (
            attr_len(n, "width").filter(|v| *v > 0.0),
            attr_len(n, "height").filter(|v| *v > 0.0),
        ) else {
            return Ok(());
        };
        let rx = attr_len(n, "rx").filter(|v| *v > 0.0);
        let ry = attr_len(n, "ry").filter(|v| *v > 0.0);
        let (rx, ry) = match (rx, ry) {
            (Some(a), Some(b)) => (a, b),
            (Some(a), None) | (None, Some(a)) => (a, a),
            _ => (0.0, 0.0),
        };
        let (rx, ry) = (rx.min(w / 2.0), ry.min(h / 2.0));
        let mut b = PathBuilder::new(t);
        let o = &mut self.out;
        if rx <= 0.0 || ry <= 0.0 {
            b.move_to(Vec2::new(x, y));
            b.line_to(o, Vec2::new(x + w, y))?;
            b.line_to(o, Vec2::new(x + w, y + h))?;
            b.line_to(o, Vec2::new(x, y + h))?;
            b.close(o)?;
        } else {
            b.move_to(Vec2::new(x + rx, y));
            b.line_to(o, Vec2::new(x + w - rx, y))?;
            b.arc_to(o, rx, ry, 0.0, false, true, Vec2::new(x + w, y + ry))?;
            b.line_to(o, Vec2::new(x + w, y + h - ry))?;
            b.arc_to(o, rx, ry, 0.0, false, true, Vec2::new(x + w - rx, y + h))?;
            b.line_to(o, Vec2::new(x + rx, y + h))?;
            b.arc_to(o, rx, ry, 0.0, false, true, Vec2::new(x, y + h - ry))?;
            b.line_to(o, Vec2::new(x, y + ry))?;
            b.arc_to(o, rx, ry, 0.0, false, true, Vec2::new(x + rx, y))?;
            b.close(o)?;
        }
        Ok(())
    }

    fn ellipse(&mut self, t: &Affine, c: Vec2, rx: f64, ry: f64) -> Result<(), ImportError> {
        if (rx - ry).abs() <= 1e-12 * rx.max(ry)
            && let Some(s) = t.similarity()
        {
            return self.out.push(ImportShape::Circle {
                center: t.apply(c),
                radius: rx * s,
            });
        }
        let mut shapes = Vec::new();
        ellipse(t, c, rx, ry, &mut shapes);
        for s in shapes {
            self.out.push(s)?;
        }
        Ok(())
    }
}

/// Runs one path command (one set of arguments). `None`: malformed data.
fn run_command(
    out: &mut Imported,
    b: &mut PathBuilder,
    sc: &mut Scanner,
    c: char,
) -> Option<Result<(), ImportError>> {
    let rel = c.is_ascii_lowercase();
    let cur = b.cur;
    let base = if rel { cur } else { Vec2::new(0.0, 0.0) };
    let pt = |x: f64, y: f64| Vec2::new(base.x + x, base.y + y);
    Some(match c.to_ascii_uppercase() {
        'M' => {
            let [x, y] = sc.nums::<2>()?;
            b.move_to(pt(x, y));
            Ok(())
        }
        'L' => {
            let [x, y] = sc.nums::<2>()?;
            b.line_to(out, pt(x, y))
        }
        'H' => {
            let [x] = sc.nums::<1>()?;
            b.line_to(out, Vec2::new(base.x + x, cur.y))
        }
        'V' => {
            let [y] = sc.nums::<1>()?;
            b.line_to(out, Vec2::new(cur.x, base.y + y))
        }
        'C' => {
            let [x1, y1, x2, y2, x, y] = sc.nums::<6>()?;
            b.cubic_to(out, pt(x1, y1), pt(x2, y2), pt(x, y))
        }
        'S' => {
            let [x2, y2, x, y] = sc.nums::<4>()?;
            let c1 = b.reflected_cubic();
            b.cubic_to(out, c1, pt(x2, y2), pt(x, y))
        }
        'Q' => {
            let [x1, y1, x, y] = sc.nums::<4>()?;
            b.quad_to(out, pt(x1, y1), pt(x, y))
        }
        'T' => {
            let [x, y] = sc.nums::<2>()?;
            let q = b.reflected_quad();
            b.quad_to(out, q, pt(x, y))
        }
        'A' => {
            let [rx, ry, rot] = sc.nums::<3>()?;
            let large = sc.flag()?;
            let sweep = sc.flag()?;
            let [x, y] = sc.nums::<2>()?;
            b.arc_to(out, rx, ry, rot, large, sweep, pt(x, y))
        }
        _ => return None,
    })
}

// ----------------------------------------------------------- path builder

/// Tracks the current point of a path in local (user) space and emits shapes
/// mapped through `t`.
struct PathBuilder<'t> {
    t: &'t Affine,
    cur: Vec2,
    start: Vec2,
    /// Output length when the current subpath got its first shape.
    sub_first: Option<usize>,
    last_c2: Option<Vec2>,
    last_q: Option<Vec2>,
}

impl<'t> PathBuilder<'t> {
    fn new(t: &'t Affine) -> Self {
        let z = Vec2::new(0.0, 0.0);
        PathBuilder {
            t,
            cur: z,
            start: z,
            sub_first: None,
            last_c2: None,
            last_q: None,
        }
    }

    fn move_to(&mut self, p: Vec2) {
        self.cur = p;
        self.start = p;
        self.sub_first = None;
        self.last_c2 = None;
        self.last_q = None;
    }

    fn mark(&mut self, out: &Imported) {
        if self.sub_first.is_none() {
            self.sub_first = Some(out.shapes.len());
        }
    }

    fn reflected_cubic(&self) -> Vec2 {
        match self.last_c2 {
            Some(c) => Vec2::new(2.0 * self.cur.x - c.x, 2.0 * self.cur.y - c.y),
            None => self.cur,
        }
    }

    fn reflected_quad(&self) -> Vec2 {
        match self.last_q {
            Some(c) => Vec2::new(2.0 * self.cur.x - c.x, 2.0 * self.cur.y - c.y),
            None => self.cur,
        }
    }

    fn line_to(&mut self, out: &mut Imported, p: Vec2) -> Result<(), ImportError> {
        let (a, b) = (self.t.apply(self.cur), self.t.apply(p));
        self.mark(out);
        self.last_c2 = None;
        self.last_q = None;
        self.cur = p;
        if a == b {
            return Ok(());
        }
        out.push(ImportShape::Line { a, b })
    }

    fn cubic_to(
        &mut self,
        out: &mut Imported,
        c1: Vec2,
        c2: Vec2,
        p: Vec2,
    ) -> Result<(), ImportError> {
        self.mark(out);
        let r = out.push(ImportShape::Cubic {
            a: self.t.apply(self.cur),
            c1: self.t.apply(c1),
            c2: self.t.apply(c2),
            b: self.t.apply(p),
        });
        self.cur = p;
        self.last_c2 = Some(c2);
        self.last_q = None;
        r
    }

    fn quad_to(&mut self, out: &mut Imported, q: Vec2, p: Vec2) -> Result<(), ImportError> {
        // Exact degree elevation.
        let c = self.cur;
        let c1 = Vec2::new(c.x + 2.0 / 3.0 * (q.x - c.x), c.y + 2.0 / 3.0 * (q.y - c.y));
        let c2 = Vec2::new(p.x + 2.0 / 3.0 * (q.x - p.x), p.y + 2.0 / 3.0 * (q.y - p.y));
        self.cubic_to(out, c1, c2, p)?;
        self.last_c2 = None;
        self.last_q = Some(q);
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn arc_to(
        &mut self,
        out: &mut Imported,
        rx: f64,
        ry: f64,
        rot_deg: f64,
        large: bool,
        sweep: bool,
        p: Vec2,
    ) -> Result<(), ImportError> {
        let p1 = self.cur;
        let (mut rx, mut ry) = (rx.abs(), ry.abs());
        if p1 == p {
            return Ok(());
        }
        if rx == 0.0 || ry == 0.0 {
            return self.line_to(out, p);
        }
        // SVG implementation notes F.6.5: endpoint → centre parameterisation.
        let phi = rot_deg.to_radians();
        let (sp, cp) = phi.sin_cos();
        let (dx, dy) = ((p1.x - p.x) / 2.0, (p1.y - p.y) / 2.0);
        let x1p = cp * dx + sp * dy;
        let y1p = -sp * dx + cp * dy;
        let lambda = (x1p / rx).powi(2) + (y1p / ry).powi(2);
        if lambda > 1.0 {
            let s = lambda.sqrt();
            rx *= s;
            ry *= s;
        }
        let num = rx * rx * ry * ry - rx * rx * y1p * y1p - ry * ry * x1p * x1p;
        let den = rx * rx * y1p * y1p + ry * ry * x1p * x1p;
        let mut coef = if den > 0.0 {
            (num / den).max(0.0).sqrt()
        } else {
            0.0
        };
        if large == sweep {
            coef = -coef;
        }
        let cxp = coef * rx * y1p / ry;
        let cyp = -coef * ry * x1p / rx;
        let cx = cp * cxp - sp * cyp + (p1.x + p.x) / 2.0;
        let cy = sp * cxp + cp * cyp + (p1.y + p.y) / 2.0;
        let ux = (x1p - cxp) / rx;
        let uy = (y1p - cyp) / ry;
        let vx = (-x1p - cxp) / rx;
        let vy = (-y1p - cyp) / ry;
        let theta1 = uy.atan2(ux);
        let mut dtheta = vy.atan2(vx) - theta1;
        if !sweep && dtheta > 0.0 {
            dtheta -= std::f64::consts::TAU;
        } else if sweep && dtheta < 0.0 {
            dtheta += std::f64::consts::TAU;
        }
        let centre = Vec2::new(cx, cy);
        self.mark(out);
        self.last_c2 = None;
        self.last_q = None;
        self.cur = p;
        if !(cx.is_finite() && cy.is_finite() && dtheta.is_finite()) {
            return Ok(());
        }
        if (rx - ry).abs() <= 1e-9 * rx.max(ry) && self.t.similarity().is_some() {
            // A true circular arc, with the file's own end points so paths
            // stay connected.
            return out.push(circular_arc(self.t, centre, p1, p, dtheta > 0.0));
        }
        let mut segs = Vec::new();
        arc_segments(self.t, centre, rx, ry, phi, theta1, dtheta, &mut segs);
        if let Some(ImportShape::Cubic { a, .. }) = segs.first_mut() {
            *a = self.t.apply(p1);
        }
        if let Some(ImportShape::Cubic { b, .. }) = segs.last_mut() {
            *b = self.t.apply(p);
        }
        for s in segs {
            out.push(s)?;
        }
        Ok(())
    }

    fn close(&mut self, out: &mut Imported) -> Result<(), ImportError> {
        let (a, b) = (self.t.apply(self.cur), self.t.apply(self.start));
        let first = self.sub_first;
        self.cur = self.start;
        self.last_c2 = None;
        self.last_q = None;
        self.sub_first = None;
        if a == b {
            return Ok(());
        }
        let dist = (a.x - b.x).hypot(a.y - b.y);
        if dist < JOIN_MM {
            // Float noise from relative coordinates: snap the end shut so the
            // outline stays one connected path.
            if first.is_some_and(|f| f < out.shapes.len())
                && let Some(last) = out.shapes.last_mut()
            {
                snap_nearest_end(last, a, b);
            }
            return Ok(());
        }
        out.push(ImportShape::Line { a, b })
    }
}

fn snap_nearest_end(s: &mut ImportShape, from: Vec2, to: Vec2) {
    let d = |p: Vec2| (p.x - from.x).hypot(p.y - from.y);
    match s {
        ImportShape::Line { a, b } | ImportShape::Cubic { a, b, .. } => {
            if d(*b) <= d(*a) {
                *b = to;
            } else {
                *a = to;
            }
        }
        ImportShape::Arc { start, end, .. } => {
            if d(*end) <= d(*start) {
                *end = to;
            } else {
                *start = to;
            }
        }
        ImportShape::Circle { .. } => {}
    }
}
