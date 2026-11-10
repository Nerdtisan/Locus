# ADR-0010: Offsets that keep true arcs

- **Status:** Accepted
- **Date:** 2026-11-10
- **Spec:** `docs/SPEC.md §6.2` (offset), `§7` (kerf, drag-knife blade offset), `§8` (operation transforms), `§3` (crate candidates), `§4` (tolerances)
- **Issue:** Nerdtisan/Locus#161 (blocks #164, #173, #174)

## Context

Three features need one offset engine:

- the live offset tool (§6.2),
- kerf compensation on Cut layers (§7, #173),
- drag-knife blade offsets (§7, #174).

Laser and DXF output wants true arcs (ADR 0008), so offsets must keep them.

Requirements:

- Offset closed areas (outer loops and holes, nested parts) and open chains, either side.
- Corner joins: round (the true offset, what a beam cuts) and miter (the sharp corners the offset tool should give).
- Self-intersections, loops and collapsed features removed. Examples: a notch narrower than 2·d closes; a slot shrunk by its radius vanishes instead of leaving a zero-width sliver.
- Result within `EXPORT_TOL` (0.001 mm) of the exact offset, on the shapes sketches produce all the time:
  - tight inside corners,
  - radii smaller than the offset,
  - slots,
  - exact touching (offset curves that meet exactly at the grid),
  - nesting.
- Béziers through arcs: the cubic entity (#159) is already lowered to G1 biarcs within `EXPORT_TOL` (`Cubic::to_pieces`).
- Pure Rust, builds for `wasm32-unknown-unknown`, `deny.toml` unchanged.
- Fast enough for export and for recomputing a live offset after each solve.

§3 named `cavalier_contours` as the candidate, and ADR 0008 kept it open for offsets.

## Options

### Option A: Locus-owned, Minkowski sum or difference through the ADR 0008 booleans

The offset of an area A by d is a Minkowski sum or difference with a disc D of radius |d|:

- **grow:** A ∪ (∂A ⊕ D)
- **shrink:** A − (∂A ⊕ D)

∂A ⊕ D is built from exact pieces, all true lines and arcs:

- one **band** per boundary edge: the points whose nearest point on the edge is interior and at most |d| away. That is a rectangle for a line, and an annular sector for an arc (a wedge once |d| ≥ r).
- a **disc** at every corner that opens a gap on the offset side. A corner opens a gap when its turn is away from that side, or at a 180° cusp.
- for a miter join, a **kite** (corner, two offset points, miter point) instead of the disc, while the miter stays under a limit.

One `locus_region` boolean then gives the exact result. The arrangement does the cleanup:

- splits at every crossing,
- classifies each piece by winding number,
- traces the loops,
- merges pieces on one carrier.

Each result edge keeps the source entity of the boundary edge it came from.

**Open chains:** the boundary of the chain's tube (chain ⊕ D) is cut into runs. A run keeps the pieces whose nearest chain point is interior and on the chosen side.

**Specks:** loops whose mean width (2·area / perimeter) is under `EXPORT_TOL` are dropped. They are specks and slivers where an offset nearly vanishes, and no machine can cut them.

About 250 lines (`crates/locus-region/src/offset.rs`), on top of the arrangement.

### Option B: `cavalier_contours`

`cavalier_contours` 0.9.0 (MIT OR Apache-2.0):

- `Shape::parallel_offset` for areas with holes and islands,
- `Polyline::parallel_offset` for open polylines.

Arcs are bulges. It uses the classic raw-offset → self-intersection slicing → distance filtering → stitching pipeline. Joins are round only.

## Measurements

Harness: `spikes/offsets/` (standalone workspace; Option A is the product code in `locus-region`), `cargo run --release -- 2000`. Built in the `locus-dev` container (Rust 1.99) on the owner's Windows 11 host, native x86-64 release, single thread.

**Oracle.** The round offset of A by d is the set of points that are:

- **grow:** in A, or within d of ∂A;
- **shrink:** in A and farther than |d| from ∂A.

Distances to lines and arcs are exact (`Seg::distance`), so no reference implementation is needed. A result **passes** when:

- every boundary point, sampled every 0.01 mm (0.05 mm in the sweeps), is |d| from ∂A within `EXPORT_TOL`;
- every probe on a 200 × 200 grid (60 × 60 in the sweeps) is classified like the oracle. Probes within `EXPORT_TOL` of the true offset curve are skipped.
- every loop is closed;
- there is no sliver loop (|area| < 1e-6 mm²).

Open chains pass when every output point is |d| from the chain, on the requested side, and the runs are continuous.

**Same filter for both.** Both options get the same speck filter, so only the engines differ. Without it, B fails 57 more grid cases, all on degenerate leftover loops. One example: a rectangle 20 wide shrunk by 10 comes back as a 2-vertex zero-area loop, which would be exported as a double cut.

### Fixtures

Cells: verdict · arcs/lines · loops · time per offset.

| Fixture | A: Locus-owned | B: cavalier_contours |
|---|---|---|
| rect 40×20, d = +1 / −1 / −10 | pass · 4/4, 0/4, empty · 0.03–0.06 ms | pass (−10 only after the speck filter) · < 0.01 ms |
| star, 12 points, tight inside corners, d = +1 / −1 / −3 | pass · 12/24, 12/24, 12/0 · 0.6–1.4 ms | pass · same counts · 0.01 ms |
| comb, 0.3 mm notches, d = +0.15 (notches close exactly) | pass · **22/13** · 0.56 ms | pass · 22/31 (leftover collinear pieces) |
| comb, d = +0.2 / −0.2 | pass · 22/13, 18/40 · 0.9 ms | pass · 22/13, 18/40 |
| rounded rect, r 0.5 (small radii), d = +1 / −0.5 / −1 | pass · 4/4, 0/4, 0/4 | pass · same |
| circle r 1, d = −0.999 / −1 / +2 | pass · **1/0**, empty, **1/0** | pass · 2/0, empty, 2/0 |
| slot r 3, d = +2 / −2.9 / −3 (collapses) | pass · 2/2, 2/2, empty | pass (−3 only after the speck filter) |
| nested: plate, hole, island, slot hole, d = ±0.1 | pass · 7/10 and 11/10 · 0.15–0.19 ms | pass · 8/10 and 12/10 |
| nested, d = +5 (island grows to touch the hole exactly) / +6 (bridges it) | pass · 3 loops · 0.2 ms | pass · 2 and 3 loops |
| gear, 60 teeth (120 arcs), d = +0.5 / −0.5 / −1 | pass · **60/0**, **240/0**, 240/0 · 4.0 / 11.2 / 13.2 ms | pass · 120/0, 300/0, 300/0 · 0.06 ms |
| plate with 50 holes, d = ±0.1 | pass · **54/4**, 51 loops · 0.5 ms | pass · 104/4 · 0.06 ms |
| gear, 240 teeth (480 arcs), d = +0.5 / −0.5 | pass · 240/0, 960/0 · **24 / 73 ms** | pass · 480/0, 1200/0 · 0.3 ms |
| open: zigzag with sharp turns, left / right 1 | pass · 0.07 ms | pass |
| open: J hook, r 1 bend, inside 2 (bend collapses) / outside 2 | pass | pass |
| open: S-wave of arcs, left 1 / right 2.5 | pass | pass |

Arc counts: A merges pieces that lie on one circle, so a circle comes back as one circle, and teeth whose offsets meet merge into one arc each. B returns split arcs (2–2.5× more segments on gears and holes).

### Randomised robustness sweeps, 2,000 cases each

Each case is a random area: a rectangle, circle, slot, rounded rectangle or star, combined with one or two more by a random boolean, so holes, cusps and shared edges appear. It is offset by a random d in [−3, 3].

| Sweep | A: Locus-owned | B: cavalier_contours |
|---|---|---|
| 0.5 mm grid, d in 0.25 steps (exact touching, collapse) | **2,000 / 0 fail** · 0.28 ms mean | 1,998 / 2 fail (1,970 / 30 without the speck filter) · 0.01 ms |
| same, second seed | **2,000 / 0 fail** · 0.29 ms | 1,997 / 3 fail (1,968 / 32) · 0.01 ms |
| general position (random floats) | **2,000 / 0 fail** · 0.28 ms | 2,000 / 0 · 0.01 ms |
| Panics | 0 | 0 |

**B's remaining failures** leave boundary pieces 0.37–1.5 mm off the true offset, on grid cases where offset curves meet exactly. Examples: sweep 0 #1376 with d = −1.5 (dev 1.50 mm), and sweep 1 #336 with d = −1 (dev 0.37 mm). These are the configurations ADR 0008 found it weak on. Run `cargo run --release -- case <sweep> <n>` in `spikes/offsets` to dump one.

**A's path to zero failures.** Three bugs were found and fixed on the way, and each is now a regression test or proptest in `locus-region`:

1. **Tangent cusps (shared boolean code).** A band side is tangent to the corner disc by construction. Within 0.1 µm of that tangency, the boolean's fixed side probe (`PROBE` = 1e-4 mm) landed across the other curve, an edge was misclassified, a loop failed to close, and whole results came back empty: 10 / 600 cases in the first run. The fix is in `locus_region::boolean`: each probe is pulled in to half the distance to the nearest other edge, floored at 1e-9 mm. This is a general robustness fix for booleans too.
2. **180° corners.** At a cusp that doubles back, the sign of the turn is noise, so the corner disc was skipped. Discs now go on both sides at a reversal.
3. **Sub-tolerance specks.** An 8-point star whose inradius is a hair over d leaves a 0.3 µm speck. This is a true result, and B returns it too, but it is uncuttable. Handled by the speck filter.

### Speed

The boolean classified every edge against every loop, O(E · L), which made A 140 ms on the 60-tooth gear in the first run. Three changes in `locus_region`'s boolean and winding test bring it to the times above, with all ADR 0008 fixtures and proptests unchanged:

- a uniform grid over loop and edge boxes,
- per-edge boxes cached for the winding test,
- an allocation-free arc winding test.

A is still 30–300× slower than B. It is roughly linear in boundary size: about 0.05 ms per boundary edge when growing, about 0.15 ms per edge when shrinking. That is well inside export needs, and inside a per-solve recompute for typical parts. A drag frame (16 ms) would only be at risk above about 300 edges of offset source; the live offset recomputes after the solve, not per frame (as regions do, #121).

### wasm32 and licences

| | A: Locus-owned | B: cavalier_contours |
|---|---|---|
| `wasm32-unknown-unknown` build | yes | yes |
| cdylib, `opt-level = "z"`, LTO (raw / gzip) | 116 KB / 45 KB, of which almost all is the arrangement Locus already ships for booleans | 159 KB / 61 KB |
| New dependencies | none | `cavalier_contours`, `ahash`, `zerocopy`, `smallvec`, `static_aabb2d_index`, `num-traits` |
| `cargo deny check`, `deny.toml` unchanged | ok | ok (MIT OR Apache-2.0) |

## Decision

**Option A: Locus-owned offsets in `locus-region` (`offset`, `offset_open`, `Join`), built on the ADR 0008 arrangement, with no new dependencies.**

- **Robust:** it is the only option with zero failures across 6,000 random cases and every fixture, including exact touching and collapse.
- **Exact:** deviation is at f64 rounding (below 1e-14 mm).
- **Minimal arcs:** circles stay circles, merged teeth stay one arc.
- **Complete:** it does miter joins, which `cavalier_contours` cannot, and it handles holes, islands and open chains through the same code.
- **Traceable:** every result edge keeps the source entity it was offset from, which the live offset (#164) and the preview (#176) use.

`cavalier_contours` is rejected for the same reason as in ADR 0008: it is wrong on exactly-touching configurations, which grid-snapped sketches produce all the time. Without a filter it also leaves zero-area loops. Its large speed lead does not matter at the sizes Locus offsets.

## Consequences

- `locus-region` owns offsets:
  - `offset(&Area, d, Join)`: d > 0 grows; outer loops CCW, holes CW.
  - `offset_open(&[Edge], d, Join)`: d > 0 to the left of travel.
  - Result loops thinner than `EXPORT_TOL` are dropped.
- Kerf (#173) offsets each closed path on its own by half the kerf width, with round joins. The side comes from nesting depth: outer contours grow and holes shrink for outside compensation.
- The blade offset (#174) uses the same pieces.
- The offset tool (#164) defaults to miter joins (Fusion behaviour) with a limit, and round where the limit is exceeded.
- Béziers are offset through their biarc pieces (`Cubic::to_pieces(EXPORT_TOL)`), so their offsets are arcs within `EXPORT_TOL`. An offset of a Bézier is not a Bézier.
- **Boolean changes (shared with ADR 0008's code).**
  - Probes are now adaptive (0.1 µm, or half the distance to the nearest other edge, down to 1e-9 mm). That refines ADR 0008's "features thinner than 0.1 µm are below the classifier".
  - Classification uses a uniform grid, which replaces the O(n) per-probe scan ADR 0008 listed as the known next step.
- The oracle sweep is a `proptest` in `crates/locus-region/tests/offsets.rs`. The spike stays in `spikes/offsets/` for re-measurement, then is removed and kept at a git tag, like the earlier spikes.
- §3, §6.2, §12 and §0 now point here.
