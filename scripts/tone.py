"""Writes a 1 kHz sine (or white noise with a 4th arg "noise"): tone.py OUT.wav SECONDS AMPLITUDE [noise]."""
import math, struct, wave, sys
sr, secs, amp, freq = 48000, float(sys.argv[2]), float(sys.argv[3]), 1000.0
w = wave.open(sys.argv[1], "wb"); w.setnchannels(1); w.setsampwidth(2); w.setframerate(sr)
import random
noise = len(sys.argv) > 4 and sys.argv[4] == "noise"
sample = (lambda i: random.uniform(-1, 1)) if noise else (lambda i: math.sin(2*math.pi*freq*i/sr))
w.writeframes(b"".join(struct.pack("<h", int(amp*32767*sample(i))) for i in range(int(sr*secs))))
