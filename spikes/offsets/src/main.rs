// SPDX-License-Identifier: AGPL-3.0-or-later

//! Runs the fixtures, the randomised robustness sweeps and the timings, and
//! prints Markdown tables for ADR 0010.
//!
//! `cargo run --release -- <n>` (n cases per sweep, default 2000).

use core::f64::consts::PI;
use locus_region::{Area, BoolOp, Edge, Loop, Seg, boolean};
use offsets_spike::geom::*;
use offsets_spike::metrics::{Score, score, score_open};
use offsets_spike::METHODS;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::Instant;

struct Fx {
    name: String,
    a: Area,
    d: f64,
}

fn fx(name: &str, a: Area, ds: &[f64]) -> Vec<Fx> {
    ds.iter().map(|&d| Fx { name: format!("{name}, d = {d:+}"), a: a.clone(), d }).collect()
}

fn nested() -> Area {
    area(vec![
        rounded(0.0, 0.0, 100.0, 60.0, 5.0),
        reversed(&rect(20.0, 10.0, 80.0, 50.0)),
        circle(v(50.0, 30.0), 10.0),
        reversed(&slot(v(5.0, 55.0), v(15.0, 55.0), 2.0)),
    ])
}

fn plate_holes() -> Area {
    let mut loops = vec![rounded(0.0, 0.0, 110.0, 60.0, 5.0)];
    for i in 0..10 {
        for j in 0..5 {
            loops.push(reversed(&circle(v(10.0 + 10.0 * f64::from(i), 10.0 + 10.0 * f64::from(j)), 2.0)));
        }
    }
    area(loops)
}

fn fixtures() -> Vec<Fx> {
    let mut f = Vec::new();
    f.extend(fx("rect 40×20", area(vec![rect(0.0, 0.0, 40.0, 20.0)]), &[1.0, -1.0, -10.0]));
    f.extend(fx("star, 12 points (tight inside corners)", area(vec![star(v(0.0, 0.0), 12, 20.0, 8.0)]), &[1.0, -1.0, -3.0]));
    f.extend(fx("comb, 0.3 mm notches", area(vec![comb(40.0, 10.0, 9, 0.3, 4.0)]), &[0.15, 0.2, -0.2]));
    f.extend(fx("rounded rect, r 0.5 (small radii)", area(vec![rounded(0.0, 0.0, 30.0, 20.0, 0.5)]), &[1.0, -0.5, -1.0]));
    f.extend(fx("circle r 1", area(vec![circle(v(0.0, 0.0), 1.0)]), &[-0.999, -1.0, 2.0]));
    f.extend(fx("slot r 3", area(vec![slot(v(0.0, 0.0), v(20.0, 0.0), 3.0)]), &[2.0, -2.9, -3.0]));
    f.extend(fx("nested: plate, hole, island, slot hole", nested(), &[0.1, -0.1, 5.0, 6.0]));
    f.extend(fx("gear, 60 teeth", gear(30.0, 60, 1.2), &[0.5, -0.5, -1.0]));
    f.extend(fx("plate − 50 holes", plate_holes(), &[0.1, -0.1]));
    f.extend(fx("gear, 240 teeth (480 arcs)", gear(100.0, 240, 1.2), &[0.5, -0.5]));
    f
}

struct OpenFx {
    name: &'static str,
    chain: Vec<Edge>,
    d: f64,
}

fn open_fixtures() -> Vec<OpenFx> {
    let zig = polyline(&[v(0.0, 0.0), v(10.0, 3.0), v(1.0, 6.0), v(11.0, 9.0), v(2.0, 12.0)]);
    let j = chain(vec![
        Seg::Line { a: v(0.0, 10.0), b: v(0.0, 0.0) },
        Seg::Arc { center: v(1.0, 0.0), radius: 1.0, start: PI, sweep: PI },
        Seg::Line { a: v(2.0, 0.0), b: v(2.0, 5.0) },
    ]);
    let wave = chain(
        (0..6)
            .map(|i| {
                let x = 4.0 * f64::from(i);
                let s = if i % 2 == 0 { -1.0 } else { 1.0 };
                Seg::Arc { center: v(x + 2.0, 0.0), radius: 2.0, start: PI, sweep: s * PI }
            })
            .collect(),
    );
    vec![
        OpenFx { name: "zigzag (sharp turns), left 1", chain: zig.clone(), d: 1.0 },
        OpenFx { name: "zigzag (sharp turns), right 1", chain: zig, d: -1.0 },
        OpenFx { name: "J hook, r 1 arc, inside 2 (arc collapses)", chain: j.clone(), d: -2.0 },
        OpenFx { name: "J hook, outside 2", chain: j, d: 2.0 },
        OpenFx { name: "S-wave of arcs, left 1", chain: wave.clone(), d: 1.0 },
        OpenFx { name: "S-wave of arcs, right 2.5", chain: wave, d: -2.5 },
    ]
}

