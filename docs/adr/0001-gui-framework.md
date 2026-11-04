# ADR-0001: GUI framework and canvas renderer: eframe + egui_dock, egui painter first

- **Status:** Accepted
- **Date:** 2026-11-04
- **Spec:** `docs/SPEC.md §12.1` (GUI framework), `§12.2` (canvas renderer), `§13 M0`
- **Issue:** Nerdtisan/Locus#46 (spikes #44, #45)

## Context

Locus needs a GUI that draws a precise 2D canvas at millimetre scale (0.001 mm to 2 m visible extents), gives native-feeling docked panels, packages for Windows, macOS and Linux, and runs as a full editor in the browser (§0, §1). The owner's two existing Rust apps use different egui-based stacks, so both were spiked with the same scope, scene and measurement protocol:

- **Spike A** (#44): `eframe 0.35` + `egui_dock 0.20` + `wgpu 29`, as in Parchment. Results: [`spikes/gui-eframe/RESULTS.md`](https://github.com/Nerdtisan/Locus/blob/c7379f9/spikes/gui-eframe/RESULTS.md) (folder deleted in #67; linked at commit `c7379f9`).
- **Spike B** (#45): `bevy 0.17.3` + `bevy_egui 0.38` + `egui_dock 0.18`, as in the DD-L0 simulator. Results: [`spikes/gui-bevy/RESULTS.md` at tag `archive/spike-gui-bevy`](https://github.com/Nerdtisan/Locus/blob/archive/spike-gui-bevy/spikes/gui-bevy/RESULTS.md).

Both drew the same 5 000-segment seeded scene, with pan/zoom about the cursor, an adaptive grid, click-click lines, hover hit-testing and egui_dock panels. Their axes, grid labels and line-tool marker were matched afterwards (#90) so the owner could compare them side by side.

## Options

### Option A: eframe + egui_dock (Parchment's stack)

egui owns the window, input and painting; the canvas is an ordinary dock tab with its own painter. Three canvas renderers were tried in it: the **egui painter** (epaint tessellation), **lyon → egui mesh**, and a **wgpu paint callback** (instanced quads with analytic AA, f32 buffer re-anchored for deep zoom).

**Pros:** fastest and smallest in every measurement; one input model; the canvas, panels and text all come from egui; the same stack as Parchment. **Cons:** egui changes API every minor release; the default font lacks symbols a CAD app needs (`→`, `Ø`, `±`).

### Option B: Bevy + bevy_egui + egui_dock (the simulator's stack)

Bevy renders the canvas with 2D gizmos; egui (through bevy_egui) draws panels and rulers on top and owns pointer input.

**Pros:** familiar from the simulator; a full engine if Locus ever needed one. **Cons:** about 3× the binary and 1.7× the web download; twice the build time; two input worlds (Bevy's ECS adds little to a 2D editor, as spike B noted); dropped clicks seen in automated web tests (possibly a test artefact, not resolved).

## Measurements

Same host for both: Windows 11, Ryzen 9 9950X3D, RTX 5080; window 1280 × 800, scale 1.0; release builds from the `locus-dev` container with `target/` on a Docker volume. Scene, motion protocol and build profiles are identical (see each RESULTS.md, "Method").

| Criterion | A: eframe | B: Bevy |
|---|---|---|
| Native frame, 5 000 lines, vsync off (mean / p95) | **1.07 / 1.43 ms** (egui painter); 0.38 / 0.48 ms (wgpu callback); 3.01 / 3.41 ms (lyon) | 1.24 / 1.44 ms |
| Web frame, 5 000 lines | 60 Hz rAF cap; CPU per frame 1.63 ms (painter, WebGL2), 0.69 ms (wgpu callback) | 60 Hz rAF cap; CPU per frame 1.06 ms |
| wasm (release, wasm-opt -Oz), gzip | **2.47 MB** (6.12 MB raw) | 4.27 MB (11.98 MB raw) |
| Windows release `.exe`, stripped | **14.6 MB** | 45.5 MB |
| Clean / incremental build (Windows release) | **32.3 s / 10.4 s** | 67.0 s / 20.5 s |
| Web clean / incremental build | **92.6 s / 54.5 s** | 147 s / 92 s |
| Deep-zoom precision (0.001 mm extent) | exact, painter and GPU path | exact (f64 view maths, unit-tested) |
| Web click handling | no dropped clicks in a visible browser pane | dropped clicks under automation (unresolved) |
| Owner's hand test | **felt more responsive** | — |

Not measured or not differentiating: dock-tab dragging by hand (synthetic drags don't reach egui_dock; both use the same crate, so it isn't a differentiator; re-check in the M1 app shell, #65); Firefox/Safari; native macOS and Linux runs (compile-checked only).

## Decision

**Use eframe 0.35 + egui_dock + wgpu (Option A).** It is faster, about a third of the binary size, half the web download and half the build time; it has one input model; it matches Parchment; and the owner's hand test agreed with the numbers.

**Canvas renderer: the egui painter first.** 1.07 ms for 5 000 lines leaves an order of magnitude of headroom under the 16 ms frame budget, epaint's anti-aliasing is clean with nothing to configure, and it is the simplest path. The **wgpu paint callback** from spike A (0.38 ms, analytic AA, deep-zoom re-anchoring) is the measured escape hatch: adopt it for sketch geometry if profiling with real documents shows the painter is the bottleneck. **lyon** is not used for hairlines (slowest, aliased without MSAA); keep it in mind for fills and hatching (§7) where tessellation is the point.

## Consequences

- `locus-app` depends on `eframe` (wgpu backend, `default_fonts`, `x11`, `wayland`), `egui` and `egui_dock`. Core crates stay GUI-free (§3).
- The M1 app shell (#65) starts from `spikes/gui-eframe/`: its f64 view maths (`view.rs`), adaptive grid and rulers (`grid.rs`) and dock layout move into `locus-app`. The spike folder was deleted with #67.
- Spike B is archived: removed from `dev` in this PR and kept at the git tag `archive/spike-gui-bevy`.
- The web build (#38) uses trunk with the same `wasm-opt` flags as the spike; eframe picks WebGPU and falls back to WebGL2.
- Bundle a font with CAD symbols (`µ`, `°`, `Ø`, `±`, `→`) under an OFL-compatible licence (§9).
- Keep `target/` on a Docker volume, not the OneDrive bind mount: whole-second timestamps there make wgpu rebuild every time (spike A, "Awkward bits"). Fold this into CLAUDE.md's build loop with the next dev-loop change.
- Expect an egui API migration per minor release; pin exact versions and upgrade deliberately.
- Revisit if egui stops supporting the web or wgpu backends, or if canvas needs (e.g. 3D previews) outgrow a 2D painter.
