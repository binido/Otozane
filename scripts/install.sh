#!/usr/bin/env bash
# Installs Otozane.
#
# From the web, picks the package for this distribution from the latest GitHub release:
#   curl -fsSL https://raw.githubusercontent.com/binido/Otozane/main/scripts/install.sh | bash
#   Arch family: builds otozane-bin with makepkg. Debian/Ubuntu: .deb. Fedora/openSUSE: .rpm.
#   Anything else: the release tarball into ~/.local.
# From an unpacked release tarball, copies its files into a prefix:
#   ./install.sh [--prefix DIR]   (default prefix ~/.local)
# Removes a tarball install (not a package): install.sh --uninstall [--prefix DIR],
#   or curl ... | bash -s -- --uninstall
# Env: OTOZANE_VERSION=1.2.3 picks a release, OTOZANE_FORMAT=tarball skips the package manager.
set -euo pipefail

REPO=https://github.com/binido/Otozane
here=""
[[ -z "${BASH_SOURCE[0]:-}" ]] || here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

fail() { echo "otozane install: $*" >&2; exit 1; }

install_files() {
  local src="$1" prefix="$2" apps icons f
  apps="$prefix/share/applications"
  icons="$prefix/share/icons/hicolor"
  install -Dm755 "$src/usr/bin/otozane" "$prefix/bin/otozane"
  install -Dm755 "$src/usr/bin/otozaned" "$prefix/bin/otozaned"
  mkdir -p "$prefix/lib/otozane"
  cp -a "$src/usr/lib/otozane/." "$prefix/lib/otozane/"
  install -Dm644 "$src/usr/share/icons/hicolor/scalable/apps/otozane.svg" "$icons/scalable/apps/otozane.svg"
  # Absolute Exec, so the menu entry works even when the prefix is not in PATH.
  mkdir -p "$apps"
  sed "s|^Exec=.*|Exec=$prefix/bin/otozane|" "$src/usr/share/applications/otozane.desktop" > "$apps/otozane.desktop"
  for f in licenses/otozane doc/otozane; do
    mkdir -p "$prefix/share/$f"
    cp -a "$src/usr/share/$f/." "$prefix/share/$f/"
  done
  command -v update-desktop-database >/dev/null && update-desktop-database "$apps" >/dev/null 2>&1 || true
  command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q "$icons" >/dev/null 2>&1 || true
  echo "Installed Otozane into $prefix. Run: $prefix/bin/otozane"
}

uninstall_files() {
  local prefix="$1"
  systemctl --user disable --now otozane.service >/dev/null 2>&1 || true
  rm -f "$prefix/bin/otozane" "$prefix/bin/otozaned" \
    "$prefix/share/applications/otozane.desktop" \
    "$prefix/share/icons/hicolor/scalable/apps/otozane.svg" \
    "${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user/otozane.service"
  rm -rf "$prefix/share/licenses/otozane" "$prefix/share/doc/otozane" "$prefix/lib/otozane"
  echo "Removed Otozane from $prefix. Settings in ~/.config/otozane are kept."
}

prefix="$HOME/.local" action=install
while [[ $# -gt 0 ]]; do
  case "$1" in
    --prefix) prefix="${2:?--prefix needs a value}"; shift 2 ;;
    --uninstall) action=uninstall; shift ;;
    *) fail "unknown argument: $1" ;;
  esac
done
[[ "$action" == install ]] || { uninstall_files "$prefix"; exit 0; }

# Running from an unpacked tarball.
if [[ -n "$here" && -x "$here/usr/bin/otozaned" ]]; then
  install_files "$here" "$prefix"
  exit 0
fi

# Running from the web.
command -v curl >/dev/null || fail "curl is required"
case "$(uname -m)" in
  x86_64|amd64) arch=x86_64 deb_arch=amd64 ;;
  aarch64|arm64) arch=aarch64 deb_arch=arm64 ;;
  *) fail "unsupported CPU architecture: $(uname -m)" ;;
esac

version="${OTOZANE_VERSION:-}"
if [[ -z "$version" ]]; then
  latest="$(curl -fsSLI -o /dev/null -w '%{url_effective}' "$REPO/releases/latest")"
  version="${latest##*/v}"
fi
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || fail "could not find the latest release (got '$version')"
base="$REPO/releases/download/v$version"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
cd "$tmp"

fetch() {
  curl -fsSLO "$base/$1"
  curl -fsSL "$base/SHA256SUMS" | grep " $1\$" | sha256sum -c --quiet - || fail "checksum mismatch for $1"
}

sudo=""
[[ "$(id -u)" -eq 0 ]] || sudo=sudo

format="${OTOZANE_FORMAT:-}"
if [[ -z "$format" ]]; then
  if command -v pacman >/dev/null && command -v makepkg >/dev/null; then format=arch
  elif command -v apt-get >/dev/null; then format=deb
  elif command -v dnf >/dev/null || command -v zypper >/dev/null; then format=rpm
  else format=tarball
  fi
fi

echo "Installing Otozane $version ($format, $arch)"
case "$format" in
  arch)
    [[ "$(id -u)" -ne 0 ]] || fail "makepkg does not run as root, run this as a regular user"
    fetch PKGBUILD
    makepkg -si --noconfirm --needed
    ;;
  deb)
    f="otozane_${version}-1_${deb_arch}.deb"
    fetch "$f"
    $sudo apt-get install -y "./$f"
    ;;
  rpm)
    f="otozane-${version}-1.${arch}.rpm"
    fetch "$f"
    if command -v dnf >/dev/null; then $sudo dnf install -y "./$f"
    else $sudo zypper --non-interactive install --allow-unsigned-rpm "./$f"
    fi
    ;;
  tarball)
    f="otozane-v${version}-linux-${arch}.tar.gz"
    fetch "$f"
    tar xzf "$f"
    install_files "$tmp/otozane-v${version}-linux-${arch}" "$prefix"
    echo "Otozane needs PipeWire from your distribution."
    ;;
  *) fail "unknown OTOZANE_FORMAT: $format" ;;
esac
