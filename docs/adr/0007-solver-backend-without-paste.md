# ADR-0007: Solver backend without `paste`

- **Status:** Accepted
- **Date:** 2026-11-04
- **Supersedes:** [ADR 0002](0002-solver-backend.md) (its solving backend; its harness, fixtures and Jacobian-rank analysis carry over)
- **Spec:** `docs/SPEC.md §3` (Solver backend, Solver requirements), `§5`, `§2` (`cargo deny`), `§12.3`
- **Issue:** Nerdtisan/Locus#57 (backend), M1 solver #57–#64

## Context

ADR 0002 chose `ezpz` 0.2.29 for solving. Its tree `faer → gemm` pulls in `paste`, which carries RUSTSEC-2024-0436 ("unmaintained"), so `cargo deny check` fails with ezpz in a core crate. ADR 0002 asked the owner to approve an `ignore` entry. **Owner decision, 2026-11-04: do not accept the advisory. Find an alternative first.**

The requirements are unchanged (§3, §5): pure Rust, builds for `wasm32-unknown-unknown`, every §5 v1 constraint, drag re-solve under 16 ms at 500 entities, a DOF count, and redundant and conflicting constraints named. Every candidate must pass the repo's `deny.toml` unchanged.

## Is `paste` still in the latest ezpz tree?

Checked on crates.io, 2026-11-04:

| Crate | Latest | Depends on `paste`? |
|---|---|---|
| `ezpz` | 0.2.29 (2026-08-05) | via `faer ^0.24` (non-optional) |
| `faer` | 0.24.4 | via `gemm ^0.19` and `pulp ^0.22` (both non-optional) |
| `gemm`, `gemm-common`, `gemm-f64` | 0.19.0 | **yes**, a direct non-optional dependency |
| `pulp` | 0.22.3 | **yes**, a direct non-optional dependency |

ezpz has no other linear-algebra backend and no feature to drop `faer`. The latest versions still need `paste`, so ezpz is out.

Paste-free crates found: `nalgebra` 0.35, `simba` 0.10 and `nalgebra-sparse` 0.12 (older `simba` pulled in `paste`; 0.10 no longer does). `sketch-solver` 0.1.0 depends only on `libm`.

## Options

All candidates were run on ADR 0002's harness (`spikes/solver/`): same fixtures, same independent residual checker, native and wasm.

### Option A: ezpz + `deny.toml` ignore

The owner rejected it. Measured here for reference only.

### Option B: a Locus-owned sparse Levenberg–Marquardt solver, own linear algebra (zero dependencies)

`spikes/solver/spike/src/own/`, about 1,100 lines with the adapter:

