#!/usr/bin/env bash
# Builds the Hush runtime (libweya_nc.so, .dylib on macOS) from the official Hush sources and
# copies it, the model bundle and their licences into OUT_DIR.
# Usage: scripts/build-hush.sh [OUT_DIR]   (default: third_party/hush, used by dev builds)
# Needs git, cargo and a C toolchain. Pinned to one Hush commit; the model is checksummed.
set -euo pipefail

HUSH_REPO=https://github.com/pulp-vision/Hush
HUSH_COMMIT=9f6414e91461a8f4bdf9840c0cdcdcb7da986339
MODEL=advanced_dfnet16k_model_best_onnx.tar.gz
MODEL_SHA256=45632ccaa82b71bb743d6caa7c78e983fe2f2790a3af7f6ec48e6ed7ba085df6

root="$(cd "$(dirname "$0")/.." && pwd)"
out="${1:-$root/third_party/hush}"
mkdir -p "$out"
out="$(cd "$out" && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

git -C "$work" init -q src
git -C "$work/src" fetch -q --depth 1 "$HUSH_REPO" "$HUSH_COMMIT"
git -C "$work/src" checkout -q FETCH_HEAD
src="$work/src"
echo "$MODEL_SHA256  $src/deployment/models/$MODEL" | shasum -a 256 -c --quiet -

crate="$src/native/weya_nc_build/weya_nc"
CARGO_TARGET_DIR="$work/target" cargo build --release --locked --manifest-path "$crate/Cargo.toml"

lib=libweya_nc.so
[[ "$(uname)" == Darwin ]] && lib=libweya_nc.dylib
# BSD install on macOS has no -D, $out already exists.
install -m755 "$work/target/release/$lib" "$out/$lib"
install -m644 "$src/deployment/models/$MODEL" "$out/$MODEL"
install -m644 "$src/LICENSE" "$out/Hush-LICENSE"
vendor="$src/native/weya_nc_build/vendor/deep_filter"
install -m644 "$vendor/LICENSE-MIT" "$out/DeepFilterNet-LICENSE-MIT"
install -m644 "$vendor/LICENSE-APACHE" "$out/DeepFilterNet-LICENSE-APACHE"
# Rust crates statically linked into the library, with their licences.
cargo tree --locked --manifest-path "$crate/Cargo.toml" -e normal --prefix none \
  --format '{p} ({l})' | grep -v '(proc-macro)' | sed 's/ (\*)$//; s| (/[^)]*)||' | sort -u > "$out/Hush-crates.txt"
echo "Hush $HUSH_COMMIT" > "$out/VERSION"
echo "Wrote $out"
