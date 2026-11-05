# ADR-0008: Booleans that keep true arcs

- **Status:** Accepted
- **Date:** 2026-11-05
- **Spec:** `docs/SPEC.md §6.3` (Regions & booleans, "Known tension"), `§3` (crate candidates), `§4` / `§8` (tolerances), `§2` (`cargo deny`)
- **Issue:** Nerdtisan/Locus#120 (blocks #121, #123)

## Context

§6.3 asks for automatic regions and non-destructive union / subtract / intersect of them. DXF R12 and laser software want true `ARC`/`CIRCLE` entities, not polylines, so a boolean must give back arcs. §6.3 names the tension: most Rust boolean crates flatten to polygons. It proposes flatten → boolean → **arc refit**, or `cavalier_contours` where it applies.

Requirements for the chosen approach:

- Result within the 0.001 mm export tolerance (§4, §8) of the exact result.
- Arcs come back as arcs, and as few segments as the geometry needs.
- Robust on what sketches produce all the time: shared edges, T-junctions, touching corners, tangent and coincident circles, positions equal only to the solver tolerance (1e-6 mm, §4).
- Operands with holes and several loops (a region is an outer loop plus holes).
- Pure Rust, builds for `wasm32-unknown-unknown` with no filesystem, `deny.toml` unchanged.
- Fast enough to recompute after every solve (§6.3 "recomputed on solve").

Locus geometry today is lines, circles and arcs only (§4 v1; Bézier is still to come).

## Options

### Option A: flatten → `i_overlay` → arc refit

`i_overlay` 9.1.0 (MIT; the engine inside `geo`). Each loop is flattened to chords with ≤ 0.2 µm sagitta, combined with the `i64` engine and the non-zero fill rule, then each output contour is refitted greedily: the longest run of vertices that fits a line, or an arc (vertices **and chord midpoints** within 0.5 µm), wins. Arc ends are exact polygon vertices, so loops stay closed. A0 is the same without the refit (polygon only, 0.5 µm sagitta), for scale.

### Option B: `cavalier_contours`

`cavalier_contours` 0.9.0 (MIT OR Apache-2.0). Polylines with bulges: arcs go in and come out as arcs. Booleans are **one closed polyline against another**. Holes and multi-loop operands must be composed by the caller from repeated pline booleans.

### Option C: Locus-owned exact line/arc arrangement

About 300 lines (`spikes/booleans/src/own.rs`) plus a shared winding test:

1. Split every segment at every crossing, T-junction, tangency and overlap end. Vertices are snapped within 1e-6 mm (`SOLVER_TOL`).
2. Drop duplicate edges (coincident pieces from both operands).
3. Classify each edge by each operand's winding number 0.1 µm left and right of its midpoint. Keep it where the result changes, oriented with the result on its left.
4. Trace loops, taking the sharpest left turn at branch vertices (curvature breaks tangent ties).
5. Merge pieces that share a line or a circle back into one segment (a full circle comes back as one circle).

