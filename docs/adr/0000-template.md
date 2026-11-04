# ADR-NNNN: <Short decision title>

- **Status:** Proposed | Accepted | Superseded by ADR-NNNN | Rejected
- **Date:** YYYY-MM-DD
- **Spec:** `docs/SPEC.md §n` (the section and §12 item this ADR closes)
- **Issue:** Nerdtisan/Locus#NN

## Context

What problem or open decision is this? What forces are in play (spec requirements, platform targets incl. web/`wasm32`, licence policy §2, performance budgets)? Keep it to what a reader needs to judge the options.

## Options

### Option A — <name>

Short description. Pros / cons.

### Option B — <name>

Short description. Pros / cons.

## Measurements

What was spiked and how it was measured, so it can be reproduced: commit or branch, hardware, build profile, target (native / `wasm32`), test fixture.

| Criterion | Option A | Option B |
|---|---|---|
| e.g. drag re-solve, 500 entities (ms) | | |
| e.g. `wasm32` bundle size (MB) | | |
| e.g. licence (`cargo deny`) | | |

Note anything you could not measure and why.

## Decision

The chosen option, in one or two sentences, and the main reason.

## Consequences

What changes as a result: crates and dependencies added, follow-up issues, risks accepted, what would make us revisit this decision. If the result conflicts with the spec, say so and update the spec in the same PR.
