// SPDX-License-Identifier: AGPL-3.0-or-later
//! End-to-end tests and the S5 benchmark (ADR 0019 decision 8).

use super::*;

/// An RGBA image drawn by `ink(x, y)` -> grey level.
fn image(w: u32, h: u32, ink: impl Fn(f64, f64) -> u8) -> Vec<u8> {
    let mut v = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let g = ink(f64::from(x) + 0.5, f64::from(y) + 0.5);
            v.extend([g, g, g, 255]);
        }
    }
    v
}

fn run(w: u32, h: u32, rgba: &[u8], mode: Mode) -> Result<Traced, TraceError> {
    let params = Params {
        mode,
        ..Params::default()
    };
    trace(
        w,
        h,
        rgba,
        &params,
        &Limits::default(),
        &mut Budget::new(u64::MAX),
    )
}

#[test]
fn a_black_square_on_white_traces_to_its_four_edges() {
    let img = image(60, 60, |x, y| {
        if (10.0..50.0).contains(&x) && (10.0..50.0).contains(&y) {
            0
        } else {
            255
        }
    });
    let t = run(60, 60, &img, Mode::Threshold { level: None }).unwrap();
    assert_eq!(t.groups.len(), 1);
    assert_eq!(t.groups[0].paths.len(), 1);
    assert_eq!(t.segments(), 4);
}

#[test]
fn blank_and_bad_images_never_panic() {
    let white = image(20, 20, |_, _| 255);
    let t = run(20, 20, &white, Mode::Threshold { level: Some(128) }).unwrap();
    assert_eq!(t.segments(), 0);
    assert_eq!(
        run(20, 20, &white[..7], Mode::Threshold { level: None }),
        Err(TraceError::BadImage)
    );
    assert_eq!(
        run(0, 0, &[], Mode::Centreline { level: None }),
        Err(TraceError::BadImage)
    );
    // Every mode on noise stays in bounds.
    let noise = image(64, 64, |x, y| ((x * 7.0 + y * 13.0) as u32 % 251) as u8);
    for mode in [
        Mode::Threshold { level: None },
        Mode::Edges { level: None },
        Mode::Colours { count: 3 },
        Mode::Centreline { level: None },
    ] {
        assert!(run(64, 64, &noise, mode).is_ok(), "{mode:?}");
    }
}

#[test]
fn colours_trace_each_colour_as_its_own_group() {
    let mut img = Vec::new();
    for y in 0..40u32 {
        for x in 0..60u32 {
            let c: [u8; 3] = if x < 20 {
                [220, 30, 30]
            } else if x < 40 {
                [30, 30, 220]
            } else {
                [255, 255, 255]
            };
            let _ = y;
            img.extend([c[0], c[1], c[2], 255]);
        }
    }
    let t = run(60, 40, &img, Mode::Colours { count: 3 }).unwrap();
    assert_eq!(t.groups.len(), 3);
    assert!(
        t.groups
            .iter()
            .any(|g| g.colour[0] > 200 && g.colour[2] < 60)
    );
}

#[test]
fn the_limits_hold() {
    let img = image(200, 200, |x, y| {
        if ((x as u32 / 3) + (y as u32 / 3)).is_multiple_of(2) {
            0
        } else {
            255
        }
    });
    let params = Params {
        speckle: 0,
        ..Params::default()
    };
    let tight = Limits {
        max_segments: 10,
        ..Limits::default()
    };
    assert!(matches!(
        trace(200, 200, &img, &params, &tight, &mut Budget::new(u64::MAX)),
        Err(TraceError::TooMany { .. })
    ));
    assert_eq!(
        trace(
            200,
            200,
            &img,
            &params,
            &Limits::default(),
            &mut Budget::new(1_000)
        ),
        Err(TraceError::Budget)
    );
    let mut asked = 0;
    let mut cancel = || {
        asked += 1;
        true
    };
    assert_eq!(
        trace(
            200,
            200,
            &img,
            &params,
            &Limits::default(),
            &mut Budget::new(u64::MAX).with_cancel(&mut cancel)
        ),
        Err(TraceError::Cancelled)
    );
}

#[test]
fn downsampled_traces_come_back_at_source_scale() {
    let img = image(400, 400, |x, y| {
        if (x - 200.0).hypot(y - 200.0) < 120.0 {
            0
        } else {
            255
        }
    });
    let limits = Limits {
        max_pixels: 20_000,
        ..Limits::default()
    };
    let t = trace(
        400,
        400,
        &img,
        &Params::default(),
        &limits,
        &mut Budget::new(u64::MAX),
    )
    .unwrap();
    assert!(t.factor > 1);
    for s in &t.groups[0].paths[0].segments {
        let r = s.at(0.0).distance(Vec2::new(200.0, 200.0));
        assert!((r - 120.0).abs() < 2.0 * t.factor as f64, "r = {r}");
    }
}

// ── Spike S5: benchmark (run with --ignored --nocapture) ────────────────

