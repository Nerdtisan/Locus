# Spikes

Throwaway M0 experiments (SPEC §12, §13). Each spike is a **standalone Cargo
workspace** (its own `[workspace]` table and `Cargo.lock`), so it stays out of
the root workspace and its CI gate. A spike ends in an ADR in `docs/adr/` with
measured results; its code is not product code and is not maintained after
the decision.

Build a spike from its own folder in the dev container, e.g.:

```powershell
docker run --rm -e CARGO_HOME=/workspace/.cargo-home -v "${PWD}:/workspace" `
  -w /workspace/spikes/<spike> locus-dev cargo build --release
```

| Folder | Issue | What |
|---|---|---|
| ~~`gui-eframe/`~~ | #44 | GUI spike A (eframe). Chosen ([ADR 0001](../docs/adr/0001-gui-framework.md)); its view maths, grid and rulers now live in `crates/locus-app`, and the folder was removed in #67. Results are in the ADR; the code is in git history. |
| ~~`solver/`~~ | #39–#42, #43 | Solver spike: harness, SolidCraft, ezpz and the own sparse LM ([ADR 0002](../docs/adr/0002-solver-backend.md), [ADR 0007](../docs/adr/0007-solver-backend-without-paste.md)). Absorbed into `locus-solver` (its tests and `cargo bench -p locus-solver`); removed from `dev`, kept at git tag `archive/spike-solver`. |
| ~~`gui-bevy/`~~ | #45 | GUI spike B (Bevy). Not chosen ([ADR 0001](../docs/adr/0001-gui-framework.md)); removed from `dev`, kept at git tag `archive/spike-gui-bevy`. |
| `booleans/` | #120 | Booleans that keep arcs: `i_overlay` + refit vs `cavalier_contours` vs a Locus-owned arrangement ([ADR 0008](../docs/adr/0008-booleans-and-arcs.md)). Run `cargo run --release -- 5000`; `-- case <sweep> <n>` dumps one random case. To be absorbed into `locus-region`'s tests. |
| `offsets/` | #161 | Offsets that keep arcs: `locus_region::offset` (the product engine) vs `cavalier_contours`, scored by an exact distance oracle ([ADR 0010](../docs/adr/0010-offsets.md)). Run `cargo run --release -- 2000`; `-- case <sweep> <n>` dumps one random case. Its sweep is also a proptest in `crates/locus-region/tests/offsets.rs`. |
| `web-plugins/` | #252 | Browser plugin runtime: wasmtime + Pulley compiled to wasm (doesn't run) vs the component's core module in a Web Worker (chosen) ([ADR 0016](../docs/adr/0016-web-plugin-runtime.md)). Its README says how to rebuild and serve the harness. |
| `solidcore/` | #153 | Footprint check of the `solidcore` 3D B-rep kernel (git dep, pinned): native + wasm32 build, `cargo deny`. It has no constraint solver, so no solver comparison ([ADR 0018](../docs/adr/0018-solidcore-kernel.md)). `cargo build --release`. |
