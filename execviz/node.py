# =========================================================================
#  MANIFEST
# =========================================================================
#  script_name: node.py
#  script_path: execviz/node.py
#  module_name: node
#  version: 0.53.1
#  description: on the collector (machine A)
#  kind: module
#  spec: internal
#  internal_dependencies: 
#  external_dependencies: argparse, urllib
#  features: node
#  api_version: execvis-v1.0.0
#  last_updated: 2026-08-07
# =========================================================================

"""execviz.node; a capture node for a SEPARATE device.

Install alongside the program you want traced on machine B, point it at the
collector on machine A, and its spans fill in the same graph; appearing as
another host at the system tier. Same schema, same rules; only host_id differs.

    # on the collector (machine A)
    python3 api.py run.db --serve 8900 --collect

    # on the remote device (machine B)
    python3 node.py --collector http://A:8900 --host-id edge-1 -- python3 myapp.py

Spans are batched and flushed on an interval; a flush failure is retried on the
next tick, so a network blip loses nothing but ordering.
"""
import argparse, json, os, subprocess, sys, threading, time, sqlite3
import urllib.request

COLS = ["span_id","trace_id","parent_span_id","links","name","kind","start","end",
        "status","lifecycle","origin","host_id","clock_source","domain",
        "attributes","events"]

def read_new(db, sent):
    rows = db.execute("SELECT "+",".join(COLS)+" FROM spans").fetchall()
    out = []
    for r in rows:
        d = dict(zip(COLS, r))
        for jf in ("links","lifecycle","attributes","events"):
            d[jf] = json.loads(d[jf])
        # resend a span whose second phase landed after we first shipped it
        key = (d["span_id"], d["end"] is not None, d["status"])
        if key in sent: continue
        sent.add(key); out.append(d)
    return out

def push(collector, host_id, spans):
    body = json.dumps({"host_id": host_id, "spans": spans}).encode()
    req = urllib.request.Request(collector.rstrip("/")+"/api/ingest", data=body,
                                 headers={"Content-Type":"application/json"})
    with urllib.request.urlopen(req, timeout=8) as r:
        return json.loads(r.read())

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--collector", required=True)
    ap.add_argument("--host-id", default=os.uname().nodename)
    ap.add_argument("--db", default="node.db")
    ap.add_argument("--interval", type=float, default=1.0)
    ap.add_argument("cmd", nargs=argparse.REMAINDER,
                    help="-- <command to trace>  (omit to just forward an existing db)")
    a = ap.parse_args()

    cmd = a.cmd[1:] if a.cmd and a.cmd[0] == "--" else a.cmd
    proc = None
    if cmd:
        env = dict(os.environ, EXECVIZ_DB=a.db)
        proc = subprocess.Popen(cmd, env=env)

    sent, stop = set(), False
    print("[node %s] forwarding %s -> %s" % (a.host_id, a.db, a.collector), flush=True)
    total = 0
    while True:
        time.sleep(a.interval)
        try:
            db = sqlite3.connect("file:%s?mode=ro" % a.db, uri=True, timeout=2.0)
            batch = read_new(db, sent); db.close()
            if batch:
                push(a.collector, a.host_id, batch); total += len(batch)
                print("[node %s] +%d (%d total)" % (a.host_id, len(batch), total), flush=True)
        except Exception as e:
            print("[node %s] retry: %s" % (a.host_id, e), flush=True)
        if proc is not None and proc.poll() is not None:
            time.sleep(a.interval)          # final flush window
            try:
                db = sqlite3.connect("file:%s?mode=ro" % a.db, uri=True, timeout=2.0)
                batch = read_new(db, sent); db.close()
                if batch: push(a.collector, a.host_id, batch); total += len(batch)
            except Exception as e: print("[node] final flush failed:", e, flush=True)
            print("[node %s] done, %d spans forwarded" % (a.host_id, total), flush=True)
            return

if __name__ == "__main__":
    main()
