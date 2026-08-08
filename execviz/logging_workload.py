# =========================================================================
#  MANIFEST
# =========================================================================
#  script_name: logging_workload.py
#  script_path: execviz/logging_workload.py
#  module_name: logging_workload
#  version: 0.53.1
#  description: logging workload.
#  kind: module
#  spec: internal
#  internal_dependencies: 
#  external_dependencies: capture, sys
#  features: logging workload
#  api_version: execvis-v1.0.0
#  last_updated: 2026-08-07
# =========================================================================

"""A program that logs normally. No execviz calls at the log sites: it uses the
standard logging module and print, and every line lands on the span that was
running when it was written."""
import sys, os, logging, time
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import capture

store = capture.install(os.environ.get("EXECVIZ_DB", "logs.db"))

log = logging.getLogger("shop")

def price_lookup(sku):
    capture.set_domain("pricing")
    log.info("looking up %s", sku)
    time.sleep(0.02)
    if sku == "BAD":
        log.error("no price for %s", sku)
        raise KeyError(sku)
    return 9.99

def checkout(sku):
    capture.set_domain("checkout")
    log.info("checkout started for %s", sku)
    price = price_lookup(sku)
    print(f"charged {price} for {sku}")     # plain print, also attributed
    log.warning("slow payment gateway")
    return price

capture.set_trace()
log.info("service booting")                # before any span: unattributed
root = capture.span_start("service", "call")
for sku in ("A1", "B2", "BAD"):
    sid = capture.span_start(f"order {sku}", "call")
    try:
        checkout(sku); capture.span_end(sid, "ok")
    except KeyError:
        log.error("order %s failed", sku)
        capture.span_end(sid, "error")
capture.span_end(root, "ok")
capture.uninstall()

d = store.dump()
with_events = [s for s in d if s["events"]]
print("--- attribution ---", file=sys.stderr)
for s in with_events[:8]:
    for e in s["events"]:
        print(f"  {s['name'][:22]:24} [{e['level']:7}] {e['msg'][:44]}", file=sys.stderr)
print(f"spans with logs: {len(with_events)} / {len(d)}", file=sys.stderr)
print(f"unattributed (no span active): {[u['msg'] for u in capture.unattributed()]}", file=sys.stderr)