fn run<T>(f: impl FnOnce() -> T) -> Option<T> {
    catch_unwind(AssertUnwindSafe(f)).ok()
}

fn time_it(mut f: impl FnMut()) -> f64 {
    let t = Instant::now();
    let mut n = 0;
    while n < 3 || (t.elapsed().as_secs_f64() < 0.2 && n < 200) {
        f();
        n += 1;
    }
    t.elapsed().as_secs_f64() / f64::from(n) * 1e3
}

fn cell(s: &Option<Score>, ms: f64) -> String {
    match s {
        None => "**PANIC**".into(),
        Some(s) => {
            let verdict = if s.pass { "pass".to_string() } else { "**FAIL**".to_string() };
            let mut why = Vec::new();
            if s.dev > locus_geom::EXPORT_TOL {
                why.push(format!("dev {:.1e}", s.dev));
            }
            if s.wrong > 0 {
                why.push(format!("{} wrong", s.wrong));
            }
            if s.open {
                why.push("open loop".into());
            }
            if s.slivers > 0 {
                why.push(format!("{} sliver", s.slivers));
            }
            let why = if why.is_empty() { String::new() } else { format!(" ({})", why.join(", ")) };
            format!("{verdict}{why} · {}/{} · {} loops · {ms:.2} ms", s.arcs, s.lines, s.loops)
        }
    }
}

/// xorshift64*: deterministic, no dependency.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
    }
    fn range(&mut self, a: f64, b: f64) -> f64 {
        a + (b - a) * self.next()
    }
}

/// A random area: a primitive combined with one or two more. On the grid,
/// coordinates and d are multiples of 0.5 / 0.25, so exact touching,
/// collapsing and shared offset curves are common.
fn random_case(r: &mut Rng, grid: bool) -> (Area, f64) {
    let q = |r: &mut Rng, a: f64, b: f64| {
        let x = r.range(a, b);
        if grid { (x * 2.0).round() / 2.0 } else { x }
    };
    let prim = |r: &mut Rng| -> Loop {
        let (x, y) = (q(r, 0.0, 20.0), q(r, 0.0, 20.0));
        let (w, h) = (q(r, 1.0, 15.0).max(0.5), q(r, 1.0, 15.0).max(0.5));
        match (r.next() * 5.0) as u32 {
            0 => rect(x, y, x + w, y + h),
            1 => circle(v(x, y), w / 2.0),
            2 => slot(v(x, y), v(x + w, y + h - 7.5), (h / 4.0).max(0.5)),
            3 => rounded(x, y, x + w.max(2.0), y + h.max(2.0), q(r, 0.5, 1.0).min(w.max(2.0) / 2.0)),
            _ => star(v(x, y), 3 + (r.next() * 6.0) as usize, w.max(1.0), w.max(1.0) * r.range(0.3, 0.8)),
        }
    };
    let mut a = area(vec![prim(r)]);
    for _ in 0..(1 + (r.next() * 2.0) as u32) {
        let b = area(vec![prim(r)]);
        let op = match (r.next() * 3.0) as u32 {
            0 => BoolOp::Union,
            1 => BoolOp::Subtract,
            _ => BoolOp::Intersect,
        };
        let c = boolean(&a, &b, op);
        if !c.is_empty() {
            a = c;
        }
    }
    let mut d = if grid { (r.range(-3.0, 3.0) * 4.0).round() / 4.0 } else { r.range(-3.0, 3.0) };
    if d == 0.0 {
        d = 0.25;
    }
    (a, d)
}

