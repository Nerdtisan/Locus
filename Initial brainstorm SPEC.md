# Constraint Vector Editor — Initial Spec (v0.1)

> Working name: **TBD** (placeholder `cve` used for crate names below).
> Status: idea stage. This document is the brief for Claude Code to scaffold the workspace and execute Milestone 0–1.

---

## 1. Purpose

A cross-platform Rust desktop app for creating precise 2D vector designs using **Fusion 360–style constrained sketching**, outputting **SVG, PDF and DXF** for:

- Pen plotters
- Vinyl cutters / Cricut-class machines
- Laser cutters / engravers

It replaces the Inkscape/Affinity workflow where precision is achieved by eye and snapping, with a geometric constraint solver.

### Goals
- Exact geometry via geometric + dimensional constraints, solved live.
- Machine-oriented output: layers map to **operations** (cut / score / engrave / draw), not visual styles.
- Clean, validated exports that machines and their host software (LightBurn, Cricut Design Space, plotter tools) accept first time.
- Native desktop on Linux, Windows, macOS.

### Non-goals (explicit)
- General illustration / Illustrator replacement (no gradients, effects, raster editing, artboards for print design).
- Parametric feature timeline / history tree. **One constrained sketch per document.**
- 3D of any kind.
- Re-importing exported SVG/DXF as editable native documents (import as reference geometry only, later milestone).
- Direct machine control / G-code streaming (possible far-future plugin, out of scope).

---

## 2. Licence & governance

- **AGPL-3.0-or-later.**
- **CLA required from the first external contribution** (preserves option for dual-licensing or a hosted/WASM offering later). Set up CLA bot before accepting PRs.
- Dependency licence policy: MIT / Apache-2.0 / BSD / Zlib / LGPL / GPLv3+ allowed. Anything GPLv2-only or proprietary is **forbidden**. Enforce with `cargo-deny` in CI from day one.

---

## 3. Architecture

Cargo workspace. Core is UI-agnostic and headless-testable; must also compile to `wasm32-unknown-unknown` (no filesystem/threads assumptions in core crates).

```
crates/
  cve-geom/      # Entities, math, tolerances, arc/line/spline primitives (f64, mm)
  cve-solver/    # Solver trait + backend(s); DOF analysis; conflict reporting
  cve-doc/       # Document model, layers, IDs, undo/redo command stack, file format (de)serialisation
  cve-region/    # Closed-profile/region detection, non-destructive booleans, offsets
  cve-export/    # Validation pipeline + SVG / PDF / DXF writers, kerf/tool offset application
  cve-text/      # Single-line (Hershey/EMS) + outline font → geometry
  cve-plugin-api/# WIT interface definitions + Rust guest SDK for plugin authors (see §15)
  cve-plugin-host/# wasmtime-based plugin runtime, capability enforcement, plugin registry
  cve-mcp/       # MCP server exposing document/sketch operations to external AI clients (see §16)
  cve-app/       # Desktop GUI shell (framework TBD — see §12)
  cve-cli/       # Headless: validate/export a .cve file (used in tests + CI)
```

### Key crate candidates (verify licences + maintenance before adopting)
| Need | Candidate |
|---|---|
| Curve math | `kurbo` |
| Offsets with true arcs (kerf, inset) | `cavalier_contours` (polyline-with-bulge, keeps arcs) |
| Booleans | `i_overlay` or `geo` (flattening — see §6.3 tension) |
| Tessellation for rendering | `lyon` |
| DXF write | `dxf` crate |
| PDF write | `krilla` or `printpdf` |
| Font parsing | `ttf-parser` / `rustybuzz` |
| Serialisation | `serde` + `ron` (or JSON) |

### Solver backend
- Define a `Solver` trait in `cve-solver`; backends are swappable.
- **Spike both** in Milestone 0, pick one:
  - **libslvs** (SolveSpace, GPLv3+) — stable C API (`slvs.h`), simplest FFI via `bindgen`.
  - **PlaneGCS** (FreeCAD, LGPL-2.1+) — C++, needs `cxx` bridge, richer 2D constraint set, Eigen dependency.
  - **SolidCraft's sketch solver** (pure Rust, MIT OR Apache-2.0 — see §18). **Spike this first.** If its 2D sketch/constraint code is separable (or can be extracted as a crate), it removes C/C++ FFI entirely and keeps the whole stack Rust/WASM-friendly. Assess: crate boundaries, coupling to its 3D kernel, robustness on the §5 constraint set, drag performance, test coverage. Project is young and largely AI-assisted, so verify quality with our own test suite rather than trusting it. Retain upstream copyright/licence notices on any reused code.
  - Also check for any other mature pure-Rust 2D GCS; adopt only if it handles the constraint set in §5 with acceptable robustness.
