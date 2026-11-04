# Spike B results: Bevy 0.17 + bevy_egui + egui_dock (#45)

Reference canvas for the GUI framework decision (SPEC §12.1, §12.2, §13 M0),
built on the stack of the owner's Bevy simulator: `bevy =0.17.3`
(default features off), `bevy_egui =0.38.1` (egui 0.33), `egui_dock =0.18.0`.
Input for ADR 0001 (#46); this file does not make the decision.

## What was built

- **Canvas**: mm-scale pan/zoom in f64. Wheel and pinch zoom about the cursor;
  middle- or right-drag pans. Zoom is clamped so the short side shows at least
  0.001 mm and the long side at most 2 m (`src/view.rs`, unit-tested).
- **Adaptive grid**: power-of-ten minor and major levels. The minor level fades
  in as it opens up, so zooming never pops. Lines snap to physical pixel
  centres to stay crisp. Major-line labels are drawn by egui over the canvas.
- **Line tool**: click-click, chained like Fusion's line tool. Esc or
  right-click ends the chain. A rubber band and start marker show while
  drawing.
- **Hit-testing**: hovering within 6 px highlights the line (brute-force f64
  distance check). The Select tool picks a line; its endpoints are editable in
  Properties; Del deletes it.
- **Docking**: egui_dock with Canvas | Properties + Benchmark | Layers (stub).
  Panels can be rearranged, tabbed and floated; the canvas can't float or be
  closed.
- **Benchmark**: `--bench <dir>` natively, `?bench` on the web (`src/auto.rs`).
  Same protocol as spike A (see Method).

### Rendering choice: Bevy gizmos in screen space, from f64

All geometry, grid and highlights are drawn with **Bevy gizmos**, rebuilt every
frame. Each point is converted world → screen in **f64, relative to the view
centre**, and clipped to the view (Liang–Barsky, f64) **before** the cast to
f32. That makes this the most precise option:

- Bevy world coordinates, meshes and `Transform` are f32. Near 2000 mm the f32
  spacing is about 1.2 × 10⁻⁴ mm, which is 12 % of a 0.001 mm view. A retained
  mesh in world space would visibly jitter at the smallest extent.
- Screen-space vertices stay small (about the size of the canvas in pixels),
  so f32 is exact enough for them. The `extent-0.001mm` screenshot centres
  the view 0.2 µm / 0.1 µm from a line end at (−195.953675, −136.056263) mm.
  The end lands exactly that far off-centre (148 px / 74 px at
  742 000 px/mm), on a 0.1 µm grid.
- lyon was not needed: segments are the only primitive so far.
  Arcs and Béziers would need tessellation (lyon, or flattening into gizmo
  polylines).

Cost: gizmos are immediate-mode, so the CPU rebuilds every vertex every frame
(about 0.85 ms for 5 000 lines, see below). A retained mesh with a
camera-relative transform would be cheaper, but it needs care to keep the
precision.

## Results

Host: Windows 11, NVIDIA GeForce RTX 5080 (driver 621.01; Vulkan natively,
WebGL2 through ANGLE/D3D11 in the browser), 32-thread CPU seen by the
container. Window 1280 × 800 (canvas 960 × 742 pt), scale factor 1.0.

| Metric | Result |
|---|---|
| Frame time, 5 000 lines, native Windows (release, vsync off) | **1.24 ms mean / 1.44 ms p95** frame interval (≈ 810 fps). Main-schedule CPU 1.09 / 1.19 ms; scene build (gizmos) 0.85 / 0.87 ms. 5 runs, range 1.231–1.264 ms mean |
| Frame time, 5 000 lines, web (Chromium, WebGL2) | **16.68 ms mean / 17.80 ms p95**: capped at 60 Hz by rAF. Main-schedule CPU 1.06 / 1.70 ms; scene build 0.50 / 0.70 ms (browser timer resolution is 0.1 ms) |
| wasm bundle size (release, after wasm-opt -Oz) | **11.98 MB** raw (11 983 043 B), **4.27 MB** gzip -9; JS glue 103 KB (15 KB gz) |
| Clean build time (Windows release, container) | **67.0 s** (registry already downloaded; earlier run 73.5 s). Web clean: **146.9 s** (fat LTO + opt-level z + wasm-opt) |
| Incremental build time (touch `main.rs`) | Windows release **20.5 s**, Windows dev **15.4 s**, web **92.0 s** |
| Release binary size (Windows `.exe`, stripped) | **45.5 MB** (45 549 056 B), 15.7 MB gzip -9 |

Also measured: idle CPU ≈ 0 (0.03 s of CPU in 10 s, redraw on input only)
and 392 MB working set at idle.

### Method

Same protocol as spike A (agreed in the shared session notes):

- **Scene**: 5 000 segments from xorshift64* with seed `0x9E3779B97F4A7C15`.
  Starts are uniform in a 400 × 300 mm area (centred on the origin here),
  angles are random, lengths 2–42 mm (`doc::stress_lines`). Same distribution
  as spike A, but not guaranteed to be the same lines.
- **Run**: fit all, then 30 warm-up frames and 600 measured frames. Over a
  600-frame cycle the view pans ±20 / ±10 mm and zooms ±25 %, so every frame is
  a new transform. Vsync is off natively (`PresentMode::AutoNoVsync`,
  continuous redraw); the browser always runs at rAF rate.
- **Reported numbers**: *frame interval* is the wall clock between frames.
  *Main-schedule CPU* is Bevy's First → Last. Natively the render world runs
  in parallel (pipelined rendering), so this excludes rendering; on wasm
  everything runs on one thread. *Scene build* is our gizmo `draw` system
  alone.
- **Builds**: `measure.sh` in the `locus-dev` container. The target directory
  is a Docker volume, not the OneDrive bind mount. Profiles: `release` =
  opt-level 3, thin LTO, 16 codegen units, strip. `wasm-release` = release
  with opt-level `z`, fat LTO and 1 codegen unit, then trunk's
  `wasm-opt -Oz`.
- Native numbers come from the plain release `.exe`, run on the Windows host.
  The web numbers come from the trunk release build, served from the container
  and run in the Claude desktop app's built-in Chromium pane.

## Notes

### Anti-aliasing and text

- **Lines**: gizmo lines are 1 px wide with Bevy's default 4× MSAA. That is
  passable, but diagonal lines show visible stair-stepping at 100 % scale
  (`results/native-overview-5000-lines.png`, zoomed). There is no analytic or
  shader AA without writing a custom line material. The 2.5 px highlight
  lines look noticeably smoother.
- **Grid**: pixel-snapped 1 px lines are crisp at every zoom.
- **Text**: all text is egui (rulers, panels, status bar). It is clean and
  hinted at 1.0 scale and matches egui elsewhere. Bevy's own text (`bevy_text`)
  was not needed and is not compiled in.

### Docking feel

The canvas is a transparent egui_dock tab. A Bevy camera renders into the
tab's rectangle through a camera viewport, and a second window-sized camera
composites egui over it. This is the pattern the owner's simulator already
ships. The layout, tab bars and panels render and clip correctly in every
screenshot.

**Not checked by hand:** dragging tabs to split or float them, and resizing
the window. Desktop control of the native window wasn't granted for this
session, and the web pane's automation clicks were unreliable (below). These
are egui_dock features, the same as under eframe, so they are not a
differentiator between A and B. Small limits found:

- The canvas tab can't be floated into an egui window, because the viewport
  belongs to the main window. Native OS-window tear-off would need a second
  Bevy window and camera; the simulator does this, but it is extra code.
- The leaf close-all and collapse buttons had to be turned off by hand, or the
  canvas leaf could be closed.

### Awkward bits

- **Two input worlds.** egui (through bevy_egui) owns pointer input, so all
  canvas input is read from egui's `Response`. Bevy's own `ButtonInput` and
  picking aren't used. That works, but Bevy's ECS adds little to a 2D editor
  here: the canvas is ~150 lines of immediate drawing.
- **Dropped clicks in the web build (unverified, probably a test artefact).**
  With the browser-pane automation, synthetic instant clicks were sometimes
  lost, including clicks on dock tabs. A click after a pointer move in the same
  frame also took the later pointer position. During the tests the pane was
  hidden, and rAF was measured at 0 frames/2 s, so events piled up between
  sporadic frames. That makes the result unreliable. One real fix came out of
  it: line endpoints now come from the button-release event, not the latest
  pointer position (`ui.rs`). **Re-test by hand** in a visible browser before
  the ADR, and run the same check on spike A's web build.
- **Codegen variance.** Turning on the `capture` feature (PNG encoding for the
  benchmark's screenshots) gave builds whose scene build ranged from 0.86 to
  2.4 ms on identical code. The plain release build was stable across 5 runs.
  Thin LTO with 16 codegen units seems to make inlining of the hot gizmo loop
  build-dependent. Keep this in mind when comparing small differences.
- **wasm build**: Rust's default bulk-memory output needs
  `data-wasm-opt-params="--enable-bulk-memory …"` for trunk's wasm-opt.
  wasm-bindgen also warns that `strip = true` removes the `target_features`
  section. Trunk's file watcher doesn't see edits through the Windows bind
  mount, so restart `trunk serve` after each change.
- **Bundle size**: 4.27 MB gzipped is with only the renderer, core pipeline,
  gizmos and sprite-render (which hosts the 2D gizmo pipeline). Of Bevy's
  render stack, PBR, text, UI, audio, gamepads, scenes and image formats were
  all left out.
- **Container**: no Dockerfile change was needed. Without `bevy_audio` and
  `bevy_gilrs` there is no `libasound`/`libudev` dependency, and winit's `x11`
  backend loads libX11 at run time. Native Linux is compile-checked only.

## Screenshots

| | |
|---|---|
| Native, 5 000 lines, line #7 selected | `results/native-overview-5000-lines.png` |
| Native, 0.001 mm extent at a line end (precision) | `results/native-extent-0.001mm.png` |
| Native, 2 m extent | `results/native-extent-2m.png` |
| Web, 5 000 lines | `results/web-overview-5000-lines.jpg` |
| Web, line tool: committed segment + rubber band | `results/web-line-tool-rubber-band.jpg` |
| Web, hover highlight | `results/web-hover-highlight.jpg` |

Native screenshots are the app's own window captures (`--bench` with
`--features capture`). Desktop control of the window wasn't used.

## Reproduce

```powershell
# from spikes/gui-bevy; target on a Docker volume (owned by uid 1000)
docker run --rm -e CARGO_HOME=/workspace/.cargo-home -v "${PWD}\..\..:/workspace" `
  -v locus-spike-bevy-target:/workspace/spikes/gui-bevy/target `
  -w /workspace/spikes/gui-bevy locus-dev bash measure.sh
.\dist\locus-spike-bevy.exe --bench .\dist      # writes dist\bench.txt
# web: trunk serve --release --cargo-profile wasm-release --address 0.0.0.0 --port 8091
#      (publish -p 8091:8091), then open http://localhost:8091/?bench
```
