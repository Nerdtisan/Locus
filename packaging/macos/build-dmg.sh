#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
#
# Builds Locus.app (universal arm64 + x86_64) and a .dmg from two prebuilt
# release binaries (packaging #246, SPEC §0). Runs on a macOS host only.
#
# Usage: build-dmg.sh <version> <universal-locus> <universal-locus-cli> <out-dir>
set -euo pipefail

version=$1 bin=$2 cli=$3 out=$4
here=$(cd "$(dirname "$0")" && pwd)
icon="$here/../icon/locus-1024.png"

work=$(mktemp -d)
app="$work/dmg/Locus.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources" "$out"

cp "$bin" "$app/Contents/MacOS/locus"
cp "$cli" "$app/Contents/Resources/locus-cli"
chmod +x "$app/Contents/MacOS/locus" "$app/Contents/Resources/locus-cli"
# Info.plist versions are numeric-ish; keep only the leading x.y.z for them.
sed "s/@VERSION@/${version%%-*}/" "$here/Info.plist" > "$app/Contents/Info.plist"
plutil -lint "$app/Contents/Info.plist"

# Icon: build the .icns from the 1024 px PNG.
iconset="$work/Locus.iconset"
mkdir "$iconset"
for s in 16 32 128 256 512; do
  sips -z "$s" "$s" "$icon" --out "$iconset/icon_${s}x${s}.png" >/dev/null
  sips -z $((s * 2)) $((s * 2)) "$icon" --out "$iconset/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$iconset" -o "$app/Contents/Resources/Locus.icns"

# Ad-hoc signature: arm64 binaries must carry one to run at all (lipo
# invalidates the linker's). This is NOT a Developer ID signature; the release
# workflow passes SIGN_IDENTITY (a Developer ID) once MACOS_CERT_* secrets exist.
codesign --force --deep --sign "${SIGN_IDENTITY:--}" "$app"

cp "$here/FIRST-RUN.txt" "$work/dmg/READ ME FIRST.txt"
ln -s /Applications "$work/dmg/Applications"
dmg="$out/Locus-${version}-macos-universal.dmg"
rm -f "$dmg"
hdiutil create -volname "Locus" -srcfolder "$work/dmg" -ov -format UDZO "$dmg"
echo "$dmg"
