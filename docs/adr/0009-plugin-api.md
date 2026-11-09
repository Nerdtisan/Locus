# ADR-0009: Plugin API: WASM components, one world per kind, nothing imported

- **Status:** Accepted
- **Date:** 2026-11-05
- **Spec:** `docs/SPEC.md §15` (plugin system), `§9` (text: plugin or core), `§13 M2`
- **Issue:** Nerdtisan/Locus#138 (with #136 internal `Exporter` trait, #137 WIT draft)

## Context

M3 builds the plugin host (#22). Before that, M2 has to fix the shape of the interface so the host, the Rust guest SDK and first-party plugins are built against one design (§13 M2). The spec already settles the big choices: WebAssembly components with WIT, run by wasmtime (§15.1); five kinds (§15.2); plugins never touch the document (§15.3); deny-by-default capabilities and per-call limits (§15.4); cached output in documents (§15.5); a permissive SDK and an AGPL plugin exception (§15.6, ADR 0006). Plugins are desktop-only at first; a browser runtime is a later epic (§0, #31).

What remained open: how to express the kinds in WIT, how to enforce the sandbox, what exporters receive, how parameters are described, and whether text (§9) is a first-party plugin.

The internal `Exporter` trait (#136) already proved the exporter boundary: the built-in SVG, DXF and PDF writers take a validated `ExportModel` and return bytes, and the app and CLI pick writers from a registry.

## Options

### Kinds as one interface vs one world per kind

- **One big world** exporting every kind: simplest manifest, but every plugin must stub kinds it doesn't provide, and the host can't tell from the component what it really does.
- **One world per kind** (chosen): `generator-plugin`, `modifier-plugin`, `exporter-plugin`, `importer-plugin`, `validator-plugin`, each exporting `info` plus its kind. A component may target several. The host matches the component's exports against the manifest's declared kinds.

### Enforcing the sandbox

- **WASI with a restricted context:** works, but every world would carry WASI imports and the policy lives in host configuration, where a mistake grants access.
- **Import nothing** (chosen): the worlds declare no imports, so a component that tries to use the filesystem, network, clock, environment or randomness fails to instantiate. The sandbox is a property of the interface, checked by a test (`tests/wit.rs`). Grantable capabilities (§15.4) are added later as explicit, separately-named imports, only when a real need appears.

### What exporters receive

- **The document:** rejected. It breaks §15.3 and exposes constraints and IDs that exporters don't need.
- **The validated `ExportModel`** (chosen): exactly what the built-in writers get (layers with operation and settings, chained paths of true lines, arcs and circles, Reference and export-disabled layers already removed). Plugin and built-in exporters are interchangeable in the registry.

## Decision

The interface is `crates/locus-plugin-api/wit/plugin.wit`, package **`locus:plugin@1.0.0`**, licensed MIT OR Apache-2.0 (§15.6). It is a **draft** until the host ships in M3; after that, breaking changes only on a major bump, and the host supports the current and previous major (§15.3).

- **Shared types:** millimetres and radians in f64; shapes (line, CCW arc, circle), paths, layers with the §7 operation and its settings, and `export-model`.
- **Parameters:** each plugin returns a schema (number with unit and range, integer, boolean, choice, text, document-parameter binding); the host draws the panel. Values come back as a keyed list.
- **Generators** return points, entities that refer to points by index (so shared corners stay shared and constrainable), and optional constraints from a fixed subset (coincident, horizontal, vertical, parallel, perpendicular, tangent, equal, distance, radius). The host anchors the group with a constrainable frame (§15.3).
- **Modifiers** take paths and parameters and return paths; the host re-runs them when the source changes.
- **Exporters** return bytes plus a `format-info` (id, name, extension, MIME), matching the built-in `Exporter` trait.
- **Importers** (M4) return paths for a locked reference layer; **validators** (M4) return findings with severity and the affected layer/path.
- **Errors** are structured (`bad-parameter`, `unsupported`, `failed`). Traps, timeouts and memory overruns are reported by the host and leave the document unchanged (§15.4).
- **Determinism** is a contract: identical inputs give identical outputs; randomness only from a seed parameter (§15.5).

**Text (§9) ships in core, not as a plugin.** `locus-text` stays a core crate.
- Text must work in the browser, but plugins are desktop-only at first (§0).
- Text is a live, constrainable entity (anchor and baseline, §9), not one-shot generated geometry.
- The plugin API gets proven by the M3 example plugins instead: the finger-joint box generator and the HPGL exporter.

## Consequences

- M3 host (#22): instantiate components with no imports linked; reject any component that needs one. Enforce fuel/epoch CPU limits, a 256 MB memory cap and per-call timeouts (§15.4). Store `plugin_id`, `plugin_version`, parameters and the cached output in the document (§15.5).
- M3 Rust guest SDK: generate bindings from this WIT (`wit-bindgen`), plus a template repository.
- The WIT test keeps the file valid and enforces the "imports nothing" rule; changing it means changing this ADR.
- Constraint kinds a generator can emit are a subset of §5; widening it is additive (minor version).
- Revisit when the first plugin genuinely needs a capability (e.g. a seeded RNG helper or system fonts), or when the browser runtime (#31) lands.
