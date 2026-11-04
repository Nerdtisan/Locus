# Solver spike (M0)

Benchmark harness and candidate backends for the solver decision: issues #41 (harness), #39 (SolidCraft), #40 (survey), #42 (rank-analysis prototype). Results and the decision are in [ADR 0002](../../docs/adr/0002-solver-backend.md). Spec: `docs/SPEC.md` §3, §5, §13.

This is a standalone Cargo workspace, kept apart from the root one so candidate dependencies (a git dependency on SolidCraft, ezpz, criterion) stay out of the core crates and the root `Cargo.lock`. CI doesn't build it.

| Path | What it is |
|---|---|
| `harness/` | `solver-harness`: no dependencies. The harness sketch model (§4 v1 curves, every §5 v1 constraint), an independent residual checker, the fixtures and the measurement suite behind a `Backend` trait. Written to move into `locus-solver` in M1. |
| `spike/` | `solver-spike`: adapters for SolidCraft (`solidcraft.rs`) and ezpz (`ezpz_backend.rs`), the Jacobian-rank DOF/redundancy prototype (`analysis.rs`), the native table runner, the criterion bench and the wasm entry points. |
| `web/index.html` | Loads the wasm build in a browser and prints the same table. |

## Fixtures

- **fillet-rect**: the §13 M0 sketch. A 60 × 40 mm rectangle with 5 mm filleted corners (lines tangent to arcs, sharing end points), a circle concentric to one fillet, and a distance dimension. Fully constrained (DOF 0, counted by hand in `fixtures.rs`). Variants: **-under** (no fillet radius, DOF 1), **-redundant** (adds a parallel implied by two horizontals), **-conflict** (adds a horizontal distance that fights the distance dimension).
- **generated**: 72 cells in one connected chain, 504 curves + 720 points, mixing 20 constraint kinds. DOF 37 by hand. Two drags of 30 frames each along feasible paths. **drag-chain** slides the middle cell's free gap, which moves half the sketch. **drag-local** turns a free spoke around its circle. **generated-conflict** adds a gap dimension that fights an existing one.
- **coverage**: one tiny sketch per §5 v1 constraint and curve pairing (27 cases), each starting at least 1 mm off.

Solves are judged by `harness/src/check.rs`, never by the backend's own "solved" flag: every constraint and every arc's implicit equal-radius condition must hold within 1e-6.

## Running (in the dev container, from the repo root)

```powershell
# Unit tests (harness fixtures, rank analysis)
docker run --rm -e CARGO_HOME=/workspace/.cargo-home -v "${PWD}:/workspace" -w /workspace/spikes/solver locus-dev cargo test --release
# The comparison table (optional argument: cells; 72 = 504 curves)
docker run --rm -e CARGO_HOME=/workspace/.cargo-home -v "${PWD}:/workspace" -w /workspace/spikes/solver locus-dev cargo run --release --bin table
# The table, then criterion timings of full solve and one drag frame
docker run --rm -e CARGO_HOME=/workspace/.cargo-home -v "${PWD}:/workspace" -w /workspace/spikes/solver locus-dev cargo bench
```

`cargo bench` runs inside this workspace. The issue #41 wording (`cargo bench -p locus-solver`) applies once the harness moves into `locus-solver` in M1.

### wasm

```powershell
# Build (stripped, no debug info) and copy next to the page
docker run --rm -e CARGO_HOME=/workspace/.cargo-home -e CARGO_PROFILE_RELEASE_DEBUG=false -e CARGO_PROFILE_RELEASE_STRIP=true -v "${PWD}:/workspace" -w /workspace/spikes/solver locus-dev bash -c "cargo build --release --target wasm32-unknown-unknown -p solver-spike --lib && cp target/wasm32-unknown-unknown/release/solver_spike.wasm web/solver_spike_all.wasm"
# Serve and open http://127.0.0.1:8765/index.html?wasm=solver_spike_all.wasm
python -m http.server 8765 --bind 127.0.0.1 --directory spikes/solver/web
```

The module imports only `env.now_ms` (the page passes `performance.now()`), with no wasm-bindgen. Without cross-origin isolation, browsers round `performance.now()` to 0.1 ms, so sub-millisecond wasm figures are coarse. Each backend can be built alone (`--no-default-features --features solidcraft` or `ezpz`) to measure its share of the wasm size.
