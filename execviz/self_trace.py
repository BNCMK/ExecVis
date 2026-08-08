# =========================================================================
#  MANIFEST
# =========================================================================
#  script_name: self_trace.py
#  script_path: execviz/self_trace.py
#  module_name: self_trace
#  version: 0.53.1
#  description: 1. the store, exercised under observation
#  kind: module
#  spec: internal
#  internal_dependencies: 
#  external_dependencies: capture, sys, time
#  features: self trace, store
#  api_version: execvis-v1.0.0
#  last_updated: 2026-08-07
# =========================================================================

"""execviz observing its own machinery.

The tool's Python side is a program like any other, so it can be traced by the
tool. This runs the store, the emitter, log attachment and a query against a
capture, all while under capture, so what fires when the debugger works is
visible in the debugger.
"""
import sys, os, logging, json
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import capture, store as store_mod

OUT = os.environ.get("EXECVIZ_DB", "self.db")
SUBJECT = os.environ.get("EXECVIZ_SUBJECT", "subject.db")

meta = capture.install(OUT)          # the outer capture, watching everything below
log = logging.getLogger("execviz.self")

capture.set_trace()
capture.set_domain("execviz.core")
root = capture.span_start("self_observation", "call")

# 1. the store, exercised under observation
capture.set_domain("execviz.store")
sid = capture.span_start("exercise_store", "call")
log.info("opening a subject store")
if os.path.exists(SUBJECT): os.remove(SUBJECT)
subject = store_mod.Store(SUBJECT)
import time
for i in range(4):
    s = {"span_id": f"x{i}", "trace_id": "sub", "name": f"unit_{i}", "kind": "call",
         "start": time.time(), "domain": "subject"}
    subject.begin(s)
    if i != 3:                        # one left open on purpose
        subject.finish(f"x{i}", time.time(), "ok")
log.info("wrote 4 spans, left 1 open as a death signal")
capture.span_end(sid, "ok")

# 2. the emitter's own two-phase behaviour, observed
capture.set_domain("execviz.emitter")
sid = capture.span_start("exercise_emitter", "call")
inner = capture.span_start("nested_unit", "call")
log.info("a nested span is open while this line is written")
capture.span_end(inner, "ok")
log.info("and closed again")
capture.span_end(sid, "ok")

# 3. reading a capture back, observed
capture.set_domain("execviz.read")
sid = capture.span_start("read_back", "io")
rows = subject.dump()
open_spans = [r for r in rows if r["end"] is None]
log.warning("subject store has %d spans, %d still running", len(rows), len(open_spans))
capture.span_end(sid, "ok")

capture.span_end(root, "ok")
capture.uninstall()

d = meta.dump()
logged = sum(len(s["events"]) for s in d)
print(f"self-trace: {len(d)} spans of execviz's own execution, {logged} log lines attached")
