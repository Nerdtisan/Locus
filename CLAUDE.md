# CLAUDE.md

This file defines how Claude should behave in this project. Follow these guidelines consistently across all sessions.

---

## Project Context

- **Project Name** — Locus
- **Purpose** — A Fusion-360-style **constrained 2D vector editor** for pen plotters, vinyl cutters and laser cutters. Geometry is driven by a geometric constraint solver, layers map to machine operations (cut / score / engrave / draw), and output is clean SVG, PDF and DXF.
- **Tech Stack** — Rust (edition 2021) Cargo workspace of `locus-*` crates (spec §3). GUI is **chosen by ADR after the M0 spike** (§12.1): (A) `eframe` 0.35 + `egui_dock` + `wgpu`, or (B) Bevy 0.17 + `bevy_egui` + `egui_dock`. Pure-Rust constraint solver (§3). `wasmtime` plugin host (§15) and `rmcp` MCP server (§16), both desktop-only at first.
- **Architecture** — **UI-agnostic core crates** (`locus-geom`, `locus-solver`, `locus-doc`, `locus-region`, `locus-export`, `locus-text`) with no GUI dependencies, which **must build for `wasm32-unknown-unknown`**. Every document mutation goes through the serialisable command API in `locus-doc` (§11); the GUI, CLI, plugins and MCP server are all clients of it. `locus-app` is the GUI shell (desktop + web); `locus-cli` builds the headless `locus` binary.
- **Target Environment** — Windows, macOS, Linux and **web** (full editor on GitHub Pages; browser open/download + OPFS/IndexedDB autosave). The Rust toolchain lives **only in the Docker dev container** — never installed on the host. macOS is built and tested on a native CI runner only.
- **The spec is the contract** — [`docs/SPEC.md`](docs/SPEC.md). Read it before non-trivial work and **cite section numbers** (e.g. `§8`, `§15.4`) in code comments, commits, PRs and ADRs. Decisions taken so far are in §0; open items and their ADRs are in §12.

**Current state:** pre-code. The repo holds the spec, this file, the ADR template and the licence. Next is **M0 — spikes & scaffolding** (§13). Tasks are tracked as GitHub issues on `Nerdtisan/Locus`.

---

## Core Principles

- **Simplicity First** — Make every change as simple as possible. Minimise code impact.
- **Security First** — All code must be secure. No exceptions.
- **No Laziness** — Find root causes. No temporary fixes. Senior developer standards only.
- **Minimal Impact** — Touch only what's necessary. Avoid introducing regressions.
- **Spec is the contract** — `docs/SPEC.md` governs. When in doubt, follow it and cite the section. If a request or a spike result conflicts with it, **flag the conflict** — don't silently resolve it (§17). Never renumber spec sections; issues cite them.
- **Command API only** — Every document mutation goes through the serialisable command API in `locus-doc` (§11). Never mutate the document from UI, plugin-host or MCP code directly. One user action = one undoable command.
- **Core stays UI-agnostic and wasm32-clean** — Core crates have no GUI dependencies and no filesystem / thread / clock assumptions, and must build for `wasm32-unknown-unknown`. Platform I/O lives in the app shell or behind traits.
- **Milestone discipline** — Work only on the current milestone (§13). Don't build features beyond it. Every spike ends in an ADR with measured results before a choice is committed (§12, `docs/adr/`).

---

## Model Strategy & Task Delegation

- **Architectural Planning:** Use `opus` capabilities to analyze system requirements, map out multi-file changes, and construct detailed step-by-step implementation specs.
- **Code Implementation:** Delegate file modifications, routine refactoring, and main feature implementation to `sonnet` agents.
- **Boilerplate & Testing:** Utilize `haiku` subagents for lightweight tasks such as generating standard unit tests, routine linting, or simple file reading.
- **Guardrails:** Do not allow `sonnet` or `haiku` execution steps to make macro-architectural decisions independently. If an implementation step breaks the core plan, escalate back to `opus` for plan revision.

---

## Branching & Git Workflow

The repository uses a three-branch model:

- **`main`** — production branch. Holds released, production-ready code.
- **`staging`** — an isolated pre-production branch for testing a release candidate before it is merged into `main`.
- **`dev`** — the primary integration branch. It is the source for automated tests and deployment to the development/staging environment. **All feature and bug-fix branches are taken off `dev`.**

