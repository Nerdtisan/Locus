// SPDX-License-Identifier: AGPL-3.0-or-later

//! Runs the fixtures, the randomised robustness sweep and the timings, and
//! prints Markdown tables for ADR 0008.

use booleans_spike::geom::{Loop, P, Shape, circle, p, rect, reversed, rounded_rect, slot};
use booleans_spike::metrics::{reference, score};
use booleans_spike::own::Op;
use booleans_spike::{METHODS, Method};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::Instant;

struct Fixture {
    name: &'static str,
    a: Shape,
    b: Shape,
    op: Op,
    /// Minimal exact result (arcs, lines), where known by hand.
    expect: Option<(usize, usize)>,
}

fn s(l: Loop) -> Shape {
    Shape::new(vec![l])
}

fn fixtures() -> Vec<Fixture> {
    let f = |name, a, b, op, expect| Fixture { name, a, b, op, expect };
    vec![
        f("rect ∪ circle on corner", s(rect(0.0, 0.0, 20.0, 10.0)), s(circle(p(20.0, 10.0), 5.0)), Op::Union, Some((1, 4))),
        f("slot − circle over cap", s(slot(p(0.0, 0.0), p(20.0, 0.0), 5.0)), s(circle(p(22.0, 3.0), 4.0)), Op::Subtract, None),
        f("circle ∩ circle (lens)", s(circle(p(0.0, 0.0), 10.0)), s(circle(p(12.0, 0.0), 10.0)), Op::Intersect, Some((2, 0))),
        f("rect ∪ rect, shared edge", s(rect(0.0, 0.0, 10.0, 10.0)), s(rect(10.0, 0.0, 20.0, 10.0)), Op::Union, Some((0, 4))),
        f("rect ∪ rect, partial shared edge", s(rect(0.0, 0.0, 10.0, 10.0)), s(rect(10.0, 5.0, 20.0, 15.0)), Op::Union, Some((0, 8))),
        f("rect ∪ rect, touching corner", s(rect(0.0, 0.0, 10.0, 10.0)), s(rect(10.0, 10.0, 20.0, 20.0)), Op::Union, Some((0, 8))),
        f("rect − rect, notch on edge", s(rect(0.0, 0.0, 20.0, 10.0)), s(rect(5.0, 5.0, 15.0, 10.0)), Op::Subtract, Some((0, 8))),
        f("circle − concentric circle", s(circle(p(0.0, 0.0), 10.0)), s(circle(p(0.0, 0.0), 5.0)), Op::Subtract, Some((2, 0))),
        f("circle ∪ circle, external tangent", s(circle(p(0.0, 0.0), 5.0)), s(circle(p(10.0, 0.0), 5.0)), Op::Union, Some((2, 0))),
        f("circle − circle, internal tangent", s(circle(p(0.0, 0.0), 10.0)), s(circle(p(5.0, 0.0), 5.0)), Op::Subtract, Some((2, 0))),
        f("slot ∪ circle on its cap (coincident arc)", s(slot(p(0.0, 0.0), p(20.0, 0.0), 5.0)), s(circle(p(20.0, 0.0), 5.0)), Op::Union, Some((2, 2))),
        f("circle ∪ same circle", s(circle(p(3.0, 4.0), 7.0)), s(circle(p(3.0, 4.0), 7.0)), Op::Union, Some((1, 0))),
        f("rect ∩ circle, tangent at a point", s(rect(0.0, 0.0, 10.0, 10.0)), s(circle(p(5.0, 15.0), 5.0)), Op::Intersect, Some((0, 0))),
        f("rounded rect − slot inside (hole)", s(rounded_rect(0.0, 0.0, 40.0, 20.0, 4.0)), s(slot(p(10.0, 10.0), p(30.0, 10.0), 3.0)), Op::Subtract, Some((6, 6))),
        f(
            "annulus ∪ bar (holed operand)",
            Shape::new(vec![circle(p(0.0, 0.0), 10.0), reversed(&circle(p(0.0, 0.0), 5.0))]),
            s(rect(-2.0, -12.0, 2.0, 12.0)),
            Op::Union,
            Some((4, 8)),
        ),
    ]
}

