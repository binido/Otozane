#!/bin/sh
# Prints the CHANGELOG.md section for a version (without its heading).
# Fails if there is none, so a release cannot go out without notes.
# Usage: scripts/release-notes.sh 0.3.0
set -eu
notes="$(awk -v v="$1" '/^## /{p = ($2 == v)} p && !/^## /' "$(dirname "$0")/../CHANGELOG.md")"
[ -n "$(printf %s "$notes" | tr -d '[:space:]')" ] || { echo "CHANGELOG.md has no section for $1" >&2; exit 1; }
printf '%s\n' "$notes"
