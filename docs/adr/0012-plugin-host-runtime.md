# ADR-0012: Plugin host runtime: wasmtime 50, empty linker, fresh instance per call

- **Status:** Proposed
- **Date:** 2026-11-11
- **Spec:** `docs/SPEC.md §15.1`, `§15.4`, `§15.5`, `§15.8`; builds on ADR 0009
- **Issue:** Nerdtisan/Locus#181 (epic #23), with #186

## Context

ADR 0009 fixed the interface (`locus:plugin@1.0.0`, one world per kind, nothing imported). #181 has to run those components on desktop under the §15.4 rules: deny by default, a CPU limit, a 256 MB memory cap and a timeout per call, contained traps, and validated output. Plugins are desktop-only at first (§0), so the web build must not link the runtime. Every dependency must pass `cargo deny` with `deny.toml` unchanged (§2).

The choices left open were: which wasmtime, how to enforce "nothing imported", how to bound CPU deterministically, whether to keep instances alive between calls, and how to build fixture plugins for the §15.8 suite.

## Options

### Instance lifetime

- **One long-lived instance per plugin:** cheaper calls, but state leaks between calls (a plugin could behave differently the second time), and one trap poisons the instance.
- **A fresh instance per call** (chosen): the component is compiled once at load; each call instantiates from an `InstancePre` into a new `Store`. No state survives, so output depends only on the inputs (§15.5), and a trap can't affect the next call.

### CPU limit

- **Epoch interruption only:** wall-clock, so the point where a heavy plugin fails depends on the machine.
- **Fuel only:** deterministic, but a pathological plugin could still hold the UI longer than §15.4's timeout on a slow machine.
- **Both** (chosen): fuel is the deterministic budget; an epoch deadline (10 ms ticks from one background thread) enforces the wall-clock timeout. Whichever runs out first aborts the call.

### Fixture plugins (§15.8)

- **Rust guests built for `wasm32` during the test run:** realistic, but needs the `wasm32-unknown-unknown` target, which the macOS CI job doesn't install, and can't produce truly hostile output (bad pointers, invalid UTF-8).
- **Prebuilt `.wasm` files committed to the repo:** opaque binaries in review.
- **Hand-written core WAT, wrapped into components in-process with `wit-component`** (chosen): each fixture lays out its return values from Rust, so it can return exactly the bytes a hostile plugin would. Runs on every CI platform with no wasm toolchain. The first-party example plugins (#185) are the realistic Rust guests.

## Measurements

Linux dev container (`locus-dev`, Rust 1.99.1), debug build, `cargo test -p locus-plugin-host`.

| Criterion | Result |
|---|---|
| wasmtime version | **50.0.1** (current major; 51 is still a release candidate) |
| Features | `std`, `runtime`, `cranelift`, `component-model`, `wat` (no WASI, no async, no cache) |
| `cargo deny check` | advisories, bans, licences, sources **ok**, `deny.toml` unchanged; only duplicate-version warnings (`thiserror` 1 and 2) |
| New crates in `Cargo.lock` | 65 (wasmtime, cranelift, wasm-tools crates); all Apache-2.0 WITH LLVM-exception, MIT or BSD |
| Fixture suite (24 tests, incl. 200 ms timeout case) | 0.24 s total |
| Infinite loop, 200 ms timeout | aborted with `Timeout`, well inside the 5 s test bound |
| Infinite loop, 200 M fuel | aborted with `CpuLimit` |

## Decision

- **wasmtime 50.0.1**, as a `cfg(not(target_arch = "wasm32"))` dependency of `locus-plugin-host`. The plain types and validation in that crate build everywhere; the runtime does not exist on `wasm32`, so the web build never links it.
- **Empty linker.** A component is rejected at load if it imports anything that grants access: functions, resources, modules or components. Interfaces imported only for their types (`types`, `params`) carry nothing and are allowed. This is the same rule as `locus-plugin-api/tests/wit.rs`.
- **API version from the export names** (`locus:plugin/<iface>@<version>`): a major outside `SUPPORTED_API_MAJORS` (today `[1]`, later current and previous, §15.3) is refused before anything runs.
- **Limits per call:** fuel 20 G units, memory 256 MB summed over all of an instance's memories, timeout 5 s (§15.4 defaults); limits on instance, table and memory counts too. `Plugin::set_limits` changes them per plugin.
- **Determinism (§15.5):** NaN canonicalisation and deterministic relaxed SIMD are on.
- **Errors:** every failure is a `LoadError` or `CallError` (`CpuLimit`, `Timeout`, `MemoryLimit`, `Trap`, `Malformed`, `Plugin`, …). Nothing panics, and the host never receives the document (§15.3), so a failed call can't change it.
- **Output validation** (`validate.rs`): at most 100 000 points, entities, constraints, paths or segments per call; coordinates finite and within ±1 km; indices must resolve; strings ≤ 4 KiB with no control characters; plugin-reported messages are truncated and stripped rather than shown raw.
- **Installed plugins are binaries:** `Component::from_binary`, never WAT, and at most 64 MB.

## Consequences

- The §15.8 suite lives in `crates/locus-plugin-host/tests/sandbox.rs` with the builder in `tests/common/`. New hostile cases are a few lines of data.
- Guest plugins (#185) are built for `wasm32-unknown-unknown` and wrapped into components with `wit-component`, not built for `wasm32-wasip2`: WASI targets import WASI interfaces, which this host refuses by design.
- wasmtime ships a new major roughly monthly. Upgrading is a major-version bump (CLAUDE.md: ask first), and must keep this suite green.
- **What would change this:** a plugin that needs a grantable capability (ADR 0009), the browser plugin runtime (#31), or a measured need to keep instances warm between calls.
