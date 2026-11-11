"""Prints peak and RMS per 100 ms block of a 16-bit WAV file (first channel)."""
import wave, struct, sys, math
w = wave.open(sys.argv[1]); n = w.getnframes(); ch = w.getnchannels()
d = struct.unpack("<%dh" % (n*ch), w.readframes(n))
x = [v/32768 for v in d[::ch]]
blk = 4800
for i in range(0, len(x)-blk+1, blk):
    seg = x[i:i+blk]; print("t=%.1fs peak=%.3f rms=%.4f" % (i/48000, max(map(abs,seg)), math.sqrt(sum(v*v for v in seg)/blk)))