/// The four S5 fixtures at 2000 × 2000 (4 MP), drawn procedurally so the
/// repo carries no image files: a logo, scanned line art, a photo-like
/// gradient with noise, and a handwritten-style signature.
fn fixtures() -> Vec<(&'static str, Vec<u8>)> {
    let n = 2000u32;
    let s = f64::from(n);
    let logo = image(n, n, |x, y| {
        let (u, v) = (x / s, y / s);
        let ring = ((u - 0.5).hypot(v - 0.5) - 0.3).abs() < 0.04;
        let bar = (0.3..0.7).contains(&u) && (0.47..0.53).contains(&v);
        let tri = v > 0.7 && v < 0.85 && (u - 0.5).abs() < (v - 0.7);
        if ring || bar || tri { 0 } else { 255 }
    });
    let line_art = image(n, n, |x, y| {
        let (u, v) = (x / s, y / s);
        let mut ink = false;
        for k in 0..12 {
            let r = 0.05 + 0.035 * f64::from(k);
            ink |= ((u - 0.5).hypot(v - 0.5) - r).abs() < 0.002;
        }
        ink |= ((u - v).abs() < 0.002) || ((u + v - 1.0).abs() < 0.002);
        // Scan grain.
        let grain = ((x * 12.9898 + y * 78.233).sin() * 43_758.545)
            .fract()
            .abs()
            < 0.03;
        if ink ^ grain { 20 } else { 235 }
    });
    let photo = image(n, n, |x, y| {
        let (u, v) = (x / s, y / s);
        let base = 128.0 + 100.0 * ((u * 9.0).sin() * (v * 7.0).cos());
        let noise = ((x * 3.1 + y * 7.7).sin() * 10_000.0).fract() * 30.0;
        (base + noise).clamp(0.0, 255.0) as u8
    });
    let signature = image(n, n, |x, y| {
        let (u, v) = (x / s, y / s);
        let curve = 0.5 + 0.15 * (u * 18.0).sin() * (1.0 - (u - 0.5).abs() * 1.6);
        if (0.1..0.9).contains(&u) && (v - curve).abs() < 0.006 {
            0
        } else {
            255
        }
    });
    vec![
        ("logo", logo),
        ("line art", line_art),
        ("photo", photo),
        ("signature", signature),
    ]
}

/// Largest distance from the source boundary to the traced curves, pixels:
/// every 4th lattice boundary point to the nearest piece of the curves,
/// each curve cut into pieces of at most 4 px.
fn deviation(t: &Traced, paths: &[LatticePath]) -> f64 {
    const CELL: f64 = 8.0;
    let mut pieces: Vec<(Vec2, Vec2)> = Vec::new();
    for s in t
        .groups
        .iter()
        .flat_map(|g| &g.paths)
        .flat_map(|p| &p.segments)
    {
        let len = s.at(0.0).distance(s.at(0.5)) + s.at(0.5).distance(s.at(1.0));
        let n = ((len / 4.0).ceil() as usize).clamp(1, 4096);
        for k in 0..n {
            pieces.push((s.at(k as f64 / n as f64), s.at((k + 1) as f64 / n as f64)));
        }
    }
    let mut grid: std::collections::HashMap<(i64, i64), Vec<usize>> =
        std::collections::HashMap::new();
    for (i, (a, b)) in pieces.iter().enumerate() {
        let m = a.lerp(*b, 0.5);
        grid.entry(((m.x / CELL).floor() as i64, (m.y / CELL).floor() as i64))
            .or_default()
            .push(i);
    }
    let dist = |q: Vec2, (a, b): (Vec2, Vec2)| {
        let e = b - a;
        let l2 = e.length_sq();
        let t = if l2 > 0.0 {
            ((q - a).dot(e) / l2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        q.distance(a + e * t)
    };
    let mut worst = 0.0_f64;
    for path in paths {
        for &(x, y) in path.points.iter().step_by(4) {
            let q = Vec2::new(x as f64, y as f64);
            let (cx, cy) = ((q.x / CELL).floor() as i64, (q.y / CELL).floor() as i64);
            let mut best = f64::INFINITY;
            for r in 1..=8 {
                for dx in -r..=r {
                    for dy in -r..=r {
                        for i in grid.get(&(cx + dx, cy + dy)).into_iter().flatten() {
                            best = best.min(dist(q, pieces[*i]));
                        }
                    }
                }
                if best <= (r as f64 - 1.0) * CELL {
                    break;
                }
            }
            if best.is_finite() {
                worst = worst.max(best);
            }
        }
    }
    worst
}

#[test]
#[ignore = "S5 benchmark: cargo test -p locus-trace --release -- --ignored --nocapture"]
fn s5_benchmark() {
    let n = 2000u32;
    for (name, img) in fixtures() {
        for (label, mode) in [
            ("threshold", Mode::Threshold { level: None }),
            ("centreline", Mode::Centreline { level: None }),
        ] {
            let params = Params {
                mode,
                ..Params::default()
            };
            let mut budget = Budget::new(u64::MAX);
            let t0 = std::time::Instant::now();
            let result = trace(n, n, &img, &params, &Limits::default(), &mut budget);
            let ms = t0.elapsed().as_secs_f64() * 1e3;
            let steps = u64::MAX - budget.left();
            match result {
                Ok(t) => {
                    let nodes: usize = t
                        .groups
                        .iter()
                        .flat_map(|g| &g.paths)
                        .map(|p| p.segments.len() + usize::from(!p.closed))
                        .sum();
                    let dev = if matches!(mode, Mode::Threshold { .. }) {
                        let lum: Vec<u8> = img.chunks(4).map(|c| c[0]).collect();
                        let b = bitmap::threshold(n as usize, n as usize, &lum, otsu(&lum));
                        let paths = decompose(&b, 4, &mut Budget::new(u64::MAX)).unwrap();
                        format!("{:.2}", deviation(&t, &paths))
                    } else {
                        "n/a".to_owned()
                    };
                    println!(
                        "S5 | {name:<9} | {label:<10} | {:>7} segments | {nodes:>7} nodes | max dev {dev:>5} px | {ms:>8.0} ms | {steps:>11} steps",
                        t.segments()
                    );
                }
                Err(e) => {
                    println!("S5 | {name:<9} | {label:<10} | {e} | {ms:.0} ms | {steps} steps")
                }
            }
        }
    }
}
