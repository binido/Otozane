#!/bin/bash
# Captures the UI (optionally a page: overview|effects|profiles|settings) to a PNG.
# Usage: scripts/screenshot.sh OUT.png [page] [dark|light]
set -eu
raw=$(mktemp)
size=$(OTOZANE_SCREENSHOT=$raw OTOZANE_PAGE=${2:-overview} OTOZANE_THEME=${3:-} ./target/debug/otozane | tail -1)
python3 -c "import sys; from PIL import Image; w,h=map(int,sys.argv[2].split('x')); Image.frombytes('RGBA',(w,h),open(sys.argv[1],'rb').read()).save(sys.argv[3])" "$raw" "$size" "$1"
rm -f "$raw"
echo "$1 ($size)"
