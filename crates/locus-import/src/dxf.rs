// SPDX-License-Identifier: AGPL-3.0-or-later

//! ASCII DXF → reference shapes (#242, SPEC §1 non-goals, §13 M4).
//!
//! Supported: `LINE`, `ARC`, `CIRCLE`, `ELLIPSE`, `LWPOLYLINE` and R12
//! `POLYLINE`/`VERTEX` (with bulges), `SPLINE` (sampled into short lines) and
//! `INSERT` (blocks flattened, with scale, rotation and column/row arrays).
//! Units come from `$INSUNITS` (none: mm). Layers, colours, text, hatches and
//! dimensions are ignored; skipped entity kinds are reported as warnings.
//!
//! Security: the text is read as bounded group-code pairs, nothing is
//! evaluated or fetched, block nesting and instance counts are limited, and
//! malformed values skip the entity instead of failing the file.

use std::collections::HashMap;

use locus_doc::reference::ImportShape;
use locus_geom::Vec2;

use crate::affine::{Affine, arc_segments, circular_arc};
use crate::{ImportError, Imported, MAX_DEPTH, MAX_INPUT_BYTES};

/// Most group-code pairs read (a hostile file of one-byte pairs).
const MAX_PAIRS: usize = 4_000_000;
/// Most entity visits, counting every block expansion.
const MAX_WORK: usize = 2_000_000;
/// Most copies one INSERT array makes.
const MAX_ARRAY: usize = 10_000;
/// Samples per spline knot span.
const SPLINE_SAMPLES: usize = 12;
/// Highest spline degree evaluated.
const MAX_DEGREE: usize = 11;

type Pair<'a> = (i32, &'a str);

/// Parses `bytes` as ASCII DXF into shapes in mm.
pub fn import_dxf(bytes: &[u8]) -> Result<Imported, ImportError> {
    if bytes.len() > MAX_INPUT_BYTES {
        return Err(ImportError::TooLarge);
    }
    if bytes.starts_with(b"AutoCAD Binary DXF") {
        return Err(ImportError::BinaryDxf);
    }
    // R12 files are ANSI: keep going on bad UTF-8.
    let text = String::from_utf8_lossy(bytes);
    let pairs = read_pairs(&text)?;
    let groups = split_groups(&pairs);
    if !groups.iter().any(|g| g.kind == "SECTION") {
        return Err(ImportError::BadDxf);
    }

    let mut unit_scale = 1.0;
    let mut blocks: HashMap<String, Block> = HashMap::new();
    let mut top: Vec<Ent> = Vec::new();
    let mut section = "";
    let mut open_block: Option<(String, Block)> = None;
    for g in &groups {
        match g.kind {
            "SECTION" => {
                section = g.str_of(2).unwrap_or("");
                if section == "HEADER" {
                    unit_scale = insunits_scale(g.d);
                }
            }
            "ENDSEC" => section = "",
            "BLOCK" if section == "BLOCKS" => {
                let base = Vec2::new(g.num(10).unwrap_or(0.0), g.num(20).unwrap_or(0.0));
                let name = g.str_of(2).unwrap_or("").to_ascii_uppercase();
                open_block = Some((
                    name,
                    Block {
                        base,
                        ents: Vec::new(),
                    },
                ));
            }
            "ENDBLK" => {
                if let Some((name, b)) = open_block.take() {
                    blocks.entry(name).or_insert(b);
                }
            }
            "EOF" => break,
            _ => match section {
                "ENTITIES" => top.push(*g),
                "BLOCKS" => {
                    if let Some((_, b)) = &mut open_block {
                        b.ents.push(*g);
                    }
                }
                _ => {}
            },
        }
    }

    let mut cx = Ctx {
        out: Imported::default(),
        blocks: &blocks,
        skipped: HashMap::new(),
        work: 0,
    };
    cx.emit(&top, &Affine::scale(unit_scale, unit_scale), 0)?;
    let mut skipped: Vec<_> = cx.skipped.into_iter().collect();
    skipped.sort();
    for (kind, n) in skipped {
        cx.out
            .warn(format!("skipped {n} {kind} entities: not supported"));
    }
    if cx.out.shapes.is_empty() {
        return Err(ImportError::Empty);
    }
    Ok(cx.out)
}