Steps 1, 2 and 4 are the planar arrangement and face tracing that region detection (#121) needs anyway.

## Measurements

Harness: `spikes/booleans/` (standalone workspace), `cargo run --release -- 5000`. Built in the `locus-dev` container (Rust 1.99.1) on the owner's Windows 11 host, native x86-64 release, single thread.

**Scoring.** Each result is compared with an independent fine reference: `i_overlay`, `i64` engine, 0.1 µm sagitta.

- *Dev out→ref*: the largest distance from the result's boundary (sampled every 0.01 mm) to the reference boundary.
- *Dev ref→out*: the reverse.
- *ΔArea*: area difference. Areas are computed exactly for arcs (Green's theorem).

A result **passes** when all of these hold:

- out→ref ≤ 0.001 mm
- ΔArea ≤ ½ · 0.001 mm · perimeter
- every loop is closed
- no sliver loops (|area| < 1e-6 mm²)

Reference contours thinner than 1e-5 mm are true-but-sub-tolerance slivers, so dropping them counts as correct. That is also why ref→out can be large while a result still passes: the reference keeps a 1e-7 mm attached sliver that a correct result removes. Panics are caught and counted.

### Fixtures

Arcs / lines are counted against the minimal exact result, where it is known by hand. Times are per boolean.

| Fixture | A: refit | B: cavalier | C: own |
|---|---|---|---|
| rect ∪ circle on a corner | pass · 1/4 arcs/lines · 1.4 ms | pass · 2/4 · 1 µs | pass · **1/4** · 6 µs |
| slot − circle over its cap | pass, dev 2.0e-4 · 3/2 | pass, 1e-7 · 4/2 | pass, 1e-7 · 3/2 |
| circle ∩ circle (lens) | pass, dev 1.8e-4, ΔA 1.8e-3 · 2/0 | pass · 4/0 | pass · **2/0** |
| rect ∪ rect, shared edge | pass · 0/4 | **FAIL**: 2 loops, sliver, dev 5 mm | pass · 0/4 |
| rect ∪ rect, partly shared edge | pass · 0/8 | **FAIL**: sliver, dev 2.5 mm | pass · 0/8 |
| rect ∪ rect, touching corner | pass (2 loops) | pass (1 pinched loop) | pass (2 loops) |
| rect − rect, notch on an edge | pass | pass | pass |
| circle − concentric circle | pass · 4/0 · 6.6 ms | pass · 4/0 | pass · **2/0** |
| circle ∪ circle, external tangent | pass · 4/0 | pass · 4/0 | pass · **2/0** |
| circle − circle, internal tangent | pass · 4/0 | pass · 4/0 | pass · **2/0** |
| slot ∪ circle on its cap (coincident arc) | pass · 3/2 | pass · 3/2 | pass · **2/2** |
| circle ∪ the same circle | pass · 2/0 | pass · 2/0 | pass · **1/0** |
| rect ∩ circle, tangent at one point | pass (empty) | pass (empty) | pass (empty) |
| rounded rect − slot inside (hole) | pass · 8/6 | pass · 6/6 | pass · **6/6** |
| annulus ∪ bar (operand with a hole) | pass, dev 1.7e-4 · 4/8 | **n/a** (single plines only) | pass · **4/8** · 19 µs |
| plate − 50 holes | pass · 105/4 · 47 ms | pass¹ · 104/4 · 46 µs | pass · **54/4** · 0.70 ms |
| union of a chain of 40 circles | pass · 78/0 · 9.6 ms | pass¹ · 80/0 · 0.24 ms | pass · **78/0** · 1.3 ms |
| disc ∪ 60 teeth | pass, dev 2.8e-4 · 120/0 · 11.6 ms | pass¹ · 182/0 · 0.77 ms | pass · **120/0** · 2.5 ms |

¹ Folded loop by loop by the harness; cavalier cannot take a multi-loop operand.

Where C passes, its deviation is 1e-7 mm. That figure is the reference's own sagitta, so C's real error is below what the reference can measure. A's deviation is its fit tolerance, 2–3e-4 mm: within tolerance, but it uses up a third of the export budget. A0 (polygons only, 0.5 µm sagitta) passes the fixtures but needs 170–7,300 segments per result where C needs 2–58, and spends half the export tolerance on chords (dev 4.9–6.9e-4 mm).

### Randomised robustness sweep, 5,000 cases each

Each case takes two random shapes (rectangle, circle, slot, rounded rectangle) and a random operation.

| Sweep | A: refit | A0: lines | B: cavalier | C: own |
|---|---|---|---|---|
| 0.5 mm grid: exact shared edges, T-junctions and tangencies | 4,989 pass, 11 fail | 4,971 / 29 | 4,946 / **54** | **5,000 / 0** |
| same, operand B shifted by up to 1e-7 mm (solver-tolerance near-coincidence) | 4,964 / 36 | 4,949 / 51 | 4,922 / **78** | **5,000 / 0** |
| general position (random floats) | 4,991 / 9 | 4,945 / 55 | 5,000 / 0 | **5,000 / 0** |
| Panics | 0 | 0 | 0 | 0 |
| Arcs / lines in all results (grid sweep) | 11,971 / 15,867 | 0 / 489,092 | 12,075 / 16,108 | **10,354 / 15,797** |

Failure modes:

- **A**: the refit drifts past tolerance (1.1–2.7e-3 mm) where it meets intersection vertices, which lie on chords rather than on the curve. On some grid cases it misses whole features (up to 2.8 mm).
- **B**: on shared and near-shared edges it returns sliver loops or the wrong side (up to 4.5 mm off).
- **C**: worst out→ref deviation is 1.6e-4 mm (grid) and 5.1e-4 mm (near-coincident). Both come from deliberately collapsing a lens or sliver thinner than 1e-6 mm into a touching point, which is correct at this tolerance.

C's first run failed 4 of 3,000 grid cases. The cause: a probe ray passed exactly through a vertex where the arc's computed endpoint (cos/sin) and the neighbouring line's stored endpoint differed by one ulp. The fix gives each loop vertex one canonical position in the winding test, and the failures went to zero. In the product this comes for free: loop vertices are shared sketch points.

### wasm32 and licences

| | A: i_overlay + refit | B: cavalier_contours | C: own |
|---|---|---|---|
| `wasm32-unknown-unknown` build | yes | yes | yes |
| cdylib size, `opt-level = "z"`, LTO (raw / gzip) | 195 KB / 63 KB | 105 KB / 40 KB | **69 KB / 28 KB** (includes the spike's shared geometry) |
| New dependencies | `i_overlay`, `i_float`, `i_shape`, `i_tree`, `i_key_sort`, `libm` | `cavalier_contours`, `ahash`, `zerocopy`, `smallvec`, `static_aabb2d_index`, `num-traits` | none |
| `cargo deny check` with the repo `deny.toml`, unchanged | ok | ok | ok |

Not measured: timings inside a browser (wasm32 timings were within 1.5× of native for the solver, ADR 0007), and Bézier segments, which Locus doesn't have yet.

## Decision

**Option C: a Locus-owned exact line/arc arrangement in `locus-region`, with no new dependencies.** It is the only option with zero failures across 15,000 random cases and all fixtures, including the shared-edge and solver-tolerance cases that sketches produce all the time. Its results are exact (deviation below the reference's 1e-7 mm resolution, so nothing to refit). Arc counts are minimal: a circle stays one circle, coincident pieces merge back. It handles holes and multi-loop operands directly. It reuses the planar arrangement that region detection needs anyway, which also follows the ADR 0007 precedent of owning small, focused geometry code.

- `cavalier_contours` is rejected: it fails on shared edges, the most common sketch case, and it only combines single polylines. It stays the §3 candidate for **offsets** (kerf, inset), which this ADR does not decide.
- Flatten + refit is rejected: it is the slowest option (1–47 ms), spends a third of the export tolerance and still misses on 0.2–0.7 % of cases.

## Consequences

- `locus-region` owns the arrangement: splitting at intersections and overlaps with `SOLVER_TOL` snapping, de-duplication, loop tracing and carrier merging. Booleans and region detection (#121) are both built on it. Booleans go through edge classification by winding number.
- Tolerances are the named §4 constants: snapping and on-curve tests at `SOLVER_TOL` (1e-6 mm). Features thinner than that collapse into touching points. Classification probes sit 1e-4 mm off an edge (or 10 % of a very short edge), so features thinner than 0.1 µm are below the classifier's resolution. That is 10× under `EXPORT_TOL`.
- The product port must keep the canonical-vertex rule (one position per loop vertex, shared by both segments meeting there). The spike's random sweeps become `proptest` property tests in `locus-region`.
- The spike's pair search is O(n²) and each edge's winding test is O(n). That is fine for the M2 sketches measured here: 0.7–2.5 ms for 50–120 arcs. A bounding-box sweep or grid is the known next step if a large imported sketch makes a recompute slow.
- Bézier (§4 v1 "limited constraints") is not covered. When it lands, its segments need intersection support in the arrangement, or flattening with a recorded deviation. Revisit then.
- §6.3, §12 and §0 now point here.
- The spike stays in `spikes/booleans/` until #121 / #123 absorb its tests, then it is removed and kept at a git tag, like the solver spike.
