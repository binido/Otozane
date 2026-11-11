#!/bin/bash
# End-to-end check without touching real devices:
#   tone -> temporary test source -> otozaned -> Otozane Microphone -> pw-record
# All PipeWire objects created here are session-only and removed on exit.
# Usage: scripts/e2e.sh [input.wav] [chain-json]   (env: BIN, OUT, SHOW_LINKS)
# The service runs with an isolated config directory and socket under $OUT,
# so it never talks to (or changes) a service you already have running.
set -u
DIR=$(cd "$(dirname "$0")" && pwd)
OUT=${OUT:-${XDG_RUNTIME_DIR:-/tmp}/otozane-e2e}
BIN=${BIN:-./target/debug/otozaned}
mkdir -p "$OUT"
export OTOZANE_SOCKET="$OUT/otozane.sock" OTOZANE_NODE_PREFIX=oz_test_
TONE=${1:-$OUT/tone.wav}
[ -f "$TONE" ] || python3 "$DIR/tone.py" "$TONE" 4 0.25
pw-loopback --capture-props='media.class=Audio/Sink node.name=oz_test_sink audio.position=[MONO] node.dont-move=true' \
  --playback-props='media.class=Audio/Source node.name=oz_test_src audio.position=[MONO] node.dont-move=true' >"$OUT/loop.log" 2>&1 &
LP=$!; sleep 1
XDG_CONFIG_HOME="$OUT/config" $BIN >"$OUT/svc.log" 2>&1 &
SV=$!; sleep 1
kill -0 $SV 2>/dev/null || { echo "otozaned failed to start:"; cat "$OUT/svc.log"; kill $LP; exit 1; }
python3 "$DIR/ipc.py" '{"cmd":"set_input","device":"oz_test_src"}' ${2:+"{\"cmd\":\"set_chain\",\"chain\":$2}"} >/dev/null
sleep 0.5
pw-record -P '{ node.dont-move = true }' --target oz_test_otozane_microphone --rate 48000 --channels 2 --format s16 "$OUT/rec.wav" &
RC=$!; sleep 0.5
[ -n "${SHOW_LINKS:-}" ] && pw-link -l | grep -A2 "^oz_test_otozane"
pw-play -P '{ node.dont-move = true }' --target oz_test_sink "$TONE"; sleep 0.5
kill $RC; sleep 0.3; kill $SV; sleep 0.5
echo "otozane nodes left after stop: $(pw-cli ls Node | grep -c oz_test_otozane)"
kill $LP; wait 2>/dev/null
python3 "$DIR/analyze.py" "$OUT/rec.wav"