- **One residual definition.** Each constraint is a block of one or two rows touching at most 8 unknowns. It is written once over forward-mode dual numbers (`dual.rs`), which give exact gradients. Solving and the rank analysis use the same rows. ADR 0002 had two definitions (ezpz's and ours) kept in step only by the checker.
- **LM with `λI` damping** (`lm.rs`, Nielsen's update). `(JᵀJ + λI)⁻¹Jᵀ` tends to the minimum-norm step, so under-constrained geometry moves as little as possible, and the system stays positive definite at any DOF.
- **Sparse LDLᵀ** (`ldl.rs`): a minimum-degree ordering on the explicit elimination graph, then an up-looking LDLᵀ after T. A. Davis's LDL (elimination tree, row patterns by tree reach). The pattern of `JᵀJ` depends only on which unknowns each block touches. Ordering and symbolic analysis therefore run once per constraint set, at drag start for a drag. Every iteration and every drag frame only refactors numerically.
- **DOF and redundancy:** ADR 0002's sparse row-echelon rank analysis (`rank.rs`), now on the exact gradients.
- **Conflicts:** at a least-squares minimum, `Jᵀr = 0` with `r ≠ 0`, so the residual vector is itself a dependency among the rows. The constraints it leaves violated are the conflicting set.

### Option C: Option B's solver with `nalgebra-sparse` 0.12 for the linear solve

Same code, same ordering and assembly. The factorisation is `nalgebra-sparse`'s left-looking `CscCholesky`, which has no fill-reducing ordering of its own, behind the same `LinSolve` trait (`nalg.rs`). It adds 19 crates (`nalgebra`, `simba`, `num-*`, `wide`/`safe_arch`, `matrixmultiply`, and proc macros via `nalgebra-macros`/`syn`). It passes `cargo deny check` unchanged (checked by adding it to `locus-solver`: advisories, bans, licences and sources all ok).

### Option D: `sketch-solver` 0.1.0 / `acs-solver` / `laman`

These are still the single-release, few-hundred-download crates ADR 0002 surveyed (`sketch-solver` has had no release since 2026-10-07). They don't meet §3's "mature" bar, so they weren't run. `acs-solver` and `laman` use dense `nalgebra`. A dense solve at 1,512 unknowns costs about 1 GFLOP per factorisation, two orders of magnitude over the drag budget.

## Measurements

**Setup:** branch `spike/solver-no-paste`, as in ADR 0002. AMD Ryzen 9 9950X3D, Windows 11, natively in the `locus-dev` container (Rust 1.99, `--release`, x86-64 Linux). wasm32: release, stripped, in Chromium 152 (V8). The page isn't cross-origin isolated, so wasm times round to 0.1 ms. Native runs vary about ±20%. Every result below is checked by the harness checker (every constraint within 1e-6). **Max drag frame includes the first frame**, which also runs the drag's ordering and symbolic analysis. Later frames are numeric only.

### 500 entities (504 curves + 720 points, 1,512 unknowns): native | wasm

| Criterion | A: ezpz + rank analysis | **B: own LM + own LDLᵀ** | C: own LM + nalgebra-sparse |
|---|---|---|---|
| §5 v1 coverage (27 cases, checker-verified) | 27/27 | 27/27 | 27/27 |
| M0 fixture full solve (ms) | 0.088 \| 0.11 | **0.010 \| 0.016** | 0.011 \| 0.016 |
| M0 DOF (0) / under (1) / redundant named | 0 / 1 / yes | 0 / 1 / yes | 0 / 1 / yes |
| M0 conflict: detected, both named (ms) | yes, 0.37 \| 0.4 | **yes, 0.025 \| 0.1** | yes, 0.025 \| 0.1 |
| 500 full solve (ms) | 1.34 \| 2.1 | **0.42 \| 0.6** | 0.42 \| 0.6 |
| 500 DOF (37) / time (ms) | 37 ✓ / 1.0 \| 2.0 | **37 ✓ / 0.23 \| 0.3** | 37 ✓ / 0.24 \| 0.3 |
| **500 drag-chain frame mean / max (ms)** | 1.29 / 1.36 \| 2.06 / 2.2 | **0.46 / 1.42 \| 0.59 / 1.7** | 0.45 / 1.64 \| 0.61 / 1.9 |
| **500 drag-local frame mean / max (ms)** | 0.54 / 0.61 \| 1.05 / 1.3 | **0.08 / 1.00 \| 0.10 / 1.2** | 0.09 / 1.32 \| 0.12 / 1.5 |
| 500 drag frames verified | 60/60 | 60/60 | 60/60 |
| 500 conflict: detect + name (ms) | 7.7 \| 10.6 | **2.0 \| 2.6** | 1.9 \| 2.6 |
| wasm size, stripped, raw / gzip (incl. harness) | 658 / 208 KB | **188 / 66 KB** | 238 / 81 KB |
| `cargo deny check` (repo `deny.toml`, unchanged) | **fail**: `paste` | pass (no dependencies) | pass |
| Unexpected wasm imports | none | none | none |

### 1,000 entities (1,008 curves + 1,440 points): native | wasm

| Criterion | A: ezpz + rank analysis | **B: own LM + own LDLᵀ** | C: own LM + nalgebra-sparse |
|---|---|---|---|
| full solve (ms) | 3.0 \| 8.2 | **1.13 \| 2.0** | 1.22 \| 1.7 |
| DOF (73) / time (ms) | 73 ✓ / 2.4 \| 7.9 | **73 ✓ / 0.45 \| 1.1** | 73 ✓ / 0.45 \| 0.6 |
| drag-chain frame mean / max (ms) | 3.3 / 4.1 \| 7.5 / 10.6 | **1.24 / 3.0 \| 1.9 / 5.8** | 1.22 / 3.5 \| 1.75 / 4.3 |
| drag-local frame mean / max (ms) | 1.36 / 1.40 \| 5.4 / 6.5 | **0.16 / 2.0 \| 0.21 / 2.4** | 0.18 / 2.6 \| 0.24 / 2.8 |
| conflict: detect + name (ms) | 18.7 \| 39.4 | **1.1 \| 1.5** | 1.1 \| 1.5 |

**Drag quality** (`cargo run --release --bin probe [cells]`) compares B and C with ezpz on the same drags. They give identical motion per frame (9.35 mm RMS at 500, 13.22 mm at 1,000). Geometry on the fixed side of the gap moves at most 4e-7 mm at 500 and 8e-5 mm at 1,000, the same as ezpz (3.8e-7 and 8.1e-5). The minimum-norm step adds no drift.

**One tuning finding.** At first, λ started at 1e-6. On the 1,000-curve chain drag, frames then took 29 LM iterations (8 ms). Tracing showed soft modes along the long chain with σ² ≈ 1e-7: a λ that large over-damps them, and LM creeps. Starting at λ = 1e-9 cuts this to about 4 iterations per frame. λ still rises by itself when a step is rejected or a pivot isn't positive. Scaling the direction rows (parallel, perpendicular, angle, fillet tangency) to millimetres made no measurable difference, but it is kept: every row then shares one unit and one tolerance.

**Reproduce:** see `spikes/solver/README.md`. Run `cargo run --release --no-default-features --features ezpz,nalgebra --bin table [cells]`, and the same build for wasm, opened in `web/index.html`.

## Decision

**Option B: `locus-solver` owns the whole solver, with sparse Levenberg–Marquardt over Locus's own residuals and its own sparse LDLᵀ, with no third-party dependencies.** It and Option C are the only paste-free candidates. Both are faster than ezpz at every size, natively and in wasm. Both meet §3's drag target by a wide margin: worst frame 1.4 ms native and 1.7 ms wasm at 500 curves, and 3.0 / 5.8 ms at 1,000.

Between B and C, speed is a tie. B wins on:
- no dependency tree to audit (0 crates against 19, including proc macros);
- a wasm bundle 15 KB smaller (gzip);
- no `assert!` or `unwrap()` inside the factorisation (`nalgebra-sparse` has both on its symbolic path; CLAUDE.md forbids panics on solver paths);
- one owner for the code that decides numerical behaviour.

C stays the fallback behind the same `LinSolve` trait. It is about 100 lines and is already measured equal.

This is §3's "our own" option. It replaces ADR 0002's hybrid of "another Rust GCS" for solving and "our own" for analysis.

## Consequences

- **M1 (`locus-solver`):** port `own/` (dual numbers, model, LM, LDLᵀ, rank analysis) into `locus-solver` behind the `Solver` trait (#57), with its own solver-input model (parameters plus constraint residual blocks), independent of `locus-doc`. Move `harness/` in as the backend test suite, with `cargo bench -p locus-solver` printing the table (#41's wording, #63). Delete `spikes/solver/` once it is absorbed. ADR 0002's plan to keep two residual definitions in step falls away: there is now one.
- **Tests we now own:** the LDLᵀ checked against a dense reference on random SPD systems. The dual numbers checked against finite differences. LM convergence on the harness fixtures, plus the #64 property tests (random valid constraint sets converge; adding then removing a constraint restores the DOF).
- **No `deny.toml` change.** RUSTSEC-2024-0436 never enters the tree. ADR 0002's request for an `ignore` entry is withdrawn.
- **Licence notices:** none needed. No third-party code was copied. The LDLᵀ is written from the published algorithm (Davis, "Algorithm 849: A concise sparse Cholesky factorization package", ACM TOMS 2005), not from LDL's LGPL source.
- **Risks accepted:** we maintain the numerics. Mitigations: the harness checker on every solve, the tests above, and Option C as a measured drop-in. Minimum degree on the explicit graph is O(Σ deg²). It is fine for sketches (2 ms at 3,024 unknowns, once per drag or edit), but if huge sketches make it a cost, an approximate-minimum-degree (AMD) ordering is the upgrade.
- **Carried over from ADR 0002 as M1 follow-ups:** branch-preservation fixtures for dimension edits; a non-constructive (cyclic) 500-entity fixture; Firefox, Safari and macOS timings; minimal conflict sets from the rank analysis if the violated-residual set proves too broad.
- **Revisit if:** a §5 v2 constraint (ellipse, spline, stroke-edge reference) needs more than 8 unknowns per row (raise `dual::N`), or a drag frame exceeds 16 ms on a real M1 sketch.
