# =========================================================================
#  MANIFEST
# =========================================================================
#  script_name: sysdemo.py
#  script_path: execviz/sysdemo.py
#  module_name: sysdemo
#  version: 0.53.1
#  description: sysdemo.
#  kind: module
#  spec: internal
#  internal_dependencies: 
#  external_dependencies: capture, sys
#  features: sysdemo
#  api_version: execvis-v1.0.0
#  last_updated: 2026-08-07
# =========================================================================

import sys, os, time, tempfile
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import capture
store = capture.install(os.environ.get("EXECVIZ_DB","sysdemo.db"))
F = tempfile.NamedTemporaryFile(delete=False, mode="w+")
capture.set_trace(); capture.set_domain("app")
time.sleep(0.5)                                   # let the collector attach
root = capture.span_start("batch", "call")
for i in range(3):
    sid = capture.span_start("write_chunk_%d" % i, "io")
    for j in range(8):
        F.write("x"*64+"\n"); F.flush(); os.fsync(F.fileno())
    capture.span_end(sid, "ok")
idle = capture.span_start("think", "call"); time.sleep(0.05); capture.span_end(idle,"ok")
capture.span_end(root, "ok")
capture.uninstall()
print("sysdemo spans:", len(store.dump()), flush=True)
