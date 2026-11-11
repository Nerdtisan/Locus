#!/usr/bin/env bash
# Builds this plugin for wasm32 and packs it into a .locusplugin (SPEC §15.7).
#
# Run from this folder. Needs the wasm32-unknown-unknown Rust target and
# locus-cli: set LOCUS_CLI to its path, or put it on PATH.
set -euo pipefail
cd "$(dirname "$0")"

cargo build --release --target wasm32-unknown-unknown
name=$(sed -n 's/^name = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)
wasm="target/wasm32-unknown-unknown/release/${name//-/_}.wasm"

"${LOCUS_CLI:-locus-cli}" plugin pack \
  --manifest plugin.toml \
  --wasm "$wasm" \
  --readme README.md \
  --out "$name.locusplugin"
echo "packed $name.locusplugin"
