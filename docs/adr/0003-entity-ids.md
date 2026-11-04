# ADR-0003: Entity ID scheme — document-scoped monotonic `u64`

- **Status:** Accepted
- **Date:** 2026-11-04
- **Spec:** `docs/SPEC.md §4` (Entities), closes §12 item 4
- **Issue:** Nerdtisan/Locus#47

## Context

Every entity has a stable `EntityId` that is never reused (§4). The ID is written into `.locus` files (§10), referenced by constraints, carried in serialised commands (§11), returned by plugins (§15.5) and quoted by AI clients over MCP (§16.3). The spec leaves two candidates open: UUIDv7 or a monotonic `u64`.

Forces:

- **Determinism.** Serialisation must be stable (§10): save → load → save byte-identical, golden-file tests (§14), and plugin output deterministic for identical inputs (§15.5).
- **Core crates are wasm32-clean with no clock or RNG assumptions** (§3, CLAUDE.md). `locus-doc` mints IDs.
- **Diffs and size** matter because the format is chosen for readable git diffs (§10).
- **MCP:** an AI client reads IDs back and sends them in later calls; long opaque strings cost tokens and are easy to mistype.
- **Merging:** two branches of one `.locus` file, or pasting between documents, must not produce two entities with one ID.

## Options

### Option A — UUIDv7 (128-bit, time-ordered, random tail)

Globally unique without coordination; sortable by creation time.

- Pro: no collisions across documents or branches; paste/import never needs remapping.
- Con: needs a clock and an RNG at the point of creation. On `wasm32-unknown-unknown` the `uuid` crate refuses to build without choosing a randomness source (`js`, `rng-getrandom`, `rng-rand`); with `js` it pulls in `wasm-bindgen`/`js-sys` and JS imports. That puts a platform dependency into a core crate.
- Con: non-deterministic. The same command script run twice produces different files, so golden tests must normalise IDs, and a generator plugin can't be deterministic unless the host remaps anyway.
- Con: 36-character strings in the file and in every MCP response.

### Option B — monotonic `u64`, scoped to the document

`EntityId(u64)` newtype; the document stores `next_id` and bumps it on every allocation. Deleted IDs are never reused because the counter only goes up and is saved in the file.

- Pro: deterministic — the same commands give the same IDs, so golden files and round-trips are byte-stable with no normalisation.
- Pro: no clock, no RNG, no extra dependency; trivially wasm32-clean.
- Pro: short in files, diffs and MCP payloads (`12` vs `"01a1a416-5ffa-7198-843f-7a2fccbeba92"`).
- Con: IDs are only unique within one document. Two branches that each add an entity both allocate the same next ID; pasting or importing from another document needs remapping.

### Option C (considered) — `(replica, counter)` pair

A Lamport-style pair would make concurrent edits mergeable. It solves a problem Locus does not have yet (no real-time collaboration in §1 goals) and is a superset of B, so B can grow into it later.

## Measurements

Spike: [`spikes/file-format/`](../../spikes/file-format/) (shared with ADR-0004), branch `docs/m0-adrs` off `dev` @ `63bea1e`, run in the `locus-dev` container (Rust 1.99.1). The same document is serialised with `u64` IDs and with UUIDv7 IDs; outputs in `spikes/file-format/samples/`.

| Criterion | UUIDv7 | `u64` |
|---|---|---|
| Sample (11 entities, 6 constraints) — RON bytes | 3 693 (+84 %) | 2 004 |
| Sample — JSON bytes | 4 368 (+61 %) | 2 717 |
| 510-entity grid — RON bytes | 125 947 (+78 %) | 70 739 |
| 510-entity grid — JSON bytes | 144 586 (+60 %) | 90 414 |
| Typical reference in a diff line | `layer: "01a1a416-5ff9-7774-97dd-40b88ba5988d",` | `layer: 1,` |
| Builds for `wasm32-unknown-unknown` with defaults | **No** — `compile_error!`: "specify a source of randomness" | Yes, no dependency |
| `wasm32` cost (release, `opt-level="s"`, LTO) | +22.4 KB raw / +7.7 KB gzipped over the baseline probe, plus `wasm-bindgen` JS imports (`uuid/js`) | 0 |
| Same commands → same file | No (IDs depend on time + RNG) | Yes |
| Licence (`cargo deny`) | MIT OR Apache-2.0 — fine | n/a |

Diff noise: with both schemes an edit only touches the lines it changes (the maps are `BTreeMap`-ordered, ADR-0004), so UUIDs don't add *more* changed lines — they make each changed line about 30 characters longer and unreadable to a human reviewer. UUIDv7's time ordering keeps new entities at the end of the map, the same as a counter.

Not measured: merge-conflict rates on real branches (there are no real `.locus` files yet); the cost argument below is qualitative.

## Decision

**Option B: `EntityId(u64)`, allocated from a document-wide counter `next_id` that is saved in the file and only ever increases.** It is the only option that keeps serialisation deterministic and the core crates free of clock/RNG dependencies, and it roughly halves the ID share of file size and MCP payloads. The one real cost — IDs are not globally unique — is handled by remapping at the few places documents meet.

Rules that come with it:

1. **One counter for all IDs in a document** (entities, constraints, layers, regions, plugin objects), each kind with its own newtype (`EntityId`, `ConstraintId`, `LayerId`, …) so they can't be mixed up in code. A shared counter means an ID number is unambiguous in logs, error messages and MCP replies.
2. **Never reused.** Deleting an entity does not lower `next_id`; undo of a create gives the same ID back because the command stores it (§11).
3. **IDs are minted only by `locus-doc`.** Plugins return geometry with *local* indices (0, 1, 2 …); the host maps them to fresh `EntityId`s when it turns the result into a command (§15.3). MCP write tools likewise return the IDs the host assigned.
4. **Paste, import and "insert document" remap** every incoming ID through the target document's counter, rewriting references.
5. **Upper bound:** the counter stays below 2^53 so IDs are exact in JSON numbers on JS-based MCP clients. A document would need nine quadrillion allocations to get there; `locus-doc` rejects files whose `next_id` exceeds it as malformed.
6. **Loading validates:** every ID in the file must be `< next_id`; duplicate or dangling references are a structured load error, not a panic (CLAUDE.md, untrusted input).

## Consequences

- `locus-doc` gets `EntityId(u64)` and siblings as `#[serde(transparent)]` newtypes, plus `next_id` in the document header (§10 top-level list gains `next_id`; no renumbering).
- No `uuid` dependency in core crates.
- **Merging two branches of one `.locus` file** that both added entities conflicts textually on the `next_id` line and possibly on the new entries. That is the honest outcome — the two new entities really do need distinct IDs — and git shows it rather than silently merging. A `locus-cli merge` helper that renumbers one side is a possible later tool; not in M0–M1.
- MCP (§16) and plugin results (§15.5) speak plain integers, which suits LLM clients and keeps payloads small.

**What would change this decision:** a requirement for real-time or offline multi-user collaboration on one document (CRDT-style merging). Then move to Option C, a `(replica, counter)` pair, which keeps determinism per replica; UUIDv7 would still lose on determinism and on wasm32.
