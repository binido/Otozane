#!/usr/bin/env bash
# Fetches the ysfx sources (JSFX compiler and runtime) with their submodules into OUT_DIR.
# Usage: scripts/fetch-ysfx.sh [OUT_DIR]   (default: third_party/ysfx, built by otozane-plugins/build.rs)
# Needs git. Pinned to one ysfx commit.
set -euo pipefail

YSFX_REPO=https://github.com/jpcima/ysfx
YSFX_COMMIT=8077347ccf4115567aed81400281dca57acbb0cc

root="$(cd "$(dirname "$0")/.." && pwd)"
out="${1:-$root/third_party/ysfx}"
if [[ "$(git -C "$out" rev-parse HEAD 2>/dev/null)" != "$YSFX_COMMIT" ]]; then
  rm -rf "$out"
  mkdir -p "$out"
  git -C "$out" init -q
  git -C "$out" fetch -q --depth 1 "$YSFX_REPO" "$YSFX_COMMIT"
  git -C "$out" checkout -q FETCH_HEAD
fi
git -C "$out" submodule update -q --init --depth 1
# The source-size bound lives in a tracked patch, not in the ignored checkout.
patch="$root/crates/otozane-plugins/patches/ysfx-source-budget.patch"
if git -C "$out" apply --reverse --check "$patch" 2>/dev/null; then
  : # already applied
elif git -C "$out" apply --check "$patch" 2>/dev/null; then
  git -C "$out" apply "$patch"
else
  # Independent local edits are preserved; do not reset the checkout.
  echo "error: $out has source edits that conflict with $patch" >&2
  echo "       nothing was changed; restore the pinned files and retry" >&2
  exit 1
fi
echo "Wrote $out"
