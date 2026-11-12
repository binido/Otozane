#!/usr/bin/env bash
# Builds the release packages from already built binaries in target/release.
# Usage: scripts/package-linux.sh VERSION ARCH OUT_DIR   (ARCH: x86_64 | aarch64)
# Env: RNNOISE_LICENSE - RNNoise COPYING file, shipped when RNNoise is linked statically.
# Writes otozane-vVERSION-linux-ARCH.tar.gz, .deb (needs dpkg-dev) and .rpm (needs rpmbuild).
set -euo pipefail

version="$1" arch="$2" out="$3"
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "Bad version: $version" >&2; exit 1; }
case "$arch" in
  x86_64) deb_arch=amd64 ;;
  aarch64) deb_arch=arm64 ;;
  *) echo "Bad arch: $arch" >&2; exit 1 ;;
esac

root="$(cd "$(dirname "$0")/.." && pwd)"
mkdir -p "$out"
out="$(cd "$out" && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# Shared payload, laid out as under /usr.
payload="$work/payload"
install -Dm755 "$root/target/release/otozane" "$payload/usr/bin/otozane"
install -Dm755 "$root/target/release/otozaned" "$payload/usr/bin/otozaned"
install -Dm644 "$root/packaging/otozane.desktop" "$payload/usr/share/applications/otozane.desktop"
install -Dm644 "$root/packaging/otozane.svg" "$payload/usr/share/icons/hicolor/scalable/apps/otozane.svg"
install -Dm644 "$root/README.md" "$payload/usr/share/doc/otozane/README.md"
lic="$payload/usr/share/licenses/otozane"
install -Dm644 "$root/LICENSE" "$lic/LICENSE"
install -Dm644 "$root/THIRD_PARTY.md" "$lic/THIRD_PARTY.md"
[[ -z "${RNNOISE_LICENSE:-}" ]] || install -Dm644 "$RNNOISE_LICENSE" "$lic/RNNoise-COPYING"
desktop-file-validate "$payload/usr/share/applications/otozane.desktop"

# Tarball: the payload plus the installer that copies it into a prefix.
name="otozane-v${version}-linux-${arch}"
mkdir "$work/$name"
cp -a "$payload/usr" "$work/$name/"
install -m755 "$root/scripts/install.sh" "$work/$name/install.sh"
tar -C "$work" -czf "$out/$name.tar.gz" "$name"
echo "Wrote $out/$name.tar.gz"

if command -v dpkg-deb >/dev/null; then
  deb="$work/deb"
  cp -a "$payload" "$deb"
  mkdir -p "$deb/DEBIAN" "$work/debian"
  # Debian keeps licence text in /usr/share/doc/<package>/copyright.
  cat "$lic/LICENSE" "$lic/THIRD_PARTY.md" ${RNNOISE_LICENSE:+"$lic/RNNoise-COPYING"} \
    > "$deb/usr/share/doc/otozane/copyright"
  rm -rf "$deb/usr/share/licenses"
  printf 'Source: otozane\n\nPackage: otozane\nArchitecture: any\n' > "$work/debian/control"
  shlibs="$(cd "$work" && dpkg-shlibdeps -O "$deb/usr/bin/otozane" "$deb/usr/bin/otozaned")"
  shlibs="${shlibs#shlibs:Depends=}"
  cat > "$deb/DEBIAN/control" <<EOF
Package: otozane
Version: ${version}-1
Architecture: ${deb_arch}
Maintainer: binido <binido@users.noreply.github.com>
Section: sound
Priority: optional
Homepage: https://github.com/binido/Otozane
Installed-Size: $(du -sk "$deb/usr" | cut -f1)
Depends: ${shlibs}, libegl1 | libgl1, libxkbcommon0
Recommends: pipewire, wireplumber, libwayland-client0, libwayland-egl1, xdg-desktop-portal, hicolor-icon-theme
Description: Real-time microphone processor for PipeWire
 Captures a microphone, runs it through an effect chain and publishes
 the result as the virtual input device "Otozane Microphone".
EOF
  deb_file="$out/otozane_${version}-1_${deb_arch}.deb"
  dpkg-deb --root-owner-group --build "$deb" "$deb_file"
  dpkg-deb -f "$deb_file" Depends | grep -q libpipewire || { echo "No libpipewire in Depends" >&2; exit 1; }
  echo "Wrote $deb_file"
fi

if command -v rpmbuild >/dev/null; then
  rpmbuild -bb --quiet \
    --define "_topdir $work/rpm" \
    --define "oz_version $version" \
    --define "oz_payload $payload" \
    --target "$arch" \
    "$root/packaging/linux/otozane.spec"
  rpm_file="$out/otozane-${version}-1.${arch}.rpm"
  cp "$work/rpm/RPMS/$arch/otozane-${version}-1.${arch}.rpm" "$rpm_file"
  rpm -qp --requires "$rpm_file" | grep -q '^libpipewire' || { echo "AutoReq did not run" >&2; exit 1; }
  echo "Wrote $rpm_file"
fi