To work on a new feature or bug fix, create a descriptively named branch off `dev` — e.g. `feature/solver-spike`, `bugfix/arc-tangent`, `docs/adr-gui` — and open the PR back into `dev`. Reference the GitHub issue it closes.

### Never commit directly to `dev` or `main`

This is a firm rule, not a default:

- **Branch before the first edit**, not after. If you have already started editing on `dev`, move the work to a branch before committing.
- **Follow-up fixes get their own branch and PR too.** A fix that surfaces while verifying merged work is not exempt, however small.
- **Merging a feature branch into `dev` locally is not a substitute for a PR.** Push the branch and raise one.
- **Only push `dev` directly when explicitly asked to.** Otherwise `dev` advances through merged PRs.

Applies to docs and config as much as to code — this file included.

---

## Environment Setup

> **Status:** there is no code yet. The commands below describe the loop **once M0 scaffolding lands** (workspace, `.devcontainer/`, CI). Adjust image and crate names here if scaffolding settles on different ones.

### The toolchain is containerised (firm rule)

There is **no host Rust toolchain**. All compiling, linting and testing happen inside the dev container built from `.devcontainer/Dockerfile`. The container covers Linux build/test, the Windows cross-compile (MinGW + `x86_64-pc-windows-gnu`) and the web build (`wasm32-unknown-unknown` + `trunk`). **macOS is built in CI only**, on a native macOS runner — it cannot be built locally.

> **GUI-verification limits:** the Linux container cannot *run* the GUI (no display or GPU). A container build proves the code compiles and the tests pass — not that the GUI works. Don't claim GUI behaviour works from a container build alone.

> **Running the GUI to verify (the host is Windows):** cross-compile the Windows `.exe` in the container, then launch that binary on the host. Use this loop instead of saying "I can't run the GUI from here":
>
> ```powershell
> docker run --rm -e CARGO_HOME=/workspace/.cargo-home -v "${PWD}:/workspace" -w /workspace `
>   locus-dev cargo build --release --target x86_64-pc-windows-gnu -p locus-app
> Start-Process .\target\x86_64-pc-windows-gnu\release\locus.exe
> ```
>
> The `.exe` lands in the bind-mounted `target/`. Stop it with `Stop-Process -Name locus`. For a visual check, the `computer-use` MCP can screenshot and drive the window. For the **web build**, run `trunk serve` in the container with the port published and open it in the built-in browser.

### Prerequisites

- Docker (Linux containers). That's it for the build/test loop.

### Build / verify loop (run from the repo root, once M0 scaffolding lands)

```powershell
# Build the dev image once (rebuild only when the Dockerfile changes)
docker build -t locus-dev .devcontainer

