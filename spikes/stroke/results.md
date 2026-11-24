# Stroke spike results (S1–S3, ADR 0019, #425)

Run in the `locus-dev` container (Rust 1.99) on the owner's Windows 11 host: native x86-64 release, single thread. Regenerate with `cargo run --release`. Every table below is that run's output, except the wasm32 and canvas sections, which were measured on the host.
These results include the `locus-region` fixes from #485. Before them, 2 of the first 100 random strokes and 2 of the 12 S2 cases lost their outline.

## S1: variable-width outline (profile), target T = EXPORT_TOL = 0.001 mm

| Fixture | fitted cubics | (a) lines/arcs | (a) error mm | (b) cubics/lines/arcs | (b) error mm | (b) runs full/trimmed/fallback | expand ms | map-back ms | canvas tris |
|---|---|---|---|---|---|---|---|---|---|
| taper line 50 mm, w 0.5→4 | 4 | 34 (0/34) | 0.00014 (0.00014 / 0.00005) | 6 (4/0/2) | 0.00001 (0.00001 / 0.00000) | 4/0/0 | 0.91 | 0.45 | 36 |
| S-curve cubic, bulge 0.2→5→0.2 | 85 | 230 (0/230) | 0.00050 (0.00050 / 0.00019) | 87 (85/0/2) | 0.00047 (0.00047 / 0.00018) | 85/0/0 | 4.83 | 1.54 | 124 |
| arc r 1.5, 270°, w 4 (inside flips) | 22 | 20 (0/20) | 0.00025 (0.00025 / 0.00024) | 19 (1/0/18) | 0.00025 (0.00025 / 0.00024) | 0/1/8 | 1.76 | 0.30 | 88 |
| zigzag, 30–150° corners, varying w | 20 | 86 (0/86) | 0.00019 (0.00019 / 0.00000) | 26 (20/0/6) | 0.00014 (0.00014 / 0.00000) | 12/8/0 | 1.76 | 0.58 | 68 |
| arc spiral r 6→2, w 0.5↔3 | 104 | 157 (0/157) | 0.00048 (0.00048 / 0.00014) | 62 (61/0/1) | 0.00048 (0.00048 / 0.00015) | 53/8/0 | 7.93 | 1.46 | 360 |
| closed blob (4 cubics), asymmetric | 65 | 254 (0/254) | 0.00055 (0.00055 / 0.00003) | 65 (65/0/0) | 0.00048 (0.00048 / 0.00006) | 65/0/0 | 7.71 | 3.12 | 248 |
| self-looping cubic + cusp cubic | 73 | 272 (4/268) | 0.00055 (0.00055 / 0.00018) | 78 (74/1/3) | 0.00053 (0.00053 / 0.00013) | 63/11/0 | 6.58 | 2.12 | 200 |

## Node count after Expand vs tolerance (option b nodes / option a nodes)

| Fixture | T = 0.001 | T = 0.01 | T = 0.05 |
|---|---|---|---|
| taper line 50 mm, w 0.5→4 | 6 / 34 (err 0.0000) | 4 / 18 (err 0.0000) | 4 / 10 (err 0.0000) |
| S-curve cubic, bulge 0.2→5→0.2 | 87 / 230 (err 0.0005) | 22 / 88 (err 0.0038) | 15 / 52 (err 0.0227) |
| arc r 1.5, 270°, w 4 (inside flips) | 19 / 20 (err 0.0003) | 14 / 16 (err 0.0039) | 3 / 3 (err 0.0000) |
| zigzag, 30–150° corners, varying w | 26 / 86 (err 0.0001) | 20 / 42 (err 0.0031) | 18 / 30 (err 0.0074) |
| arc spiral r 6→2, w 0.5↔3 | 62 / 157 (err 0.0005) | 39 / 85 (err 0.0047) | 28 / 51 (err 0.0200) |
| closed blob (4 cubics), asymmetric | 65 / 254 (err 0.0005) | 35 / 124 (err 0.0046) | 24 / 84 (err 0.0195) |
| self-looping cubic + cusp cubic | 78 / 272 (err 0.0005) | 45 / 139 (err 0.0041) | 32 / 88 (err 0.0214) |

## S2: calligraphic nib, size 4 mm at 45°, against the exact sweep path ⊕ nib

| Path | roundness | normal-offset w(θ) error mm | support-point error mm | support (a) lines/arcs | support (b) pieces |
|---|---|---|---|---|---|
| S-curve cubic | 0 | 1.7844 | 0.00025 | 180 | 28 |
| S-curve cubic | 0.3 | 0.1305 | 0.00047 | 266 | 118 |
| S-curve cubic | 1 | 0.0005 | 0.00047 | 192 | 41 |
| arc r 5, 330° | 0 | 1.4329 | 0.00050 | 62 | 55 |
| arc r 5, 330° | 0.3 | 0.3883 | 0.00048 | 197 | 128 |
| arc r 5, 330° | 1 | 0.0005 | 0.00050 | 39 | 35 |
| zigzag (corners) | 0 | 1.6869 | 0.00000 | 6 | 6 |
| zigzag (corners) | 0.3 | 0.2531 | 0.00027 | 113 | 113 |
| zigzag (corners) | 1 | 0.0000 | 0.00001 | 10 | 10 |
| looping cubic | 0 | 0.6317 | 0.00019 | 122 | 23 |
| looping cubic | 0.3 | 0.3459 | 0.00047 | 206 | 108 |
| looping cubic | 1 | 0.0005 | 0.00047 | 103 | 29 |

## Timing, native x86-64 release, single thread, 500 random styled paths

| Mode | 500 paths ms | per path ms | output (triangles or pieces) |
|---|---|---|---|
| canvas: sampled strip mesh, no cleanup (tol 0.013 mm) | 16.6 | 0.033 | 117451 |
| canvas tol, fit + biarcs + union (a) | 2386.1 | 4.772 | 75110 |
| export (a): fit + biarcs + union, T = 0.001 | 3908.7 | 7.817 | 163811 |
| export (b): (a) + map back to cubics | 5575.7 | 11.151 | 37380 |

Stage split at T: side fitting 711.5 ms of 3924.2 ms (the rest is lowering + per-band normalise + union).

Raw union without per-band normalisation: 3605.9 ms for 500.

Random set, all 500 paths: worst error (a) 0.00071 mm, (b) 0.00054 mm; 0 over T.

## wasm32 (same code, `wasm32-unknown-unknown`, opt-level 3, fat LTO; timed from JS in the desktop app's Chromium pane)

| Mode | 500 paths | per path |
|---|---|---|
| canvas strip mesh (0.013 mm) | 44.3 ms | 0.089 ms |
| canvas tolerance, fit + biarcs + union (a) | 8 951 ms | 17.9 ms |
| export (a), T = 0.001 | 13 971 ms | 27.9 ms |
| export (b), T = 0.001 | 22 724 ms | 45.4 ms |

## S3: canvas frame cost (`canvas-bench`, eframe 0.35 + wgpu, Windows host, vsync off, 600 frames after 60 warm-up)

500 random styled paths, 117 451 triangles.

| Mode | ms/frame wall | ms/frame building shapes |
|---|---|---|
| plain centrelines (`painter.line`, today) | 7.60 | 0.22 |
| cached `Arc<Mesh>` in world space under a layer transform | 7.53 | 0.03 |
| strip meshes transformed to screen every frame | 8.11 | 3.06 |
| scanline fill of the cleaned outlines (the region-fill method) | 45.71 | 36.29 |
