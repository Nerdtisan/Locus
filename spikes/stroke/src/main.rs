// SPDX-License-Identifier: AGPL-3.0-or-later

//! Runs S1 and S2 and the native timings; prints Markdown tables.
//! `cargo run --release` (all), `-- s1`, `-- s2`, `-- time`.

use locus_geom::EXPORT_TOL;
use std::time::Instant;
use stroke_spike::build::{self, Outline};
use stroke_spike::oracle::{self, NibOracle, ProfileOracle};
use stroke_spike::path::Model;
use stroke_spike::*;

const SAG: f64 = 1e-5;
const PROBES: usize = 240;

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1e3
}

fn s1() {
    println!("## S1: variable-width outline (profile), target T = EXPORT_TOL = {EXPORT_TOL} mm\n");
    println!(
        "| Fixture | fitted cubics | (a) lines/arcs | (a) error mm | (b) cubics/lines/arcs | (b) error mm | (b) runs full/trimmed/fallback | expand ms | map-back ms | canvas tris |"
    );
    println!("|---|---|---|---|---|---|---|---|---|---|");
    for (name, path, pr) in s1_fixtures() {
        let model = Model::Profile(pr);
        let o = ProfileOracle::new(&path, &model);
        let t = Instant::now();
        let mut ex = build::expand(&path, &model, EXPORT_TOL * 0.5, EXPORT_TOL * 0.5);
        let t_ex = ms(t);
        let a = Outline::from_area(&ex.area);
        let ea = oracle::measure(&o, &a.polylines(SAG), PROBES);
        let t = Instant::now();
        let b = build::map_back(&mut ex, EXPORT_TOL * 0.5);
        let t_mb = ms(t);
        let eb = oracle::measure(&o, &b.polylines(SAG), PROBES);
        let (al, aa, _) = a.counts();
        let (bl, ba, bc) = b.counts();
        let mut tris = Vec::new();
        build::canvas_mesh(&path, &model, CANVAS_TOL, &mut tris);
        let s = ex.stats;
        println!(
            "| {name} | {} | {} ({al}/{aa}) | {:.5} ({:.5} / {:.5}) | {} ({bc}/{bl}/{ba}) | {:.5} ({:.5} / {:.5}) | {}/{}/{} | {t_ex:.2} | {t_mb:.2} | {} |",
            s.fitted,
            al + aa,
            ea.hausdorff(),
            ea.out_to_true,
            ea.true_to_out,
            bl + ba + bc,
            eb.hausdorff(),
            eb.out_to_true,
            eb.true_to_out,
            s.runs_full,
            s.runs_trimmed,
            s.runs_fallback,
            tris.len(),
        );
    }
    println!();
}

fn nodes() {
    println!(
        "## Node count after Expand vs tolerance (option b nodes / option a nodes)
"
    );
    println!("| Fixture | T = 0.001 | T = 0.01 | T = 0.05 |");
    println!("|---|---|---|---|");
    for (name, path, pr) in s1_fixtures() {
        let model = Model::Profile(pr);
        let mut cells = Vec::new();
        for t in [0.001, 0.01, 0.05] {
            let mut ex = build::expand(&path, &model, t * 0.5, t * 0.5);
            let na = ex.area.edges().count();
            let b = build::map_back(&mut ex, t * 0.5);
            let (l, a, c) = b.counts();
            let o = ProfileOracle::new(&path, &model);
            let e = oracle::measure(&o, &b.polylines(SAG), 120);
            cells.push(format!("{} / {} (err {:.4})", l + a + c, na, e.hausdorff()));
        }
        println!("| {name} | {} | {} | {} |", cells[0], cells[1], cells[2]);
    }
    println!();
}

