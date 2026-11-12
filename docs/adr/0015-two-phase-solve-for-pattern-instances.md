# ADR-0015: Two-phase solve for constraints onto pattern instances

- **Status:** Accepted
- **Date:** 2026-11-12
- **Spec:** `docs/SPEC.md §6.2.1` (Constraint behaviour), `§11` (undo)
- **Issues:** Nerdtisan/Locus#194; follow-up to ADR 0011 (#166, #168, #169)

## Context

ADR 0011 keeps pattern instances out of the solver: they are written after each solve from the sources' solved positions. §6.2.1 says other constraints may refer to instances ("dimension to an instance"), but the first cut refused every constraint onto one because an instance is not a solver variable.

## Decision

A constraint that touches a **pattern** instance (linear or circular) is solved in a **second phase**, after the pattern has been regenerated:

1. **Main phase** (unchanged): solve every constraint that does *not* touch a pattern instance, write the solution, regenerate the recipes (so instances sit where the solved sources put them).
2. **Late phase**: lower the whole sketch again with the pattern instances *included*, and hold **fixed**: every instance, the pattern's sources, and whatever else the pattern reads (circular centre, direction lines), with the points they are built on. Solve all constraints (the late ones now have variables on their free side). Only geometry the pattern does not depend on can move.

Because the sources and everything the pattern reads are fixed in phase 2, the regenerated instances cannot move, so no iteration is needed: one pass, deterministic. A constraint that would need the pattern or its sources to move is unsatisfiable and is refused whole with a structured `DocError::PatternHeld` naming the constraints, the instances and their sources, and suggesting to constrain the source or a pattern parameter instead (MCP kind `pattern_held`); the document and history are untouched.

* Both phases run inside the same command and the same undo transaction (§11). Phase 2 is skipped when no constraint touches an instance, so ordinary documents pay nothing.
* After a drag ends, phase 2 runs and its changes join the drag's undo step. During a drag only the main phase runs: a follower catches up on release.
* Driven (reference) dimensions onto instances are measured in phase 2, so they are exact and follow the pattern (the stale-value problem ADR 0011 named).
* `Sketcher::new` runs both phases for a loaded document.
* Pre-checks (redundancy / conflict analysis on add) do not see late constraints; phase 2 reports an unsatisfiable one. The DOF count and the fully-constrained colouring come from the main phase, so geometry pinned only by a constraint to an instance shows as under-constrained.
* Shrinking a pattern so a referenced instance would disappear is refused (`EntityInUse`, existing behaviour), never silent.
* **Still refused:** constraints onto offset and plugin-object instances (they stay unsolved and constraint-free); mirror instances are real solver entities and unchanged.

## Alternatives considered

* **Iterate to a fixed point**, letting phase 2 move the sources and regenerating until stable. Needs a cap, can fail to converge, and an instance edited through its source then silently moves the user's other geometry. Not taken.
* **Why not iterate (design B, a bounded fixed-point loop):** convergence is not guaranteed and the result would depend on iteration order, and a constraint on an instance would silently move the user's source. Kept as a possible later extension; Fusion's behaviour, where an instance can drive the source, is the long-term reference (M5, #221).
* **Make instances solver variables** with a symmetric / transform constraint per copy. Large systems (10 000 copies) and no new information over phase 2. Not taken.

## Measured

`crates/locus-doc/tests/patterns.rs`: a follower point coincident with, or at a driving distance from, an instance stays exact after pitch changes and a source drag and undoes in one step; driven dimensions re-measure; a fixed point 1 mm from a fixed instance is refused with history untouched; constraints round-trip through the file; circular patterns behave the same.
