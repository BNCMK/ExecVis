# =========================================================================
#  MANIFEST
# =========================================================================
#  script_name: measure.py
#  script_path: execviz/measure.py
#  module_name: measure
#  version: 0.53.1
#  description: Loop-detection accuracy
#  kind: module
#  spec: internal
#  internal_dependencies: 
#  external_dependencies: capture, importlib, sqlite3, sys
#  features: measure, detect
#  api_version: execvis-v1.0.0
#  last_updated: 2026-08-07
# =========================================================================

"""Measurements the spec flagged as decide-by-measuring rather than by argument:
loop-detection accuracy and name stability across a refactor."""
import importlib, json, os, subprocess, sys, tempfile, textwrap
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

HERE = os.path.dirname(os.path.abspath(__file__))

# =========================================================================
# LOOP-DETECTION ACCURACY
# =========================================================================
LOOP_PROG = '''
import sys, os
sys.path.insert(0, {here!r})
import capture
store = capture.install(os.environ["EXECVIZ_DB"])
def unit(x):        # called a known number of times
    return x * 2
def run(n):
    t = 0
    for i in range(n):
        t += unit(i)
    return t
capture.set_trace()
root = capture.span_start("run", "call")
for n in {sizes!r}:
    run(n)
capture.span_end(root, "ok")
capture.uninstall()
'''

def loop_accuracy(sizes):
    db = tempfile.mktemp(suffix=".db")
    src = tempfile.mktemp(suffix=".py")
    open(src, "w").write(LOOP_PROG.format(here=HERE, sizes=sizes))
    env = dict(os.environ, EXECVIZ_DB=db)
    subprocess.run([sys.executable, src], env=env, capture_output=True, timeout=180)
    import sqlite3
    rows = sqlite3.connect(db).execute(
        "SELECT name,kind,attributes FROM spans WHERE kind='loop' OR name='unit'").fetchall()
    aggregated = [json.loads(a).get("iterations", 0) for n, k, a in rows if k == "loop"]
    individual = sum(1 for n, k, a in rows if k != "loop")
    return {"expected_calls": sum(sizes), "aggregated_spans": len(aggregated),
            "counted_by_aggregates": sum(aggregated), "recorded_individually": individual,
            "total_accounted": sum(aggregated) + individual}

# =========================================================================
# NAME STABILITY ACROSS A REFACTOR
# =========================================================================
BEFORE = '''
def fetch_user(uid):    return uid
def fetch_orders(uid):  return [uid]
def render(uid):        return str(uid)
def handle(uid):
    fetch_user(uid); fetch_orders(uid); return render(uid)
'''
AFTER_RENAME = BEFORE.replace("fetch_orders", "load_orders")          # one rename
AFTER_MOVE = BEFORE                                                   # same names, new file

RUNNER = '''
import sys, os
sys.path.insert(0, {here!r}); sys.path.insert(0, {tmp!r})
import capture
store = capture.install(os.environ["EXECVIZ_DB"])
import {mod} as m
capture.set_trace()
root = capture.span_start("run", "call")
for i in range(3): m.handle(i)
capture.span_end(root, "ok")
capture.uninstall()
'''

def signatures(db):
    import sqlite3
    rows = sqlite3.connect(db).execute("SELECT domain,name,kind FROM spans").fetchall()
    return {(d, n, k) for d, n, k in rows}

def name_stability():
    tmp = tempfile.mkdtemp()
    out = {}
    for label, body, mod in (("baseline", BEFORE, "svc_a"),
                             ("renamed_one_function", AFTER_RENAME, "svc_a"),
                             ("moved_to_new_module", AFTER_MOVE, "svc_b")):
        open(os.path.join(tmp, mod + ".py"), "w").write(body)
        db = tempfile.mktemp(suffix=".db")
        run = tempfile.mktemp(suffix=".py")
        open(run, "w").write(RUNNER.format(here=HERE, tmp=tmp, mod=mod))
        subprocess.run([sys.executable, run], env=dict(os.environ, EXECVIZ_DB=db),
                       capture_output=True, timeout=120)
        out[label] = signatures(db)
    base = out["baseline"]
    res = {}
    for label in ("renamed_one_function", "moved_to_new_module"):
        kept = base & out[label]
        res[label] = {"baseline": len(base), "after": len(out[label]),
                      "preserved": len(kept),
                      "pct": round(100.0 * len(kept) / max(1, len(base)), 1),
                      "lost": sorted(f"{d}/{n}" for d, n, k in (base - out[label]))[:6]}
    return res

if __name__ == "__main__":
    print("== loop-detection accuracy ==")
    for sizes in ([10], [50], [400], [10, 50, 400]):
        print(f"  loops {sizes}: {loop_accuracy(sizes)}")
    print("== name stability across a refactor ==")
    for k, v in name_stability().items():
        print(f"  {k}: {v}")
