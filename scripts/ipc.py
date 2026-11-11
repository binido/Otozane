"""Sends JSON requests (one per argument) to otozaned and prints the responses."""
import json, os, socket, sys
path = os.path.join(os.environ.get("XDG_RUNTIME_DIR", "/tmp"), "otozane.sock")
s = socket.socket(socket.AF_UNIX); s.connect(path); f = s.makefile("rw")
for req in sys.argv[1:]:
    f.write(req + "\n"); f.flush()
    print(f.readline().strip())