fn main() {
    if std::env::args().nth(1).as_deref() == Some("prof") { prof(); return; }
    let n: usize = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(2000);
    if std::env::args().nth(1).as_deref() == Some("case") {
        let sweep: u64 = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(0);
        let k: usize = std::env::args().nth(3).and_then(|s| s.parse().ok()).unwrap_or(0);
        let mut r = Rng(0x9E37_79B9_7F4A_7C15 ^ (sweep + 1));
        let mut c = random_case(&mut r, sweep != 2);
        for _ in 0..k {
            c = random_case(&mut r, sweep != 2);
        }
        println!("{:#?}\nd = {}", c.0, c.1);
        for m in METHODS {
            let out = m.offset(&c.0, c.1);
            println!("{}: {:?}\n{:#?}", m.name(), score(&c.0, c.1, &out, 0.01, 200), out);
        }
        return;
    }

    println!("## Fixtures (closed areas, round joins)\n");
    println!("Cells: verdict · arcs/lines · loops · time per offset.\n");
    println!("| Fixture | {} | {} |", METHODS[0].name(), METHODS[1].name());
    println!("|---|---|---|");
    for f in fixtures() {
        let mut row = format!("| {} |", f.name);
        for m in METHODS {
            let out = run(|| m.offset(&f.a, f.d));
            let s = out.as_ref().map(|o| score(&f.a, f.d, o, 0.01, 200));
            let ms = if out.is_some() { time_it(|| drop(m.offset(&f.a, f.d))) } else { 0.0 };
            row += &format!(" {} |", cell(&s, ms));
        }
        println!("{row}");
    }

    println!("\n## Fixtures (open chains, one side)\n");
    println!("| Fixture | {} | {} |", METHODS[0].name(), METHODS[1].name());
    println!("|---|---|---|");
    for f in open_fixtures() {
        let mut row = format!("| {} |", f.name);
        for m in METHODS {
            let out = run(|| m.offset_open(&f.chain, f.d));
            let s = out.as_ref().map(|o| score_open(&f.chain, f.d, o, 0.01));
            let ms = if out.is_some() { time_it(|| drop(m.offset_open(&f.chain, f.d))) } else { 0.0 };
            row += &format!(" {} |", cell(&s, ms).replace(" loops", " runs"));
        }
        println!("{row}");
    }

    println!("\n## Randomised sweeps, {n} cases each\n");
    println!("| Sweep | {} | {} |", METHODS[0].name(), METHODS[1].name());
    println!("|---|---|---|");
    for (sweep, label) in [(0u64, "0.5 mm grid, d in 0.25 steps (exact touching / collapse)"), (1, "grid shapes, d in 0.25 steps (second seed)"), (2, "general position (random floats)")] {
        let mut r = Rng(0x9E37_79B9_7F4A_7C15 ^ (sweep + 1));
        let cases: Vec<(Area, f64)> = (0..n).map(|_| random_case(&mut r, sweep != 2)).collect();
        let mut row = format!("| {label} |");
        for m in METHODS {
            let (mut pass, mut fail, mut panics, mut worst, mut ms) = (0, 0, 0, 0.0f64, 0.0);
            let mut first_fail = None;
            for (i, (a, d)) in cases.iter().enumerate() {
                let t = Instant::now();
                let out = run(|| m.offset(a, *d));
                ms += t.elapsed().as_secs_f64() * 1e3;
                match out {
                    None => panics += 1,
                    Some(o) => {
                        let s = score(a, *d, &o, 0.05, 60);
                        if s.pass {
                            pass += 1;
                        } else {
                            fail += 1;
                            worst = worst.max(s.dev);
                            first_fail.get_or_insert(i);
                        }
                    }
                }
            }
            row += &format!(
                " {pass} pass / {fail} fail / {panics} panic · worst failing dev {worst:.1e} · mean {:.2} ms{} |",
                ms / n as f64,
                first_fail.map(|i| format!(" · first fail #{i}")).unwrap_or_default()
            );
        }
        println!("{row}");
    }
}
#[allow(dead_code)]
pub fn prof() {
    let g = gear(100.0, 240, 1.2);
    let _ = offsets_spike::own::offset(&g, -0.5, offsets_spike::own::Join::Round);
}
