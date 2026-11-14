#!/usr/bin/env bash
# Builds static RNNoise (librnnoise.a) into OUT_DIR, for systems that do not package it.
# Usage: scripts/build-rnnoise.sh [OUT_DIR]   (default: third_party/rnnoise, picked up by dev builds)
set -euo pipefail

VERSION=0.2
SHA256=90fce4b00b9ff24c08dbfe31b82ffd43bae383d85c5535676d28b0a2b11c0d37

root="$(cd "$(dirname "$0")/.." && pwd)"
out="${1:-$root/third_party/rnnoise}"
mkdir -p "$out"
out="$(cd "$out" && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

curl -fsSLo "$work/rnnoise.tar.gz" \
  "https://github.com/xiph/rnnoise/releases/download/v$VERSION/rnnoise-$VERSION.tar.gz"
echo "$SHA256  $work/rnnoise.tar.gz" | shasum -a 256 -c --quiet -
tar -C "$work" -xzf "$work/rnnoise.tar.gz"
cd "$work/rnnoise-$VERSION"
# The 0.2 tarball misses os_support.h, which the NEON code includes.
printf '#include "common.h"\n#define OPUS_CLEAR(dst, n) RNN_CLEAR(dst, n)\n' > src/os_support.h
./configure --disable-shared --enable-static --with-pic --disable-examples --disable-doc
make -j"$(getconf _NPROCESSORS_ONLN)"
cp .libs/librnnoise.a "$out/"
cp COPYING "$out/RNNoise-COPYING"
echo "Wrote $out"