fn s2() {
    println!("## S2: calligraphic nib, size 4 mm at 45°, against the exact sweep path ⊕ nib\n");
    println!(
        "| Path | roundness | normal-offset w(θ) error mm | support-point error mm | support (a) lines/arcs | support (b) pieces |"
    );
    println!("|---|---|---|---|---|---|");
    for (name, path) in s2_paths() {
        for rnd in [0.0, 0.3, 1.0] {
            let nb = nib(4.0, rnd);
            let o = NibOracle::new(&path, nb);
            let mut res = Vec::new();
            for model in [Model::NibNormal(nb), Model::NibSupport(nb)] {
                let mut ex = build::expand(&path, &model, EXPORT_TOL * 0.5, EXPORT_TOL * 0.5);
                let a = Outline::from_area(&ex.area);
                let e = oracle::measure(&o, &a.polylines(SAG), PROBES);
                let b = build::map_back(&mut ex, EXPORT_TOL * 0.5);
                let (bl, ba, bc) = b.counts();
                res.push((e, a.counts(), bl + ba + bc));
            }
            let (en, _, _) = res[0];
            let (es, (al, aa, _), bn) = res[1];
            println!(
                "| {name} | {rnd} | {:.4} | {:.5} | {} | {} |",
                en.hausdorff(),
                es.hausdorff(),
                al + aa,
                bn
            );
        }
    }
    println!();
}

fn time() {
    println!("## Timing, native x86-64 release, single thread, 500 random styled paths\n");
    let paths = random_paths(500, 42);
    if std::env::var("ACC_ONLY").is_err() {
        let reps = 3;
        let row = |label: &str, f: &dyn Fn() -> usize| {
            let mut best = f64::INFINITY;
            let mut out = 0;
            for _ in 0..reps {
                let t = Instant::now();
                out = f();
                best = best.min(ms(t));
            }
            println!("| {label} | {best:.1} | {:.3} | {out} |", best / 500.0);
        };
        println!("| Mode | 500 paths ms | per path ms | output (triangles or pieces) |");
        println!("|---|---|---|---|");
        row(
            "canvas: sampled strip mesh, no cleanup (tol 0.013 mm)",
            &|| bench_canvas(&paths, CANVAS_TOL),
        );
        row("canvas tol, fit + biarcs + union (a)", &|| {
            bench_export(&paths, false, CANVAS_TOL)
        });
        row("export (a): fit + biarcs + union, T = 0.001", &|| {
            bench_export(&paths, false, EXPORT_TOL)
        });
        row("export (b): (a) + map back to cubics", &|| {
            bench_export(&paths, true, EXPORT_TOL)
        });
        // Stage split for (a) at T.
        let (mut fit_ms, mut tot) = (0.0, 0.0);
        for (p, m) in &paths {
            let t = Instant::now();
            let mut n = 0;
            for k in 0..p.pieces.len() {
                let fl = |t: f64| m.sides(p, k, t).0;
                let mut cs = Vec::new();
                fit::fit(&fl, 0.0, 1.0, EXPORT_TOL * 0.5, &mut cs);
                fit::fit(
                    &|t: f64| m.sides(p, k, t).1,
                    0.0,
                    1.0,
                    EXPORT_TOL * 0.5,
                    &mut cs,
                );
                n += cs.len();
            }
            std::hint::black_box(n);
            fit_ms += ms(t);
            let t = Instant::now();
            std::hint::black_box(build::expand(p, m, EXPORT_TOL * 0.5, EXPORT_TOL * 0.5));
            tot += ms(t);
        }
        println!(
            "\nStage split at T: side fitting {fit_ms:.1} ms of {tot:.1} ms (the rest is lowering + per-band normalise + union).\n"
        );
        let t = Instant::now();
        for (p, m) in &paths {
            std::hint::black_box(build::expand_with(
                p,
                m,
                EXPORT_TOL * 0.5,
                EXPORT_TOL * 0.5,
                build::Cleanup::RawUnion,
            ));
        }
        println!(
            "Raw union without per-band normalisation: {:.1} ms for 500.\n",
            ms(t)
        );
    }
    // Accuracy over the random set.
    let mut worst = (0.0f64, 0.0f64);
    let mut over = 0;
    for (idx, (p, m)) in paths.iter().enumerate() {
        let o = ProfileOracle::new(p, m);
        let mut ex = build::expand(p, m, EXPORT_TOL * 0.5, EXPORT_TOL * 0.5);
        let ea = oracle::measure(&o, &Outline::from_area(&ex.area).polylines(SAG), 120);
        let b = build::map_back(&mut ex, EXPORT_TOL * 0.5);
        let eb = oracle::measure(&o, &b.polylines(SAG), 120);
        worst = (worst.0.max(ea.hausdorff()), worst.1.max(eb.hausdorff()));
        if ea.hausdorff() > EXPORT_TOL || eb.hausdorff() > EXPORT_TOL {
            over += 1;
            println!(
                "  over T: path {idx}: (a) {:.5} ({:.5}/{:.5}) (b) {:.5}",
                ea.hausdorff(),
                ea.out_to_true,
                ea.true_to_out,
                eb.hausdorff()
            );
        }
    }
    println!(
        "Random set, all 500 paths: worst error (a) {:.5} mm, (b) {:.5} mm; {over} over T.\n",
        worst.0, worst.1
    );
}

