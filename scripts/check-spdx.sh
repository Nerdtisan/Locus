#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
#
# Fails if a tracked Rust source file doesn't start with the SPDX header for
# its crate's licence (SPEC §2, §15.6). locus-plugin-api is MIT OR Apache-2.0
# so plugin authors don't inherit AGPL obligations; everything else is AGPL.
#
# Usage: scripts/check-spdx.sh   (from the repo root; needs git and bash only)

set -euo pipefail

expected_for() {
  case "$1" in
    crates/locus-plugin-api/*) echo "// SPDX-License-Identifier: MIT OR Apache-2.0" ;;
    *) echo "// SPDX-License-Identifier: AGPL-3.0-or-later" ;;
  esac
}

bad=0
checked=0
while IFS= read -r -d '' file; do
  checked=$((checked + 1))
  want=$(expected_for "$file")
  # Strip a CR so Windows checkouts with CRLF line endings still match.
  first=$(head -n 1 "$file" | tr -d '\r')
  if [ "$first" != "$want" ]; then
    echo "::error file=$file,line=1::expected '$want', found '$first'"
    bad=$((bad + 1))
  fi
done < <(git ls-files -z -- '*.rs')

if [ "$bad" -ne 0 ]; then
  echo "$bad of $checked Rust files have a missing or wrong SPDX header."
  exit 1
fi
echo "All $checked Rust files have the right SPDX header."
