#!/usr/bin/env bash
# Builds the Otozane Microphone audio driver (Otozane.driver) into OUT_DIR.
# Usage: scripts/build-driver.sh [OUT_DIR]   (default: third_party/driver)
# Needs git, cmake and Xcode command line tools. libASPL is pinned to one release tag.
set -euo pipefail

LIBASPL_REPO=https://github.com/gavv/libASPL
LIBASPL_TAG=v3.1.2
LIBASPL_COMMIT=47f688ed6bb637ab8b7f4b36864734b2b1f69b6b

root="$(cd "$(dirname "$0")/.." && pwd)"
out="${1:-$root/third_party/driver}"
mkdir -p "$out"
out="$(cd "$out" && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

git -C "$work" init -q libASPL
# libASPL takes its version from the tag, so the tag itself is fetched.
git -C "$work/libASPL" fetch -q --depth 1 "$LIBASPL_REPO" tag "$LIBASPL_TAG"
git -C "$work/libASPL" checkout -q "$LIBASPL_TAG"
[[ "$(git -C "$work/libASPL" rev-parse HEAD)" == "$LIBASPL_COMMIT" ]] || { echo "libASPL $LIBASPL_TAG moved" >&2; exit 1; }

version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)"
cmake -S "$root/packaging/macos/driver" -B "$work/build" -DCMAKE_BUILD_TYPE=Release \
  -DLIBASPL_SOURCE_DIR="$work/libASPL" -DDRIVER_VERSION="$version" >/dev/null
cmake --build "$work/build" --config Release -j "$(getconf _NPROCESSORS_ONLN)" >/dev/null

rm -rf "$out/Otozane.driver"
cp -R "$work/build/Otozane.driver" "$out/"
codesign --force --sign - "$out/Otozane.driver"
cp "$work/libASPL/LICENSE" "$out/libASPL-LICENSE"
cp "$work/libASPL/LICENSE.apple2012" "$out/libASPL-LICENSE.apple2012"
cp "$work/libASPL/LICENSE.apple2020" "$out/libASPL-LICENSE.apple2020"
echo "Wrote $out/Otozane.driver"