fn main() {
    let arg = std::env::args().nth(1).unwrap_or_default();
    if std::env::var("LSQ").is_ok() {
        stroke_spike::fit::LEAST_SQUARES.store(true, std::sync::atomic::Ordering::Relaxed);
        println!(
            "(fitter: least-squares tangent lengths)
"
        );
    }
    if arg.is_empty() || arg == "s1" {
        s1();
    }
    if arg.is_empty() || arg == "nodes" {
        nodes();
    }
    if arg.is_empty() || arg == "s2" {
        s2();
    }
    if arg.is_empty() || arg == "time" {
        time();
    }
    if arg == "bug" {
        let which: usize = std::env::args()
            .nth(2)
            .and_then(|x| x.parse().ok())
            .unwrap_or(1);
        let rnd: f64 = std::env::args()
            .nth(3)
            .and_then(|x| x.parse().ok())
            .unwrap_or(1.0);
        bug(which, rnd);
    }
    if arg == "dump" {
        // Raw (un-normalised) bands of S2 "arc r 5, 330°", round nib, as Rust.
        let (_, path) = &s2_paths()[1];
        let nb = nib(4.0, 1.0);
        let ex = build::expand_with(
            path,
            &Model::NibSupport(nb),
            EXPORT_TOL * 0.5,
            EXPORT_TOL * 0.5,
            build::Cleanup::RawUnion,
        );
        let v = |p: locus_geom::Vec2| format!("v({:?}, {:?})", p.x, p.y);
        for (i, a) in ex.pieces.iter().enumerate() {
            println!("// piece {i}");
            println!("vec![");
            for l in &a.loops {
                println!("  vec![");
                for e in &l.edges {
                    match e.seg {
                        locus_region::Seg::Line { a, b } => {
                            println!("    line({}, {}),", v(a), v(b))
                        }
                        locus_region::Seg::Arc {
                            center,
                            radius,
                            start,
                            sweep,
                        } => println!(
                            "    arc({}, {:?}, {:?}, {:?}),",
                            v(center),
                            radius,
                            start,
                            sweep
                        ),
                    }
                }
                println!("  ],");
            }
            println!("],");
        }
    }
    if arg == "rnd" {
        let idx: usize = std::env::args()
            .nth(2)
            .and_then(|x| x.parse().ok())
            .unwrap_or(31);
        let paths = random_paths(500, 42);
        let (p, m) = &paths[idx];
        let ex = build::expand(p, m, EXPORT_TOL * 0.5, EXPORT_TOL * 0.5);
        for l in &ex.area.loops {
            let bb = l.bbox();
            println!(
                "loop {} edges area {:.6} bbox {:?}",
                l.edges.len(),
                l.area(),
                bb
            );
        }
        let o = ProfileOracle::new(p, m);
        for l in &ex.area.loops {
            let pl = build::Outline::from_area(&locus_region::Area {
                loops: vec![l.clone()],
            })
            .polylines(SAG);
            let e = oracle::measure(&o, &pl, 10);
            println!("  loop err out->true {:.5}", e.out_to_true);
        }
    }
    if arg == "dbg" {
        let which: usize = std::env::args()
            .nth(2)
            .and_then(|x| x.parse().ok())
            .unwrap_or(0);
        let rnd: f64 = std::env::args()
            .nth(3)
            .and_then(|x| x.parse().ok())
            .unwrap_or(1.0);
        let (_, path) = &s2_paths()[which];
        let nb = nib(4.0, rnd);
        let o = NibOracle::new(path, nb);
        debug_case(path, &Model::NibSupport(nb), &o);
    }
}

