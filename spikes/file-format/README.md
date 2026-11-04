# File-format spike (RON vs JSON)

Throwaway spike behind [ADR-0004](../../docs/adr/0004-file-format.md) (SPEC §10) and the ID-size figures in [ADR-0003](../../docs/adr/0003-entity-ids.md) (SPEC §4). It is a standalone Cargo workspace (its own `[workspace]` table), so it is not part of the main workspace, its lockfile or its CI gate. Nothing here ships.

- `src/lib.rs` — a cut-down document model (layers, points, lines, circle, constraints, a parameter, `BTreeMap`-ordered), generic over the ID type; a hand-built sample; a synthetic 510-entity grid; wasm size probes.
- `src/bin/spike.rs` — writes `samples/` and `samples/results.md` (determinism, f64 round-trip, file sizes).
- `samples/` — committed outputs: `sample.{ron,json}`, `sample-edited.{ron,json}` (width 60 → 65 mm, one point and one constraint added), `edit.{ron,json}.diff`, `sample-uuid.{ron,json}` (same document with UUIDv7 IDs; the UUIDs change every run).

## Reproduce

From the repo root, in the dev container (Rust 1.99.1):

```sh
docker run --rm -e CARGO_HOME=/workspace/.cargo-home -v "${PWD}:/workspace" \
  -w /workspace/spikes/file-format locus-dev \
  cargo run --release --features uuid --bin spike

# wasm32 size probes (release, opt-level "s", LTO, panic=abort, stripped)
docker run --rm -e CARGO_HOME=/workspace/.cargo-home -v "${PWD}:/workspace" \
  -w /workspace/spikes/file-format locus-dev bash -c '
  for f in "" "--features ron" "--features json" "--features uuid-js"; do
    cargo build -q --release --lib --target wasm32-unknown-unknown --no-default-features $f
    w=target/wasm32-unknown-unknown/release/file_format_spike.wasm
    echo "[$f] $(stat -c %s $w) bytes, $(gzip -9c $w | wc -c) gzipped"; rm $w
  done'

# Regenerate the edit diffs
git diff --no-index samples/sample.ron samples/sample-edited.ron > samples/edit.ron.diff
git diff --no-index samples/sample.json samples/sample-edited.json > samples/edit.json.diff
```

Crate versions resolved (see `Cargo.lock`): `ron` 0.12.3, `serde_json` 1.0.154, `serde` 1.0.229, `uuid` 1.29.0.
