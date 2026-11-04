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

Planned in M0 (spec §12): GUI framework (eframe vs Bevy), canvas renderer, solver backend (pure Rust), entity ID scheme, RON vs JSON, plugin licence exception wording, MCP SDK and spec revision.
