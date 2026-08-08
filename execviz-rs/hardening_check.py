# =========================================================================
#  MANIFEST
# =========================================================================
#  script_name: hardening_check.py
#  script_path: execviz-rs/hardening_check.py
#  module_name: hardening_check
#  version: 0.53.1
#  description: a body larger than the limit must be refused before it is allocated
#  kind: module
#  spec: internal
#  internal_dependencies: 
#  external_dependencies: json, socket, sys, urllib
#  features: hardening check
#  api_version: execvis-v1.0.0
#  last_updated: 2026-08-07
# =========================================================================

"""Checks the refusals the store must make (hardening pass).

Every case here was accepted before validation was added at ingest, and each one
produces a capture that quietly disagrees with itself rather than failing loudly.
"""
import json
import socket
import sys
import urllib.error
import urllib.request
from urllib.parse import urlparse

GREEN = "\033[32mPASS\033[0m"
coll = sys.argv[1]

bad = [
    {"span_id": "v1", "trace_id": "t", "name": "backwards", "kind": "call", "start": 9.0, "end": 8.0},
    {"span_id": "v2", "trace_id": "t", "name": "", "kind": "call", "start": 1.0, "end": 2.0},
    {"span_id": "v3", "parent_span_id": "v3", "trace_id": "t", "name": "self", "kind": "call",
     "start": 1.0, "end": 2.0},
    {"trace_id": "t", "name": "no id", "kind": "call", "start": 1.0, "end": 2.0},
    {"span_id": "v5", "trace_id": "t", "name": "fine", "kind": "call", "start": 1.0, "end": 2.0},
]
req = urllib.request.Request(
    coll + "/api/ingest",
    data=json.dumps({"host_id": "hardening", "spans": bad}).encode(),
    headers={"Content-Type": "application/json"})
d = json.loads(urllib.request.urlopen(req).read().decode())
assert d["ingested"] == 1, d
assert d["rejected"] == 4, d
assert d.get("reasons"), "a refusal must say why, or an adapter cannot be fixed"
print("  %s 4 malformed spans refused with reasons, 1 good span stored" % GREEN)

# a body larger than the limit must be refused before it is allocated
u = urlparse(coll)
s = socket.create_connection((u.hostname, u.port), timeout=8)
s.sendall(b"POST /api/ingest HTTP/1.1\r\nHost: x\r\n"
          b"Content-Length: 10000000000\r\nConnection: close\r\n\r\n")
line = s.recv(64).decode(errors="replace").split("\r\n")[0]
s.close()
assert "413" in line, line
print("  %s a 10GB body claim is refused before allocation" % GREEN)

# a number that overflows to infinity is refused: SQLite would store it as NULL,
# turning a timestamp into an absent value
try:
    urllib.request.urlopen(urllib.request.Request(
        coll + "/api/ingest",
        data=b'{"spans":[{"span_id":"inf","name":"n","kind":"call","start":1e400}]}',
        headers={"Content-Type": "application/json"}))
    raise SystemExit("FAIL: a non-finite timestamp was accepted")
except urllib.error.HTTPError as e:
    assert e.code == 400, e.code
    print("  %s a non-finite timestamp is refused rather than stored as absent" % GREEN)
