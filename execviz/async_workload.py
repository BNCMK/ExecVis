# =========================================================================
#  MANIFEST
# =========================================================================
#  script_name: async_workload.py
#  script_path: execviz/async_workload.py
#  module_name: async_workload
#  version: 0.53.1
#  description: a gather is a fan-in: the join records both children in links
#  kind: module
#  spec: internal
#  internal_dependencies: 
#  external_dependencies: capture, sys
#  features: async workload
#  api_version: execvis-v1.0.0
#  last_updated: 2026-08-07
# =========================================================================

"""A real asyncio program, traced. Proves the parent chain survives awaits:
concurrent tasks interleave, and each span must still nest under the task that
created it rather than under whatever frame happened to be on top."""
import sys, os, asyncio, tempfile
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import capture

store = capture.install(os.environ.get("EXECVIZ_DB", "async.db"))
F = tempfile.NamedTemporaryFile(delete=False, mode="w+")

async def fetch_user(uid):
    capture.set_domain("users")
    async with capture.async_span("fetch_user_%d" % uid, "call"):
        await capture.await_span(asyncio.sleep(0.05 + uid * 0.02), "db_user", "io")
        return {"id": uid}

async def fetch_orders(uid):
    capture.set_domain("orders")
    async with capture.async_span("fetch_orders_%d" % uid, "call"):
        await capture.await_span(asyncio.sleep(0.08), "db_orders", "io")
        return [1, 2, 3]

async def render(uid):
    capture.set_domain("render")
    async with capture.async_span("render_%d" % uid, "call"):
        F.write("u%d\n" % uid); F.flush()
        await capture.await_span(asyncio.sleep(0.02), "template", "wait")

async def handle(uid):
    capture.set_domain("api")
    async with capture.async_span("GET /profile/%d" % uid, "call"):
        # a gather is a fan-in: the join records both children in links
        await capture.gather_span("profile_fanin", fetch_user(uid), fetch_orders(uid))
        await render(uid)

async def watchdog():
    capture.set_domain("watchdog")
    sid = capture.span_start("never_returns", "wait")
    capture.span_lifecycle(sid, "suspended")
    await asyncio.sleep(3600)          # cancelled at exit: stays stale-running

async def main():
    capture.set_trace(); capture.set_domain("api")
    async with capture.async_span("service", "call"):
        w = capture.spawn(watchdog(), "watchdog")
        # three concurrent requests: their spans interleave in time but must not
        # interleave in the causal tree
        await asyncio.gather(*[handle(i) for i in range(3)])
        w.cancel()

asyncio.run(main())
capture.uninstall()

d = store.dump()
byid = {s["span_id"]: s for s in d}
def parent_name(s):
    p = byid.get(s["parent_span_id"])
    return p["name"] if p else None

print("spans:", len(d))
# the correctness check: every span created inside a request must trace back to
# that request, not to a sibling request that happened to be running
bad = 0
for s in d:
    if s["name"].startswith("fetch_user_") or s["name"].startswith("fetch_orders_"):
        uid = s["name"].rsplit("_", 1)[1]
        chain, cur, depth = [], s, 0
        while cur and depth < 12:
            chain.append(cur["name"]); cur = byid.get(cur["parent_span_id"]); depth += 1
        root_req = [c for c in chain if c.startswith("GET /profile/")]
        ok = root_req and root_req[0].endswith("/" + uid)
        if not ok: bad += 1
        print(("  OK  " if ok else "  WRONG ") + s["name"] + "  <- " + " <- ".join(chain[1:4]))
joins = [s for s in d if s["links"]]
stale = [s for s in d if s["end"] is None]
print("fan-in joins with links:", [(s["name"], len(s["links"])) for s in joins])
print("stale-running:", [s["name"] for s in stale])
print("MISATTRIBUTED PARENTS:", bad)
