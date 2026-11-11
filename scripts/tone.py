"""Writes a 1 kHz sine: tone.py OUT.wav SECONDS AMPLITUDE."""
import math, struct, wave, sys
sr, secs, amp, freq = 48000, float(sys.argv[2]), float(sys.argv[3]), 1000.0
w = wave.open(sys.argv[1], "wb"); w.setnchannels(1); w.setsampwidth(2); w.setframerate(sr)
w.writeframes(b"".join(struct.pack("<h", int(amp*32767*math.sin(2*math.pi*freq*i/sr))) for i in range(int(sr*secs))))
