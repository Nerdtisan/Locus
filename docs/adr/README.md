# Architecture Decision Records

Decisions that close a `docs/SPEC.md §12` item, or any other choice that is costly to reverse, are recorded here. Every M0 spike ends in an ADR with **measured results** before a choice is committed (spec §13, §17).

## Conventions

- **File name:** `NNNN-short-kebab-title.md`, where `NNNN` is the next free four-digit number (`0001`, `0002`, …). Numbers are assigned when the ADR is written and are never reused.
- **Template:** copy [`0000-template.md`](0000-template.md). Sections: Context / Options / Measurements / Decision / Consequences.
- **Status:** `Proposed` while the PR is open, `Accepted` when merged. To change a decision, write a new ADR and mark the old one `Superseded by ADR-NNNN` — don't rewrite accepted ADRs.
- **Links:** each ADR cites the spec section it closes and the GitHub issue; update the matching §12 item to point back at the ADR in the same PR.

## Index

| ADR | Title | Status | Spec |
|---|---|---|---|
| [0000](0000-template.md) | Template | — | — |
| [0001](0001-gui-framework.md) | GUI framework: eframe + egui_dock + wgpu; egui painter canvas | Accepted | §12.1, §12.2 |
| [0002](0002-solver-backend.md) | Solver backend: ezpz, plus Locus's own DOF/redundancy analysis | Superseded by 0007 | §3, §12.3 |
| [0003](0003-entity-ids.md) | Entity ID scheme — document-scoped monotonic `u64` | Accepted | §4, §12.4 |
| [0004](0004-file-format.md) | Native `.locus` format — RON | Accepted | §10, §12.5 |
| [0005](0005-mcp-sdk.md) | MCP SDK — `rmcp` 3.8, MCP revision 2026-07-28 | Accepted | §16.2, §12.8 |
| [0006](0006-plugin-licence-exception.md) | Plugin licence exception — draft wording (legal review pending) | Accepted (draft) | §15.6, §12.7 |
| [0007](0007-solver-backend-without-paste.md) | Solver backend without `paste`: Locus-owned sparse LM + LDLᵀ | Accepted | §3, §12.3 |
| [0008](0008-booleans-and-arcs.md) | Booleans that keep true arcs: Locus-owned exact line/arc arrangement | Accepted | §6.3, §12.12 |
| [0009](0009-plugin-api.md) | Plugin API: WASM components, one world per kind, nothing imported; text stays in core | Accepted | §15, §9 |
| [0011](0011-associative-recipes.md) | Associative recipes: instances are real driven entities; mirror solved by Symmetric constraints, patterns regenerated after the solve | Accepted | §6.2.1 |
| [0010](0010-offsets.md) | Offsets that keep true arcs: Locus-owned, through the ADR 0008 arrangement | Accepted | §6.2, §7, §12.13 |
| [0012](0012-text-entity.md) | Text entity: container with cached geometry, fonts by reference, format v9 | Accepted | §9, §10 |

All §12 M0 decisions are recorded.
