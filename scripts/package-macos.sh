#!/usr/bin/env bash
# Builds Otozane.app and otozane-vVERSION-macos-arm64.dmg from release binaries.
# Usage: scripts/package-macos.sh VERSION OUT_DIR
# Env: TARGET_DIR - cargo target directory (default target),
#      RNNOISE_LICENSE - RNNoise COPYING file (default third_party/rnnoise/RNNoise-COPYING),
#      HUSH_DIR - output of scripts/build-hush.sh (default third_party/hush); required.
#      DRIVER_DIR - output of scripts/build-driver.sh (default third_party/driver); required.
# The app is signed ad-hoc, without notarization.
set -euo pipefail

version="$1" out="$2"
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "Bad version: $version" >&2; exit 1; }
root="$(cd "$(dirname "$0")/.." && pwd)"
bin="${TARGET_DIR:-$root/target}/release"
rnnoise_license="${RNNOISE_LICENSE:-$root/third_party/rnnoise/RNNoise-COPYING}"
hush="${HUSH_DIR:-$root/third_party/hush}"
[[ -f "$hush/libweya_nc.dylib" ]] || { echo "No Hush runtime in $hush; run scripts/build-hush.sh" >&2; exit 1; }
driver="${DRIVER_DIR:-$root/third_party/driver}"
[[ -d "$driver/Otozane.driver" ]] || { echo "No driver in $driver; run scripts/build-driver.sh" >&2; exit 1; }
mkdir -p "$out"
out="$(cd "$out" && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

app="$work/dmg/Otozane.app/Contents"
mkdir -p "$app/MacOS" "$app/Resources/licenses"
sed "s/@VERSION@/$version/g" "$root/packaging/macos/Info.plist" > "$app/Info.plist"
install -m755 "$bin/otozane" "$bin/otozaned" "$app/MacOS/"
install -m644 "$root/LICENSE" "$root/THIRD_PARTY.md" "$rnnoise_license" "$app/Resources/licenses/"
# Hush runtime, found by the service as ../Resources/hush.
mkdir -p "$app/Resources/hush"
install -m755 "$hush/libweya_nc.dylib" "$app/Resources/hush/"
install -m644 "$hush/advanced_dfnet16k_model_best_onnx.tar.gz" "$app/Resources/hush/"
for f in Hush-LICENSE DeepFilterNet-LICENSE-MIT DeepFilterNet-LICENSE-APACHE Hush-crates.txt; do
  install -m644 "$hush/$f" "$app/Resources/licenses/"
done
# Audio driver, copied to /Library/Audio/Plug-Ins/HAL by Settings -> Install. Signed by build-driver.sh.
cp -R "$driver/Otozane.driver" "$app/Resources/"
install -m644 "$driver"/libASPL-LICENSE* "$app/Resources/licenses/"

# Icon from the 256 px PNG, the largest bundled raster.
icons="$work/otozane.iconset"
mkdir "$icons"
for s in 16 32 128 256; do
  sips -z $s $s "$root/crates/otozane-ui/assets/otozane.png" --out "$icons/icon_${s}x${s}.png" >/dev/null
done
iconutil -c icns "$icons" -o "$app/Resources/otozane.icns"

# Nested code is signed first: the Hush library, the service under its own identifier, then the bundle.
codesign --force --sign - "$app/Resources/hush/libweya_nc.dylib"
codesign --force --sign - --identifier io.github.binido.otozane.service "$app/MacOS/otozaned"
codesign --force --sign - "$work/dmg/Otozane.app"
codesign --verify --strict "$work/dmg/Otozane.app"

name="otozane-v${version}-macos-arm64"
ln -s /Applications "$work/dmg/Applications"
rm -f "$out/$name.dmg"
hdiutil create -quiet -volname Otozane -fs HFS+ -format UDZO -srcfolder "$work/dmg" "$out/$name.dmg"
echo "Wrote $out/$name.dmg"
