# ADR-0011: Associative recipes (mirror, patterns, offsets)

- **Status:** Accepted
- **Date:** 2026-11-10
- **Spec:** `docs/SPEC.md §6.2.1` (Mirror & pattern), `§4` (IDs), `§10` (format), `§11` (undo)
- **Issues:** Nerdtisan/Locus#166 (framework), #167 (mirror), #168 / #169 (patterns), #164 (live offset, reuses this)

## Context

§6.2.1 wants mirror, linear and circular patterns (and §6.2 a live offset) to be *associative*: stored as source IDs plus parameters, regenerated so editing the source updates every copy, explodable into plain geometry. It also says instances are *driven*: other constraints may refer to them, but they can't be dragged. The tension is how a derived copy coexists with the solver.

## Decision

* **Recipes live in the document** (`Document.recipes: BTreeMap<RecipeId, Recipe>`, format v5). A `Recipe` is `{ kind, sources, pool, active, constraints }`. `RecipeKind` is an enum (`Mirror { axis }` today; patterns and `Offset` add variants).
* **Instances are real entities** in the normal entity store, so constraints and dimensions can reference them. `Document::driven_by(id)` / `driven_entities()` say which entity belongs to which recipe. Driven entities refuse drag, `ModifyEntity` and delete (`DocError::Driven` / `RecipeInUse`); sources and the mirror axis refuse delete, layer move and layer delete while a recipe uses them; constraints a recipe owns can't be removed on their own (`ConstraintOwned`).
* **Mirror is solved.** Each copied point gets a `Symmetric` constraint to its source about the axis, each circle an `Equal` radius, exactly as Fusion does (§6.2.1). No new solver constraint types.
* **Patterns are not solved** (#168/#169): their instance points stay out of the solver and are *written after each solve* from the sources' solved positions, in the same undo transaction. Regenerating is derived state, like solver output (§10).
* **Commands** (all through `Command`/`Sketcher`, one undo step each): `AddRecipe`, `ExplodeRecipe` (drop the recipe, keep instances and owned constraints as ordinary geometry), `RemoveRecipe` (also deletes instances and owned constraints, refused while anything else refers to them). Moving construction on a source toggles it on the instances.
* **ID policy (ADR 0003).** Instance IDs come from the document's normal allocator when first created. The recipe holds them in `pool` (every ID ever allocated to it, including copies that don't currently exist), so a count decrease followed by an increase, or an undo, re-creates entities under *the recipe's own* IDs. An ID is never given to anything else and `next_id` never decreases.
* **Limits.** At most `MAX_INSTANCES` = 10 000 copies per recipe and `MAX_INSTANCE_ENTITIES` = 200 000 generated entities; exceeding either is `DocError::InstanceLimit` (surfaced as the §6.2.1 warning). Files are validated against both (no panics on hostile input).

## Known limitation (recorded in §6.2.1) — lifted for patterns by [ADR 0015](0015-two-phase-solve-for-pattern-instances.md)

§6.2.1 says instances "can be referenced by other constraints (e.g. dimension to an instance)". For **pattern** (and offset) instances v1 refuses *every* constraint, driven (reference) dimensions included, with a `DocError` that says why, because the instances are not solver variables and a measured value would go stale (measuring them needs the same two-phase solve). Mirror instances are solver entities and accept any constraint the solver can satisfy. Lifting this (a two-phase solve with the instances fixed) is tracked as an M4 follow-up.

## Consequences for the offsets session

* Add `RecipeKind::Offset { .. }` and handle it in the match arms in `recipe.rs` (`solved`, `instance_count`, `references`, `name`) and in `add_recipe` / `transform` / `copy_geom`.
* An offset's output geometry is variable (the number of arcs and lines can change), so it fits the *non-solved* path: regenerate the instance entities after each solve; IDs for entities that stop existing stay in the recipe's `pool` per the ID policy.
* Everything else (patches for undo, Explode, Remove, delete guards, file round-trip and validation) is reused as is.

## Deferred

Mirror's "merge geometry that lands on the mirror line" option, and per-instance suppress, are not in the first cut.

## Patterns as built (#168, #169)

`RecipeKind::Linear { dir1, dir2 }` and `Circular { centre, count, angle, symmetric, rotate }`; counts, spacings and angles are expressions over the document parameters. `crate::pattern::regenerate` runs after every command and solve (same hook as offsets): it re-evaluates the expressions, creates or removes copies to match the count (IDs from the recipe's pool), and writes positions from the sources. `SetInstanceSuppressed` removes a copy's entities but keeps its IDs. No format bump: new `RecipeKind` variants and an optional `suppressed` field are additive.
