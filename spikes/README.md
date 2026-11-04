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
| ~~`gui-bevy/`~~ | #45 | GUI spike B (Bevy). Not chosen ([ADR 0001](../docs/adr/0001-gui-framework.md)); removed from `dev`, kept at git tag `archive/spike-gui-bevy`. |
