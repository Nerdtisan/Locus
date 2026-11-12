// SPDX-License-Identifier: MIT OR Apache-2.0

//! HPGL exporter and importer: a first-party example plugin (SPEC §13 M3,
//! §15.2, #185, #233).
//!
//! Writes HP-GL for pen plotters and vinyl cutters: one pen per layer (the
//! layer's pen number for Draw layers, otherwise pens 1–8 in layer order),
//! lines as `PA`, true arcs and circles as `AA`, in plotter units of
//! 0.025 mm. Locus's y axis points up, like HP-GL's, so coordinates are used
//! as they are. The host does the file I/O; this only turns the validated
//! export model into bytes (§15.4).
//!
//! The importer reads the same subset back (`IN`, `SP`, `PU`, `PD`, `PA`,
//! `PR`, `AA`, `CI`; other instructions are skipped) as reference geometry:
//! each pen-down run becomes one path.

use locus_plugin_api::prelude::*;

/// HP-GL plotter units per millimetre.
const UNITS_PER_MM: f64 = 40.0;

fn xy(p: &Vec2) -> String {
    let u = |v: f64| (v * UNITS_PER_MM).round() as i64;
    format!("{},{}", u(p.x), u(p.y))
}

/// Where a segment starts, taking `reversed` into account.
fn start(seg: &Segment) -> Vec2 {
    match (&seg.shape, seg.reversed) {
        (Shape::Line(l), false) => l.a,
        (Shape::Line(l), true) => l.b,
        (Shape::Arc(a), false) => a.start,
        (Shape::Arc(a), true) => a.end,
        (Shape::Circle(c), _) => Vec2 {
            x: c.center.x + c.radius,
            y: c.center.y,
        },
    }
}

/// Counter-clockwise sweep from `a` to `b` around `c`, in degrees, (0, 360].
fn sweep(c: &Vec2, a: &Vec2, b: &Vec2) -> f64 {
    let a0 = (a.y - c.y).atan2(a.x - c.x);
    let a1 = (b.y - c.y).atan2(b.x - c.x);
    let s = (a1 - a0).rem_euclid(std::f64::consts::TAU).to_degrees();
    if s <= 0.0 { 360.0 } else { s }
}

fn segment(out: &mut String, seg: &Segment) {
    let dir = if seg.reversed { -1.0 } else { 1.0 };
    match &seg.shape {
        Shape::Line(l) => {
            let end = if seg.reversed { &l.a } else { &l.b };
            out.push_str(&format!("PA{};\n", xy(end)));
        }
        Shape::Arc(a) => {
            let deg = sweep(&a.center, &a.start, &a.end) * dir;
            out.push_str(&format!("AA{},{deg:.3};\n", xy(&a.center)));
        }
        Shape::Circle(c) => {
            out.push_str(&format!("AA{},{:.3};\n", xy(&c.center), 360.0 * dir));
        }
    }
}

fn pen(index: usize, layer: &Layer) -> u32 {
    match &layer.operation {
        Operation::Draw(d) if d.pen_number > 0 => d.pen_number,
        _ => (index % 8) as u32 + 1,
    }
}

struct Hpgl;

impl InfoGuest for Hpgl {
    fn describe() -> PluginInfo {
        PluginInfo {
            id: "io.locus.hpgl".into(),
            name: "HPGL".into(),
            version: "1.1.0".into(),
        }
    }
}

impl ExporterGuest for Hpgl {
    fn format() -> FormatInfo {
        FormatInfo {
            id: "hpgl".into(),
            name: "HPGL".into(),
            extension: "plt".into(),
            mime: "application/vnd.hp-hpgl".into(),
        }
    }

    fn export(model: ExportModel) -> Result<Vec<u8>, PluginError> {
        let mut out = String::from("IN;\n");
        for (i, layer) in model.layers.iter().enumerate() {
            if layer.paths.is_empty() {
                continue;
            }
            out.push_str(&format!("SP{};\n", pen(i, layer)));
            for path in &layer.paths {
                let Some(first) = path.segments.first() else {
                    continue;
                };
                out.push_str(&format!("PU{};\nPD;\n", xy(&start(first))));
                for seg in &path.segments {
                    segment(&mut out, seg);
                }
                out.push_str("PU;\n");
            }
        }
        out.push_str("SP0;\n");
        Ok(out.into_bytes())
    }
}

// ── Import (#233) ───────────────────────────────────────────────────────────

/// Most segments one import may produce (the host's own bound is 100 000).
const MAX_SEGMENTS: usize = 100_000;

fn mm(units: f64) -> f64 {
    units / UNITS_PER_MM
}

fn unsupported(msg: &str) -> PluginError {
    PluginError::Unsupported(msg.into())
}

/// Turns pen moves into paths: each pen-down run becomes one path.
struct Plotter {
    at: Vec2,
    down: bool,
    relative: bool,
    run: Vec<Segment>,
    run_start: Vec2,
    paths: Vec<Path>,
    segments: usize,
}

impl Plotter {
    fn new() -> Plotter {
        let origin = Vec2 { x: 0.0, y: 0.0 };
        Plotter {
            at: origin,
            down: false,
            relative: false,
            run: Vec::new(),
            run_start: origin,
            paths: Vec::new(),
            segments: 0,
        }
    }

    fn push(&mut self, seg: Segment) -> Result<(), PluginError> {
        self.segments += 1;
        if self.segments > MAX_SEGMENTS {
            return Err(unsupported("the file has too many segments"));
        }
        if self.run.is_empty() {
            self.run_start = self.at;
        }
        self.run.push(seg);
        Ok(())
    }

