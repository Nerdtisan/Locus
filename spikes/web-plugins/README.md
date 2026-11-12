# Web plugin runtime spike (#252, ADR 0016)

How to run `.locusplugin` components in the browser with the desktop sandbox's
limits. Two of the three options were built and measured; the decision and
the numbers are in [ADR 0016](../../docs/adr/0016-web-plugin-runtime.md).

| Folder / file | What |
|---|---|
| `probe-wasmtime/` | **Option B:** wasmtime 50 + the Pulley interpreter compiled to `wasm32-unknown-unknown`, with the "custom platform" C-ABI shims it needs. Builds and links with no imports (4.3 MB), but panics on the first compile: Cranelift's `timing` feature, which wasmtime turns on unconditionally, calls `std::time::Instant`, which doesn't exist on this target. |
| `www/worker.js` | The harness worker. `pulley` mode drives the probe; `native` mode runs the component's **core module** in the browser's own engine (**option C**) and calls `info.describe`; `spin` loops forever so the page can check termination. |
| `www/index.html` | Runs every mode in a fresh Web Worker and terminates any that doesn't answer in time. |
| `wasm_imports.py` | Lists a core module's imports and size. |

Built files (`*.wasm`, `*.locusplugin`) are not committed. To rebuild, from the repo root in the dev container:

```bash
# Option B probe
cd spikes/web-plugins/probe-wasmtime && cargo build --release --target wasm32-unknown-unknown
cp <target>/wasm32-unknown-unknown/release/probe_wasmtime.wasm ../www/probe.wasm
# The two example plugins
cargo build -p locus-cli
for e in finger-box hpgl; do
  cargo build --release --locked --target wasm32-unknown-unknown --manifest-path examples/$e/Cargo.toml
  locus-cli plugin pack --manifest examples/$e/plugin.toml \
    --wasm examples/$e/target/wasm32-unknown-unknown/release/locus_${e//-/_}.wasm \
    --out spikes/web-plugins/www/$e.locusplugin
done
```

Then unzip each `plugin.wasm` to `www/<name>.wasm`, serve `www/` (e.g. `python -m http.server`) and open it.
