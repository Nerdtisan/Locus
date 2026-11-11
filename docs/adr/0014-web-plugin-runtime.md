# ADR-0014: Web plugin runtime: the component's core module in a Web Worker, canonical ABI in Rust

- **Status:** Accepted
- **Date:** 2026-11-12
- **Spec:** `docs/SPEC.md §15.1`, `§15.4`, `§15.7`, `§15.8`, `§0`, `§13`; builds on ADR 0009 and ADR 0013
- **Issue:** Nerdtisan/Locus#252 (epic #31); implemented by #253, tested by #254

## Context

The owner moved the browser plugin runtime into M4 on 2026-11-11 (§0, §13; it was "after M4"). The web build must install and run the same `.locusplugin` files as the desktop, with the same safety (§15.4): nothing imported, a CPU/time limit, a 256 MB memory cap, contained traps, validated output, and a failed call leaving the document unchanged. The desktop host is wasmtime 50 (ADR 0013), which the web build does not link.

Every Locus plugin is a component made by `wit-component` from one `wit-bindgen` core module. The worlds import nothing (ADR 0009), so the component is that core module plus canonical-ABI adapters for a fixed set of interfaces.

## Options

### A. jco: transpile components to JS + core modules

- Transpiling has to happen at install time in the browser, so the app would ship jco's own transpiler (a large component itself) and then run **generated JavaScript** loaded from a `blob:` URL.
- The transpiled plugin still runs on the browser's engine, so its limits are exactly option C's; jco adds a JS toolchain and dynamic code generation without adding safety.
- **Not built or measured:** it needs Node and npm in the dev container and CI (a `.devcontainer/` and workflow change). Rejected on the points above.

### B. A component runtime compiled to wasm: wasmtime + Pulley

Probe: `spikes/web-plugins/probe-wasmtime/`.

- wasmtime 50 with `runtime`, `component-model`, `cranelift`, `pulley` builds for `wasm32-unknown-unknown` only with `custom-virtual-memory`, and then needs **nine embedder-supplied C-ABI functions** (`wasmtime_mmap_new`, `wasmtime_munmap`, `wasmtime_mprotect`, `wasmtime_page_size`, `wasmtime_memory_image_*`, `wasmtime_tls_get/set`). The probe supplies them with `unsafe extern "C"` shims over the allocator: `unsafe` code, which the workspace denies.
- It links with **no imports**, but compiling either example plugin **panics**: `wasmtime-internal-cranelift` turns on Cranelift's `timing` feature unconditionally, and that calls `std::time::Instant::now`, which `wasm32-unknown-unknown` doesn't support. Fixing it means patching wasmtime.
- **4.3 MB** (1.33 MB gzipped) on top of the 8.6 MB (3.4 MB gzipped) web app: +50 % download, and an interpreter far slower than the browser's JIT.
- Upside it would have had: exact desktop parity, including the deterministic fuel budget.

### C. Core-module adaptation (chosen)

- The host reads the component, checks its shape (one core module, no imports that grant anything) and takes out the core module.
- It caps the module's memory by rewriting its declared maximum to ≤ 256 MB (4 096 pages); the browser engine enforces `WebAssembly.Memory` maxima.
- The module runs in a **dedicated Web Worker**, instantiated with an **empty import object**: anything it imports fails to link, which is the "nothing imported" rule enforced by the engine.
- A call that runs past its timeout is ended by **terminating the worker**; a fresh worker serves the next call.
- The canonical ABI for the fixed `locus:plugin@1.x` interfaces (lower parameters through `cabi_realloc`, lift results, call `post-return`) is implemented once in Rust in `locus-plugin-host`, which already builds for `wasm32`. It is compiled into the worker.
- Results come back as the same host types the desktop returns and go through the **same validation** (`validate.rs`).

## Measurements

Built-in browser (Chromium) via the spike harness (`spikes/web-plugins/www`), Windows host. Release builds of the M3 example plugins (`finger-box` 52 KB, `hpgl` 51 KB components). `performance.now()` is coarsened to 0.1 ms here, so "< 0.1" means below the clock's resolution.

