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
| [0003](0003-entity-ids.md) | Entity ID scheme — document-scoped monotonic `u64` | Accepted | §4, §12.4 |
| [0004](0004-file-format.md) | Native `.locus` format — RON | Accepted | §10, §12.5 |
| [0005](0005-mcp-sdk.md) | MCP SDK — `rmcp` 3.8, MCP revision 2026-07-28 | Accepted | §16.2, §12.8 |
| [0006](0006-plugin-licence-exception.md) | Plugin licence exception — draft wording (legal review pending) | Accepted (draft) | §15.6, §12.7 |

Still planned in M0 (spec §12): GUI framework (eframe vs Bevy) and canvas renderer (ADR 0001), solver backend (ADR 0002).
