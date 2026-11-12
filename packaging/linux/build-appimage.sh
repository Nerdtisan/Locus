#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
#
# Builds the AppImage from the staged tree (SPEC §0, #247).
# Usage: build-appimage.sh <version> <bin-dir> <appimagetool> <out-dir>
set -euo pipefail

version=$1 bin=$2 tool=$3 out=$4
here=$(cd "$(dirname "$0")" && pwd)
work=$(mktemp -d)
appdir="$work/Locus.AppDir"

bash "$here/stage.sh" "$bin" "$appdir"
# AppImage root entries: desktop file + icon + AppRun.
cp "$here/locus.desktop" "$appdir/locus.desktop"
cp "$appdir/usr/share/icons/hicolor/256x256/apps/locus.png" "$appdir/locus.png"
cp "$appdir/locus.png" "$appdir/.DirIcon"
ln -s usr/bin/locus "$appdir/AppRun"

mkdir -p "$out"
# --appimage-extract-and-run: CI runners have no FUSE.
ARCH=x86_64 "$tool" --appimage-extract-and-run --no-appstream "$appdir" "$out/Locus-${version}-linux-x86_64.AppImage"
