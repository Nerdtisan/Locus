# ADR-0002: Solver backend

- **Status:** Accepted
- **Date:** 2026-11-04
- **Spec:** `docs/SPEC.md §3` (Solver backend, Solver requirements), `§5`, closes `§12.3`
- **Issue:** Nerdtisan/Locus#43 (spikes #41 harness, #39 SolidCraft, #40 survey, #42 own-solver prototype)

## Context

Every Locus sketch is solved by one geometric constraint solver (§4: one shared solver space). §0 and §3 already decided it must be **pure Rust**, because it has to build for `wasm32-unknown-unknown` with the rest of the core. That rules out libslvs and PlaneGCS. Left open was *which* pure-Rust solver: SolidCraft's, another Rust GCS, or our own.

What the solver has to do (§3, §5):

- Every §5 v1 constraint: 13 geometric kinds and 6 dimensional ones, with distance in three variants (arc length is v2).
- Incremental re-solve during a drag in **< 16 ms at 500 entities**.
- A **DOF count**, plus identification of **over-constrained / redundant / conflicting** constraints. §5 Solver UX shows the remaining DOF, highlights the conflicting set, and refuses a new conflicting constraint. That makes conflict detection interactive too.
- Licence compatible with AGPL-3.0-or-later (§2, `cargo deny`). Builds and runs in the browser.

## Options

### Option A: SolidCraft's sketch solver (#39)

`solidcraft-sketch` from github.com/tangency-labs/solidcraft, MIT OR Apache-2.0, assessed at `6178d1a`. It is a clean-room Levenberg–Marquardt solver: a constructive seed places points that two loci pin down, then a damped Gauss–Newton runs per connected component on a dense finite-difference Jacobian. Rank analysis by Gaussian elimination gives DOF, redundant and failing constraints.

**Crate boundaries and coupling.** The solver is separable from the 3D kernel. `solidcraft-sketch` depends only on `solidcraft-geom` (vectors and frames), `solidcore-record` (a git dependency on the B-rep record crate, used here for `Vec2`/`Seg2`), and text shaping (`harfrust`, `ttf-parser`, `unicode-bidi`). It does not depend on `kernel`, `smlib` or the GUI. But the crate is far more than a solver: of its 31 k lines, the solver proper (solver, constructive seed, decomposition, loci, rank analysis, cursor) is about 7 k. The rest is profiles/regions, offsets, patterns, links, text and splines. It is not on crates.io, and it starts every sketch with a fixed origin point at index 0, which our adapter had to work around.

**Coverage.** It has a kind for every §5 v1 constraint and more (arc length, slots, splines). The only gaps were a missing line–line distance kind (mapped to point–line distance) and unsigned horizontal/vertical distances.

**Quality and maintenance.** It has 351 tests in the sketch crate, many of them issue regressions, and it explicitly avoids flipping branches on dimension edits (it records "folded" constraints). The repository history starts on 2026-10-07: 4,466 commits in four weeks, almost every recent one co-authored by an AI model, version `0.40.0-rc.2`, edition 2024, MSRV 1.95.

**Pros:** complete diagnostics (DOF, redundant, conflicting), the richest constraint set, careful branch handling.
**Cons:** dense per-component linear algebra, which made it the slowest candidate and lets it miss the drag target past ~1000 curves (measured below). Seconds-long conflict detection at 500 entities. Not packaged. Very young, fast-moving API. Git dependencies that `deny.toml` rejects. An unmaintained-advisory dependency (`ttf-parser`, RUSTSEC-2026-0192). MSRV above ours (1.92).

### Option B: ezpz, with Locus's own DOF/redundancy analysis (#40 + #42)

`ezpz` 0.2.29 from KittyCAD/Zoo (crates.io, MIT), the solver behind Zoo Design Studio's KCL sketches. It runs a sparse Levenberg–Marquardt on `faer`. Users supply constraint requests (with priorities) and initial guesses; it is stateless between solves. It has been on crates.io since 2024-11, with 13 releases, about 240 k downloads and 105 tests (including property tests).

**Coverage.** Every §5 v1 constraint, some by composition: collinear is two point–line distances, concentric is coincident centres, diameter is a radius. Arcs have no radius variable and no line–arc tangency, so the adapter gives each arc a radius variable tied to both end distances, and the arc then acts as a circle. All of this lives in the adapter (`spikes/solver/spike/src/ezpz_backend.rs`).

