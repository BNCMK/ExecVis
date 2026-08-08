# =========================================================================
#  MANIFEST
# =========================================================================
#  script_name: sysdemo_push.py
#  script_path: execviz/sysdemo_push.py
#  module_name: sysdemo_push
#  version: 0.53.1
#  description: sysdemo push.
#  kind: module
#  spec: internal
#  internal_dependencies: 
#  external_dependencies: capture, sys
#  features: sysdemo push
#  api_version: execvis-v1.0.0
#  last_updated: 2026-08-07
# =========================================================================

"""The same demo as sysdemo.py, but the recorder writes nothing to disk here.
Run under a syscall capture and the only writes attributed to the program are
the program's own."""
import sys, os, time, tempfile
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import capture
store = capture.install_push(os.environ.get("EXECVIZ_COLLECTOR","http://127.0.0.1:9200"),
                             host_id="push-1")
F = tempfile.NamedTemporaryFile(delete=False, mode="w+")
capture.set_trace(); capture.set_domain("app")
time.sleep(0.5)
root = capture.span_start("batch","call")
for i in range(3):
    sid = capture.span_start("write_chunk_%d"%i,"io")
    for j in range(8):
        F.write("x"*64+"\n"); F.flush(); os.fsync(F.fileno())
    capture.span_end(sid,"ok")
capture.span_end(root,"ok")
capture.uninstall()
print("push demo done", flush=True)