fn stress() -> Vec<Fixture> {
    let holes: Vec<Loop> = (0..10).flat_map(|i| (0..5).map(move |j| circle(p(10.0 + 10.0 * f64::from(i), 10.0 + 10.0 * f64::from(j)), 2.0))).collect();
    let chain: Vec<Loop> = (1..40).map(|i| circle(p(4.0 * f64::from(i), 0.0), 3.0)).collect();
    let teeth: Vec<Loop> = (0..60)
        .map(|i| {
            let a = f64::from(i) * std::f64::consts::TAU / 60.0;
            circle(p(30.0 * a.cos(), 30.0 * a.sin()), 1.2)
        })
        .collect();
    vec![
        Fixture { name: "plate − 50 holes", a: s(rounded_rect(0.0, 0.0, 110.0, 60.0, 5.0)), b: Shape::new(holes), op: Op::Subtract, expect: Some((54, 4)) },
        Fixture { name: "chain of 40 circles ∪", a: s(circle(p(0.0, 0.0), 3.0)), b: Shape::new(chain), op: Op::Union, expect: Some((78, 0)) },
        Fixture { name: "disc ∪ 60 teeth", a: s(circle(p(0.0, 0.0), 30.0)), b: Shape::new(teeth), op: Op::Union, expect: Some((120, 0)) },
    ]
}

/// cavalier combines one pline with another: multi-loop operands are folded
/// in loop by loop (enough for these stress fixtures).
fn cav_fold(fx: &Fixture) -> Option<Shape> {
    if fx.b.loops.len() == 1 {
        return Method::Cavalier.run(&fx.a, &fx.b, fx.op);
    }
    let mut outer = fx.a.clone();
    let mut holes = Vec::new();
    for l in &fx.b.loops {
        let r = Method::Cavalier.run(&outer, &s(l.clone()), fx.op)?;
        let (pos, neg): (Vec<Loop>, Vec<Loop>) = r.loops.into_iter().partition(|l| booleans_spike::geom::loop_area(l) > 0.0);
        if pos.len() != 1 {
            return None;
        }
        outer = Shape::new(pos);
        holes.extend(neg);
    }
    outer.loops.extend(holes);
    Some(outer)
}

fn run(m: Method, fx: &Fixture) -> Option<Shape> {
    if m == Method::Cavalier { cav_fold(fx) } else { m.run(&fx.a, &fx.b, fx.op) }
}

fn time_us(f: &dyn Fn()) -> f64 {
    f();
    let t = Instant::now();
    let mut n = 0u32;
    while t.elapsed().as_millis() < 200 && n < 2000 {
        f();
        n += 1;
    }
    t.elapsed().as_secs_f64() * 1e6 / f64::from(n)
}

fn fmt_dev(d: f64) -> String {
    if d.is_infinite() { "∞".into() } else if d < 1e-9 { "<1e-9".into() } else { format!("{d:.1e}") }
}

fn table(title: &str, fxs: &[Fixture]) {
    println!("\n### {title}\n");
    println!("| Fixture | Option | Pass | Dev out→ref (mm) | Dev ref→out (mm) | ΔArea (mm²) | Arcs / lines (expected) | Loops | µs |");
    println!("|---|---|---|---|---|---|---|---|---|");
    for fx in fxs {
        let r = reference(&fx.a, &fx.b, fx.op);
        for m in METHODS {
            let exp = fx.expect.map_or("?".into(), |(a, l)| format!("{a} / {l}"));
            match catch_unwind(AssertUnwindSafe(|| run(m, fx))) {
                Err(_) => println!("| {} | {} | PANIC | | | | | | |", fx.name, m.name()),
                Ok(None) => println!("| {} | {} | n/a | | | | | | |", fx.name, m.name()),
                Ok(Some(out)) => {
                    let sc = score(&out, &r);
                    let us = time_us(&|| {
                        let _ = run(m, fx);
                    });
                    println!(
                        "| {} | {} | {} | {} | {} | {:.1e} | {} / {} ({exp}) | {} | {us:.0} |",
                        fx.name,
                        m.name(),
                        if sc.pass() { "yes" } else if sc.slivers > 0 { "NO (sliver)" } else if !sc.closed { "NO (open)" } else { "NO" },
                        fmt_dev(sc.dev),
                        fmt_dev(sc.dev_back),
                        sc.darea,
                        sc.arcs,
                        sc.lines,
                        sc.loops
                    );
                }
            }
        }
    }
}

struct Rng(u64, bool);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    /// Multiple of 0.5 in [lo, hi].
    fn grid(&mut self, lo: f64, hi: f64) -> f64 {
        if self.1 {
            return lo + (hi - lo) * self.unit();
        }
        let n = ((hi - lo) / 0.5) as u64 + 1;
        lo + 0.5 * (self.next() % n) as f64
    }
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}