#[allow(dead_code)]
pub fn debug_case(path: &stroke_spike::path::Path, model: &Model, o: &dyn oracle::Oracle) {
    let ex = build::expand(path, model, EXPORT_TOL * 0.5, EXPORT_TOL * 0.5);
    for (i, l) in ex.area.loops.iter().enumerate() {
        println!("loop {i}: {} edges, area {:.4}", l.edges.len(), l.area());
        for e in &l.edges {
            println!("   {:?}", e.seg);
        }
    }
    let polys = Outline::from_area(&ex.area).polylines(SAG);
    let e = oracle::measure(o, &polys, 60);
    println!("{e:?}");
    let bb = o.bbox();
    println!("bbox {bb:?}");
    for j in 0..30 {
        let mut s = String::new();
        for i in 0..60 {
            let q = locus_geom::Vec2::new(
                bb.min.x + (bb.max.x - bb.min.x) * (i as f64 + 0.5) / 60.0,
                bb.max.y - (bb.max.y - bb.min.y) * (j as f64 + 0.5) / 30.0,
            );
            let a = o.inside(q);
            let b = ex.area.contains(q);
            s.push(match (a, b) {
                (true, true) => '#',
                (false, false) => '.',
                (true, false) => 'O',
                (false, true) => 'R',
            });
        }
        println!("{s}");
    }
}

/// Finds the first union step that loses area on S2 path `which`, then
/// shrinks the operands (drop whole loops while the failure stays).
#[allow(dead_code)]
pub fn bug(which: usize, rnd: f64) {
    use locus_region::{Area, BoolOp, boolean};
    let (_, path) = &s2_paths()[which];
    let nb = nib(4.0, rnd);
    let ex = build::expand(
        path,
        &Model::NibSupport(nb),
        EXPORT_TOL * 0.5,
        EXPORT_TOL * 0.5,
    );
    let fails = |a: &Area, b: &Area| {
        let u = boolean(a, b, BoolOp::Union);
        // A union never has less area than either operand.
        u.area() + 1e-6 < a.area().max(b.area())
    };
    let mut acc = Area::default();
    for (i, p) in ex.pieces.iter().enumerate() {
        if fails(&acc, p) {
            println!(
                "step {i} fails: acc {:.4} + piece {:.4} -> {:.4}",
                acc.area(),
                p.area(),
                boolean(&acc, p, BoolOp::Union).area()
            );
            let (mut a, b) = (acc.clone(), p.clone());
            // Shrink a by loops, then by merging: just report.
            let mut k = 0;
            while k < a.loops.len() {
                let mut t = a.clone();
                t.loops.remove(k);
                if !t.loops.is_empty() && fails(&t, &b) {
                    a = t;
                } else {
                    k += 1;
                }
            }
            println!("A ({} loops):", a.loops.len());
            for l in &a.loops {
                println!("  loop");
                for e in &l.edges {
                    println!("    {:?}", e.seg);
                }
            }
            println!("B ({} loops):", b.loops.len());
            for l in &b.loops {
                println!("  loop");
                for e in &l.edges {
                    println!("    {:?}", e.seg);
                }
            }
            return;
        }
        acc = boolean(&acc, p, BoolOp::Union);
    }
    println!("no failing step");
}
