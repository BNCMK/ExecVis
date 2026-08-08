# =========================================================================
#  MANIFEST
# =========================================================================
#  script_name: workload.py
#  script_path: execviz/workload.py
#  module_name: workload
#  version: 0.53.1
#  description: Worker consuming a queue (context propagation across threads)
#  kind: module
#  spec: internal
#  internal_dependencies: 
#  external_dependencies: capture, sys
#  features: workload
#  api_version: execvis-v1.0.0
#  last_updated: 2026-08-07
# =========================================================================

"""A real program, traced. Simulates a small service handling requests:
gateway → auth → order logic → cache/db (file IO), a worker consuming a
queue, a hot loop, one failing request, and one function that HANGS
(never completes); the stale-running death signal, captured for real."""
import sys, time, threading, queue, tempfile, os
import os
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import capture

store = capture.install(os.environ.get("EXECVIZ_DB", "run.db"))

DB_FILE = tempfile.NamedTemporaryFile(delete=False, mode="w+")

def db_write(txt):                      # real file IO → io spans from c_calls
    DB_FILE.write(txt); DB_FILE.flush()

def db_read():
    DB_FILE.seek(0); return DB_FILE.read()

def check_token(tok):
    time.sleep(0.03)                    # wait span w/ suspended/resumed
    return tok.startswith("ok")

def authorize(tok):
    capture.set_domain("auth")
    good = check_token(tok)
    capture.set_domain("gateway")
    if not good: raise PermissionError("bad token")
    return True

def load_order(oid):
    capture.set_domain("orders")
    db_write(f"order {oid}\n")
    time.sleep(0.02)
    return db_read()

def checksum(data):                     # hot loop → aggregation
    total = 0
    for ch in data[:400]:
        total = crunch(total, ch)
    return total

def crunch(acc, ch):                    # called ~400× from same parent
    return (acc * 31 + ord(ch)) % 100003

def render(order):
    capture.set_domain("orders")
    cs = checksum(order * 20)
    time.sleep(0.01)
    return f"<order checksum={cs}>"

def handle_request(oid, tok):
    capture.set_domain("gateway")
    authorize(tok)
    order = load_order(oid)
    return render(order)

# =========================================================================
# WORKER CONSUMING A QUEUE (CONTEXT PROPAGATION ACROSS THREADS)
# =========================================================================
jobs = queue.Queue()
def worker():
    capture.set_domain("worker")
    item, qspan = capture.q_get(jobs)
    sid = capture.span_start("process_job", "call")
    time.sleep(0.04)
    db_write(f"job {item}\n")
    capture.span_end(sid, "ok")
    capture.q_done(qspan)

def hang_forever():                     # an unfinished span: never completes
    capture.set_domain("billing")
    sid = capture.span_start("acquire_lock", "wait")
    capture.span_lifecycle(sid, "suspended")
    time.sleep(3600)                    # daemon thread; process exits first

def main():
    capture.set_trace()
    capture.set_domain("gateway")

    root = capture.span_start("service_run", "call")

    for i in range(3):                  # three good requests
        sid = capture.span_start(f"GET /order/{100+i}", "call")
        handle_request(100 + i, "ok-token")
        capture.span_end(sid, "ok")

    t = threading.Thread(target=worker, name="worker-1")
    t.start()
    capture.q_put(jobs, "invoice-7", name="enqueue_job")
    t.join()

    bad = capture.span_start("GET /order/999", "call")   # failing request
    try:
        handle_request(999, "bad-token")
        capture.span_end(bad, "ok")
    except PermissionError:
        err = capture.span_start("permission_denied", "error")
        capture.span_end(err, "error")
        capture.span_end(bad, "error")

    threading.Thread(target=hang_forever, name="billing-1", daemon=True).start()
    time.sleep(0.05)                    # let the hang start its span

    capture.span_end(root, "ok")

main()
capture.uninstall()
d = store.dump()
running = [s for s in d if s["status"] == "running"]
errors  = [s for s in d if s["status"] == "error"]
loops   = [s for s in d if s["kind"] == "loop"]
waits   = [s for s in d if s["kind"] == "wait"]
ios     = [s for s in d if s["kind"] == "io"]
queues  = [s for s in d if s["kind"] == "queue"]
print(f"captured {len(d)} spans | running(stale)={len(running)} "
      f"errors={len(errors)} loops={len(loops)} waits={len(waits)} "
      f"io={len(ios)} queue={len(queues)}")
for s in running: print("  STALE-RUNNING:", s["name"], "in", s["domain"])
for s in loops:   print("  LOOP:", s["name"], s["attributes"])
for s in queues:  print("  QUEUE:", s["name"], [l["type"] for l in s["lifecycle"]])
