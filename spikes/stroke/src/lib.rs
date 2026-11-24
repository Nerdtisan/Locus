// SPDX-License-Identifier: AGPL-3.0-or-later

//! Stroke-expansion spike (#425, ADR 0019 S1–S3). Throwaway code.

pub mod build;
pub mod fit;
pub mod oracle;
pub mod path;

use core::f64::consts::PI;
use locus_geom::{Cubic, EXPORT_TOL, Vec2};
use locus_region::Seg;
use path::{Model, Nib, Path, Piece, Profile};

/// Canvas tolerance: 0.05 screen px at 100 % zoom (96 dpi: 3.78 px/mm).
pub const CANVAS_TOL: f64 = 0.05 / (96.0 / 25.4);

pub fn line(a: (f64, f64), b: (f64, f64)) -> Piece {
    Piece::Seg(Seg::Line {
        a: Vec2::new(a.0, a.1),
        b: Vec2::new(b.0, b.1),
    })
}

pub fn cubic(p: [(f64, f64); 4]) -> Piece {
    let v = |q: (f64, f64)| Vec2::new(q.0, q.1);
    Piece::Cubic(Cubic::new(v(p[0]), v(p[1]), v(p[2]), v(p[3])))
}

pub fn arc(c: (f64, f64), r: f64, start_deg: f64, sweep_deg: f64) -> Piece {
    Piece::Seg(Seg::Arc {
        center: Vec2::new(c.0, c.1),
        radius: r,
        start: start_deg.to_radians(),
        sweep: sweep_deg.to_radians(),
    })
}

fn prof(p: &[(f64, f64, f64)]) -> Profile {
    Profile { pts: p.to_vec() }
}

/// S1 fixtures: (name, path, profile).
pub fn s1_fixtures() -> Vec<(&'static str, Path, Profile)> {
    let polyline =
        |pts: &[(f64, f64)]| -> Vec<Piece> { pts.windows(2).map(|w| line(w[0], w[1])).collect() };
    let spiral = {
        let mut v = Vec::new();
        let (mut c, mut start) = ((0.0, 0.0), 0.0f64);
        let mut prev_r: f64 = 6.0;
        for r in [6.0, 5.0, 4.0, 3.0, 2.5, 2.0] {
            // Each half-turn shares its start point with the previous end.
            let s = Vec2::from_angle(start.to_radians()) * prev_r + Vec2::new(c.0, c.1);
            let cc = s - Vec2::from_angle(start.to_radians()) * r;
            c = (cc.x, cc.y);
            v.push(arc(c, r, start, 180.0));
            start += 180.0;
            prev_r = r;
        }
        v
    };
    vec![
        (
            "taper line 50 mm, w 0.5→4",
            Path::new(vec![line((0.0, 0.0), (50.0, 0.0))], false),
            prof(&[(0.0, 0.25, 0.25), (1.0, 2.0, 2.0)]),
        ),
        (
            "S-curve cubic, bulge 0.2→5→0.2",
            Path::new(
                vec![cubic([
                    (0.0, 0.0),
                    (20.0, 30.0),
                    (30.0, -30.0),
                    (50.0, 0.0),
                ])],
                false,
            ),
            prof(&[(0.0, 0.1, 0.1), (0.5, 2.5, 2.5), (1.0, 0.1, 0.1)]),
        ),
        (
            "arc r 1.5, 270°, w 4 (inside flips)",
            Path::new(vec![arc((0.0, 0.0), 1.5, 0.0, 270.0)], false),
            Profile::constant(2.0),
        ),
        (
            "zigzag, 30–150° corners, varying w",
            Path::new(
                polyline(&[
                    (0.0, 0.0),
                    (10.0, 8.0),
                    (14.0, -6.0),
                    (24.0, 4.0),
                    (27.0, -9.0),
                    (40.0, 0.0),
                ]),
                false,
            ),
            prof(&[
                (0.0, 0.25, 0.25),
                (0.3, 1.0, 1.0),
                (0.6, 0.4, 0.4),
                (1.0, 0.75, 0.75),
            ]),
        ),
        (
            "arc spiral r 6→2, w 0.5↔3",
            Path::new(spiral, false),
            prof(&[
                (0.0, 0.25, 0.25),
                (0.25, 1.5, 1.5),
                (0.5, 0.5, 0.5),
                (0.75, 1.5, 1.5),
                (1.0, 0.25, 0.25),
            ]),
        ),
        (
            "closed blob (4 cubics), asymmetric",
            Path::new(
                vec![
                    cubic([(0.0, 0.0), (10.0, -8.0), (25.0, -8.0), (30.0, 0.0)]),
                    cubic([(30.0, 0.0), (35.0, 8.0), (30.0, 20.0), (20.0, 20.0)]),
                    cubic([(20.0, 20.0), (10.0, 20.0), (12.0, 10.0), (5.0, 12.0)]),
                    cubic([(5.0, 12.0), (-2.0, 14.0), (-10.0, 8.0), (0.0, 0.0)]),
                ],
                true,
            ),
            prof(&[(0.0, 1.0, 0.5), (0.5, 2.0, 1.0), (1.0, 1.0, 0.5)]),
        ),
        (
            "self-looping cubic + cusp cubic",
            Path::new(
                vec![
                    cubic([(0.0, 0.0), (30.0, 30.0), (-10.0, 30.0), (20.0, 0.0)]),
                    cubic([(20.0, 0.0), (35.0, 15.0), (25.0, 15.0), (40.0, 0.0)]),
                ],
                false,
            ),
            prof(&[(0.0, 0.4, 0.4), (0.5, 1.2, 1.2), (1.0, 0.4, 0.4)]),
        ),
    ]
}