fn random_loop(r: &mut Rng) -> Loop {
    match r.next() % 4 {
        0 => {
            let (x, y) = (r.grid(0.0, 8.0), r.grid(0.0, 8.0));
            rect(x, y, x + r.grid(0.5, 6.0), y + r.grid(0.5, 6.0))
        }
        1 => circle(p(r.grid(0.0, 10.0), r.grid(0.0, 10.0)), r.grid(0.5, 4.0)),
        2 => {
            let c1 = p(r.grid(0.0, 10.0), r.grid(0.0, 10.0));
            let mut c2 = p(r.grid(0.0, 10.0), r.grid(0.0, 10.0));
            if c2.dist(c1) < 0.5 {
                c2 = c1 + p(2.0, 0.0);
            }
            slot(c1, c2, r.grid(0.5, 3.0))
        }
        _ => {
            let rr = r.grid(0.5, 2.0);
            let (x, y) = (r.grid(0.0, 6.0), r.grid(0.0, 6.0));
            rounded_rect(x, y, x + 2.0 * rr + r.grid(0.5, 6.0), y + 2.0 * rr + r.grid(0.5, 6.0), rr)
        }
    }
}

fn shift(l: &Loop, d: P) -> Loop {
    use booleans_spike::geom::Seg;
    l.iter()
        .map(|s| match *s {
            Seg::Line { a, b } => Seg::Line { a: a + d, b: b + d },
            Seg::Arc { c, r, a0, sweep } => Seg::Arc { c: c + d, r, a0, sweep },
        })
        .collect()
}

fn sweep(label: &str, cases: usize, jitter: f64, general: bool, only: Option<usize>) {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15, general);
    let mut pass = [0usize; 4];
    let mut panics = [0usize; 4];
    let mut na = [0usize; 4];
    let mut worst = [0f64; 4];
    let mut arcs = [0usize; 4];
    let mut lines = [0usize; 4];
    let mut fails: Vec<Vec<String>> = vec![Vec::new(); 4];
    for case in 0..cases {
        let a = s(random_loop(&mut rng));
        let d = p((rng.unit() * 2.0 - 1.0) * jitter, (rng.unit() * 2.0 - 1.0) * jitter);
        let b = s(shift(&random_loop(&mut rng), d));
        let op = [Op::Union, Op::Intersect, Op::Subtract][(rng.next() % 3) as usize];
        if only.is_some_and(|o| o != case) {
            continue;
        }
        let r = reference(&a, &b, op);
        if only.is_some() {
            println!("A = {a:?}
B = {b:?}
op = {op:?}");
            let refs: Vec<(usize, f64)> = r.iter().map(|c| (c.len(), booleans_spike::metrics::poly_area(c))).collect();
            println!("reference contours (points, area): {refs:?}");
            let out = booleans_spike::own::boolean(&a, &b, op);
            for l in &out.loops {
                println!("C loop area {:.6}: {l:?}", booleans_spike::geom::loop_area(l));
            }
        }
        for (k, m) in METHODS.iter().enumerate() {
            match catch_unwind(AssertUnwindSafe(|| m.run(&a, &b, op))) {
                Err(_) => panics[k] += 1,
                Ok(None) => na[k] += 1,
                Ok(Some(out)) => {
                    let sc = score(&out, &r);
                    arcs[k] += sc.arcs;
                    lines[k] += sc.lines;
                    if sc.pass() {
                        pass[k] += 1;
                    } else {
                        if fails[k].len() < 3 {
                            fails[k].push(format!("case {case} {op:?} dev {:.1e} slivers {} closed {}", sc.dev, sc.slivers, sc.closed));
                        }
                    }
                    if sc.dev.is_finite() {
                        worst[k] = worst[k].max(sc.dev);
                    }
                }
            }
        }
    }
    println!("\n### Random sweep: {label} ({cases} cases)\n");
    println!("| Option | Pass | Fail | Panic | n/a | Worst finite dev (mm) | Arcs / lines total |");
    println!("|---|---|---|---|---|---|---|");
    for (k, m) in METHODS.iter().enumerate() {
        let fail = cases - pass[k] - panics[k] - na[k];
        println!("| {} | {} | {fail} | {} | {} | {} | {} / {} |", m.name(), pass[k], panics[k], na[k], fmt_dev(worst[k]), arcs[k], lines[k]);
    }
    for (k, m) in METHODS.iter().enumerate() {
        for f in &fails[k] {
            eprintln!("{label} {}: {f}", m.name());
        }
    }
}

fn main() {
    std::panic::set_hook(Box::new(|_| {}));
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("case") {
        let (k, idx): (usize, usize) = (args[2].parse().unwrap_or(0), args[3].parse().unwrap_or(0));
        let (j, g) = [(0.0, false), (1e-7, false), (0.0, true)][k];
        sweep("debug", idx + 1, j, g, Some(idx));
        return;
    }
    let n: usize = args.get(1).and_then(|v| v.parse().ok()).unwrap_or(1000);
    table("Fixtures", &fixtures());
    table("Stress", &stress());
    sweep("0.5 mm grid (exact coincidences and tangencies)", n, 0.0, false, None);
    sweep("0.5 mm grid, operand B shifted by up to 1e-7 mm", n, 1e-7, false, None);
    sweep("general position (random floats)", n, 0.0, true, None);
}