// ------------------------------------------------------------ reading

fn read_pairs(text: &str) -> Result<Vec<Pair<'_>>, ImportError> {
    let mut pairs = Vec::new();
    let mut lines = text.lines();
    while let (Some(c), Some(v)) = (lines.next(), lines.next()) {
        let Ok(code) = c.trim().parse::<i32>() else {
            return Err(ImportError::BadDxf);
        };
        if pairs.len() >= MAX_PAIRS {
            return Err(ImportError::TooLarge);
        }
        pairs.push((code, v.trim()));
    }
    Ok(pairs)
}

/// One entity (or section marker): its type and the pairs up to the next
/// code 0.
#[derive(Clone, Copy)]
struct Ent<'a> {
    kind: &'a str,
    d: &'a [Pair<'a>],
}

impl<'a> Ent<'a> {
    fn str_of(&self, code: i32) -> Option<&'a str> {
        self.d.iter().find(|p| p.0 == code).map(|p| p.1)
    }
    fn num(&self, code: i32) -> Option<f64> {
        self.d
            .iter()
            .find(|p| p.0 == code)
            .and_then(|p| p.1.parse::<f64>().ok())
            .filter(|v| v.is_finite())
    }
    fn nums(&self, code: i32) -> impl Iterator<Item = f64> + 'a {
        self.d
            .iter()
            .filter(move |p| p.0 == code)
            .filter_map(|p| p.1.parse::<f64>().ok().filter(|v| v.is_finite()))
    }
    fn flags(&self) -> i64 {
        self.num(70).map_or(0, |v| v as i64)
    }
    fn pt(&self, cx: i32, cy: i32) -> Option<Vec2> {
        Some(Vec2::new(self.num(cx)?, self.num(cy)?))
    }
    /// Object coordinate system mirroring: extrusion (0,0,-1) flips X.
    fn ocs(&self, m: &Affine) -> Affine {
        if self.num(230).is_some_and(|z| z < 0.0) {
            m.then_after(&Affine::scale(-1.0, 1.0))
        } else {
            *m
        }
    }
}

