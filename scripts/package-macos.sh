#!/usr/bin/env bash
# Builds Otozane.app and otozane-vVERSION-macos-arm64.dmg from release binaries.
# Usage: scripts/package-macos.sh VERSION OUT_DIR
# Env: TARGET_DIR - cargo target directory (default target),
#      RNNOISE_LICENSE - RNNoise COPYING file (default third_party/rnnoise/RNNoise-COPYING).
# The app is signed ad-hoc, without notarization.
set -euo pipefail

version="$1" out="$2"
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "Bad version: $version" >&2; exit 1; }
root="$(cd "$(dirname "$0")/.." && pwd)"
bin="${TARGET_DIR:-$root/target}/release"
rnnoise_license="${RNNOISE_LICENSE:-$root/third_party/rnnoise/RNNoise-COPYING}"
mkdir -p "$out"
out="$(cd "$out" && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

app="$work/dmg/Otozane.app/Contents"
mkdir -p "$app/MacOS" "$app/Resources/licenses"
sed "s/@VERSION@/$version/g" "$root/packaging/macos/Info.plist" > "$app/Info.plist"
install -m755 "$bin/otozane" "$bin/otozaned" "$app/MacOS/"
install -m644 "$root/LICENSE" "$root/THIRD_PARTY.md" "$rnnoise_license" "$app/Resources/licenses/"

# Icon from the 256 px PNG, the largest bundled raster.
icons="$work/otozane.iconset"
mkdir "$icons"
for s in 16 32 128 256; do
  sips -z $s $s "$root/crates/otozane-ui/assets/otozane.png" --out "$icons/icon_${s}x${s}.png" >/dev/null
done
iconutil -c icns "$icons" -o "$app/Resources/otozane.icns"

# The service is signed on its own first, under its own identifier, then the bundle.
codesign --force --sign - --identifier io.github.binido.otozane.service "$app/MacOS/otozaned"
codesign --force --sign - "$work/dmg/Otozane.app"
codesign --verify --strict "$work/dmg/Otozane.app"

name="otozane-v${version}-macos-arm64"
ln -s /Applications "$work/dmg/Applications"
rm -f "$out/$name.dmg"
hdiutil create -quiet -volname Otozane -fs HFS+ -format UDZO -srcfolder "$work/dmg" "$out/$name.dmg"
echo "Wrote $out/$name.dmg"
