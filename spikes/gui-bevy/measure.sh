#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
#
# Build-time and size measurements for RESULTS.md (#45). Run inside the
# locus-dev container from spikes/gui-bevy, with the crate registry already
# downloaded (so "clean" means a clean target, not a cold registry):
#
#   docker run --rm -e CARGO_HOME=/workspace/.cargo-home -v "${PWD}:/workspace" `
#     -v locus-spike-bevy-target:/workspace/spikes/gui-bevy/target `
#     -w /workspace/spikes/gui-bevy locus-dev bash measure.sh
#
# Puts the Windows .exe files and the web bundle in dist/ (git-ignored).
set -euo pipefail

WIN=x86_64-pc-windows-gnu
ms() { date +%s%N; }
took() { echo "$1 $(( ($(ms) - $2) / 1000000 )) ms"; }
touch_main() { echo "// measure.sh touch" >> src/main.rs; }
untouch_main() { sed -i '$ d' src/main.rs; }
trap 'grep -q "measure.sh touch" src/main.rs && untouch_main || true' EXIT

echo "cores: $(nproc)"
mkdir -p dist

rm -rf "target/$WIN"
s=$(ms); cargo build -q --release --target "$WIN"; took "windows release clean:" "$s"
touch_main
s=$(ms); cargo build -q --release --target "$WIN"; took "windows release incremental:" "$s"
untouch_main
cargo build -q --target "$WIN"
touch_main
s=$(ms); cargo build -q --target "$WIN"; took "windows dev incremental:" "$s"
untouch_main
cp "target/$WIN/release/locus-spike-bevy.exe" dist/
echo "windows release exe: $(stat -c %s dist/locus-spike-bevy.exe) bytes, gzip -9 $(gzip -9 -c dist/locus-spike-bevy.exe | wc -c) bytes"

rm -rf target/wasm32-unknown-unknown target/wasm-opt dist/web
s=$(ms); trunk build -q --release --cargo-profile wasm-release --dist dist/web; took "web (trunk, wasm-release + wasm-opt -Oz) clean:" "$s"
touch_main
s=$(ms); trunk build -q --release --cargo-profile wasm-release --dist dist/web; took "web incremental:" "$s"
untouch_main
trunk build -q --release --cargo-profile wasm-release --dist dist/web
for f in dist/web/*.wasm dist/web/*.js; do
  echo "$(basename "$f"): $(stat -c %s "$f") bytes, gzip -9 $(gzip -9 -c "$f" | wc -c) bytes"
done

cargo build -q --release --target "$WIN" --features capture
cp "target/$WIN/release/locus-spike-bevy.exe" dist/locus-spike-bevy-capture.exe
