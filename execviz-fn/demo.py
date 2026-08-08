# =========================================================================
#  MANIFEST
# =========================================================================
#  script_name: demo.py
#  script_path: execviz-fn/demo.py
#  module_name: demo
#  version: 0.53.1
#  description: demo.
#  kind: module
#  spec: internal
#  internal_dependencies: 
#  external_dependencies: capture, execviz_fn, os
#  features: demo
#  api_version: execvis-v1.0.0
#  last_updated: 2026-08-07
# =========================================================================

"""A function invoked several times in one sandbox, with a freeze between two."""
import os, sys, time
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import execviz_fn
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "execviz"))
import capture

execviz_fn.install(os.environ.get("EXECVIZ_COLLECTOR"), domain="orders")


@execviz_fn.handler(name="POST /orders")
def create_order(event, context):
    sid = capture.span_start("validate", "call")
    time.sleep(0.01)
    capture.span_end(sid, "ok")
    q = capture.span_start("db_insert", "io")
    time.sleep(0.02)
    capture.span_end(q, "ok")
    if event.get("sku") == "BAD":
        raise ValueError("unknown sku")
    return {"ok": True}


class Ctx:
    aws_request_id = "req-1"


if __name__ == "__main__":
    create_order({"sku": "A1"}, Ctx())          # cold start
    create_order({"sku": "A2"}, Ctx())          # warm
    time.sleep(0.4)                              # the sandbox is frozen here
    create_order({"sku": "A3"}, Ctx())          # resumed after a freeze
    try:
        create_order({"sku": "BAD"}, Ctx())     # and one that fails
    except ValueError:
        pass
    print("function demo complete")
