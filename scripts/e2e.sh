#!/bin/bash
# End-to-end check without touching real devices:
#   tone -> temporary test source -> otozaned -> Otozane Microphone -> pw-record
# All PipeWire objects created here are session-only and removed on exit.
# Usage: scripts/e2e.sh [tone.wav]   (env: BIN, SVC_ARGS, OUT, SHOW_LINKS)
set -u
DIR=$(cd "$(dirname "$0")" && pwd)
OUT=${OUT:-${XDG_RUNTIME_DIR:-/tmp}/otozane-e2e}
BIN=${BIN:-./target/debug/otozaned}
mkdir -p "$OUT"
TONE=${1:-$OUT/tone.wav}
[ -f "$TONE" ] || python3 "$DIR/tone.py" "$TONE" 4 0.25
pw-loopback --capture-props='media.class=Audio/Sink node.name=oz_test_sink audio.position=[MONO] node.dont-move=true' \
  --playback-props='media.class=Audio/Source node.name=oz_test_src audio.position=[MONO] node.dont-move=true' >"$OUT/loop.log" 2>&1 &
LP=$!; sleep 1
$BIN ${SVC_ARGS:-} >"$OUT/svc.log" 2>&1 &
SV=$!; sleep 1.5
pw-record -P '{ node.dont-move = true }' --target otozane_microphone --rate 48000 --channels 2 --format s16 "$OUT/rec.wav" &
RC=$!; sleep 0.5
[ -n "${SHOW_LINKS:-}" ] && pw-link -l | grep -A2 "^otozane"
pw-play -P '{ node.dont-move = true }' --target oz_test_sink "$TONE"; sleep 0.5
kill $RC; sleep 0.3; kill $SV; sleep 0.5
echo "otozane nodes left after stop: $(pw-cli ls Node | grep -c otozane)"
kill $LP; wait 2>/dev/null
python3 "$DIR/analyze.py" "$OUT/rec.wav"