fn split_groups<'a>(pairs: &'a [Pair<'a>]) -> Vec<Ent<'a>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < pairs.len() {
        if pairs[i].0 == 0 {
            let start = i + 1;
            let mut j = start;
            while j < pairs.len() && pairs[j].0 != 0 {
                j += 1;
            }
            out.push(Ent {
                kind: pairs[i].1,
                d: &pairs[start..j],
            });
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

/// mm per drawing unit from the header's `$INSUNITS` (absent or 0: mm).
fn insunits_scale(header: &[Pair]) -> f64 {
    let v = header
        .windows(2)
        .find(|w| w[0].0 == 9 && w[0].1 == "$INSUNITS")
        .and_then(|w| w[1].1.parse::<i64>().ok())
        .unwrap_or(0);
    match v {
        1 => 25.4,
        2 => 304.8,
        3 => 1_609_344.0,
        5 => 10.0,
        6 => 1000.0,
        7 => 1.0e6,
        10 => 914.4,
        13 => 0.001,
        14 => 100.0,
        15 => 10_000.0,
        16 => 100_000.0,
        _ => 1.0,
    }
}

struct Block<'a> {
    base: Vec2,
    ents: Vec<Ent<'a>>,
}

// ------------------------------------------------------------ emitting

struct Ctx<'a, 'b> {
    out: Imported,
    blocks: &'b HashMap<String, Block<'a>>,
    skipped: HashMap<String, usize>,
    work: usize,
}

impl Ctx<'_, '_> {
    fn emit(&mut self, ents: &[Ent], m: &Affine, depth: usize) -> Result<(), ImportError> {
        let mut i = 0;
        while i < ents.len() {
            self.work += 1;
            if self.work > MAX_WORK {
                return Err(ImportError::TooMuchGeometry);
            }
            let e = ents[i];
            i += 1;
            match e.kind {
                "LINE" => {
                    if let (Some(a), Some(b)) = (e.pt(10, 20), e.pt(11, 21)) {
                        let (a, b) = (m.apply(a), m.apply(b));
                        if a != b {
                            self.out.push(ImportShape::Line { a, b })?;
                        }
                    }
                }
                "CIRCLE" => {
                    if let (Some(c), Some(r)) = (e.pt(10, 20), e.num(40))
                        && r > 0.0
                    {
                        self.circle(&e.ocs(m), c, r)?;
                    }
                }
                "ARC" => self.arc(&e, m)?,
                "ELLIPSE" => self.ellipse(&e, m)?,
                "LWPOLYLINE" => {
                    let mut v: Vec<(Vec2, f64)> = Vec::new();
                    let mut x = None;
                    for &(code, val) in e.d {
                        let n = val.parse::<f64>().ok().filter(|n| n.is_finite());
                        match (code, n) {
                            (10, Some(n)) => {
                                x = Some(n);
                            }
                            (20, Some(n)) => {
                                if let Some(x) = x.take() {
                                    v.push((Vec2::new(x, n), 0.0));
                                }
                            }
                            (42, Some(n)) => {
                                if let Some(last) = v.last_mut() {
                                    last.1 = n;
                                }
                            }
                            _ => {}
                        }
                    }
                    self.polyline(&e.ocs(m), &v, e.flags() & 1 != 0)?;
                }
                "POLYLINE" => {
                    let mut v: Vec<(Vec2, f64)> = Vec::new();
                    while i < ents.len() && ents[i].kind != "SEQEND" {
                        self.work += 1;
                        if self.work > MAX_WORK {
                            return Err(ImportError::TooMuchGeometry);
                        }
                        let ve = ents[i];
                        i += 1;
                        // Skip spline-frame control points (16) and anything
                        // that is not a plain 2-D vertex.
                        if ve.kind != "VERTEX" || ve.flags() & 16 != 0 {
                            continue;
                        }
                        if let Some(p) = ve.pt(10, 20) {
                            v.push((p, ve.num(42).unwrap_or(0.0)));
                        }
                    }
                    // 16: polygon mesh, 64: polyface mesh: not 2-D curves.
                    if e.flags() & (16 | 64) != 0 {
                        *self.skipped.entry("3D mesh".to_owned()).or_default() += 1;
                    } else {
                        self.polyline(&e.ocs(m), &v, e.flags() & 1 != 0)?;
                    }
                }
                "SPLINE" => self.spline(&e, m)?,
                "INSERT" => self.insert(&e, m, depth)?,
                "VERTEX" | "SEQEND" => {}
                other => *self.skipped.entry(other.to_owned()).or_default() += 1,
            }
        }
        Ok(())
    }

    fn circle(&mut self, m: &Affine, c: Vec2, r: f64) -> Result<(), ImportError> {
        if let Some(s) = m.similarity() {
            return self.out.push(ImportShape::Circle {
                center: m.apply(c),
                radius: r * s,
            });
        }
        self.beziers(m, c, r, r, 0.0, 0.0, std::f64::consts::TAU)
    }

    #[allow(clippy::too_many_arguments)]
    fn beziers(
        &mut self,
        m: &Affine,
        c: Vec2,
        rx: f64,
        ry: f64,
        phi: f64,
        t0: f64,
        dt: f64,
    ) -> Result<(), ImportError> {
        let mut segs = Vec::new();
        arc_segments(m, c, rx, ry, phi, t0, dt, &mut segs);
        for s in segs {
            self.out.push(s)?;
        }
        Ok(())
    }

    fn arc(&mut self, e: &Ent, m: &Affine) -> Result<(), ImportError> {
        let (Some(c), Some(r), Some(a1), Some(a2)) =
            (e.pt(10, 20), e.num(40), e.num(50), e.num(51))
        else {
            return Ok(());
        };
        if r <= 0.0 {
            return Ok(());
        }
        let m = e.ocs(m);
        let mut da = (a2 - a1).rem_euclid(360.0);
        if da < 1e-9 {
            da = 360.0;
        }
        if da >= 360.0 - 1e-9 {
            return self.circle(&m, c, r);
        }
        let at = |deg: f64| {
            let (s, co) = deg.to_radians().sin_cos();
            Vec2::new(c.x + r * co, c.y + r * s)
        };
        if m.similarity().is_some() {
            return self
                .out
                .push(circular_arc(&m, c, at(a1), at(a1 + da), true));
        }
        self.beziers(&m, c, r, r, 0.0, a1.to_radians(), da.to_radians())
    }

    fn ellipse(&mut self, e: &Ent, m: &Affine) -> Result<(), ImportError> {
        let (Some(c), Some(major), Some(ratio)) = (e.pt(10, 20), e.pt(11, 21), e.num(40)) else {
            return Ok(());
        };
        let rx = major.x.hypot(major.y);
        let ry = rx * ratio;
        if !(rx > 0.0 && ry > 0.0) {
            return Ok(());
        }
        let phi = major.y.atan2(major.x);
        let t0 = e.num(41).unwrap_or(0.0);
        let mut dt =
            (e.num(42).unwrap_or(std::f64::consts::TAU) - t0).rem_euclid(std::f64::consts::TAU);
        if dt < 1e-9 {
            dt = std::f64::consts::TAU;
        }
        self.beziers(&e.ocs(m), c, rx, ry, phi, t0, dt)
    }

    fn polyline(&mut self, m: &Affine, v: &[(Vec2, f64)], closed: bool) -> Result<(), ImportError> {
        let n = v.len();
        let segs = if closed { n } else { n.saturating_sub(1) };
        for i in 0..segs {
            let (p0, b) = v[i];
            let p1 = v[(i + 1) % n].0;
            self.segment(m, p0, p1, b)?;
        }
        Ok(())
    }

    /// A line, or the arc a bulge describes (tan of a quarter of the
    /// included angle; positive: counter-clockwise).
    fn segment(&mut self, m: &Affine, p0: Vec2, p1: Vec2, bulge: f64) -> Result<(), ImportError> {
        if p0 == p1 {
            return Ok(());
        }
        if bulge.abs() < 1e-12 || !bulge.is_finite() {
            return self.out.push(ImportShape::Line {
                a: m.apply(p0),
                b: m.apply(p1),
            });
        }
        let (dx, dy) = (p1.x - p0.x, p1.y - p0.y);
        let chord = dx.hypot(dy);
        // Centre on the chord's left for a counter-clockwise minor arc.
        let off = chord / 2.0 * (1.0 - bulge * bulge) / (2.0 * bulge);
        let c = Vec2::new(
            (p0.x + p1.x) / 2.0 - dy / chord * off,
            (p0.y + p1.y) / 2.0 + dx / chord * off,
        );
        if m.similarity().is_some() {
            return self.out.push(circular_arc(m, c, p0, p1, bulge > 0.0));
        }
        let r = (p0.x - c.x).hypot(p0.y - c.y);
        let t0 = (p0.y - c.y).atan2(p0.x - c.x);
        self.beziers(m, c, r, r, 0.0, t0, 4.0 * bulge.atan())
    }

    fn spline(&mut self, e: &Ent, m: &Affine) -> Result<(), ImportError> {
        let xs: Vec<f64> = e.nums(10).collect();
        let ys: Vec<f64> = e.nums(20).collect();
        let n = xs.len().min(ys.len());
        let degree = e
            .num(71)
            .map_or(3, |d| d as i64)
            .clamp(1, MAX_DEGREE as i64) as usize;
        let knots: Vec<f64> = e.nums(40).collect();
        let mut weights: Vec<f64> = e.nums(41).collect();
        if weights.len() != n {
            weights = vec![1.0; n];
        }
        let pts: Vec<Vec2> = (0..n).map(|i| Vec2::new(xs[i], ys[i])).collect();
        let mut poly: Vec<Vec2> = Vec::new();
        if n > degree && knots.len() == n + degree + 1 && knots.windows(2).all(|w| w[0] <= w[1]) {
            let (lo, hi) = (knots[degree], knots[n]);
            for span in degree..n {
                let (a, b) = (knots[span], knots[span + 1]);
                if b - a <= 0.0 {
                    continue;
                }
                for s in 0..=SPLINE_SAMPLES {
                    let u = (a + (b - a) * s as f64 / SPLINE_SAMPLES as f64).clamp(lo, hi);
                    if let Some(p) = de_boor(degree, &knots, &pts, &weights, span, u)
                        && poly.last() != Some(&p)
                    {
                        poly.push(p);
                    }
                }
            }
        } else {
            // No usable control net: fall back to the fit points.
            let fx: Vec<f64> = e.nums(11).collect();
            let fy: Vec<f64> = e.nums(21).collect();
            poly = fx.iter().zip(&fy).map(|(x, y)| Vec2::new(*x, *y)).collect();
        }
        if poly.len() < 2 {
            *self.skipped.entry("SPLINE".to_owned()).or_default() += 1;
            return Ok(());
        }
        self.out
            .warn("approximated a SPLINE with straight segments");
        let closed = e.flags() & 1 != 0;
        let v: Vec<(Vec2, f64)> = poly.into_iter().map(|p| (p, 0.0)).collect();
        self.polyline(m, &v, closed)
    }

    fn insert(&mut self, e: &Ent, m: &Affine, depth: usize) -> Result<(), ImportError> {
        if depth >= MAX_DEPTH {
            self.out.warn("skipped blocks nested too deeply");
            return Ok(());
        }
        let name = e.str_of(2).unwrap_or("").to_ascii_uppercase();
        let Some(block) = self.blocks.get(&name) else {
            self.out.warn("skipped an INSERT of a missing block");
            return Ok(());
        };
        let ins = e.pt(10, 20).unwrap_or(Vec2::new(0.0, 0.0));
        let (sx, sy) = (e.num(41).unwrap_or(1.0), e.num(42).unwrap_or(1.0));
        let rot = e.num(50).unwrap_or(0.0);
        let cols = e.num(70).map_or(1, |v| v as i64).clamp(1, MAX_ARRAY as i64) as usize;
        let rows = e.num(71).map_or(1, |v| v as i64).clamp(1, MAX_ARRAY as i64) as usize;
        let (dc, dr) = (e.num(44).unwrap_or(0.0), e.num(45).unwrap_or(0.0));
        if cols * rows > MAX_ARRAY {
            self.out.warn("limited a block array to 10000 copies");
        }
        let m = e.ocs(m);
        let local =
            Affine::scale(sx, sy).then_after(&Affine::translate(-block.base.x, -block.base.y));
        'grid: for r in 0..rows {
            for c in 0..cols {
                if r * cols + c >= MAX_ARRAY {
                    break 'grid;
                }
                let t = m
                    .then_after(&Affine::translate(ins.x, ins.y))
                    .then_after(&Affine::rotate(rot))
                    .then_after(&Affine::translate(c as f64 * dc, r as f64 * dr))
                    .then_after(&local);
                if t.is_finite() {
                    self.emit(&block.ents, &t, depth + 1)?;
                }
            }
        }
        Ok(())
    }
}

/// Weighted point of a clamped or open B-spline at `u` (de Boor).
fn de_boor(p: usize, knots: &[f64], ctrl: &[Vec2], w: &[f64], k: usize, u: f64) -> Option<Vec2> {
    // Homogeneous control points for the p+1 influencing the span.
    let mut d: Vec<[f64; 3]> = (0..=p)
        .map(|j| {
            let i = k + j - p;
            [ctrl[i].x * w[i], ctrl[i].y * w[i], w[i]]
        })
        .collect();
    for r in 1..=p {
        for j in (r..=p).rev() {
            let i = k + j - p;
            let den = knots[i + p + 1 - r] - knots[i];
            let a = if den.abs() < 1e-300 {
                0.0
            } else {
                (u - knots[i]) / den
            };
            let prev = d[j - 1];
            for (cur, pv) in d[j].iter_mut().zip(prev) {
                *cur = (1.0 - a) * pv + a * *cur;
            }
        }
    }
    let h = d[p];
    (h[2].abs() > 1e-300)
        .then(|| Vec2::new(h[0] / h[2], h[1] / h[2]))
        .filter(|v| v.x.is_finite() && v.y.is_finite())
}