| Criterion | B: wasmtime + Pulley | C: core module in a worker |
|---|---|---|
| Builds for `wasm32-unknown-unknown` | Only with `unsafe` C-ABI platform shims | Yes, no new dependencies |
| Runs the example plugins | **No:** panics in Cranelift (`std::time`) | Yes, both |
| Added download | 4.28 MB (1.33 MB gzipped) | Worker glue: 3.3 KB JS in the spike (1.4 KB gzipped); the Rust ABI code is measured in #253 |
| Core module imports | — | 0 for both examples |
| Compile (`WebAssembly.compile`) | — | 0.3 ms (finger-box), 0.4 ms (hpgl) |
| Instantiate + `describe`, first call | — | 0.2 ms |
| Instantiate + `describe`, median of 20 | — | < 0.1 ms |
| Worker start → result, wall | — | 7 ms |
| Infinite loop, 300 ms timeout | — | Worker terminated at 308 ms; page unaffected |

In a hidden or background tab, browsers clamp main-thread timers to about 1 s, so termination can land later than the timeout there (seen: 920 ms for a 500 ms timeout with the pane hidden). It is still bounded, and the page stays responsive because the plugin runs off the main thread.

## Decision

**Option C.** The web host runs the component's core module in a dedicated Web Worker with an empty import object, a memory maximum rewritten to ≤ 256 MB and termination on timeout, and implements the canonical ABI of the fixed `locus:plugin@1.x` interfaces in Rust in `locus-plugin-host`.

Limits, desktop vs web:

| §15.4 limit | Desktop (ADR 0013) | Web (this ADR) |
|---|---|---|
| Nothing imported | Empty linker; component rejected at load | Component rejected at install; core module instantiated with `{}` |
| Wall-clock timeout (5 s) | Epoch interruption | Worker terminated |
| Memory cap (256 MB) | `ResourceLimiter`, summed over memories | One memory only; its maximum rewritten to ≤ 4 096 pages |
| Deterministic CPU budget | Fuel (20 G) | **None:** the browser engine has no fuel. The timeout bounds every call. |
| Traps contained | `CallError::Trap` | The worker reports the `RuntimeError`; the main thread is unaffected |
| Output validated | `validate.rs` | The same `validate.rs` |
| Document unchanged on failure | Yes | Yes (same command path) |

The one gap is fuel, which only makes the *failure point* deterministic; safety comes from the timeout, which both hosts have. Bringing fuel to the web would mean instrumenting the module (counting at loop heads and calls) and can be added later without changing the design.

## Consequences

- #253: a `wasm32` backend in `locus-plugin-host` behind the same `Plugin` calls (`generate`, `modify`, `export`, `import`, `validate`), plus a worker entry point built by trunk. Installing reads the `.locusplugin` from a file picker and stores it in OPFS, with the same manifest and component checks.
- The canonical-ABI code is checked on desktop: a parity test runs the same lift/lower over wasmtime's **core-module** API and compares it with the component path, on the example plugins and the §15.8 fixtures (#254 runs the fixtures in a headless browser too).
- Calls on the web are asynchronous: the UI shows "running…" and applies the result as one command when the worker answers (the same shape as #214 on desktop).
- Determinism (§15.5): the desktop canonicalises NaNs and makes relaxed SIMD deterministic; browsers do neither. NaN output is rejected by validation on both hosts, and the web host refuses core modules that use relaxed-SIMD instructions, so identical inputs still give identical accepted outputs.
- Components that `wit-component` didn't make in the usual shape (several core modules, adapter modules, imports between them) are refused on the web even where the desktop runs them. Every plugin built with the SDK has the usual shape.
- **What would change this:** wasmtime gaining a supported `wasm32-unknown-unknown` build (no `unsafe` shims, no `std::time`), at a size the web build can afford; or a plugin needing an import (ADR 0009), which the worker would then provide explicitly.
