"""Sends JSON requests (one per argument) to otozaned and prints the responses."""
import json, os, socket, sys
import tempfile
# Same lookup as protocol::socket_path in otozane-core.
path = os.environ.get("OTOZANE_SOCKET") or (
    os.path.join(os.environ["XDG_RUNTIME_DIR"], "otozane.sock") if "XDG_RUNTIME_DIR" in os.environ
    else os.path.join(tempfile.gettempdir(), "otozane-%d.sock" % os.getuid()))
s = socket.socket(socket.AF_UNIX); s.connect(path); f = s.makefile("rw")
for req in sys.argv[1:]:
    f.write(req + "\n"); f.flush()
    print(f.readline().strip())