/// S2 paths for the nib comparison.
pub fn s2_paths() -> Vec<(&'static str, Path)> {
    vec![
        (
            "S-curve cubic",
            Path::new(
                vec![cubic([
                    (0.0, 0.0),
                    (20.0, 30.0),
                    (30.0, -30.0),
                    (50.0, 0.0),
                ])],
                false,
            ),
        ),
        (
            "arc r 5, 330°",
            Path::new(vec![arc((0.0, 0.0), 5.0, 0.0, 330.0)], false),
        ),
        (
            "zigzag (corners)",
            Path::new(
                vec![
                    line((0.0, 0.0), (10.0, 8.0)),
                    line((10.0, 8.0), (14.0, -6.0)),
                    line((14.0, -6.0), (24.0, 4.0)),
                ],
                false,
            ),
        ),
        (
            "looping cubic",
            Path::new(
                vec![cubic([
                    (0.0, 0.0),
                    (30.0, 30.0),
                    (-10.0, 30.0),
                    (20.0, 0.0),
                ])],
                false,
            ),
        ),
    ]
}

pub fn nib(size: f64, roundness: f64) -> Nib {
    Nib {
        a: size / 2.0,
        b: size / 2.0 * roundness,
        angle: PI / 4.0,
    }
}

/// SplitMix64, as ADR 0019 plans for scatter.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }
    pub fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        (z >> 11) as f64 / (1u64 << 53) as f64
    }
    pub fn range(&mut self, a: f64, b: f64) -> f64 {
        a + (b - a) * self.next()
    }
}

/// 500 random styled paths: 4-cubic chains, ~100 mm long, a 3-point profile
/// with half-widths 0.15–2 mm.
pub fn random_paths(n: usize, seed: u64) -> Vec<(Path, Model)> {
    let mut r = Rng::new(seed);
    (0..n)
        .map(|_| {
            let mut p = Vec2::new(r.range(0.0, 200.0), r.range(0.0, 200.0));
            let mut dir = r.range(0.0, 2.0 * PI);
            let mut pieces = Vec::new();
            for _ in 0..4 {
                let len = r.range(15.0, 35.0);
                let d0 = Vec2::from_angle(dir);
                dir += r.range(-1.6, 1.6);
                let p1 = p + Vec2::from_angle(dir) * len;
                dir += r.range(-1.2, 1.2);
                let d1 = Vec2::from_angle(dir);
                pieces.push(Piece::Cubic(Cubic::new(
                    p,
                    p + d0 * (len * r.range(0.2, 0.5)),
                    p1 - d1 * (len * r.range(0.2, 0.5)),
                    p1,
                )));
                p = p1;
            }
            let pr = Profile {
                pts: vec![
                    (0.0, r.range(0.15, 1.0), r.range(0.15, 1.0)),
                    (r.range(0.3, 0.7), r.range(0.5, 2.0), r.range(0.5, 2.0)),
                    (1.0, r.range(0.15, 1.0), r.range(0.15, 1.0)),
                ],
            };
            (Path::new(pieces, false), Model::Profile(pr))
        })
        .collect()
}

/// Export-mode expansion of every path: (a) biarcs only, or (b) also map the
/// cleaned runs back to cubics. Returns a checksum (output piece count).
pub fn bench_export(paths: &[(Path, Model)], map_back: bool, tol: f64) -> usize {
    let mut total = 0;
    for (p, m) in paths {
        let mut ex = build::expand(p, m, tol * 0.5, tol * 0.5);
        if map_back {
            let o = build::map_back(&mut ex, tol * 0.5);
            let (l, a, c) = o.counts();
            total += l + a + c;
        } else {
            total += ex.area.edges().count();
        }
    }
    total
}

/// Canvas-mode meshes (no fit, no cleanup). Returns the triangle count.
pub fn bench_canvas(paths: &[(Path, Model)], tol: f64) -> usize {
    let mut tris = Vec::new();
    for (p, m) in paths {
        build::canvas_mesh(p, m, tol, &mut tris);
    }
    tris.len()
}

pub const DEFAULT_TOL: f64 = EXPORT_TOL;

// wasm32 entry points, timed from JS (no clock in the core, §3).
#[unsafe(no_mangle)]
pub extern "C" fn wasm_bench(mode: u32, n: u32) -> u32 {
    let paths = random_paths(n as usize, 42);
    (match mode {
        0 => bench_canvas(&paths, CANVAS_TOL),
        1 => bench_export(&paths, false, CANVAS_TOL),
        2 => bench_export(&paths, false, EXPORT_TOL),
        _ => bench_export(&paths, true, EXPORT_TOL),
    }) as u32
}

/// Builds the inputs only, so JS can subtract the setup time.
#[unsafe(no_mangle)]
pub extern "C" fn wasm_setup(n: u32) -> u32 {
    random_paths(n as usize, 42).len() as u32
}