# Shorthand used below: $d = docker run --rm -e CARGO_HOME=/workspace/.cargo-home -v "${PWD}:/workspace" -w /workspace locus-dev
$d cargo fmt --all --check
$d cargo clippy --workspace --all-targets -- -D warnings
$d cargo test --workspace
$d cargo deny check
# Core crates must build for the browser (§3)
$d cargo check --target wasm32-unknown-unknown -p locus-geom -p locus-solver -p locus-doc -p locus-region -p locus-export -p locus-text
# Web build of the app shell
$d trunk build --release
# Windows cross-build
$d cargo build --release --target x86_64-pc-windows-gnu
```

(`$d` is notation, not a real PowerShell alias — spell the full `docker run …` prefix out.) `CARGO_HOME=/workspace/.cargo-home` keeps the registry cache inside the git-ignored workspace so it persists across runs. `target/` is git-ignored too.

---

## Code Style Conventions

- **Formatting & linting** — `cargo fmt` and `cargo clippy -- -D warnings`. Both must pass clean; CI runs the same.
- **Naming** — Rust standard: `snake_case` items/modules/files, `PascalCase` types/traits, `SCREAMING_SNAKE_CASE` consts. Crates are `locus-*`; the WIT package is `locus:plugin@1.0.0`; native files are `.locus`, plugin archives `.locusplugin`.
- **Errors** — `thiserror` in library crates, `anyhow` in application crates (`locus-app`, `locus-cli`). **No `unwrap()`/`expect()` in I/O, parsing (file format, expressions, DXF/SVG import) or solver paths** — return structured errors. A bad file, a failed save or a non-converging solve must never crash the app; surface it to the user (and, for MCP, as structured data — §16.3).
- **Units & tolerances** — Internal unit is millimetres, `f64` (§4). Use the named tolerance constants, never ad-hoc epsilons.
- **Determinism** — Serialisation is stable-ordered (§10); plugin output is deterministic for identical inputs (§15.5).
- **Comments** — Explain *why*, not *what*. Reference spec sections where a decision traces to the spec. Delete commented-out code before committing.
- **Logging** — `tracing`. **Never log document contents**, file names/paths, MCP tokens, or plugin payloads.

---

## Testing

### Running tests

```powershell
docker run --rm -e CARGO_HOME=/workspace/.cargo-home -v "${PWD}:/workspace" -w /workspace locus-dev cargo test --workspace
```

### Expectations (spec §14, §15.8, §16.5)

- **Golden-file tests** — `.locus` fixture → export → compare against committed SVG/DXF/PDF output (normalised).
- **Solver property tests** (`proptest`) — random valid constraint sets converge; adding then removing a constraint returns to the same DOF. Drag re-solve budget: < 16 ms for 500 entities (§3).
- **Round-trip** — save → load → save is byte-identical.
- **Migrations** — a test for every file-format version.
- **Validation** — fixtures with open paths, duplicates, self-intersections (§8).
- **Plugin host** (§15.8) — fixture plugins: well-behaved, panicking, infinite-loop, memory-hog, wrong-API-version, malformed output. Limits abort cleanly; document state unchanged. Golden tests for first-party plugins.
- **MCP** (§16.5) — integration tests driving `locus mcp` over stdio → golden documents. Security tests: non-loopback bind rejected, missing/invalid token rejected, bad `Origin` rejected, scope and approval-mode enforcement.
- **Manual acceptance** each milestone — import exports into LightBurn, Cricut Design Space and a DXF viewer.
- **New features ship with tests. Bug fixes ship with a regression test.**

---

## Definition of Done

A task is not complete until all of the following apply:

| Task Type | Done Criteria |
|-----------|--------------|
| **New Feature** | Tests written; `fmt --check`, `clippy -D warnings`, `test`, `cargo deny check` and the **`wasm32` check of core crates** all green in the dev container (and CI, incl. macOS); spec/README references updated; issue and milestone noted |
| **Bug Fix** | Root cause identified; regression test added; gate green |
| **Refactor** | Behaviour unchanged (existing tests pass); core stays UI-agnostic and wasm32-clean; gate green |
| **Spike** | An ADR in `docs/adr/` with options, **measured results** and a decision (or an explicit "inconclusive, next step"); §12 updated to point at it |
| **Dependency change** | Licence passes `cargo deny check` (§2); still builds for `wasm32` if used by a core crate; `Cargo.lock` committed; changelog reviewed for upgrades |
| **Spec change** | Section numbers unchanged; §0 decisions log updated if a decision changed |

Don't report the GUI as working off a container build alone — cross-compile the Windows `.exe` and run it on the host, or check the web build in a browser (see Environment Setup).

---

## Off-Limits Without Explicit Instruction

Never do the following without being explicitly asked:

- **Add a GPLv2-only or proprietary dependency** (§2). `cargo-deny` must stay green; don't weaken its config to make it pass.
- **Bind the MCP HTTP transport to anything but loopback** (`127.0.0.1`), or skip the bearer token / `Origin` check (§16.4).
- **Give plugins filesystem, network, clock, env or random access**, or let them call MCP (§15.4, §16.4).
- **Mutate the document outside the command API** (§11).
- **Add a GUI dependency to a core crate**, or anything that breaks its `wasm32` build.
- **Change or delete files in `.github/workflows` or `.devcontainer/`.**
- **Upgrade major dependency versions** (minor and patch are fine).
- **Renumber `docs/SPEC.md` sections.**
- **Reformat or churn files unrelated to the current task.**
- **Push directly to `main` or `staging`** (or to `dev` — see Branching).

If any of these seem necessary to complete a task, **stop and flag it** before proceeding.

---

## Communication & Ambiguity

- If a task is ambiguous, ask **one clarifying question** — the most important one — and wait. Do not ask multiple questions at once.
- If proceeding under an assumption, state it explicitly: *"I'm assuming X — let me know if that's wrong."*
- For large tasks, confirm the plan before writing any code.
- Keep status updates concise: what was done, what's next, any blockers. No padding.
- If a constraint in this file conflicts with a user instruction, flag the conflict and defer to the user.

---

## Workflow Orchestration

### Plan Mode

Enter plan mode for **any non-trivial task** (3+ steps, or involving architectural decisions).

- If something goes sideways, **stop and re-plan immediately** — don't keep pushing.
- Use plan mode for verification steps, not just building.
- Write detailed specs upfront to reduce ambiguity.

### Subagent Strategy

- Use subagents liberally to keep the main context window clean.
- Offload research, exploration, and parallel analysis to subagents.
- For complex problems, throw more compute at it via subagents.
- One task per subagent for focused execution.

### Self-Improvement Loop

After **any correction** from the user:

1. Update `tasks/lessons.md` with the pattern.
2. Write rules for yourself that prevent the same mistake.
3. Ruthlessly iterate on these lessons until the mistake rate drops.
4. Review lessons at the start of each session for relevance to the project.

### Verification Before Done

- Never mark a task complete without proving it works.
- Diff behaviour between `dev` and your changes when relevant.
- Ask yourself: *"Would a staff engineer approve this?"*
- Run tests, check logs, demonstrate correctness.

### Demand Elegance (Balanced)

- For non-trivial changes, pause and ask: *"Is there a more elegant way?"*
- If a fix feels hacky, apply this: *"Knowing everything I know now, implement the elegant solution."*
- Skip this for simple, obvious fixes — don't over-engineer.
- Challenge your own work before presenting it.

### Autonomous Bug Fixing

- When given a bug report: **just fix it**. No hand-holding required.
- Point at logs, errors, and failing tests — then resolve them.
- Zero context switching required from the user.
- Fix failing CI tests without being asked how.

---

## Task Management

**GitHub issues on `Nerdtisan/Locus` are the task tracker** (not a `tasks/todo.md` file). Issues cite `docs/SPEC.md §n` and belong to a milestone (M0–M4).

| Step | Action |
|------|--------|
| **Plan First** | Find or create the issue; write the plan as a checklist in the issue or PR description |
| **Verify Plan** | Check in before starting implementation |
| **Track Progress** | Tick checklist items as you go; link the branch/PR to the issue |
| **Explain Changes** | Provide a high-level summary at each step |
| **Document Results** | Summarise outcome in the PR; record decisions in an ADR where they apply |
| **Capture Lessons** | Update `tasks/lessons.md` after corrections |

---

## Secure Coding Guidelines

- **Secrets** — Never commit secrets. No credentials in the repo, the dev container (`ARG`/`ENV`) or CI config.
- **MCP server** (§16.4, non-negotiable) — off by default and explicitly enabled; status indicator always visible; HTTP binds **127.0.0.1 only**; per-session bearer token; validate `Origin` against DNS rebinding; read / write / export scopes granted separately, default read-only; user-selected approval modes, export always asks unless allowed; AI edits grouped into labelled undo transactions; rate limits and payload caps on write tools; no filesystem access beyond host-mediated, user-confirmed import/export.
- **Plugin sandbox** (§15.4) — capability-based, deny by default; the host does all file dialogs and I/O; per-call fuel/epoch CPU limit, memory cap (default 256 MB) and timeout; traps are contained and leave the document unchanged; plugins cannot see MCP sessions.
- **Untrusted input** — `.locus` files, imported SVG/DXF and plugin output are untrusted: validate, bound sizes and counts (e.g. the 10 000-instance pattern limit, §6.2.1), never panic on malformed data.
- **No telemetry** — the app never sends document content anywhere (§16.4).
- **Logging** — never log document contents, file names, tokens or plugin payloads.
- **Atomic saves** — temp file + rename on desktop; never destroy the existing file on a failed write. On the web, never drop the autosave copy until the new one is committed.
- **Dependencies** — `cargo deny check` (licences + advisories) in CI from day one (§2).