    /// Ends the current pen-down run.
    fn lift(&mut self) {
        if self.run.is_empty() {
            return;
        }
        let closed = (self.at.x - self.run_start.x).abs() < 1e-9
            && (self.at.y - self.run_start.y).abs() < 1e-9;
        self.paths.push(Path {
            segments: std::mem::take(&mut self.run),
            closed,
        });
    }

    fn move_to(&mut self, to: Vec2) -> Result<(), PluginError> {
        if self.down {
            if to.x != self.at.x || to.y != self.at.y {
                self.push(Segment {
                    shape: Shape::Line(Line { a: self.at, b: to }),
                    reversed: false,
                })?;
            }
        } else {
            self.lift();
        }
        self.at = to;
        Ok(())
    }

    /// Moves through coordinate pairs, absolute or relative.
    fn moves(&mut self, nums: &[f64]) -> Result<(), PluginError> {
        if nums.len() % 2 != 0 {
            return Err(unsupported("a move has an odd number of coordinates"));
        }
        for xy in nums.chunks(2) {
            let (x, y) = (mm(xy[0]), mm(xy[1]));
            let to = if self.relative {
                Vec2 {
                    x: self.at.x + x,
                    y: self.at.y + y,
                }
            } else {
                Vec2 { x, y }
            };
            self.move_to(to)?;
        }
        Ok(())
    }

    /// `AA cx,cy,sweep`: an arc from the pen position around the centre,
    /// counter-clockwise for a positive sweep. A full turn is a circle.
    fn arc(&mut self, nums: &[f64]) -> Result<(), PluginError> {
        let [cx, cy, sweep, ..] = *nums else {
            return Err(unsupported("AA needs a centre and a sweep"));
        };
        let center = Vec2 {
            x: mm(cx),
            y: mm(cy),
        };
        let (dx, dy) = (self.at.x - center.x, self.at.y - center.y);
        let radius = dx.hypot(dy);
        if radius <= 0.0 || sweep == 0.0 {
            return Ok(());
        }
        if sweep.abs() >= 360.0 {
            if self.down {
                self.lift();
                self.push(Segment {
                    shape: Shape::Circle(Circle { center, radius }),
                    reversed: sweep < 0.0,
                })?;
                self.lift();
            }
            return Ok(());
        }
        let a = dy.atan2(dx) + sweep.to_radians();
        let end = Vec2 {
            x: center.x + radius * a.cos(),
            y: center.y + radius * a.sin(),
        };
        if self.down {
            // Arcs run counter-clockwise; a clockwise sweep is the CCW arc
            // from `end` back to the pen, traversed in reverse.
            let (start, stop) = if sweep > 0.0 {
                (self.at, end)
            } else {
                (end, self.at)
            };
            self.push(Segment {
                shape: Shape::Arc(Arc {
                    center,
                    start,
                    end: stop,
                }),
                reversed: sweep < 0.0,
            })?;
        } else {
            self.lift();
        }
        self.at = end;
        Ok(())
    }

    /// `CI r`: a circle around the pen position; the pen stays where it is.
    fn circle(&mut self, nums: &[f64]) -> Result<(), PluginError> {
        let Some(r) = nums.first() else {
            return Err(unsupported("CI needs a radius"));
        };
        let radius = mm(r.abs());
        if radius > 0.0 {
            self.lift();
            self.push(Segment {
                shape: Shape::Circle(Circle {
                    center: self.at,
                    radius,
                }),
                reversed: false,
            })?;
            self.lift();
        }
        Ok(())
    }
}

/// Comma- or space-separated numbers, finite and of plotter size.
fn numbers(args: &str) -> Result<Vec<f64>, PluginError> {
    args.split([',', ' '])
        .filter(|a| !a.is_empty())
        .map(|a| {
            a.parse::<f64>()
                .ok()
                .filter(|v| v.is_finite() && v.abs() < 1e9)
                .ok_or_else(|| unsupported("a coordinate is not a number"))
        })
        .collect()
}

impl ImporterGuest for Hpgl {
    fn extensions() -> Vec<String> {
        vec!["plt".into(), "hpgl".into(), "hgl".into()]
    }

    fn import(bytes: Vec<u8>) -> Result<Vec<Path>, PluginError> {
        let text =
            std::str::from_utf8(&bytes).map_err(|_| unsupported("not an HP-GL text file"))?;
        let mut p = Plotter::new();
        for instr in text.split([';', '\n', '\r']) {
            let instr = instr.trim();
            if instr.len() < 2 || !instr.is_char_boundary(2) {
                continue;
            }
            let (op, args) = instr.split_at(2);
            match op.to_ascii_uppercase().as_str() {
                "IN" => {
                    p.lift();
                    p.at = Vec2 { x: 0.0, y: 0.0 };
                    p.down = false;
                    p.relative = false;
                }
                "PU" => {
                    p.down = false;
                    p.lift();
                    p.moves(&numbers(args)?)?;
                }
                "PD" => {
                    p.down = true;
                    p.moves(&numbers(args)?)?;
                }
                "PA" => {
                    p.relative = false;
                    p.moves(&numbers(args)?)?;
                }
                "PR" => {
                    p.relative = true;
                    p.moves(&numbers(args)?)?;
                }
                "AA" => p.arc(&numbers(args)?)?,
                "CI" => p.circle(&numbers(args)?)?,
                "SP" => p.lift(),
                // Labels, line types, speeds…: not geometry.
                _ => {}
            }
        }
        p.lift();
        if p.paths.is_empty() {
            return Err(unsupported("the file has no pen-down geometry"));
        }
        Ok(p.paths)
    }
}

locus_plugin_api::export_plugin!(Hpgl: exporter, importer);
