# =========================================================================
#  MANIFEST
# =========================================================================
#  script_name: slow_workload.py
#  script_path: execviz/slow_workload.py
#  module_name: slow_workload
#  version: 0.53.1
#  description: slow workload.
#  kind: module
#  spec: internal
#  internal_dependencies: 
#  external_dependencies: capture, sys
#  features: slow workload
#  api_version: execvis-v1.0.0
#  last_updated: 2026-08-07
# =========================================================================

"""Slow traced workload for LIVE mode: keeps emitting real work for ~35s
so the live renderer visibly updates. Hang starts early → stale visible mid-run."""
import sys, os, time, threading, queue, tempfile
import os
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import capture

store = capture.install(os.environ.get("EXECVIZ_DB", "run.db"))
F = tempfile.NamedTemporaryFile(delete=False, mode="w+")

def db_write(t): F.write(t); F.flush()
def check_token(tok): time.sleep(0.25); return tok.startswith("ok")
def authorize(tok):
    capture.set_domain("auth"); ok = check_token(tok)
    capture.set_domain("gateway")
    if not ok: raise PermissionError("bad token")
def load_order(oid):
    capture.set_domain("orders"); db_write(f"order {oid}\n"); time.sleep(0.2)
def crunch(a, ch): return (a*31+ord(ch)) % 100003
def checksum(d):
    t = 0
    for ch in d[:300]: t = crunch(t, ch)
    return t
def render(oid):
    capture.set_domain("orders"); checksum("x"*300*1); time.sleep(0.15)
def handle(oid, tok):
    capture.set_domain("gateway"); authorize(tok); load_order(oid); render(oid)

def hang():
    capture.set_domain("billing")
    sid = capture.span_start("acquire_lock", "wait")
    capture.span_lifecycle(sid, "suspended")
    time.sleep(3600)

jobs = queue.Queue()
def worker():
    capture.set_domain("worker")
    while True:
        item, qs = capture.q_get(jobs)
        if item is None: capture.q_done(qs); return
        sid = capture.span_start("process_job", "call")
        time.sleep(0.3); db_write(f"job {item}\n")
        capture.span_end(sid, "ok"); capture.q_done(qs)

capture.set_trace(); capture.set_domain("gateway")
root = capture.span_start("service_run", "call")
threading.Thread(target=hang, name="billing-1", daemon=True).start()
w = threading.Thread(target=worker, name="worker-1"); w.start()

for i in range(12):                       # ~35s of live activity
    sid = capture.span_start(f"GET /order/{100+i}", "call")
    tok = "ok-token" if i % 5 != 4 else "bad-token"
    try:
        handle(100+i, tok); capture.span_end(sid, "ok")
    except PermissionError:
        e = capture.span_start("permission_denied", "error")
        capture.span_end(e, "error"); capture.span_end(sid, "error")
    if i % 3 == 0: capture.q_put(jobs, f"invoice-{i}", name="enqueue_job")
    time.sleep(1.6)

capture.q_put(jobs, None, name="shutdown"); w.join()
capture.span_end(root, "ok")
print("slow workload done", flush=True)