**Diagnostics.** ezpz names the constraints a failed solve leaves unsatisfied, which is enough for conflict reporting. It does **not** give a DOF count; its "freedom analysis" lists underconstrained *variables* (414 of them for a sketch with 37 DOF). It does **not** detect redundant constraints. Its freedom analysis also takes 0.2 s at 500 entities and 4 s at 1000.

So option B adds Locus's own analysis, prototyped under #42 in `spike/src/analysis.rs` (about 300 lines). It linearises our constraint definitions at the solved geometry and inserts the rows, one constraint at a time, into a sparse row echelon form with partial pivoting:

- DOF = unknowns − rank.
- A constraint whose rows reduce to zero is redundant (or conflicting, if violated).

This takes 1–3 ms at 500–1000 entities. One finding shaped the prototype: residual formulation matters for rank. `distance(centre, line) − radius` has a zero gradient at a fillet's shared end point, so tangency there must use the cosine between the line and the radius. SolidCraft gets this right.

**Pros:** fastest everywhere, sparse (scales), packaged on crates.io, used in production by a funded team, small dependency tree, Locus owns the diagnostics and their UX.
**Cons:** we maintain the analysis and the adapter. Two residual definitions (ezpz's and ours) must agree; the harness's independent checker guards this. ezpz is 0.x. Its `faer`/`gemm` tree pulls in `paste`, which has an unmaintained advisory (RUSTSEC-2024-0436).

### Option C: other pure-Rust crates (#40 survey)

Searched crates.io (constraint/geometric/sketch solver, levenberg, nonlinear least squares, CAD sketch) on 2026-11-04.

| Crate | Licence | wasm32 | Coverage / diagnostics | Maintenance | Verdict |
|---|---|---|---|---|---|
| `ezpz` 0.2.29 (KittyCAD) | MIT | yes (measured) | full §5 via adapter; conflicts, no DOF/redundancy | since 2024-11, 13 releases, ~240 k downloads, production use | **shortlisted, run** |
| `sketch-solver` 0.1.0 (neoscad) | MIT OR Apache-2.0 | likely (`libm` only) | claims DOF, redundant, conflicting, flips | 1 release (2026-10-07), ~500 downloads | watch; not run (weeks old, single release) |
| `acs-solver` 0.1.19 | MIT | yes (claims, ships wasm bindings) | claims DOF, redundant, conflicting; dense `nalgebra` dog-leg | 16 releases since 2026-10-03, ~400 downloads | watch; not run |
| `laman` 0.1.0 | MIT OR Apache-2.0 | likely | claims exact DOF and blame | first released 2026-11-01 | too new |
| `arael-sketch-solver` 0.9.3 | MIT | likely (uses `web-time`) | sketch solver on a symbolic-diff framework | since 2026-03, ~900 downloads | not run (one maintainer, heavy framework) |
| `vcad-kernel-constraints` 0.10.0 | MIT | no: depends on `wgpu` | LM sketch solver inside a kernel | 1 release | rejected (GPU dependency in a core crate) |
| `brepkit-sketch` 4.6.26 | AGPL-3.0-only | ? | GCS for a B-rep kernel | active | rejected (`AGPL-3.0-only` is not in the §2 allow-list) |
| `levenberg-marquardt` 0.15 | MIT | yes | generic dense LM on `nalgebra`; no geometry, no diagnostics | mature | not a GCS: would mean writing our own solver (#42) |
| `levenberg-marquardt-sparse` 0.2 | MIT | likely | generic sparse LM (PCG) | young | same |
| `argmin` 0.11 | MIT OR Apache-2.0 | yes | generic optimisation framework (Gauss–Newton, no sparse LM) | mature | same |
| `slvs` 0.6 | — | no (C++ libslvs) | — | — | excluded by §3 |

Of the newcomers, `sketch-solver` and `acs-solver` advertise exactly the diagnostics §5 needs and are worth re-checking on this harness in M1. They are weeks old, single-author, with a few hundred downloads, so they don't meet §3's "mature" bar today.

### Option D: our own solver (#42)

Newton/LM over residuals with graph decomposition. Only the *analysis* half was prototyped, because option B already supplies a fast, sparse solve. A full own solver would duplicate ezpz with no measured gain, so it is not needed now. It stays the fallback if ezpz stalls (see Consequences).

## Measurements

**Harness** (`spikes/solver/`, #41): a dependency-free crate with the §4/§5 model, an independent residual checker (every constraint and every arc's implicit condition within 1e-6), the fixtures, and a `Backend` trait. Backends are judged by the checker, never by their own "solved" flag. Fixtures:

- `fillet-rect`: the §13 M0 sketch, a 60 × 40 rectangle with filleted corners, a circle concentric to a fillet and a distance dimension. DOF 0, plus under, redundant and conflict variants.
- `generated`: 72 cells in one connected chain, **504 curves + 720 points** (1,512 unknowns), mixing 20 constraint kinds, DOF 37. It has two 30-frame drags: `drag-chain` moves half the sketch, `drag-local` turns one free spoke. A conflict variant adds one constraint.
- 27 per-constraint `coverage` cases.

A second size of 144 cells (1,008 curves) shows scaling. "500 entities" in §3 is read as 500 curves, not counting points; see Consequences.

**Setup:** branch `feature/solver-spike`. AMD Ryzen 9 9950X3D on Windows 11, natively in the `locus-dev` container (Rust 1.99.1, `--release`, x86-64 Linux). wasm32-unknown-unknown with the same suite, release build, stripped, in Chromium 152 (V8). The page is not cross-origin isolated, so the wasm clock rounds to 0.1 ms. The machine was shared with other builds, and native runs varied ±20% run to run. Treat sub-millisecond differences as noise: the two ezpz columns run the same solve code and differ only by run-to-run variation.

### 500 entities (504 curves): native | wasm

| Criterion | A: SolidCraft | B: ezpz alone | B: ezpz + Locus analysis |
|---|---|---|---|
| §5 v1 coverage (27 cases, checker-verified) | 27/27 | 27/27 | 27/27 |
| M0 fixture full solve (ms) | 0.062 \| 0.12 | 0.094 \| 0.11 | 0.095 \| 0.10 |
| M0 fixture DOF (0) / under (1) / redundant named | 0 / 1 / yes | not reported / not reported / no | 0 / 1 / yes |
| M0 conflict: detected, both constraints named | yes | yes | yes |
| 500 full solve (ms) | 61.4 \| 49.7 | 1.48 \| 2.1 | 1.51 \| 2.1 |
| 500 full solve, criterion mean (ms) | 46.6 | 1.38 | — |
| 500 DOF (37) / time (ms) | 37 ✓ / 70 \| 57 | not reported (190 \| 540 ms for its variable list) | 37 ✓ / 1.2 \| 1.9 |
| **500 drag-chain frame mean / max (ms)** | 3.0 / 3.3 \| 3.9 / 4.1 | 2.1 / 2.2 \| 2.0 / 2.2 | 1.4 / 1.6 \| 2.0 / 2.3 |
| **500 drag-local frame mean / max (ms)** | 1.2 / 1.3 \| 1.3 / 1.4 | 0.95 / 1.1 \| 1.0 / 1.3 | 0.66 / 0.81 \| 1.0 / 1.2 |
| 500 drag frames verified | 60/60 \| 60/60 | 60/60 \| 60/60 | 60/60 \| 60/60 |
| 500 conflict: detect + name (ms) | 2,039 \| 2,169 | 195 \| 549 | 8.1 \| 10.6 |
| wasm size, stripped (raw / gzip, incl. harness) | 677 KB / 233 KB | 658 KB / 208 KB | same build as ezpz |
| Licence (`cargo deny check licenses`, repo `deny.toml`) | pass | pass | pass |
| `cargo deny check sources` / `advisories` | **fail**: git sources; `ttf-parser` unmaintained | pass / **fail**: `paste` unmaintained (via `faer` → `gemm`) | as ezpz |
| Unexpected wasm imports | none | none | none |

### 1,000 entities (1,008 curves): native | wasm

| Criterion | A: SolidCraft | B: ezpz alone | B: ezpz + Locus analysis |
|---|---|---|---|
| full solve (ms) | 508 \| 315 | 3.5 \| 5.3 | 3.3 \| 5.3 |
| DOF (73) / time (ms) | 73 ✓ / 626 \| 419 | not reported | 73 ✓ / 2.7 \| 4.5 |
| drag-chain frame mean / max (ms) | 15.9 / **23.3** \| 7.5 / 9.0, **0/30 frames solved** | 5.3 / 5.5 \| 6.7 / 9.4 | 3.7 / 3.8 \| 5.8 / 6.5 |
| drag-local frame mean / max (ms) | 3.4 / 3.6 \| 3.7 / 5.8 | 2.7 / 2.8 \| 3.3 / 3.7 | 1.8 / 1.9 \| 3.2 / 3.4 |
| conflict: detect + name (ms) | 18,331 \| 18,990 | 4,099 \| 4,568 | 19.5 \| 27.4 |

In wasm at 1,000 curves, SolidCraft's drag frames end inside its per-frame work budget without converging: every frame was left violating constraints.

**Not measured:** branch preservation on large dimension edits (SolidCraft has explicit "fold" handling; ezpz relies on starting from the previous solution). Sketches that do not decompose constructively (our generated sketch is a chain, which suits SolidCraft's constructive seed). Firefox and Safari timings. macOS natively. Each is a follow-up below.

**Reproduce:** see [`spikes/solver/README.md`](../../spikes/solver/README.md). Run `cargo run --release --bin table [cells]` or `cargo bench` in the container, and open `web/index.html` for wasm.

## Decision

**Use `ezpz` as the v1 solving backend behind the `locus-solver` `Solver` trait, and implement DOF and redundancy analysis in `locus-solver` ourselves (Jacobian rank, as prototyped).** It is the only candidate that is fast at every size we measured and natively sparse. Its gaps against §3 (no DOF count, no redundancy) are closed by an analysis that costs about 1 ms and that Locus needs to own anyway for its §5 UX. SolidCraft is a strong behaviour reference but not a dependency.

The #39 recommendation is therefore **no** for SolidCraft as a dependency. It is not "yes-with-fork" either: a fork would carry ~7 k lines of dense-matrix solver that misses the drag budget past 1,000 curves and takes seconds to report a conflict. We keep reading it for behaviour (branch preservation, shared-end-point tangency rows, redundancy reporting).

## Consequences

- **M1 (`locus-solver`):** add `ezpz = "=0.2.29"` (exact pin, upgrades reviewed by re-running the harness) behind the `Solver` trait. Move `harness/` into `locus-solver` as its backend test suite, with `cargo bench -p locus-solver` printing the table (#41's acceptance wording). Port `analysis.rs` with Locus-owned residual definitions. Keep the harness checker as the guard that ezpz and our residuals agree.
- **Needs owner approval (flagged, not done):** `faer` → `gemm` → `paste` triggers `cargo deny` advisory RUSTSEC-2024-0436 ("unmaintained"; a build-time proc-macro, no known vulnerability). Adding ezpz to a core crate will fail the gate unless `deny.toml` gets an `ignore` entry with this justification. CLAUDE.md forbids weakening `deny.toml` without explicit instruction, so the M1 PR must ask.
- **Spec notes (flagged, not resolved):**
  - §3's "500 entities" doesn't say whether points count. This ADR measured 504 curves *plus* 720 points, the stricter reading; at 1,000 curves ezpz still holds a 6.5 ms worst frame in wasm.
  - §3 lists `ttf-parser` as the font-parsing candidate, and it now carries RUSTSEC-2026-0192 (unmaintained). That affects the text ADR (§9), not this one.
  - The decision is a hybrid of §3's "another Rust GCS" and "our own" options (third-party solve, own analysis). The spec's wording allows it; the §0 and §12.3 entries now say so.
- **Risks accepted:** ezpz is 0.x and controlled by Zoo; if it stalls or changes direction, the `Solver` trait plus the harness make swapping cheap. The fallback is our own sparse LM (#42 full scope) on the residuals the analysis already defines. Conflict naming currently uses ezpz's unsatisfied list; if it proves too broad, derive minimal conflict sets from the rank analysis.
- **Follow-ups (M1 issues):** branch-preservation fixtures for dimension edits, compared against SolidCraft's fold behaviour. A non-constructive (cyclic) 500-entity fixture. Firefox/Safari and macOS timings. Re-run `sketch-solver` and `acs-solver` on the harness once they mature.
- **Licence notices to retain:** ezpz and `faer` (MIT) need their copyright and licence texts shipped with binaries, through the third-party licence bundle M4 packaging produces. No SolidCraft code was copied. If any is adapted later, keep its `Copyright (c) 2026 SolidCraft contributors` MIT OR Apache-2.0 notice in the file and in the third-party list (§18).
- **Revisit if:** ezpz cannot express a v2 constraint (ellipses, splines, stroke-edge references) without adapter contortions, or a drag frame exceeds 16 ms on a real M1 sketch.
