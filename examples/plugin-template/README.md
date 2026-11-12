# Locus plugin template

A minimal **generator** plugin for [Locus](https://github.com/Nerdtisan/Locus): a rectangle whose sides carry horizontal and vertical constraints. Copy this folder to start your own plugin.

Licensed MIT OR Apache-2.0, like the plugin SDK, so your plugin can use any licence (SPEC §15.6).

## What a plugin is

A WebAssembly component that implements one or more plugin kinds from the `locus:plugin@1.0.0` interface (`crates/locus-plugin-api/wit/plugin.wit`):

| Kind | Implement | You get | You return |
|---|---|---|---|
| generator | `GeneratorGuest` | parameters | points, lines, arcs, circles, constraints |
| modifier | `ModifierGuest` | paths + parameters | paths |
| exporter | `ExporterGuest` | the validated export model | file bytes |
| importer | `ImporterGuest` | file bytes (Locus reads the file) | paths, placed on a locked reference layer |
| validator | `ValidatorGuest` | the validated export model + parameters | findings (error or warning, with layer and path) |

Examples: `examples/finger-box` (generator), `examples/hpgl` (exporter and importer), `examples/kerf-check` (validator).

Every plugin also implements `InfoGuest` (id, name, version). Then one line exports it:

```rust
locus_plugin_api::export_plugin!(MyPlugin: generator);
```

## Rules

- **No I/O.** Plugins can't reach the filesystem, network, clock, environment or randomness: the sandbox gives them nothing to call (SPEC §15.4). Locus does every file dialog and write.
- **Deterministic.** The same parameters must give the same output. If you need randomness, take a seed parameter (SPEC §15.5).
- **Units:** millimetres and radians, `f64`.
- **Limits:** each call gets a CPU budget, 256 MB of memory and 5 seconds. Going over aborts the call; Locus keeps running and the document is unchanged.
- **Output is checked:** at most 100 000 points/entities/constraints, finite coordinates within ±1 km, and every index must point at something.

## Build

Requirements: Rust with the `wasm32-unknown-unknown` target (`rustup target add wasm32-unknown-unknown`) and `locus-cli`.

```bash
./build.sh                       # with locus-cli on PATH
LOCUS_CLI=/path/to/locus-cli ./build.sh
```

This runs `cargo build --release --target wasm32-unknown-unknown` and then `locus-cli plugin pack`, which turns the build into a component, loads it in the same sandbox Locus uses, checks it against `plugin.toml`, and writes `my-locus-plugin.locusplugin`.

Build for `wasm32-unknown-unknown`, **not** `wasm32-wasip2`: WASI builds import system interfaces, and Locus refuses any plugin that imports anything.

Inside the Locus repo, with the dev container:

```bash
docker run --rm -e CARGO_HOME=/workspace/.cargo-home -v "$PWD:/workspace" -w /workspace locus-dev \
  bash -c "cargo build -p locus-cli && LOCUS_CLI=/workspace/target/debug/locus-cli examples/plugin-template/build.sh"
```

## Make it yours

1. Pick a reverse-DNS id (`com.yourname.thing`) and set it, the name and the version in **both** `plugin.toml` and `describe()` in `src/lib.rs`. Locus refuses a plugin whose component and manifest disagree.
2. Outside the Locus repo, switch the `locus-plugin-api` dependency in `Cargo.toml` to the git form shown there.
3. Describe your parameters in `schema()`; Locus draws the panel from it. Read them with `Params`.
4. Build the output with `Sketch`: `point`, `line`, `arc`, `circle`, `polygon`, `constrain`. Identical points are shared, so corners stay connected.

## Install

In Locus: **Plugins ▸ Install plugin…** and pick the `.locusplugin` file. Headless: `locus-cli plugin run my-locus-plugin.locusplugin --params params.ron --out out.svg`.
