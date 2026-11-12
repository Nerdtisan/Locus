#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
#
# Builds the .deb from the staged tree (SPEC §0, #249).
# Usage: build-deb.sh <version> <bin-dir> <out-dir>
set -euo pipefail

version=$1 bin=$2 out=$3
here=$(cd "$(dirname "$0")" && pwd)
work=$(mktemp -d)
root="$work/locus_${version}_amd64"

bash "$here/stage.sh" "$bin" "$root"
# Debian versions must start with a digit; "1.2.3-dev.abc" sorts below "1.2.3" via "~".
debver=${version/-/\~}

mkdir -p "$root/DEBIAN"
cat > "$root/DEBIAN/control" <<CTRL
Package: locus
Version: ${debver}
Architecture: amd64
Maintainer: Locus contributors <noreply@users.noreply.github.com>
Section: graphics
Priority: optional
Homepage: https://github.com/Nerdtisan/Locus
Depends: libc6 (>= 2.34)
Recommends: libvulkan1, mesa-vulkan-drivers, libxkbcommon0, libwayland-client0, xdg-desktop-portal
Description: Constrained 2D vector editor for plotters and cutters
 Locus is a constraint-driven 2D vector editor for pen plotters, vinyl
 cutters and laser cutters. Layers map to machine operations and output is
 clean SVG, PDF and DXF. This package includes the locus-cli headless tool.
CTRL

mkdir -p "$out"
dpkg-deb --root-owner-group --build "$root" "$out/locus_${debver}_amd64.deb"
