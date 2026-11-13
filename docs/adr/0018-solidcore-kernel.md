# ADR-0018: solidcore B-rep kernel: stay on `locus-solver` / `locus-geom` (proposed)

- **Status:** Proposed. Adopting anything needs the owner's approval; this ADR recommends and does not switch.
- **Date:** 2026-11-14
- **Spec:** `docs/SPEC.md §3` (solver, wasm32, 16 ms drag budget), `§5` (constraints), `§11` (command API), `§12` item 16
- **Issues:** Nerdtisan/Locus#153; builds on ADR 0007 (solver), ADR 0015 (two-phase solve)

## Context

#153 asks whether [`tangency-labs/solidcore`](https://github.com/tangency-labs/solidcore) is a better base than Locus's own solver (`locus-solver`, sparse LM, ADR 0007) and geometry (`locus-geom`). Measured at commit `510e41d4` (mirror of the `solidcore/` folder in tangency-labs/solidcraft; the repo says it is read-only and that development happens in SolidCraft), Apache-2.0, API "experimental (0.0.x)".

**Finding that frames everything: solidcore is not a constraint solver.** It is a 3D boundary-representation solid-modelling kernel, a Rust port of NVIDIA SMLib (usd-brep v1.0.1): NURBS curves/surfaces, B-rep topology, surface-surface intersection, booleans, fillets/chamfers, offset/shell, sweeps, healing, mass properties. About 997 000 lines in the `solidcore` crate. A search of the whole repo finds no sketch, DOF or geometric-constraint code; the "solver" hits are the fillet rolling-ball solver and curve/surface root finders. So the comparison "solidcore vs `locus-solver`" is a category mismatch: the two do not compete for the same job.

## Options

1. **Adopt**: replace `locus-solver` and/or `locus-geom` with solidcore.
2. **Partly adopt**: use solidcore for some 2D job (booleans, offsets, curve intersection) in `locus-region`/`locus-geom`.
3. **Stay**: keep `locus-solver`, `locus-geom` and the Locus-owned `locus-region` arrangement; revisit only if Locus grows a 3D need.

## Measured

Spike crate: `spikes/solidcore/` (standalone workspace, git dep pinned to the commit). Container `locus-dev`, Rust 1.99.1, `CARGO_BUILD_JOBS=8`.

| Check | Result |
|---|---|
| **Licence**, `cargo deny --config deny.toml check`, project `deny.toml` unchanged | `licenses ok`, `bans ok`, `advisories ok`. Direct deps are only `serde` and `thiserror`. Apache-2.0 with a NVIDIA NOTICE to carry. **`sources FAILED`**: it is not on crates.io, so it is a `git` source, which `unknown-git = "deny"` refuses. Adopting needs a crates.io release, or vendoring, or an owner-approved `allow-git` entry. I did not touch `deny.toml`. |
| **wasm32-unknown-unknown** | Builds (`cargo build --release --lib --target wasm32-unknown-unknown`, 2 m 13 s cold). Its `Instant` use is gated off on wasm. No linked-binary size measured: the spike links one symbol, so a figure would be meaningless. |
| **Build cost** | Cold native release: 2 m 17 s for solidcore alone (`syn` + `serde` included); `libsolidcore.rlib` is 198 MB. The whole Locus workspace is far smaller. Pinned toolchain is 1.99.1 (matches ours). |
| **2D coverage vs §5** | **None of the §5 constraint types**, no sketch, no DOF/redundancy analysis, no drag solve. Curves are 3D: line, arc, circle, ellipse, parabola, hyperbola, NURBS (which subsumes Béziers), offset and composite curves. `Point2`/`Vec2` exist, but the model is 3D B-rep: a sketch would be a planar face from `profile`/`prism`. No dimension or pattern concepts (§6.2.1). |
| **Robustness: solver fixtures and proptests** | **Not applicable**: nothing to run them through. The M0 fixture, 500-entity chain, conflict case and proptests in `locus-solver` can't be expressed in solidcore. |
| **Speed: 500-entity drag re-solve, §3 (< 16 ms)** | **Not applicable** for solidcore. For reference, `locus-solver` (ADR 0007, 500 entities, 1 512 unknowns): full solve 0.42 ms native / 0.6 ms wasm; drag-frame mean 0.46 ms, max 1.42 ms native (0.59 / 1.7 ms wasm). |
| **API fit** | See below. |

### API fit (§11, ADR 0015)

* The command API wants small, serialisable, undoable edits to a 2D document with stable `u64` ids (ADR 0003). solidcore's state is a `Kernel` plus `Brep` handles (`BrepId`, `FaceId`, ...) with kernel-owned caches ("one `Kernel` per thread"). It would sit behind `locus-doc` as a derived cache, never as the document, and would need a conversion layer both ways.
* ADR 0015's two-phase solve needs a re-lowerable constraint system with fixed/free sets and structured conflicts (`DocError::PatternHeld`). solidcore has no equivalent, so nothing to host.
* Determinism and `no panics on bad input` are explicit solidcore goals, which matches §10 and the CLAUDE.md error rules. Its exact-arithmetic ambitions don't reach 2D line/arc booleans, which `locus-region` already does exactly (ADR 0008) and offsets (ADR 0010, 6 000 / 6 000 exact).

## Decision (proposed)

**Option 3, stay.** Do not adopt solidcore for the solver or for `locus-geom`: it provides neither a constraint solver nor the 2D line/arc model, and it adds a ~1 M-line, 2-minute-to-compile dependency with a git-only source that fails `cargo deny` as configured. **Not worth a partial adoption either**: the 2D jobs it could touch (booleans keeping arcs, offsets) are already solved, measured and owned (ADR 0008, 0010).

**Revisit if** Locus gains real 3D needs (e.g. extruded or engraved-depth previews, or solid-based laser/CNC features) that are outside §1's scope today. Then solidcore, once on crates.io and past 0.x, is a credible candidate for a *separate* optional crate, never a core crate (the core must stay small and wasm32-clean, §3).

## Consequences

* No production code, `Cargo.toml`, `deny.toml` or workflow changes. The spike lives only in `spikes/solidcore/`.
* Flag to the owner, no conflict with the spec found: the spike's premise ("compare to existing") assumed an overlap that doesn't exist.
* If the owner wants solidcore for 3D later, open a new issue + ADR, and resolve the git-source / `deny.toml` question first.
