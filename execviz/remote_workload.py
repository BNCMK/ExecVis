# =========================================================================
#  MANIFEST
# =========================================================================
#  script_name: remote_workload.py
#  script_path: execviz/remote_workload.py
#  module_name: remote_workload
#  version: 0.53.1
#  description: remote workload.
#  kind: module
#  spec: internal
#  internal_dependencies: 
#  external_dependencies: capture, sys
#  features: remote workload
#  api_version: execvis-v1.0.0
#  last_updated: 2026-08-07
# =========================================================================

"""A traced program pretending to run on a second machine (edge device)."""
import sys, os, time, tempfile
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import capture
store = capture.install(os.environ.get("EXECVIZ_DB","node.db"))
F = tempfile.NamedTemporaryFile(delete=False, mode="w+")
def sensor_read(): capture.set_domain("sensors"); time.sleep(0.05); F.write("t\n"); F.flush()
def edge_infer():  capture.set_domain("inference"); time.sleep(0.08); return 42
def uplink(v):     capture.set_domain("uplink"); time.sleep(0.04); F.write(str(v)); F.flush()
capture.set_trace(); capture.set_domain("edge-agent")
root = capture.span_start("edge_loop","call")
for i in range(6):
    sid = capture.span_start("cycle_%d"%i,"call")
    sensor_read(); v = edge_infer(); uplink(v)
    capture.set_domain("edge-agent"); capture.span_end(sid,"ok")
capture.span_end(root,"ok")
print("remote workload done", flush=True)