- Solver requirements: incremental re-solve during drag (target < 16 ms for 500 entities), DOF count, identification of over-constrained / redundant / conflicting constraints.

---

## 4. Document model

### Units & precision
- Internal unit: **millimetres**, `f64`.
- Display units selectable (mm default; cm, m; inch display allowed but stored as mm).
- Global tolerance constant for coincidence/closure checks (default 1e-6 mm for solver, 0.001 mm for export validation).

### Entities (all have stable `EntityId` — UUIDv7 or monotonic u64, never reused)
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

### Solver UX
- Sketch status: under-constrained (show remaining DOF count), fully constrained (geometry turns distinct colour, as in Fusion), over-constrained (highlight conflicting set, refuse to apply new conflicting constraint, offer to make it a driven/reference dimension).
- Driven (reference) dimensions supported.

---

## 6. Tools

### 6.1 Sketch tools (v1)
Line / polyline, rectangle (2-point, centre, 3-point rotated), circle (centre, 2-point, 3-point), arc (3-point, centre, tangent), polygon (inscribed/circumscribed, n sides), slot, Bézier pen tool, point.

### 6.2 Modify tools (v1)
Trim, extend, break/split, fillet (adds tangent arc + constraints), chamfer, offset (live/constrained offset entity), mirror (adds symmetric constraints), linear pattern, circular pattern, move/rotate/scale (respecting constraints), toggle construction.

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

**Export interaction**
- Overlapping instances are flagged by the §8 duplicate/overlap validation.
- Patterns are flattened to plain geometry on export (DXF/SVG have no associative patterns).

### 6.3 Regions & booleans
- Automatic closed-profile detection from sketch geometry (as Fusion does), producing **regions**.
- Non-destructive boolean result objects (union/subtract/intersect of regions), stored as recipes referencing entity IDs and recomputed on solve.
- **Known tension:** most Rust boolean libs flatten to polylines; DXF/laser output strongly prefers true arcs. Approach: perform booleans on flattened geometry, then **arc-refit** the result (or use `cavalier_contours` for arc-preserving cases). Spike in Milestone 2.

---

## 7. Operations & machine output

Layer `operation` enum:
| Operation | Params |
|---|---|
| Cut | kerf width, inside/outside/none compensation, pass count (metadata) |
| Score | — |
| Engrave (fill) | hatch spacing, hatch angle |
| Draw (plotter) | pen number, pen width (drives hatch spacing + stroke-edge preview) |
| Drag-knife (vinyl) | blade offset |
| Reference | never exported |

Machine profiles (v2): presets bundling export format, colour mapping, units, and offsets for e.g. "LightBurn", "Cricut Design Space", "Generic HPGL plotter".

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

Export is always one-way. Native file is the source of truth.

---

## 9. Text

- **Single-line fonts** (Hershey, EMS/SVG single-stroke) for plotters/engraving — required v1-ish (Milestone 3).
- Outline TrueType/OpenType → geometry for cutting.
- Text could ship as a first-party plugin to validate the plugin API (decide in M2).
- Text entity has a constrained baseline/anchor (can be constrained like any entity, e.g. centred on a region).
- Bundle only fonts with compatible licences (OFL/public domain).

---

## 10. Native file format

- Extension: `.cve` (placeholder).
- Human-readable **RON** (or JSON) for clean git diffs and easy debugging.
- Top-level: `format_version`, `units`, `parameters`, `layers`, `entities`, `constraints`, `regions/booleans`, `view` (optional, can be ignored by diff tools).
- Stores **solved positions** as well as constraints so files open instantly and degrade gracefully if a solve fails.
- Versioned migrations in `cve-doc` (`v1 → v2 …`), tested with fixture files.
- Deterministic serialisation (stable ordering) so unchanged documents produce identical files.

---

## 11. Undo/redo

- **Architectural requirement (from M1):** every mutation goes through a **serialisable command API** in `cve-doc` (serde-derived, schema-describable). The GUI, CLI, plugins and MCP server are all just clients of this API. Never mutate the document from UI code directly — retrofitting this later is what makes MCP/automation painful.
- Command pattern in `cve-doc`. Every user action = one command; compound commands for multi-step ops (fillet, layer delete, pattern).
- Solver results are not part of command state; re-solve after undo/redo.

---

## 12. Open decisions (resolve in Milestone 0)

1. **GUI framework** — evaluate against: precise custom 2D canvas rendering, native-feeling panels, cross-platform packaging, WASM option. Candidates: `egui`, `iced`, `Slint`, `Xilem`/`vello`. Align with the stack used in my existing Rust desktop apps if it meets the criteria.
2. **Canvas renderer** — `vello`, `wgpu` + `lyon`, or framework-native.
3. **Solver backend** — libslvs vs PlaneGCS (see §3).
4. **Entity ID scheme** — UUIDv7 vs u64.
5. **RON vs JSON** for file format.
6. App name.
7. Plugin licence exception wording (see §15.6).
8. MCP SDK version + MCP spec revision to target (see §16).

