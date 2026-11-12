#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
#
# Starts a GUI command headless (Xvfb + software Vulkan/GL) and passes if it is
# still running after a few seconds (packaging #247-#249). Needs xvfb, xauth and
# mesa-vulkan-drivers (lavapipe) installed.
# Usage: smoke-gui.sh <command> [args...]
set -uo pipefail

export WGPU_BACKEND=${WGPU_BACKEND:-vulkan,gl}
set +e
xvfb-run -a -s "-screen 0 1280x800x24" timeout 12 "$@" > /tmp/locus-smoke.log 2>&1
code=$?
set -e
# timeout exits 124 when it had to kill a still-running app: that is the pass.
if [ "$code" -eq 124 ]; then echo "GUI stayed up for 12 s: ok"; exit 0; fi
echo "GUI exited early with code $code:"; cat /tmp/locus-smoke.log; exit 1
