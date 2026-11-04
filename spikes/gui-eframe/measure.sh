#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
#
# Build-time and size measurements for RESULTS.md (#44). Same steps as spike
# B's measure.sh so the numbers compare directly. Run inside the locus-dev
# container from spikes/gui-eframe, registry already downloaded ("clean" means
# a clean target for that triple, not a cold registry), with target/ on a
# Docker volume rather than the OneDrive bind mount:
#
#   docker run --rm -e CARGO_HOME=/workspace/.cargo-home -v "${PWD}\..\..:/workspace" `
#     -v locus-spike-eframe-target:/workspace/spikes/gui-eframe/target `
#     -w /workspace/spikes/gui-eframe locus-dev bash measure.sh
#
# Puts the Windows .exe and the web bundle in dist/ (git-ignored).
set -euo pipefail

WIN=x86_64-pc-windows-gnu
EXE=spike-eframe.exe
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
cargo build -q --release --target "$WIN"
cp "target/$WIN/release/$EXE" dist/
echo "windows release exe: $(stat -c %s "dist/$EXE") bytes, gzip -9 $(gzip -9 -c "dist/$EXE" | wc -c) bytes"

rm -rf target/wasm32-unknown-unknown target/wasm-opt dist/web
s=$(ms); trunk build -q --release --cargo-profile wasm-release --dist dist/web; took "web (trunk, wasm-release + wasm-opt -Oz) clean:" "$s"
touch_main
s=$(ms); trunk build -q --release --cargo-profile wasm-release --dist dist/web; took "web incremental:" "$s"
untouch_main
trunk build -q --release --cargo-profile wasm-release --dist dist/web
for f in dist/web/*.wasm dist/web/*.js; do
  echo "$(basename "$f"): $(stat -c %s "$f") bytes, gzip -9 $(gzip -9 -c "$f" | wc -c) bytes"
done

# Native Linux: compile check only (the container has no display or GPU).
s=$(ms); cargo build -q --release; took "linux release (compile check) after windows deps:" "$s"