---

## 13. Milestones

**M0 — Spikes & scaffolding**
- Workspace, CI (fmt, clippy, test, `cargo-deny`), licence headers, CLA config.
- Solver spike: bind libslvs and PlaneGCS; solve a fixed test sketch (rect + filleted corners + circle concentric to arc + dimension); measure drag re-solve time at 500 entities.
- GUI spike: pan/zoom canvas at mm scale, draw lines, hit-testing.
- Decision record for each §12 item (`docs/adr/`).

**M1 — Core sketcher**
- Points, lines, circles, arcs; geometric + dimensional constraints; live solve on drag; DOF display; conflict detection.
- Native save/load; undo/redo.
- Plain SVG export (no validation yet).

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
- Machine profiles, stroke-edge constraints, bed bounds, import reference geometry (SVG/DXF as locked reference layer), packaging/installers for all three OSes.

---

## 14. Testing strategy

- **Golden-file tests**: `.cve` fixture → export → compare against committed SVG/DXF output (normalised).
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
- Interface is **versioned** (`cve:plugin@1.0.0`). Host supports current major + previous major. Breaking changes only on major bumps.

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
- **Decision required:** add an explicit **plugin exception** (in the style of the GCC Runtime / Classpath exceptions) stating that plugins communicating solely through the published `cve-plugin-api` WIT interface may use any licence. This keeps the ecosystem open to permissive and proprietary plugins while the core stays AGPL.
- `cve-plugin-api` (WIT + guest SDK) itself is licensed **MIT/Apache-2.0** so plugin authors don't inherit AGPL obligations by depending on the SDK.
- Not legal advice — have the exception wording reviewed before the first public release.

### 15.7 Distribution
- v1: install from a local `.cveplugin` archive (manifest + `.wasm` + optional icon/readme).
- Later: optional index/registry (Git-based list is sufficient initially). No auto-update without user consent.

### 15.8 Testing
- Host test suite with fixture plugins: well-behaved, panicking, infinite-loop, memory-hog, wrong-API-version, malformed output.
- Golden tests for first-party plugins.
- `cve-cli` can run a generator headlessly: `cve plugin run <plugin> --params params.ron --out out.svg`.

---

## 16. MCP server (AI client integration)

### 16.1 Purpose
Let users connect **their own AI client of choice** (Claude Desktop, Claude Code, other MCP-capable clients, local LLM front-ends) to drive the app: create and constrain geometry, apply generator plugins, inspect DOF/validation, and export. The app ships **no AI model and no cloud dependency**.

### 16.2 Implementation
- Crate `cve-mcp`, using the official Rust MCP SDK (`rmcp`) — verify current version and spec revision at M0.
- Thin adapter over the §11 command API; no business logic in `cve-mcp`.
- Transports:
  - **stdio** — `cve-cli mcp --doc file.cve` runs headless against a file (no GUI needed; good for scripting and CI).
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
- HTTP transport binds **127.0.0.1 only**, never 0.0.0.0. Requires a per-session bearer token shown in the UI (copy to client config). Validate `Origin` header to block DNS-rebinding from browsers.
- **Scopes:** read / write / export granted separately. Default grant = read only.
- **Approval modes:** "ask for each write", "ask per session", "allow" — user-selected; export always asks unless explicitly allowed.
- AI edits are grouped into **labelled undo transactions** ("AI: added 12 entities, 9 constraints") — one undo reverts an AI action.
- No filesystem access beyond host-mediated export/import with user confirmation; no arbitrary path writes.
- No telemetry; the app never sends document content anywhere. Whatever the user's AI client does with returned data is outside the app — state this in the enable dialog.
- Rate limiting and payload size caps on write tools.
- Plugins **cannot** call MCP or see MCP sessions.

### 16.5 Testing
- Integration tests driving `cve-cli mcp` over stdio with scripted tool calls → golden documents.
- Security tests: non-loopback bind rejected, missing/invalid token rejected, bad `Origin` rejected, scope enforcement, approval-mode enforcement.

---

## 17. Instructions for Claude Code

- Start with **M0 only**. Do not build features beyond the milestone.
- Produce ADRs in `docs/adr/` for every §12 decision, with measured spike results, before committing to a choice.
- Keep core crates free of GUI dependencies and WASM-compatible.
- Run `cargo-deny` and reject any dependency violating §2.
- Flag (don't silently resolve) any conflict between this spec and what the spikes reveal.

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
