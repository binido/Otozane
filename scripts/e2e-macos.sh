#!/bin/bash
# End-to-end check on macOS: microphone -> otozaned -> BlackHole 2ch -> ffmpeg.
# Records BlackHole with effects off (expects the room noise of the microphone),
# then muted (expects digital silence). Apps writing into BlackHole break the second check.
# Usage: scripts/e2e-macos.sh   (env: BIN, OUT, INPUT = device UID, default automatic)
# The service runs with an isolated config directory and socket under $OUT.
set -u
DIR=$(cd "$(dirname "$0")" && pwd)
BIN=$(realpath "${BIN:-$DIR/../target/debug/otozaned}")
OUT=${OUT:-${TMPDIR:-/tmp}/otozane-e2e}
mkdir -p "$OUT" && cd "$OUT" || exit 1
# Relative socket path, unix socket paths are limited to 104 bytes.
export OTOZANE_SOCKET=otozane.sock
XDG_CONFIG_HOME="$OUT/config" "$BIN" >svc.log 2>&1 &
SV=$!; sleep 1
kill -0 $SV 2>/dev/null || { echo "otozaned failed to start:"; cat svc.log; exit 1; }
ipc() { python3 "$DIR/ipc.py" "$@" >/dev/null; }
ipc "{\"cmd\":\"set_input\",\"device\":${INPUT:+\"$INPUT\"}${INPUT:-null}}" '{"cmd":"set_processing","enabled":false}'
sleep 1.5
rec() { ffmpeg -hide_banner -loglevel error -y -f avfoundation -i ":BlackHole 2ch" -t "$1" -ar 48000 -c:a pcm_s16le "$2"; }
rec 2 on.wav
ipc '{"cmd":"set_muted","muted":true}'; sleep 0.5
rec 1 muted.wav
kill $SV; wait 2>/dev/null
python3 - on.wav muted.wav <<'PY'
import sys, wave, struct, math
def stats(p):
    w = wave.open(p); n = w.getnframes(); d = struct.unpack("<%dh" % (n * w.getnchannels()), w.readframes(n))
    d = d[len(d) // 4:]  # skip device start-up
    return max(map(abs, d)) / 32768, math.sqrt(sum(v * v for v in d) / len(d)) / 32768
on, muted = stats(sys.argv[1]), stats(sys.argv[2])
print("effects off: peak=%.4f rms=%.5f" % on); print("muted:       peak=%.4f rms=%.5f" % muted)
ok = on[1] > 1e-5 and muted[0] == 0
print("PASS" if ok else "FAIL"); sys.exit(0 if ok else 1)
PY
