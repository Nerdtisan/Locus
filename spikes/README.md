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
| `gui-bevy/` | #45 | GUI spike B: reference canvas on Bevy 0.17 + bevy_egui + egui_dock — see its `RESULTS.md` |
