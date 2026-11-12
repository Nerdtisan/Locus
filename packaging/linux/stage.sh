#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
#
# Lays out the files every Linux package shares under <dest>/usr (SPEC §0, #247-#249):
# both binaries, the desktop entry, the MIME types, and the icon at three sizes.
# Needs rsvg-convert (librsvg2-bin).
#
# Usage: stage.sh <dir with locus and locus-cli> <dest>
set -euo pipefail

bin=$1 dest=$2
here=$(cd "$(dirname "$0")" && pwd)
icons="$here/../icon"

install -Dm755 "$bin/locus" "$dest/usr/bin/locus"
install -Dm755 "$bin/locus-cli" "$dest/usr/bin/locus-cli"
install -Dm644 "$here/locus.desktop" "$dest/usr/share/applications/locus.desktop"
install -Dm644 "$here/locus-mime.xml" "$dest/usr/share/mime/packages/locus.xml"
install -Dm644 "$icons/locus.svg" "$dest/usr/share/icons/hicolor/scalable/apps/locus.svg"
for s in 48 256 512; do
  mkdir -p "$dest/usr/share/icons/hicolor/${s}x${s}/apps"
  rsvg-convert -w "$s" -h "$s" "$icons/locus.svg" -o "$dest/usr/share/icons/hicolor/${s}x${s}/apps/locus.png"
done
install -Dm644 "$here/../../LICENSE" "$dest/usr/share/doc/locus/copyright"
install -Dm644 "$here/../../THIRD-PARTY-NOTICES.md" "$dest/usr/share/doc/locus/THIRD-PARTY-NOTICES.md"
