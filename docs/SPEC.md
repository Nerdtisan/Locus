# Locus — Specification

> **Living spec.** This is the contract for the project: code, issues and ADRs cite it by section (e.g. `docs/SPEC.md §11`). Section numbers are stable — add new material inside existing sections or append new ones; never renumber.
> Status: pre-code. Milestone 0 (spikes & scaffolding) is next. Decisions are recorded in §0 and, once spikes complete, in ADRs under [`docs/adr/`](adr/).
> Working agreement for Claude Code: see [`CLAUDE.md`](../CLAUDE.md).

---

## 0. Decisions log

Decisions taken on **2026-11-04** after reviewing the initial brainstorm. Where an item is still being spiked, §12 notes the ADR that will close it.

| Topic | Decision |
|---|---|
| GUI | **Decided** ([ADR 0001](adr/0001-gui-framework.md)): `eframe` 0.35 + `egui_dock` + `wgpu` (as in Parchment); canvas drawn with the egui painter first, the wgpu paint-callback renderer kept as the measured upgrade path. Bevy spiked and not chosen (decided 2026-11-04). |
| Platforms | Windows, macOS, Linux and web. Web = full editor; the MCP server is desktop-only at first. Plugins run on desktop (`wasmtime`) and, from M4, in the browser: the owner moved the browser plugin runtime from "after M4" into M4 on 2026-11-11; it runs the component's core module in a Web Worker with the same limits except fuel ([ADR 0016](adr/0016-web-plugin-runtime.md), 2026-11-12). |
| Solver | Pure Rust only (SolidCraft extract, another Rust GCS, or our own). libslvs/PlaneGCS spikes dropped because they block wasm. Backend chosen 2026-11-04 by [ADR 0002](adr/0002-solver-backend.md) (`ezpz`), then superseded the same day by [ADR 0007](adr/0007-solver-backend-without-paste.md): the owner declined ezpz's `paste` advisory (RUSTSEC-2024-0436), so `locus-solver` owns the whole solver (sparse Levenberg–Marquardt, own sparse LDLᵀ, own Jacobian-rank DOF/redundancy analysis), with no third-party dependencies. |
| Toolchain | Both: Parchment-style Docker dev container (Linux build/test, Windows cross-compile, `wasm32` + `trunk`), plus a native macOS runner in CI. No host Rust toolchain. |
| Rust version | Always the latest stable Rust (1.99 today) and the newest edition (2024). `rust-version`, the dev container and the macOS CI job move together; no separate MSRV (decided 2026-11-04). |
| Naming | App **Locus**: `locus-*` crates, GUI binary `locus`, headless CLI binary `locus-cli` (decided 2026-11-04, PR #78), `.locus` files, `.locusplugin` archives, WIT package `locus:plugin@1.0.0`. Replaces the earlier placeholder name. |
| Licence | As §2 / §15.6: AGPL-3.0-or-later core, MIT OR Apache-2.0 `locus-plugin-api`, CLA, plugin exception, `cargo-deny`. MPL-2.0 dependencies allowed (decided 2026-11-04, PR #80). |
| Web hosting | GitHub Pages (https://nerdtisan.github.io/Locus/), deployed only from `main`; PRs and `dev` build the web app without publishing (decided 2026-11-04, #38). Browser open/download plus OPFS/IndexedDB autosave. |
| Packaging | Windows Inno Setup + portable zip, macOS `.dmg`, Linux AppImage, Flatpak and `.deb`. |
| Input | Mouse + keyboard first; touch/pen is an M4 epic. |
| Fonts | `skrifa` + `harfrust` instead of the unmaintained `ttf-parser` / `rustybuzz` (decided 2026-11-04). |
| Entity IDs | Document-scoped monotonic `u64` (`next_id` saved in the file, never reused); plugins and paste/import get IDs remapped by the host. [ADR-0003](adr/0003-entity-ids.md), 2026-11-04. |
| File format | `.locus` is RON (`ron` 0.12), `BTreeMap`-ordered, finite floats only; JSON only as an export view. [ADR-0004](adr/0004-file-format.md), 2026-11-04. |
| MCP SDK | `rmcp` 3.8; target MCP revision 2026-07-28, also accept 2025-11-25 clients. [ADR-0005](adr/0005-mcp-sdk.md), 2026-11-04. |
| MCP network clients | Opt-in **Allow network clients** setting, off by default: also bind one chosen private IPv4 address and accept only allowlisted private clients (empty list by default). Token, `Origin` refusal, scopes and approvals unchanged; plain HTTP, so the UI recommends an SSH tunnel or Tailscale instead (§16.4). A deliberate change to loopback-only, owner decision 2026-11-12, #272. |
| MCP Docker clients | Opt-in **Allow Docker clients** setting, off by default: the HTTP server also accepts `Host: host.docker.internal`, so clients in Docker Desktop (e.g. Open WebUI) can connect. Still bound to 127.0.0.1, token and `Origin` refusal unchanged (§16.4). Owner decision 2026-11-12, #270. |
| Plugin exception | Classpath-style "Locus Plugin Exception" drafted in [ADR-0006](adr/0006-plugin-licence-exception.md), 2026-11-04; not in effect until legal review (#83). |
| Booleans | Exact line/arc booleans owned by `locus-region`, sharing the region-detection arrangement; no flattening or arc refit (decided 2026-11-05, [ADR 0008](adr/0008-booleans-and-arcs.md)). |
| Plugin API | WIT package `locus:plugin@1.0.0` (draft until the M3 host): one world per plugin kind, none importing anything (deny by default), exporters get the same validated export model as built-in writers ([ADR-0009](adr/0009-plugin-api.md), 2026-11-05). Text ships in core, not as a plugin (§9). |
| Recipes | Mirror/patterns/offset are recipes in the document; instances are real driven entities, mirror solved by `Symmetric` constraints, patterns regenerated after the solve; instance IDs stay with the recipe ([ADR-0011](adr/0011-associative-recipes.md), 2026-11-10). |
| Offsets | Exact line/arc offsets (areas and open chains, round or miter joins) owned by `locus-region` as a Minkowski sum/difference through the ADR 0008 booleans; `cavalier_contours` measured and not chosen (decided 2026-11-10, [ADR 0010](adr/0010-offsets.md)). |
| Object style (M5) | **Decided** (#287, #284): each entity stores its own style (stroke colour or "by layer", stroke width, pen number, fill, opacity); `Operation::Draw` has no parameters any more. File format **v13** moves each Draw layer's pen onto that layer's entities; a default style is not written. Edits go through one `SetStyle` command per user action (a whole multi-selection is one undo step). Export: Draw layers write each object's own style; the machine layers (Cut, Score, Engrave, Drag-knife) write the machine profile's colour and hairline, because LightBurn and Cricut read colour as the operation. Owner decisions 2026-11-13, [ADR 0017](adr/0017-fusion-style-ui.md). |
| Raster export (M5) | PNG joins SVG, PDF and DXF as a built-in export format, in the export dialog, the OUTPUT ribbon tab and `locus-cli` (§8 Formats). Rasteriser `tiny-skia` (BSD-3-Clause) with its `png` encoder, pure Rust without SIMD so the core crates still build for wasm32. Owner request 2026-11-14, #332. |
| UI (M5) | Fusion-style sketch UI: ribbon grouped Create / Modify / Constrain / Inspect / Insert / Select, browser, sketch palette, context inspector, right-click marking menu (pan on middle-drag / Space+drag), Fusion's sketch keys, one pick → drag/type → Enter/Esc → editable-dimension tool model; built on an action registry and a tool trait. Style (stroke colour, width, pen number, fill, opacity) belongs to each object, not the layer; placed text stays re-editable. Owner decisions 2026-11-13, #293, [ADR 0017](adr/0017-fusion-style-ui.md). |
| Machine profile per document (M5) | **Decided** (owner, #419, 2026-11-18): the document's machine profile drives more than export. A profile names its machine kind (laser, pen plotter, vinyl cutter or generic), which sets the operation new layers get (Cut, Draw, Drag knife; Draw when generic or no profile) and the operations layer pickers offer. Its bed is drawn on the canvas, Machine preview uses it, and every export defaults to it, `locus-cli export` included. It is chosen from the app bar, File ▸ Machine profile and the Inspector. Changing it is one undoable `SetProfile`, and layers are never rewritten silently: the app offers to update them, as one more undo step. The document still stores only the id (format v11), so no format change. The machine kind lives in the profile JSON and defaults to generic. |
| Job placement on the bed (M5) | **Decided** (owner, #443, 2026-11-18): Machine preview became a dockable pane (it can also float in its own window). It shows the profile's bed at true proportion with the job drawn as the machine makes it. The job can be dragged on the bed, snapping to the edges, centre and corners, or placed by numeric X / Y from the machine origin. The placement is a document offset (format **v18**) set by one undoable `SetPlacement`. Once placed, export moves the design by that offset and writes the whole bed as the page. Until then, export frames the design itself, as before. The export dialog shows the same bed view. |
| Fills belong to regions (M5) | **Decided** (owner, #451, 2026-11-18, supersedes the paint-order part of #422): every closed region (a `locus-region` face) has its own fill colour and opacity. Objects have no fill. Strokes always draw above every fill, on the canvas and in SVG, PDF and PNG, so no fill can hide a line. A region's fill is set by selecting the region (#370) and using the Inspector's Fill section. Format **v19**: object fills migrate to the regions whose outer boundary they formed, the regions the canvas showed filled since #338. Holes stay open. Fills stay visual: only Draw layers export them, and machine output and Engrave hatch are unchanged. |
| Engrave modes (M5) | **Decided** (owner, #455, 2026-11-18): Engrave layers get a mode, as in LightBurn: Fill, Line, Line + Fill or Offset fill. They also get an overlap rule, Even-odd or Union. Format **v20**. Older Engrave layers migrate to Line + Fill, not Fill, because they kept their outline and their export must not change; the ticket's "existing layers become Fill" is flagged in the PR. Machine preview draws the engrave hatch or rings faint, so the spacing reads and the region's fill shows through, and the legend explains it. The mode is per layer. A per-object override doesn't fit the model, since operations belong to layers (§4); put shapes on another Engrave layer instead. |
| Regions per layer (M5) | **Decided** (owner, #457, 2026-11-19): regions are built per layer, so another layer's shape never cuts, fills or hides a layer's region. One resolver gives the canvas, the Inspector and export the same fill. When an edit on the same layer splits a filled region, every piece inside the old region keeps the fill. Format **v21** moves old cross-layer region keys (fills and booleans) to the per-layer region in the same place. |
| Styles (M6, proposed) | **Proposed** ([ADR 0019](adr/0019-line-brush-fill-styles.md), #425, 2026-11-24; owner sign-off of [`docs/ux/m6-mockup.html`](ux/m6-mockup.html) pending): each object keeps its own line style (caps, joins, dashes, width profile, multi-strokes, a brush reference); brush definitions are copied into the document and kept in a user library; gradients go on region fills only. A new core crate `locus-stroke` expands styles into real geometry (exact on ADR 0010 offsets where it can; variable width measured by spike first), bounded as §6.2.1 and wasm32-clean. Machine layers cut the centreline unless the layer is set to **Styled**; pen plotters get dashes as pen-downs and wide strokes as outline + hatch. One format bump per ticket that stores data. Gradients are a per-fill option **Rotate with shape** (default on). Auto trace is Locus's own implementation of the published Potrace algorithm. Whether variable-width outlines keep cubic Béziers or only biarcs is decided by spike S1. |
| Owner decision 2026-11-24 (M6 design, #425) | **Decided** (owner, on ADR 0019's open questions): (1) §1's non-goals are reworded: gradients are visual region fills with a defined machine result, and the timeline is undo history plus recipes, not a feature tree. (2) Each gradient fill has a **Rotate with shape** option; when on, its frame follows the region's move, rotation and scale instead of the axis-aligned box. (3) Auto trace is written in Locus from the published Potrace algorithm (Selinger 2003): no Potrace code and no licence exception. (4) Arcs vs Béziers for variable-width, calligraphic and paint outlines is a measured question in spike S1, decision pending. |
| Milestones M5 and M6 | M5 "Fusion-style UX" was added from owner feedback after M3 and accepted on 2026-11-23 (#295). M6 "Line, brush and fill styles" was planned by the owner on 2026-11-18 (epic #424) and starts with a design pass and ADR (#425). Both are listed in §13. |

---

## 1. Purpose

A cross-platform Rust app (desktop and web) for creating precise 2D vector designs using **Fusion 360–style constrained sketching**, outputting **SVG, PDF and DXF** for:

- Pen plotters
- Vinyl cutters / Cricut-class machines
- Laser cutters / engravers

It replaces the Inkscape/Affinity workflow where precision is achieved by eye and snapping, with a geometric constraint solver.

### Goals
- Exact geometry via geometric + dimensional constraints, solved live.
- Machine-oriented output: layers map to **operations** (cut / score / engrave / draw), not visual styles.
- Clean, validated exports that machines and their host software (LightBurn, Cricut Design Space, plotter tools) accept first time.
- Native desktop on Linux, Windows, macOS.
- **Web**: the full editor runs in the browser (`wasm32-unknown-unknown`, hosted on GitHub Pages) with browser open/download and OPFS/IndexedDB autosave. Plugins and the MCP server are desktop-only at first (§3).
- Mouse + keyboard first; touch/pen input follows in M4 (§13).

### Non-goals (explicit)
- General illustration / Illustrator replacement (no raster effects, raster editing or artboards for print design). Gradients are allowed only as visual region fills with a defined machine result (M6, #436; owner decision 2026-11-24).
- A parametric feature tree. **One constrained sketch per document.** The M6 timeline (#446) is the session's undo history plus a list of the document's recipes, not a feature tree (owner decision 2026-11-24).
- 3D of any kind.
- Re-importing exported SVG/DXF as editable native documents (import as reference geometry only, later milestone).
- Direct machine control / G-code streaming (possible far-future plugin, out of scope).

---

## 2. Licence & governance

- **AGPL-3.0-or-later.**
- **CLA required from the first external contribution** (preserves option for dual-licensing or a hosted/WASM offering later). Set up CLA bot before accepting PRs.
- Dependency licence policy: MIT / Apache-2.0 / BSD / Zlib / MPL-2.0 / LGPL / GPLv3+ allowed, plus permissive equivalents (ISC, BSL-1.0, Unicode-3.0, CC0-1.0, MIT-0, 0BSD); the exact list is `deny.toml`. Anything GPLv2-only or proprietary is **forbidden**. Enforce with `cargo-deny` in CI from day one.

---

## 3. Architecture

Cargo workspace. Core is UI-agnostic and headless-testable; must also compile to `wasm32-unknown-unknown` (no filesystem/threads assumptions in core crates). The web build ships the same core crates plus the GUI shell; it is a first-class target, not a demo.

**Desktop-only at first:** `locus-plugin-host` (`wasmtime` does not run inside a browser) and `locus-mcp` (needs a local process / loopback socket). The web build compiles without them; a browser plugin runtime is a later epic (§13).

```
crates/
  locus-geom/        # Entities, math, tolerances, arc/line/spline primitives (f64, mm)
  locus-solver/      # Solver trait + backend(s); DOF analysis; conflict reporting
  locus-doc/         # Document model, layers, IDs, undo/redo command stack, file format (de)serialisation
  locus-region/      # Closed-profile/region detection, non-destructive booleans, offsets
  locus-export/      # Validation pipeline + SVG / PDF / DXF writers, kerf/tool offset application
  locus-text/        # Single-line (Hershey/EMS) + outline font → geometry
  locus-plugin-api/  # WIT interface definitions + Rust guest SDK for plugin authors (see §15)
  locus-plugin-host/ # wasmtime-based plugin runtime, capability enforcement, plugin registry
  locus-mcp/         # MCP server exposing document/sketch operations to external AI clients (see §16)
  locus-app/         # GUI shell for desktop + web (framework chosen by ADR — see §12)
  locus-cli/         # Headless: validate/export a .locus file (used in tests + CI); builds the `locus-cli` binary
```

### Key crate candidates (verify licences + maintenance before adopting)
| Need | Candidate |
|---|---|
| Curve math | `kurbo` |
| Offsets with true arcs (kerf, inset) | ~~`cavalier_contours`~~ — Locus-owned in `locus-region` ([ADR 0010](adr/0010-offsets.md)) |
| Booleans | ~~`i_overlay` or `geo`~~ — Locus-owned exact arrangement in `locus-region` ([ADR 0008](adr/0008-booleans-and-arcs.md)) |
| Tessellation for rendering | `lyon` |
| DXF write | `dxf` crate |
| PDF write | `krilla` or `printpdf` |
| Font parsing & shaping | `skrifa` (parsing, Google Fonts `fontations`) / `harfrust` (shaping, HarfBuzz port). Replaces `ttf-parser` / `rustybuzz`, which are unmaintained (RUSTSEC-2026-0192). |
| Serialisation | `serde` + `ron` (or JSON) |

### Solver backend
- Define a `Solver` trait in `locus-solver`; backends are swappable.
- **Pure Rust only (decided 2026-11-04).** The solver must build for `wasm32-unknown-unknown` with the rest of the core, so C/C++ backends are out. The earlier libslvs (SolveSpace, GPLv3+) and PlaneGCS (FreeCAD, LGPL-2.1+) FFI spikes are **dropped**; both remain behaviour references (§18).
- Spike in Milestone 0, pick one by ADR:
  - **SolidCraft's sketch solver** (pure Rust, MIT OR Apache-2.0 — see §18). **Spike this first.** Determine whether its 2D sketch/constraint code is separable (or can be extracted as a crate). Assess: crate boundaries, coupling to its 3D kernel, robustness on the §5 constraint set, drag performance, test coverage. Project is young and largely AI-assisted, so verify quality with our own test suite rather than trusting it. Retain upstream copyright/licence notices on any reused code.
  - Any other mature pure-Rust 2D GCS; adopt only if it handles the constraint set in §5 with acceptable robustness.
  - **Our own** solver in `locus-solver` (e.g. Newton / Levenberg–Marquardt over constraint residuals with graph decomposition), if neither of the above is good enough.
- Solver requirements: incremental re-solve during drag (target < 16 ms for 500 entities), DOF count, identification of over-constrained / redundant / conflicting constraints.
- **Chosen ([ADR 0007](adr/0007-solver-backend-without-paste.md), superseding [ADR 0002](adr/0002-solver-backend.md)'s `ezpz`):** our own solver in `locus-solver`: sparse Levenberg–Marquardt over Locus's residuals (exact gradients by dual numbers), its own sparse LDLᵀ with a minimum-degree ordering, and its own Jacobian-rank DOF and redundancy analysis. No third-party dependencies. Measured with the harness (now `locus-solver`'s test suite and `cargo bench -p locus-solver`), natively and in `wasm32`: 500-curve worst drag frame 1.4 ms native, 1.7 ms wasm.

---

## 4. Document model

### Units & precision
- Internal unit: **millimetres**, `f64`.
- Display units selectable (mm default; cm, m; inch display allowed but stored as mm).
- Global tolerance constant for coincidence/closure checks (default 1e-6 mm for solver, 0.001 mm for export validation).

### Entities (all have stable `EntityId` — document-scoped monotonic u64, never reused; [ADR-0003](adr/0003-entity-ids.md))
**v1**
- Point
- Line segment
- Circle
- Arc (centre/start/end)
- Cubic Bézier segment (freeform; limited constraints — endpoint coincidence, endpoint tangency, fix)

**v2**
- Ellipse / elliptical arc
- B-spline / fit-point spline
- Text (as a container entity that generates geometry)

Each entity carries:
- `layer: LayerId`
- `construction: bool` — never exported, rendered dashed. Independent of layer.
- `style` (M5, #287): stroke colour, stroke width, pen number and opacity. Per object, not per layer ([ADR 0017](adr/0017-fusion-style-ui.md)). **Objects have no fill since format v19 (#451):** fills belong to regions (§6.3). The style keeps a `fill` field only for older files, which migrate it to regions; commands and files refuse a non-empty object fill.

### Layers
- Layers are **properties on geometry**, not separate sketches. One shared solver space for the whole document.
- Layer fields: `id`, `name`, `colour` (display), `operation`, `export_order`, `visible`, `locked`, `export_enabled`, operation params (see §7).

### Layer rules (decided)
1. **Constraints reference `EntityId`s only, never layers.** Moving an entity between layers preserves all constraints.
2. **Hide** has no solver effect.
3. **Lock** → that layer's geometry is treated as **fixed** by the solver. Before locking, check for constraints from unlocked layers that would become over-constrained/conflicting; warn and list them.
4. **Delete layer** → warn, listing every constraint that will be dropped (with the entities on other layers they touch). Deletion is **one atomic undoable command** restoring layer, geometry and all dropped constraints together.
5. Never silently drop or modify constraints.

---

## 5. Constraints

### Geometric (v1)
Coincident, point-on-curve, horizontal, vertical, parallel, perpendicular, tangent, collinear, concentric, equal (length/radius), symmetric (about line), midpoint, fix.

### Dimensional (v1)
Distance (point–point, point–line, line–line parallel), horizontal distance, vertical distance, angle, radius, diameter, arc length (v2).

### Expressions / parameters
- Named document parameters (e.g. `material = 3`, `kerf = 0.15`) usable in dimensional constraints: `material * 2 + 0.2`.
- Simple expression evaluator (arithmetic, parentheses, `min/max/sqrt/sin/cos/tan`, degree default for angles). No scripting language in v1.

### Stroke-edge constraint reference (v2, differentiator)
- Dimensional constraints may optionally reference a **stroke edge** (offset by pen width / kerf) rather than the zero-width centreline. Primarily relevant to plotter pen width and laser kerf compensation previews.
- **Decided ([ADR 0014](adr/0014-stroke-edge-dimensions.md), #239):** a dimension carries `edge` = centreline / inside / outside; its value is measured to that edge and the solver is given the centreline value (±half the pen width or kerf of each stroked operand). Distances with a line, radii and diameters support it; point-to-point, horizontal / vertical distances and angles do not. Changing the pen width or kerf re-solves; a change that would make a centreline value negative is refused whole. `.locus` format v12.

### Solver UX
- Sketch status: under-constrained (show remaining DOF count), fully constrained (geometry turns distinct colour, as in Fusion), over-constrained (highlight conflicting set, refuse to apply new conflicting constraint, offer to make it a driven/reference dimension).
- Driven (reference) dimensions supported.

---

## 6. Tools

### 6.1 Sketch tools (v1)
Line / polyline, rectangle (2-point, centre, 3-point rotated), circle (centre, 2-point, 3-point), arc (3-point, centre, tangent), polygon (inscribed/circumscribed, n sides), slot, Bézier pen tool, point.

### 6.2 Modify tools (v1)
Trim, extend, break/split, fillet (adds tangent arc + constraints), chamfer, offset (live/constrained offset entity), mirror (adds symmetric constraints), linear pattern, circular pattern, move/rotate/scale (respecting constraints), toggle construction.

Offsets (the offset tool, kerf and blade offsets) share one engine that keeps true arcs: `locus-region` offsets areas and open chains as an exact Minkowski sum/difference through the ADR 0008 arrangement, with round or miter joins (decided 2026-11-10, [ADR 0010](adr/0010-offsets.md)). The live offset is an ADR 0011 recipe over a chain of curves (distance signed: + grows a closed chain / goes left of an open one; a number or an expression); its curves are regenerated after every solve in the same undo step, stay out of the solver and can't take constraints until exploded (v1 limitation, like pattern instances).

Text on a path (M5, #392, owner's flow): the Text on path tool binds an **existing** text to a chain of curves (a line, arc, circle, shape outline or Bézier): click the text, then the path, as one command. The binding is part of the text (`TextData::path`, format v17: the curves, offset, side, flip and a per-glyph cache laid out with the font), so it stays one text entity, re-editable in the text dialog (#222), and its geometry is its glyphs bent along the path whenever it is drawn or exported, so it follows the path when the path changes, without the font. Dragging bound text slides it along the path (one undo step on release). The path runs left to right when open and clockwise when closed (a circle from its bottom, so centred text sits on its top); the text's own alignment places it, and the offset slides it from there. The Inspector sets side (above or below), flip and offset, and Detach puts it back as straight text at its anchor. A path that is deleted leaves the text straight.

Auto-constraint inference while drawing (horizontal/vertical/coincident/tangent snap hints), toggleable — mirrors Fusion behaviour.

### 6.2.1 Mirror & pattern (detail)

All are **associative**: stored as a recipe (source entity IDs + parameters), regenerated on solve, so editing the source updates every copy. Each has an **Explode** command to convert copies into ordinary independent geometry.

**Mirror**
- Mirror line: any line or construction line (can itself be constrained/dimensioned).
- Implemented by adding **symmetric constraints** between source and mirrored points (Fusion behaviour), not by copying coordinates — the mirror solves with the sketch.
- Option: merge mirrored geometry that lands on the mirror line (avoid duplicate segments → double cuts).

**Linear (rectangular) pattern**
- Direction 1 + optional direction 2 (each defined by a line/edge or X/Y axis).
- Per direction: count, and either spacing or total extent; symmetric option (pattern both ways from source).
- Count and spacing accept **expressions / document parameters** (e.g. `count = floor(width / pitch)`).
- Per-instance **suppress** (click to skip an instance, e.g. remove holes that hit a fold line).

**Circular pattern**
- Centre: any point (constrainable). Count + total angle (default 360°) or angular spacing; symmetric option.
- Option: rotate instances with the pattern or keep orientation fixed.
- Per-instance suppress; expression-driven parameters.

**Pattern along path (v2)**
- Instances spaced along a curve/profile by count or pitch, with orientation tangent-to-path or fixed. Useful for perforations, stitching holes, decorative borders.

**Constraint behaviour**
- Source geometry is fully editable and constrainable. Instances are **driven**: they can be referenced by other constraints (e.g. dimension to an instance) but not directly dragged.
- Pattern parameters appear as editable dimensions on canvas, like Fusion.
- Instance count limit with warning (default 10 000 instances) to protect solver/render performance.
- **Decided ([ADR 0016](adr/0015-two-phase-solve-for-pattern-instances.md), #194):** constraints onto pattern instances (geometric, driving and driven) are solved in a second phase with the pattern, its sources and what it reads held fixed, so they move only other geometry; one that would need the pattern to move is refused. Offset and plugin-object instances still take no constraints. Mirror instances are solved and accept any satisfiable constraint.

**Export interaction**
- Overlapping instances are flagged by the §8 duplicate/overlap validation.
- Patterns are flattened to plain geometry on export (DXF/SVG have no associative patterns).

### 6.3 Regions & booleans
- Automatic closed-profile detection from sketch geometry (as Fusion does), producing **regions**.
- **Regions are per layer (M5, #457, format v21).** Each layer's regions come only from that layer's curves, because layers are separate machine operations. A shape on another layer never splits, fills or hides a region; overlaps within one layer still make separate regions. Layers' regions may overlap each other: a click selects the active layer's region first, then the innermost. Region keys name entities, which belong to one layer, so keys never collide. Loading a v19 or v20 file moves every fill and boolean key that named a face cut by another layer's shape onto the per-layer region in the same place.
- **One fill resolver (#457).** The canvas, the Inspector and export all ask the same function for a region's fill: the fill stored under its key (a stored "no fill" hides an inherited one), or else the fill of a region that an edit on the same layer split. That region is rebuilt from its own curves on the current geometry, and **every piece inside it keeps the fill**; a new region outside it doesn't take the fill. Export writes the stored colour and opacity. Per-layer files carry only their layer's shapes and fills, all on the same page and origin so they line up in LightBurn and Cricut, and the export dialog says so.
- **Region fills (M5, #451).** Each region has its own fill: a colour, or none while remembering the last colour (#388), plus an opacity from 0 to 100 %. Fills are keyed by the region's stable key, the entities on its outer boundary (ADR 0008). A fill whose region disappears is kept and comes back with the region. Nested or overlapping shapes give separate regions, filled independently. For example, the ring between a rectangle and an inner loop is its own region, apart from the loop's inside.
  - Setting a fill: select regions (click inside a closed shape, #370), then use the Inspector's **Fill** section (on/off, colour, opacity). Each change is one undoable step (`SetRegionFill`).
  - Drawing: the canvas draws fills under all geometry, at their opacity, while Show fills is on.
  - Export: SVG, PDF and PNG write every fill, even-odd so holes stay open, before any stroke. SVG puts them in one `fills` group, with `fill-opacity`; PDF and PNG use alpha.
  - Machine output: fills stay visual. Only regions on Draw layers export a fill, because machine software reads every path as a cut. Engrave hatch is unchanged: it already fills the faces enclosed by the Engrave layer's paths, holes respected.
- Non-destructive boolean result objects (union/subtract/intersect of regions), stored as recipes referencing entity IDs and recomputed on solve.
- **Known tension:** most Rust boolean libs flatten to polylines; DXF/laser output strongly prefers true arcs. Approach: perform booleans on flattened geometry, then **arc-refit** the result (or use `cavalier_contours` for arc-preserving cases). Spike in Milestone 2.
- **Decided ([ADR 0008](adr/0008-booleans-and-arcs.md), 2026-11-05):** `locus-region` owns an exact line/arc arrangement (split at intersections and overlaps with `SOLVER_TOL` snapping, de-duplicate, classify edges by winding number, trace loops, merge pieces on one line or circle). Region detection and booleans share it; no flattening, no refit, no new dependencies. Measured: 15,000/15,000 random cases within tolerance (flatten + `i_overlay` + refit 99.6 %, `cavalier_contours` 98.9 %, failing on shared edges); deviation below 1e-7 mm; minimal arc counts; 28 KB gzipped in wasm32.

---

## 7. Operations & machine output

Layer `operation` enum:
| Operation | Params |
|---|---|
| Cut | kerf width, inside/outside/none compensation, pass count (metadata) |
| Score | — |
| Engrave (fill) | hatch spacing, hatch angle, crosshatch; **mode** (M5, #455, format v20): **Fill** hatches the inside without the outline, **Line** traces the outline only (the edge mark; Score marks an edge too), **Line + Fill** does both, **Offset fill** fills with concentric rings the spacing apart. Engrave layers saved before v20 hatched and kept the outline, so they migrate to Line + Fill and export unchanged; new Engrave layers start as Fill, as in LightBurn. **Overlaps** (#455): **Even-odd** (default, LightBurn's) leaves overlaps and shapes inside shapes empty, so holes and crossing overlaps are not engraved and an island in a hole is; **Union** engraves overlapping shapes as one area. Open paths can't be filled and are always kept. Fill is the machine operation, so a region's visual fill (§6.3) doesn't turn it on or off. |
| Draw (plotter) | — **M5 (format v13):** pen number and pen width are each object's style (§4), not layer parameters (owner decision 2026-11-13, #287, [ADR 0017](adr/0017-fusion-style-ui.md)). An object's pen width drives its stroke-edge preview and stroke-edge dimensions (ADR 0014) and its exported stroke width. |
| Drag-knife (vinyl) | blade offset (swivel arcs at turns over 10°) |
| Reference | never exported |

Machine profiles (M4, #236): a profile bundles an export format (an exporter id), a colour per operation, display units, the bed (size and origin corner) and default operation parameters. Built-in presets are compiled in: LightBurn (SVG, DXF), Cricut Design Space (SVG, per-shape styles), Silhouette Studio Basic (DXF R12; Cameo 12" and 24" mats), Silhouette Designer+ (SVG), generic HPGL (via the HPGL exporter plugin) and generic SVG / DXF. The laser bed defaults to a generic 400 × 400 mm and is editable. User profiles are JSON in app settings; one profile can be exported or imported as a standalone `.json` file (imports are untrusted: size-bounded and validated). A document stores only the selected profile id (§10, format v11).

**Per-document machine (M5, #419).** Each profile also names its **machine kind**: laser cutter, pen plotter, vinyl cutter or generic (profile JSON field `machine`, default generic). The kind decides what layers can be:

| Machine | New layers | Operations offered |
|---|---|---|
| Laser cutter (LightBurn presets) | Cut | Cut, Score, Engrave, Reference |
| Pen plotter (HPGL preset) | Draw | Engrave (pen hatch), Draw, Reference |
| Vinyl cutter (Cricut, Silhouette presets) | Drag knife | Score, Draw, Drag knife, Reference |
| Generic (generic SVG / DXF, no profile) | Draw | all |

A new layer also gets the profile's default kerf or blade offset. A layer whose operation the machine doesn't run keeps it, and its picker still lists it. The document's profile is chosen from the app bar (next to the document name), File ▸ Machine profile, the Inspector's Document section or the export dialog. Choosing one is a single undoable `SetProfile` (§11). It **never rewrites layers silently**. If some layers don't fit the machine, the app lists them with the operation each would get: the same operation if offered, Cut ↔ Drag knife for a through-cut, otherwise the machine's default. Accepting changes them all as one undo step. The profile's bed (size and origin corner) is drawn on the canvas, and Machine preview uses the profile (§8).

**Machine preview (#423).** The preview draws every exported layer's machine result over the sketch. It comes from the same `prepare_for` call, with the document's profile, that the exporters use:
- Cut: the kerf-compensated path with a band of the true kerf width (#224).
- Drag knife: the blade-offset path with its swivel arcs.
- Engrave: the outline and its hatch.
- Score: the hairline.
- Draw: each object's stroke at its pen width.

Each is tinted in its operation's editor colour, and a legend lists every layer's operation (#420). When the preview adds nothing, the legend says why. Examples: no machine profile; nothing exported; a Cut layer without kerf compensation; a Drag knife with no blade offset; "Draw layers have no machine compensation".

**Job placement (M5, #443).** Machine preview is a dockable pane, and it can also float in its own window; the View toggle opens and closes it. It shows the profile's bed at true proportion, with:
- its 10 mm grid;
- the origin corner;
- the machine paths, as above;
- the job's bounding box, red when it leaves the bed.

The legend and the "why" notes sit below the bed. The canvas keeps the tinted paths but no longer has the legend box.

The user places the job in one of these ways:
- Drag it. It snaps, within 8 points, to the bed's edges and centre on each axis, which includes the corners.
- Type **X / Y**: the distance from the machine's origin corner to the job's nearest corner, positive into the bed, in the display units.
- Press **Centre** or **To origin**.
- Press **Unplace** to clear the placement.

Each change is one undoable `SetPlacement`. The placement is a document offset in mm, stored in the file (§10, v18), bounded to ±20 m and finite. It applies only when the profile has a bed. The canvas draws the bed moved back by that offset, so the sketch stays where it was drawn. The pane says when the job doesn't fit: either larger than the bed, or reaching past an edge, and by how much. The export dialog shows the same bed view with the same controls.

---

## 8. Export pipeline

```
Document → Solve → Collect exportable geometry (exclude construction, reference, export_enabled=false)
        → Apply operation transforms (kerf/blade offsets, hatch fills, text → geometry)
        → Validate → Order/optimise → Write format
```

### Validation (blocking errors vs warnings, all reported in a pre-export panel)
- Open paths on Cut layers (error)
- Self-intersections (warning)
- Duplicate / overlapping segments → double cuts (error, offer auto-merge)
- Zero-length / degenerate segments (auto-remove, report)
- Geometry outside machine bed (warning, when profile has bed size)

### Optimisation
- Cut order: inner contours before outer containing contours.
- Path joining to minimise pen lifts / travel; nearest-neighbour travel ordering (v1), improved heuristics later.

### Formats
- **SVG** — plain SVG 1.1, mm units via `viewBox` + `width/height` in mm, one `<g>` per layer with operation encoded in `id`/`data-*` and stroke colour. Test against Cricut Design Space and LightBurn import.
- **PDF** — vector, true scale, one optional content group per layer.
- **DXF** — **R12** default for compatibility (LINE, ARC, CIRCLE, LWPOLYLINE-free path where needed), layers preserved, units = mm. Optional newer DXF later.
- **HPGL** — v2 / optional.
- **PNG** — raster image of the export geometry (#332): resolution as DPI (default 300) or pixel width, transparent or white background, the whole sketch or the selection, one file per layer like the vector formats. Same paint as SVG (§7); strokes under a pixel draw one pixel wide. Rasterised in `locus-export` with `tiny-skia` (no SIMD, so wasm32 builds); image size bounded (20 000 px a side, 100 MP). Also `locus-cli export --format png [--dpi n | --width-px n] [--background transparent|white]`.

**Default profile (#419).** Every export starts from the document's machine profile: the export dialog preselects it, a direct format export (File ▸ Export ▸ SVG…, PNG…) still applies its colours, hairlines and bed check, and `locus-cli export` uses it when no `--format`, `--profile` or `--profile-file` is given. The CLI knows only the built-in presets, so a document whose profile is a user profile needs `--profile-file`. **Placed jobs (#443):** when the document has a placement and the profile a bed, `prepare_for` moves the design by the placement (after the operation transforms, before validation, so the bed check sees the placed job) and sets the page to the whole bed, for every writer and for per-layer files. Without a placement the page is the design's own bounds, as before.

**Page bounds include strokes (#450).** The SVG, PDF and PNG page is the *visual* bounds: the geometry plus half of each path's stroke width (the hairline on machine layers, the object's width on Draw), so a thick stroke is never cut off at the edge. Coordinates are measured from an anchor: the frame when there is one (the #443 bed, or the whole design's page for per-layer files), otherwise the visual bounds. Padding that reaches past the anchor is written as a negative `viewBox` origin (SVG) or a `MediaBox` origin (PDF), so a placed job keeps its coordinates. Per-layer files share the whole design's page. Cap and join extents follow when M6 adds caps and joins. DXF has no page and is unchanged. This applies with or without a machine profile. With no bed, the page is simply the visual bounds. With a bed, a Draw object whose line sits inside the bed but whose pen reaches past its edge gets an "outside bed" warning, naming the stroke. The page grows to show the stroke instead of clipping it, and the Machine preview pane says the job doesn't fit. A machine layer's hairline is a file convention, not a real width, so a cut touching the edge stays inside.

Export is always one-way. Native file is the source of truth.

---

## 9. Text

- **Single-line fonts** (Hershey, EMS/SVG single-stroke) for plotters/engraving — required v1-ish (Milestone 3).
- Outline TrueType/OpenType → geometry for cutting.
- Text ships in core (`locus-text`), not as a plugin: it must work on the web, where plugins aren't available yet, and it's a live constrainable entity ([ADR-0009](adr/0009-plugin-api.md), decided in M2).
- Text entity has a constrained baseline/anchor (can be constrained like any entity, e.g. centred on a region).
- Bundle only fonts with compatible licences (OFL/public domain).
- Representation, caching and font references: [ADR-0012](adr/0012-text-entity.md). Documents store the font *reference* and the cached geometry, never the font file; user fonts and (desktop only) system fonts are resolved by the app shell.
- **System fonts (M6, #477):** the desktop lists installed fonts by asking the OS: DirectWrite on Windows, Core Text on macOS and fontconfig on Linux, with the font-folder scan as the fallback. Fonts the OS registers for the session, such as Adobe Creative Cloud fonts, are included while they are activated. A face is stored as its family name (regular) or "Family Style". Font files are read, never copied or embedded, and their paths are never logged ([ADR 0019](adr/0019-line-brush-fill-styles.md) decision 7).
- Outline glyphs whose contours overlap (common in variable fonts) are unioned into one outline when laid out, so a cutter never cuts through a letter (#222).

---

## 10. Native file format

- Extension: `.locus`.
- Human-readable **RON** for clean git diffs and easy debugging ([ADR-0004](adr/0004-file-format.md); JSON only as an export view).
- Top-level: `format_version`, `units`, `next_id` (ID counter, [ADR-0003](adr/0003-entity-ids.md)), `parameters`, `layers`, `entities`, `constraints`, `booleans` (boolean recipes over region keys, format v3, #123; regions themselves are derived and not stored), `view` (optional, can be ignored by diff tools).
- Entities carry their style (format v13, #287): omitted while it is the default, so points and unstyled curves add nothing to the file.
- Text entities (format v9, #179) store content, a font reference and the laid-out geometry, so a file opens and exports without the font ([ADR-0012](adr/0012-text-entity.md)).
- Text letter spacing (`tracking`, mm; format v14, #222) is omitted while zero, so older files load unchanged.
- Region fills (`region_fills`, region key → fill and opacity; format v19, #451) are omitted while empty. Loading an older file moves each object fill onto the regions whose outer boundary that object formed, at the object's opacity, then clears the object fills.
- Engrave `mode` and `overlap` (format v20, #455). An Engrave layer without them reads as Line + Fill with even-odd overlaps, which is what it exported before.
- Region keys (fills, boolean recipes) name per-layer regions from format v21 (#457). The shape of the file is unchanged; an older file's keys that named a face cut by another layer are moved on load.
- The job's placement on the machine bed (`placement`, an offset in mm; format v18, #443) is omitted while unset, so a v17 file migrates unchanged.
- Stores **solved positions** as well as constraints so files open instantly and degrade gracefully if a solve fails.
- Versioned migrations in `locus-doc` (`v1 → v2 …`), tested with fixture files.
- Deterministic serialisation (stable ordering) so unchanged documents produce identical files.

---

## 11. Undo/redo

- **Architectural requirement (from M1):** every mutation goes through a **serialisable command API** in `locus-doc` (serde-derived, schema-describable). The GUI, CLI, plugins and MCP server are all just clients of this API. Never mutate the document from UI code directly — retrofitting this later is what makes MCP/automation painful.
- Command pattern in `locus-doc`. Every user action = one command; compound commands for multi-step ops (fillet, layer delete, pattern).
- Solver results are not part of command state; re-solve after undo/redo.

---

## 12. Decisions (resolve in Milestone 0)

Status as of 2026-11-04. Items marked **Spiking** or **Open** close with an ADR in `docs/adr/` containing measured results (see [`docs/adr/README.md`](adr/README.md)); ADR numbers are assigned when each ADR is written.

1. **GUI framework** — **Decided** ([ADR 0001](adr/0001-gui-framework.md)): `eframe` 0.35 + `egui_dock` + `wgpu`. Faster, ~⅓ the binary, ~½ the web download and build time of Bevy 0.17 + `bevy_egui`; owner's hand test agreed.
2. **Canvas renderer** — **Decided** ([ADR 0001](adr/0001-gui-framework.md)): framework-native egui painter first (1.07 ms / 5 000 lines); a `wgpu` paint callback (0.38 ms) is the upgrade path; `lyon` reserved for fills/hatching.
3. **Solver backend** — **Decided** ([ADR 0007](adr/0007-solver-backend-without-paste.md), superseding [ADR 0002](adr/0002-solver-backend.md)): Locus's own sparse Levenberg–Marquardt solver behind the `locus-solver` `Solver` trait, with its own sparse LDLᵀ and Jacobian-rank DOF/redundancy analysis. `ezpz` was dropped because its `faer → gemm/pulp` tree needs `paste` (RUSTSEC-2024-0436). SolidCraft stays a behaviour reference (§18), not a dependency.
4. **Entity ID scheme** — **Decided:** document-scoped monotonic `u64`. [ADR-0003](adr/0003-entity-ids.md).
5. **RON vs JSON** for the file format — **Decided:** RON. [ADR-0004](adr/0004-file-format.md).
6. **App name** — **Decided:** Locus (§0).
7. **Plugin licence exception wording** (see §15.6) — **Decided in principle:** the exception will exist and the licence split is as §0. Wording drafted in [ADR-0006](adr/0006-plugin-licence-exception.md); **legal review outstanding** before the first public release (#83).
8. **MCP SDK version + MCP spec revision** (see §16) — **Decided** (desktop-only): `rmcp` 3.8, MCP revision 2026-07-28 with 2025-11-25 fallback. [ADR-0005](adr/0005-mcp-sdk.md).
9. **Toolchain** — **Decided:** Docker dev container + native macOS CI runner, no host Rust toolchain (§0).
10. **Web hosting & browser storage** — **Decided:** GitHub Pages; open/download + OPFS/IndexedDB autosave (§0).
11. **Packaging formats** — **Decided:** see §0 and M4 (§13).
12. **Booleans that keep arcs** (§6.3, M2) — **Decided** ([ADR 0008](adr/0008-booleans-and-arcs.md)): Locus-owned exact line/arc arrangement in `locus-region`; `i_overlay` + refit and `cavalier_contours` measured and not chosen.
13. **Offsets that keep arcs** (§6.2, §7, M3) — **Decided** ([ADR 0010](adr/0010-offsets.md)): Locus-owned offsets on the ADR 0008 arrangement (bands + corner discs/kites, one boolean); 6,000/6,000 random cases exact vs `cavalier_contours` 5,995 (wrong by up to 1.5 mm where offsets touch exactly); no new dependencies.
14. **Browser plugin runtime** (§15, M4) — **Decided** ([ADR 0016](adr/0016-web-plugin-runtime.md)): run the component's core module in a dedicated Web Worker (empty imports, memory maximum ≤ 256 MB, terminated on timeout), with the canonical ABI of `locus:plugin@1.x` in Rust in `locus-plugin-host`. wasmtime + Pulley compiled to wasm was measured and doesn't run (Cranelift needs `std::time`; +4.3 MB); jco not chosen (generated JS, same limits). The web host has no fuel budget; the timeout bounds every call.
15. **Fusion-style UI** (§6, §11, M5) — **Decided** ([ADR 0017](adr/0017-fusion-style-ui.md), #293): the layout, marking menu, shortcut map and tool model in [`docs/ux/m5-design.md`](ux/m5-design.md), on an action registry, a tool trait and `Canvas` split into state plus views (#294). Owner approved design revision 3 on 2026-11-13; constraints use symbols with no letter keys.
16. **solidcore kernel** (§3, M5) — **Proposed** ([ADR 0018](adr/0018-solidcore-kernel.md), #153): stay on `locus-solver` / `locus-geom`. solidcore is a 3D B-rep kernel with no constraint solver; licences pass `deny.toml`, but its git-only source fails `sources`. Needs the owner's approval before anything changes.
17. **Line, brush and fill styles** (§4, §6.3, §7, §8, §10, M6) — **Proposed** ([ADR 0019](adr/0019-line-brush-fill-styles.md), #425): style model, `locus-stroke` expansion engine, machine output per style, history/timeline, layer blending and interaction, OS fonts and auto trace; design in [`docs/ux/m6-design.md`](ux/m6-design.md). Spikes S1–S5 measure the open engine choices before their tickets (S1 also decides biarcs-only vs kept cubic Béziers for variable-width outlines). Owner answers of 2026-11-24 are in §0. Needs the owner's sign-off of the mockup before any M6 feature is built.

---

## 13. Milestones

**M0 — Spikes & scaffolding**
- Workspace, CI (fmt, clippy, test, `cargo-deny`, `wasm32` check of core crates), licence headers, CLA config.
- Docker dev container (Linux build/test, Windows cross-compile, `wasm32` + `trunk`); native macOS runner in CI.
- Solver spike (pure-Rust candidates, §3): solve a fixed test sketch (rect + filleted corners + circle concentric to arc + dimension); measure drag re-solve time at 500 entities, natively and in `wasm32`.
- GUI spike, both stacks (§12.1): pan/zoom canvas at mm scale, draw lines, hit-testing — on desktop **and** in the browser.
- Web build skeleton: `trunk` build of the GUI shell, deployed to GitHub Pages from CI.
- Decision record for each §12 item (`docs/adr/`).

**M1 — Core sketcher**
- Points, lines, circles, arcs; geometric + dimensional constraints; live solve on drag; DOF display; conflict detection.
- Native save/load; undo/redo.
- Plain SVG export (no validation yet).
- Web: the M1 sketcher runs on GitHub Pages, with browser open/download of `.locus` files and OPFS/IndexedDB autosave.

**M2 — Layers, regions, export**
- Layers with operations and the §4 rules (lock/delete checks).
- Region detection; booleans with arc refit.
- Validation pipeline; SVG/PDF/DXF R12; cut ordering.
- **Plugin API design** (WIT draft + ADR). Internal exporters written against the same `Exporter` interface so the boundary is proven before it's public.

**M3 — Production features**
- Fillet/chamfer/offset/mirror/patterns; parameters & expressions.
- Kerf/blade offsets; hatch fills; single-line + outline text.
- Bézier pen tool with limited constraints.
- **Plugin host v1**: generators, modifiers, exporters (§15). Two first-party example plugins (finger-joint box generator, HPGL exporter) + Rust guest SDK + template repo.

**M4 — Polish**
- **MCP server** (§16): stdio + localhost HTTP, read/write scopes, approval UI, preview rendering.
- Plugin importers + validators; plugin manager UI (install from file, enable/disable, view granted capabilities).
- **Browser plugin runtime** epic (run `.locusplugin` components in the web build, with the desktop's limits; [ADR 0016](adr/0016-web-plugin-runtime.md)). Moved here from "after M4" by the owner, 2026-11-11.
- Machine profiles, stroke-edge constraints, bed bounds, import reference geometry (SVG/DXF as locked reference layer).
  - Raster references (M5, #390): PNG and JPEG images on their own locked Reference layer, drawn under the sketch at their own opacity and never exported. The document keeps the original file (base64, format v16); files are bounded (32 MB, 16 384 px a side, 64 MP, 64 images) and decoded with `image`'s pure-Rust PNG and JPEG codecs, refusing damaged files without panicking.
  - Calibrate (M5, #391): as Fusion's, pick two points on a reference (snapping to its points, or anywhere on an image) and type the real distance; the whole reference (points, radii and image) scales as one `ScaleReference` command. The scale is stored in the reference, and the layer stays locked. **Pivot (#464, #468):** by default a reference scales about its **centre** (the middle of its image and geometry), so one centred on 0,0 stays centred. "Scale about" in the Calibrate dialog also offers **Anchor corner** (an image's placement origin or the bottom-left of a vector reference's geometry, so one placed at 0,0 stays at 0,0) and **First point**. The hint under the choice says which point stays put, and the choice is remembered. Typing an image's width in the Inspector uses the same choice (Centre or Anchor corner). Each scale is one undoable command.
- Packaging/installers: Windows Inno Setup installer + portable zip, macOS `.dmg`, Linux AppImage, Flatpak and `.deb`.
- **Touch/pen input** epic (tablet and stylus interaction on the canvas).

**M5 — Fusion-style UX** (owner feedback after M3/M4; closed 2026-11-23, acceptance #295)
- Fusion-style sketch UI to the owner-approved mockup and [ADR 0017](adr/0017-fusion-style-ui.md) ([`docs/ux/m5-design.md`](ux/m5-design.md)): ribbon (Create / Modify / Constrain / Inspect / Insert / Select), browser tree, sketch palette, context Inspector, marking menu, Fusion's keys, versioned dock layout with panes and dialogs as real OS windows. Built on an action registry and a tool trait.
- Fusion-style tools: pick → drag a handle or type → Enter/Esc, with editable dimensions. Move / Rotate / Scale, offset, fillet and chamfer handles, linear and circular patterns with handles and a picked direction, mirror, Bézier trim/extend/break, chain select, Delete key.
- Style per object (stroke, width, pen, opacity), and fills per region (§0). Per-layer regions, engrave modes, per-layer and PNG export.
- Machine profile per document: it drives the bed, the layer operations and the dockable Machine preview with job placement (§0).
- Text: re-editable, font picker, alignment, letter spacing, text on a path. Raster references (PNG/JPEG) and Calibrate (§13 M4 notes).

**M6 — Line, brush and fill styles** (epic #424; starts 2026-11-24)
- Design pass first (#425): a mockup the owner signs off, `docs/ux/m6-design.md` and an ADR for the style model, the core stroke-expansion engine (variable-width offset, dash splitting, path-mapped art, scatter and pattern placement; wasm32-clean, bounded counts as §6.2.1, never panics) and the UI.
- **Line styles:** width profiles and taper (#426), dashes and gaps (#427), caps and joins (#428), multi-strokes (#429).
- **Brushes:** calligraphic (#430), art (#431), scatter (#432), pattern (#433), vector paint and contour (#434), and a brush library and Brushes panel (#435).
- **Fills:** gradient fills (#436).
- **Machine output for every style:** Draw/plot emits the styled geometry, Cut/Score/Engrave use the real path or the expanded outline, SVG/PDF/PNG stay visual where the format allows, DXF/HPGL get expanded geometry, and an **Expand to geometry** command turns any style into editable curves. Styles are stored in `.locus` with a format bump and migration test, and every change goes through the command API (§11).
- Also in M6: edit timeline and history panel (#446), layer blending modes and an opt-in for cross-layer regions (#459), auto trace of raster references (#461), drag-reorder layers (#462), on-canvas move/rotate/scale handles for references (#463), larger Bézier handles (#470), and fonts listed from the OS so Adobe Creative Cloud fonts appear (#477).
- Last of all: the egui 0.37 upgrade (#397).

**Later (after M4)**
- MCP for the web build, if a safe transport exists.

---

## 14. Testing strategy

- **Golden-file tests**: `.locus` fixture → export → compare against committed SVG/DXF output (normalised).
- **Solver property tests** (`proptest`): random valid constraint sets converge; adding then removing a constraint returns to same DOF.
- **Round-trip tests**: save → load → save is byte-identical.
- **Migration tests** for every format version.
- **Validation tests**: fixtures with open paths, duplicates, self-intersections.
- Manual acceptance: import exports into LightBurn, Cricut Design Space, and a DXF viewer each milestone.

---

## 15. Plugin system

### 15.1 Approach
- Plugins are **WebAssembly components** run by `wasmtime`, with the interface defined in **WIT** (Component Model).
- Why: sandboxed by default, one binary runs on all three OSes, plugin authors can use any language that targets WASM components (Rust first-class via the SDK; others best-effort).
- Rejected: native dynamic libraries (`.so/.dll/.dylib`) — no sandbox, unstable Rust ABI, per-platform builds. Lua/Rhai scripting — possible later as a convenience layer for simple generators, but not the primary API.

### 15.2 Plugin kinds (v1 → v2)
| Kind | What it does | Milestone |
|---|---|---|
| **Generator** | Takes parameters → returns geometry (+ optional constraints). E.g. finger-joint box, living hinge, gear, Voronoi pattern. | M3 |
| **Modifier** | Non-destructive transform of selected regions/paths. E.g. tabs/bridges for cutting, dogbone corners, dashed score lines. | M3 |
| **Exporter** | Validated export geometry + layer ops → bytes. E.g. HPGL, G-code, machine-specific SVG dialects. | M3 |
| **Importer** | Bytes → reference geometry on a locked layer. | M4 |
| **Validator** | Extra pre-export checks (e.g. minimum feature size for a given material). | M4 |

Out of scope for v1: plugins adding arbitrary UI, new sketch tools with custom interaction, or new constraint types (solver stays core-only).

### 15.3 Interface principles
- Plugins **never touch the document directly**. Host passes immutable snapshots in, plugin returns data out; host turns the result into an undoable command.
- Generators/modifiers declare a **parameter schema** (number with unit + range, integer, bool, enum, string, document-parameter reference). Host renders the panel — no plugin UI code.
- Generator output geometry can be anchored by constraints like any other entity (the generated group gets a constrainable origin/frame).
- Interface is **versioned** (`locus:plugin@1.0.0`). Host supports current major + previous major. Breaking changes only on major bumps.

### 15.4 Security model (capability-based, deny by default)
- No filesystem, network, clock, env or random access unless declared in the manifest **and** granted by the user at install time.
- v1 grantable capabilities: none needed for generators/modifiers; exporters/importers receive bytes from/to the host — the **host** does the file dialog and I/O, so plugins still get no filesystem access.
- Resource limits per call: fuel/epoch-based CPU limit, memory cap (default 256 MB), timeout (default 5 s for generators; configurable). Exceeding a limit aborts the call cleanly with an error, never crashes the app.
- Plugin crashes/traps are contained and reported; document state is unchanged.
- Manifest (`plugin.toml`): id (reverse-DNS), name, version, author, licence, kinds provided, API version, requested capabilities, optional SHA-256 / signature.
- Optional signing later (minisign/sigstore); unsigned plugins allowed but labelled.

### 15.5 Documents and plugins
- Document stores, for each plugin-generated object: `plugin_id`, `plugin_version`, parameters, **and the cached output geometry**.
- A document must **open and export correctly without the plugin installed** (uses cached geometry; object becomes non-editable with a clear "plugin missing" badge).
- Re-generation only occurs when parameters change or the user explicitly updates the plugin version. Plugin output must be deterministic for identical inputs (no RNG without a seed parameter).

### 15.6 Licensing
- The core is AGPL-3.0. Whether a WASM component talking over a defined interface is a derivative work is legally unclear.
- **Decision required:** add an explicit **plugin exception** (in the style of the GCC Runtime / Classpath exceptions) stating that plugins communicating solely through the published `locus-plugin-api` WIT interface may use any licence. This keeps the ecosystem open to permissive and proprietary plugins while the core stays AGPL.
- `locus-plugin-api` (WIT + guest SDK) itself is licensed **MIT/Apache-2.0** so plugin authors don't inherit AGPL obligations by depending on the SDK.
- Draft wording: [ADR-0006](adr/0006-plugin-licence-exception.md). Not legal advice — have the exception wording reviewed before the first public release (#83).

### 15.7 Distribution
- v1: install from a local `.locusplugin` archive (manifest + `.wasm` + optional icon/readme).
- Later: optional index/registry (Git-based list is sufficient initially). No auto-update without user consent.

### 15.8 Testing
- Host test suite with fixture plugins: well-behaved, panicking, infinite-loop, memory-hog, wrong-API-version, malformed output.
- Golden tests for first-party plugins.
- `locus-cli` can run a generator headlessly: `locus-cli plugin run <plugin> --params params.ron --out out.svg`.

---

## 16. MCP server (AI client integration)

### 16.1 Purpose
Let users connect **their own AI client of choice** (Claude Desktop, Claude Code, other MCP-capable clients, local LLM front-ends) to drive the app: create and constrain geometry, apply generator plugins, inspect DOF/validation, and export. The app ships **no AI model and no cloud dependency**.

### 16.2 Implementation
- Crate `locus-mcp`, using the official Rust MCP SDK (`rmcp`) 3.8, targeting MCP revision 2026-07-28 (2025-11-25 clients also accepted) — [ADR-0005](adr/0005-mcp-sdk.md).
- Thin adapter over the §11 command API; no business logic in `locus-mcp`.
- Transports:
  - **stdio** — `locus-cli mcp --doc file.locus` runs headless against a file (no GUI needed; good for scripting and CI).
  - **Streamable HTTP, localhost only** — attaches to the running GUI so the user watches edits live.
- Reference: SolidCraft ships a built-in MCP server — review its tool design.

### 16.3 Tools (initial set)
| Tool | Scope |
|---|---|
| `get_document_summary` (layers, params, entity/constraint counts, DOF, solve status) | read |
| `list_entities` / `get_entity` (filter by layer, type, region) | read |
| `list_constraints`, `get_dof_report`, `get_conflicts` | read |
| `render_preview` → PNG of current view or a region (so the AI can *see* the result) | read |
| `validate_export` → validation report from §8 | read |
| `list_plugins` + parameter schemas | read |
| `add_entities`, `modify_entity`, `delete_entities` | write |
| `add_constraints`, `remove_constraints`, `set_dimension` | write |
| `set_parameter` (document parameters/expressions) | write |
| `create_layer`, `set_layer_props`, `move_to_layer` (subject to §4 layer rules) | write |
| `run_generator` / `apply_modifier` (installed plugins) | write |
| `export` (format + operation profile) → returns bytes or writes via host-confirmed path | export |

Resources: current document (RON snapshot), layer list, plugin catalogue. Errors returned as structured data (e.g. over-constrained set with entity IDs) so the AI can self-correct.

### 16.4 Security & privacy (non-negotiable)
- **Off by default.** User enables it explicitly per app install; status indicator always visible while active.
- HTTP transport binds **127.0.0.1**, never 0.0.0.0. The one exception is the opt-in network-clients setting below (#272). Requires a per-session bearer token shown in the UI (copy to client config). Validate `Origin` header to block DNS-rebinding from browsers.
- The `Host` header must be `127.0.0.1` or `localhost`. An opt-in setting, **Allow Docker clients** (off by default), also accepts `host.docker.internal`, so MCP clients in Docker Desktop containers on the same PC (e.g. Open WebUI) can connect; Docker forwards that name to the host's loopback, so the bind is unchanged. The token, `Origin` refusal, scopes, approvals and caps apply as before, and the enable dialog says that programs in containers can then try to connect (#270).
- **Network clients (opt-in, off by default; owner decision 2026-11-12, #272).** A deliberate exception to loopback-only, for AI clients on other machines on the owner's LAN. When on, the server *also* binds one private IPv4 address the user picks (RFC 1918 or link-local); `0.0.0.0` and public addresses are refused. Connections are accepted only from a client allowlist of private IPs or CIDR ranges, empty by default, checked before any HTTP or MCP handling. The bound address is accepted as `Host`. The token, `Origin` refusal, scopes, approvals and caps are unchanged; the token can optionally be kept across restarts, with a regenerate button. The traffic is **plain HTTP**: the enable dialog says it can be sniffed (drawing and token) and recommends an SSH tunnel or a VPN such as Tailscale to the loopback server instead. TLS is a possible follow-up.
- **Scopes:** read / write / export granted separately. Default grant = read only.
- **Approval modes:** "ask for each write", "ask per session", "allow" — user-selected; export always asks unless explicitly allowed.
- AI edits are grouped into **labelled undo transactions** ("AI: added 12 entities, 9 constraints") — one undo reverts an AI action.
- No filesystem access beyond host-mediated export/import with user confirmation; no arbitrary path writes.
- No telemetry; the app never sends document content anywhere. Whatever the user's AI client does with returned data is outside the app — state this in the enable dialog.
- Rate limiting and payload size caps on write tools.
- Plugins **cannot** call MCP or see MCP sessions.

### 16.5 Testing
- Integration tests driving `locus-cli mcp` over stdio with scripted tool calls → golden documents.
- Security tests: non-loopback bind rejected, missing/invalid token rejected, bad `Origin` rejected, scope enforcement, approval-mode enforcement.

---

## 17. Instructions for Claude Code

- Start with **M0 only**. Do not build features beyond the milestone.
- Produce ADRs in `docs/adr/` for every §12 decision, with measured spike results, before committing to a choice.
- Keep core crates free of GUI dependencies and WASM-compatible.
- Run `cargo-deny` and reject any dependency violating §2.
- Flag (don't silently resolve) any conflict between this spec and what the spikes reveal.
- The full working agreement — environment, branching, code style, testing, Definition of Done, off-limits list — lives in [`CLAUDE.md`](../CLAUDE.md). Where the two disagree, flag it.

---

## 18. Reference applications

Study these for UX, constraint behaviour and output conventions. **Do not copy code** from anything incompatible with AGPL-3.0; for permissive projects, retain notices.

| Project | Licence | Why it matters |
|---|---|---|
| **SolidCraft** — github.com/tangency-labs/solidcraft (formerly bherbruck/solidcraft) | MIT OR Apache-2.0 | Pure-Rust clean-room Fusion 360 reimplementation, with a built-in MCP server (reference for §16). Closest match to the target sketch UX (Fusion-style sketch tools, constraints, expressions in dimension fields, first-dimension-scales-sketch behaviour, sketch text). Also a candidate source for the solver (§3) and a reference for Rust CAD app structure. Desktop + browser build. |
| **Autodesk Fusion 360** (sketch environment) | Proprietary | The UX benchmark: auto-constraints while drawing, fully-constrained colour state, profile detection, construction geometry. Behaviour reference only. |
| **SolveSpace** | GPLv3+ | Mature constraint solver (libslvs), DOF feedback, SVG/DXF export. |
| **FreeCAD Sketcher** | LGPL-2.1+ | PlaneGCS solver; conflict/redundancy reporting UX. |
| **Dune 3D** | GPLv3 | Modern SolveSpace-derived UI; good reference for constraint UI polish. |
| **CAD Sketcher (Blender add-on)** | GPLv3 | Constraint sketching bolted onto a non-CAD tool — useful for hybrid freeform/constrained thinking. |
| **Cuttle** | Proprietary | Closest competitor in the cut-design space: parameters, live modifiers, scripted plugins — but **no geometric constraints**. Reference for modifier/plugin UX and cut-oriented workflow. |
| **LightBurn** | Proprietary | Industry-standard layer→operation mapping for lasers; target import compatibility. |
| **Cricut Design Space** | Proprietary | Target SVG import compatibility for vinyl/craft cutters. |
| **Vectorworks** (2D constraints) | Proprietary | Example of geometric + dimensional constraints on general 2D objects. |
| **LibreCAD / QCAD** | GPLv2 / GPLv3 | 2D CAD and DXF conventions. Note LibreCAD is GPLv2 — reference only, no code. |
