# Spike A results: eframe 0.35 + egui_dock + wgpu (#44)

Reference canvas for the GUI framework decision (SPEC §12.1, §12.2, §13 M0),
built on the stack of the owner's Parchment app: `eframe 0.35.0` (wgpu
backend only, `default_fonts`, `x11`, `wayland`), `egui_dock 0.20.1`,
`wgpu 29`. Input for ADR 0001 (#46); this file does not make the decision.

## What was built

- **Canvas**: mm-scale pan/zoom in f64. Wheel and pinch zoom about the cursor;
  middle- or right-drag pans. Zoom is clamped so the short side shows at least
  0.001 mm and the long side at most 2 m, the same rule as spike B
  (`src/view.rs`, unit-tested). World → screen is computed in f64 relative to
  the view centre and narrowed to f32 last.
- **Adaptive grid**: power-of-ten minor and major lines; the minor level fades
  in as it opens up (`src/grid.rs`). The HUD shows extent, grid step and the
  cursor position.
- **Line tool**: click-click, chained like Fusion's line tool. Esc or
  right-click ends the chain. A rubber band shows while drawing.
- **Hit-testing**: hovering within 5 pt highlights the line (brute-force f64
  distance over all lines). The Select tool picks a line and Properties shows
  its endpoints, length and layer.
- **Docking**: egui_dock with Canvas | Properties above Layers + Stats. Tabs
  can be closed, except the canvas.
- **Layers stub**: three layers (Cut / Score / Engrave) with colour, visibility
  and an active layer for new lines.
- **Three renderers**, switchable at run time (Properties → Renderer):
  1. **egui painter**: one `line_segment` per line; epaint tessellates every
     frame with its feathered anti-aliasing (`src/render.rs`).
  2. **lyon → egui mesh**: lyon's stroke tessellator builds one egui mesh per
     layer every frame (`src/render.rs`).
  3. **wgpu paint callback**: one instanced draw inside egui's render pass. Each
     segment is a quad expanded in the vertex shader, and the fragment shader
     computes coverage from the exact distance to the segment, so the AA is
     analytic (`src/gpu.rs`, `src/line.wgsl`). Lines stay on the GPU as f32
     offsets from an anchor point; a pan or zoom uploads only a 48-byte
     uniform. Because f32 can't hold a 2 m coordinate to 0.001 mm-view
     precision, the buffer is re-uploaded relative to the view centre when the
     f32 error at the centre would exceed 0.1 px, or when the drawing changes.
- **Benchmark and screenshots**: `--bench` natively, `?bench` on the web, and
  `--shot <dir>` for the native screenshots, using egui's own
  `ViewportCommand::Screenshot`. Same protocol as spike B (see Method).

## Results

Host: Windows 11, AMD Ryzen 9 9950X3D (32 threads seen by the container),
NVIDIA GeForce RTX 5080 (Vulkan natively; in the browser, WebGPU by default or
WebGL2 through ANGLE/D3D11 with `?webgl`). Window 1280 × 800 (canvas
950 × 766 pt), scale factor 1.0.

| Metric | Result |
|---|---|
| Frame time, 5 000 lines, native Windows (release, vsync off) | **egui painter: 1.07 ms mean / 1.43 ms p95** frame interval (≈ 930 fps); eframe CPU 0.87 / 1.21 ms. **wgpu callback: 0.38 / 0.48 ms**; CPU 0.19 / 0.22 ms. lyon: 3.01 / 3.41 ms; CPU 2.78 / 3.12 ms. 5 runs: painter mean 1.02–1.15 ms, wgpu 0.37–0.38 ms, lyon 2.93–3.07 ms |
| Frame time, 5 000 lines, web (Chromium) | **16.68 ms mean / 16.80 ms p95** for every renderer: capped at 60 Hz by rAF. CPU per frame, WebGL2: painter **1.63** / 2.00 ms, wgpu callback 0.69 / 0.90 ms, lyon 2.57 / 2.90 ms. WebGPU: painter 1.75 / 2.20 ms, wgpu callback 0.60 / 0.80 ms, lyon 2.66 / 3.10 ms (browser timer resolution 0.1 ms) |
| wasm bundle size (release, after wasm-opt -Oz) | **6.12 MB** raw (6 121 567 B), **2.47 MB** gzip -9; JS glue 141 KB (21 KB gz) |
| Clean build time (Windows release, container) | **32.3 s** (registry already downloaded). Web clean: **92.6 s** (fat LTO + opt-level z + wasm-opt) |
| Incremental build time (append a comment to `main.rs`) | Windows release **10.4 s**, Windows dev **8.4 s**, web **54.5 s** (cargo 27.5 s; the rest is wasm-bindgen + wasm-opt) |
| Release binary size (Windows `.exe`, stripped) | **14.6 MB** (14 607 872 B), 6.2 MB gzip -9 |

Native Linux: compile check only (`cargo build --release` 30.1 s after the
Windows build; clippy clean for host and `wasm32-unknown-unknown`).

Also measured: with 4× MSAA (`--msaa 4`) the native numbers don't change
(painter 1.17 ms, wgpu 0.38 ms, lyon 2.85 ms, one run).

### Method

Same protocol as spike B (agreed in the shared session notes, then matched to
spike B's `RESULTS.md` and `measure.sh`):

- **Scene**: 5 000 segments from xorshift64* with seed `0x9E3779B97F4A7C15`.
  Starts are uniform in a 400 × 300 mm area (from the origin here), angles are
  random, lengths 2–42 mm, layers round-robin (`doc::generate`).
- **Run**: fit all, then 30 warm-up frames and 600 measured frames per
  renderer. Over the 600-frame cycle the view pans ±20 / ±10 mm and zooms
  ±25 %, so every frame is a new transform. Vsync is off natively
  (`PresentMode::AutoNoVsync`, continuous redraw); the browser always runs at
  rAF rate. The run waits until the canvas size has been stable for 10 frames.
- **Reported numbers**: *frame interval* is the wall clock between frames
  (`unstable_dt`). *eframe CPU* is `frame.info().cpu_usage`: the whole frame's
  CPU time, excluding the vsync wait (UI, our drawing, epaint tessellation and
  wgpu submission). *Scene build* is our drawing code alone; it is 0.00 for the
  wgpu callback because a pan/zoom frame rebuilds nothing. The native
  "headline" row is egui painter, the simplest renderer and closest to spike
  B's immediate-mode gizmos; the others are listed alongside.
- **Builds**: `measure.sh` in the `locus-dev` container, the same steps as
  spike B's. The target directory is a Docker volume
  (`locus-spike-eframe-target`), not the OneDrive bind mount. Profiles match
  spike B: `release` = opt-level 3, thin LTO, 16 codegen units, strip;
  `wasm-release` = release with opt-level `z`, fat LTO and 1 codegen unit, then
  trunk's `wasm-opt -Oz`.
- Native numbers come from the release `.exe`, run on the Windows host. Web
  numbers come from the trunk release build, served from the container
  (`-p 8092:8092`) and run in the Claude desktop app's built-in Chromium pane,
  kept visible during the run.

## Notes

### Anti-aliasing and text

- **egui painter**: epaint's feathering gives clean 1-px-class AA on every
  angle (`results/native-aa-20mm-painter.png`). Nothing to configure.
- **wgpu callback**: the analytic shader AA is at least as smooth as epaint's,
  with round caps (`results/native-aa-20mm-wgpu.png`). One catch: hover and
  selection are drawn in instance order, so a highlighted line can be partly
  covered by lines drawn after it (visible in
  `results/native-overview-5000-lines-wgpu.png`). A real renderer needs a
  second pass for highlights.
- **lyon**: lyon emits hard-edged triangles, so at 1.25 px the lines are
  aliased and look broken up (`results/native-aa-20mm-lyon.png`). 4× MSAA
  (`NativeOptions::multisampling`) fixes it at no measurable cost on this GPU
  (`results/native-aa-20mm-lyon-msaa4.png`). lyon is also the slowest here,
  because it re-tessellates in screen space every frame. Its value is arcs,
  Béziers and fills, not hairline segments.
- **Text**: all text is egui: clean and hinted at 1.0 scale. egui's default
  proportional font has no `→` (U+2192): the renderer name "lyon → egui mesh"
  shows a tofu box in the combo box while the monospace HUD renders it. A
  shipping app needs a font with wider coverage (symbols, `µ`, `°`, `Ø`, `±`
  for dimensions).
- **Precision**: at the 0.001 mm extent, centred 0.2 µm / 0.1 µm from line 7's
  end, the end lands exactly 153 px / 77 px off-centre (766 000 pt/mm) with
  both the painter and the GPU path (`results/native-extent-0.001mm-*.png`).
  The re-anchoring keeps the f32 GPU buffer exact at that zoom.

### Docking feel

egui_dock renders the layout, tab bars and panels cleanly natively and in the
browser. Laying out a default dock in code is 3 lines
(`split_right`, `split_below`). The canvas is an ordinary tab that allocates a
painter, so the GPU callback, input and clipping all come from egui with no
second camera or viewport.

**Not checked by hand:** dragging tabs to split or float them, and resizing
the native window. Desktop control of the native window was not granted for
this session. In the browser pane, `left_click_drag` and a paced JavaScript
pointer sequence both failed to start an egui_dock drag, which looks like a
limit of synthetic input, not of egui_dock. These are the same egui_dock
features spike B uses, so they are not a differentiator, but **re-test by
hand** before the ADR. Small limits found:

- A tab's default `ScrollArea` would swallow the wheel; the canvas tab turns
  scroll bars off (`TabViewer::scroll_bars`).
- The leaf collapse buttons are off, so the canvas leaf can't collapse.

### Web behaviour

- Clicks were **not** dropped in the visible pane: click-click line drawing,
  tool switching and selection all worked at the first attempt
  (`results/web-line-tool-rubber-band.jpg`, `results/web-select-line.jpg`).
  This is the check spike B asked for.
- eframe picks **WebGPU** when the browser offers it and falls back to WebGL2.
  The wgpu callback (WGSL, instanced, `Uint32` attributes) runs unchanged on
  both. `?webgl` forces WebGL2 for a like-for-like comparison with spike B.
- The web build follows the system theme, so it was light in the pane. That
  exposed a white-only hover colour, now theme-aware
  (`render::hover_color`).

### Awkward bits

- **Fit-all on the first frame is wrong on the web.** The canvas is not at its
  final size on the first frames, so a one-shot fit came out zoomed far out.
  Fixed by re-fitting on resize until the user pans or zooms, and the
  benchmark waits for 10 stable frames.
- **Bind-mount timestamps.** With `target/` on the OneDrive bind mount, cargo
  records whole-second build times while generated files have sub-second
  mtimes. Any crate with a build script that writes to `OUT_DIR`
  (`glutin_wgl_sys`, pulled in by wgpu-hal's GLES backend on Windows) is seen
  as dirty on the next build, which rebuilds wgpu, egui-wgpu and eframe:
  "incremental" builds took 49–69 s instead of 10 s. A Docker volume for
  `target/` (as spike B and `measure.sh` use) avoids it. Worth doing in the
  main repo's dev loop too.
- **wasm build**: needs the same `data-wasm-opt-params="--enable-bulk-memory …"`
  as spike B; wasm-bindgen warns that `strip = true` removes the
  `target_features` section. Trunk's file watcher doesn't see edits through the
  bind mount, so restart `trunk serve` after each change.
- **egui 0.35 API churn**: `App::ui(&mut Ui, …)` replaces `App::update`, and
  `Panel` replaces `TopBottomPanel`/`SidePanel`. Easy here, but egui moves
  fast: expect a migration per minor release.
- **Container**: no Dockerfile change was needed. Native Linux is compile-only.

## Screenshots

| | |
|---|---|
| Native, 5 000 lines, line #7 selected, hover + rubber band, egui painter | `results/native-overview-5000-lines-painter.png` |
| Same, lyon / wgpu callback | `results/native-overview-5000-lines-lyon.png`, `…-wgpu.png` |
| Native, 20 mm extent, AA comparison | `results/native-aa-20mm-{painter,lyon,wgpu}.png`, `…-lyon-msaa4.png` |
| Native, 0.001 mm extent at a line end (precision) | `results/native-extent-0.001mm-{painter,wgpu}.png` |
| Native, 2 m extent | `results/native-extent-2m.png` |
| Web, 5 000 lines | `results/web-overview-5000-lines.jpg` |
| Web, line tool: committed segment + rubber band | `results/web-line-tool-rubber-band.jpg` |
| Web, hover highlight | `results/web-hover-highlight.jpg` |
| Web, selected line in Properties | `results/web-select-line.jpg` |

Native screenshots are the app's own window captures (`--shot`). The web ones
are from the browser pane.

## Reproduce

```powershell
# from spikes/gui-eframe; target on a Docker volume (owned by uid 1000:
# docker run --rm --user root -v locus-spike-eframe-target:/t locus-dev chown 1000:1000 /t)
docker run --rm -e CARGO_HOME=/workspace/.cargo-home -v "${PWD}\..\..:/workspace" `
  -v locus-spike-eframe-target:/workspace/spikes/gui-eframe/target `
  -w /workspace/spikes/gui-eframe locus-dev bash measure.sh
.\dist\spike-eframe.exe --bench --no-vsync   # writes spike-eframe-bench.md
.\dist\spike-eframe.exe --shot .\results     # writes the native screenshots
# web: trunk serve --release --cargo-profile wasm-release --address 0.0.0.0 --port 8092
#      (publish -p 8092:8092), then open http://localhost:8092/?bench (add &webgl for WebGL2)
```
