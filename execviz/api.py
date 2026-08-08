# =========================================================================
#  MANIFEST
# =========================================================================
#  script_name: api.py
#  script_path: execviz/api.py
#  module_name: api
#  version: 0.53.1
#  description: Progressive summarisation
#  kind: module
#  spec: internal
#  internal_dependencies: 
#  external_dependencies: http, json, store, urllib
#  features: api
#  api_version: execvis-v1.0.0
#  last_updated: 2026-08-07
# =========================================================================

"""execviz.api; the machine-facing surface. Headless, no browser required.

Everything the renderer shows is derived from the store; this exposes the same
derivations as JSON so other programs can consume them.

HTTP:
  GET /api/health
  GET /api/spans                     raw spans (whole store)
  GET /api/view?lod=system|field|cluster|channel|span
        [&host=&cluster=&family=&span=]      progressive summarisation:
        each tier returns aggregates, not the tier below it, so a huge trace can
        be consumed a level at a time instead of all at once.
  GET /api/query?q=stale|errors|races|slowest|hotpaths|descendants|ancestors
        [&span=&limit=&min_overlap_ms=]
  GET /api/capture                   a replay capture (same JSON the UI saves)
  POST /api/diff  {"a": <capture>, "b": <capture>}   or ?a=file&b=file
  POST /api/ingest {"host_id":..., "spans":[...]}    remote node push (see node.py)

CLI (headless):
  python3 api.py run.db --view field
  python3 api.py run.db --query races --min-overlap-ms 5
  python3 api.py run.db --diff other.json
  python3 api.py run.db --serve 8900
"""
import json, sqlite3, sys, argparse, hashlib, os
from http.server import HTTPServer, BaseHTTPRequestHandler

COLS = ["span_id","trace_id","parent_span_id","links","name","kind","start","end",
        "status","lifecycle","origin","host_id","clock_source","domain",
        "attributes","events"]
STALE_SECONDS = 2.0     # running longer than this = stale-running

def famOf(kind):
    return ("io" if kind in ("io","external") else
            "wait" if kind == "wait" else
            "boundary" if kind == "queue" else
            "fault" if kind == "error" else "control")

def load(db_path):
    db = sqlite3.connect("file:%s?mode=ro" % db_path, uri=True, timeout=3.0)
    rows = db.execute("SELECT "+",".join(COLS)+" FROM spans ORDER BY start").fetchall()
    db.close()
    out = []
    for r in rows:
        d = dict(zip(COLS, r))
        for jf in ("links","lifecycle","attributes","events"):
            d[jf] = json.loads(d[jf])
        d["family"] = famOf(d["kind"])
        d["duration_ms"] = None if d["end"] is None else round((d["end"]-d["start"])*1000, 3)
        out.append(d)
    return out

def _now_ref(spans):
    ends = [s["end"] for s in spans if s["end"] is not None]
    starts = [s["start"] for s in spans]
    return max(ends + starts) if (ends or starts) else 0.0

def is_stale(s, ref):
    return s["end"] is None and (ref - s["start"]) > STALE_SECONDS

# =========================================================================
# PROGRESSIVE SUMMARISATION
# =========================================================================
def view(spans, lod="field", host=None, cluster=None, family=None, span_id=None):
    ref = _now_ref(spans)
    def agg(group):
        return {
            "spans": len(group),
            "errors": sum(1 for s in group if s["status"] == "error"),
            "running": sum(1 for s in group if s["end"] is None),
            "stale_running": sum(1 for s in group if is_stale(s, ref)),
            "total_ms": round(sum(s["duration_ms"] or 0 for s in group), 2),
            "kinds": _count(s["kind"] for s in group),
        }
    if lod == "system":
        hosts = {}
        for s in spans: hosts.setdefault(s["host_id"] or "local", []).append(s)
        return {"lod":"system","hosts":[dict(host=h, **agg(g)) for h,g in sorted(hosts.items())]}
    if lod == "field":
        pool = [s for s in spans if host is None or (s["host_id"] or "local") == host]
        cl = {}
        for s in pool: cl.setdefault(s["domain"] or "unknown", []).append(s)
        edges = _routes(pool)
        return {"lod":"field","host":host,
                "clusters":[dict(cluster=c, **agg(g)) for c,g in sorted(cl.items())],
                "routes":edges}
    if lod == "cluster":
        pool = [s for s in spans if (s["domain"] or "unknown") == cluster]
        fam = {}
        for s in pool: fam.setdefault(s["family"], []).append(s)
        return {"lod":"cluster","cluster":cluster,
                "families":[dict(family=f, **agg(g)) for f,g in sorted(fam.items())]}
    if lod == "channel":
        pool = [s for s in spans if (s["domain"] or "unknown") == cluster
                and (family is None or s["family"] == family)]
        pool.sort(key=lambda s: s["start"])
        return {"lod":"channel","cluster":cluster,"family":family,"rows":[
            {"span_id":s["span_id"],"name":s["name"],"kind":s["kind"],
             "status":s["status"],"duration_ms":s["duration_ms"],
             "stale":is_stale(s,ref),
             "lifecycle":[l["type"] for l in s["lifecycle"]],
             "links":s["links"]} for s in pool]}
    if lod == "span":
        for s in spans:
            if s["span_id"] == span_id:
                d = dict(s); d["stale"] = is_stale(s, ref)
                d["children"] = [x["span_id"] for x in spans if x["parent_span_id"] == span_id]
                return {"lod":"span","span":d}
        return {"lod":"span","span":None,"error":"not found"}
    return {"error":"unknown lod"}

def _count(it):
    c = {}
    for x in it: c[x] = c.get(x,0)+1
    return dict(sorted(c.items(), key=lambda kv:-kv[1]))

def _routes(spans):
    byid = {s["span_id"]: s for s in spans}
    r = {}
    for s in spans:
        p = byid.get(s["parent_span_id"])
        if not p: continue
        a, b = p["domain"] or "unknown", s["domain"] or "unknown"
        if a == b: continue
        k = a+">"+b
        e = r.setdefault(k, {"from":a,"to":b,"count":0,"errors":0,
                             "cross_host": (p["host_id"] or "local") != (s["host_id"] or "local")})
        e["count"] += 1
        if s["status"] == "error": e["errors"] += 1
    return sorted(r.values(), key=lambda e:-e["count"])

# =========================================================================
# QUERIES OVER THE TWO EDGE SETS
# =========================================================================
def query(spans, q="stale", span_id=None, limit=50, min_overlap_ms=1.0):
    ref = _now_ref(spans)
    byid = {s["span_id"]: s for s in spans}
    if q == "stale":
        hits = [s for s in spans if is_stale(s, ref)]
        return {"query":"stale","note":"running past threshold; an unfinished span",
                "count":len(hits),
                "results":[{"span_id":s["span_id"],"name":s["name"],"domain":s["domain"],
                            "host":s["host_id"],"kind":s["kind"],
                            "running_for_ms":round((ref-s["start"])*1000,1)} for s in hits[:limit]]}
    if q == "errors":
        hits = [s for s in spans if s["status"] == "error"]
        return {"query":"errors","count":len(hits),
                "results":[{"span_id":s["span_id"],"name":s["name"],"domain":s["domain"],
                            "attributes":s["attributes"]} for s in hits[:limit]]}
    if q == "races":
        # temporal overlap between CAUSAL SIBLINGS: the causal graph permits it,
        # the temporal graph shows it happened (the specification; two edge sets).
        sib = {}
        for s in spans: sib.setdefault(s["parent_span_id"], []).append(s)
        out = []
        for parent, group in sib.items():
            if parent is None or len(group) < 2: continue
            for i in range(len(group)):
                for j in range(i+1, len(group)):
                    a, b = group[i], group[j]
                    ae = a["end"] if a["end"] is not None else ref
                    be = b["end"] if b["end"] is not None else ref
                    ov = min(ae, be) - max(a["start"], b["start"])
                    if ov*1000 >= min_overlap_ms:
                        out.append({"parent":parent,"a":a["name"],"b":b["name"],
                                    "a_span":a["span_id"],"b_span":b["span_id"],
                                    "overlap_ms":round(ov*1000,2),
                                    "same_domain":a["domain"]==b["domain"]})
        out.sort(key=lambda x:-x["overlap_ms"])
        return {"query":"races","note":"causal siblings overlapping in time",
                "count":len(out),"results":out[:limit]}
    if q == "slowest":
        hits = sorted([s for s in spans if s["duration_ms"] is not None],
                      key=lambda s:-s["duration_ms"])
        return {"query":"slowest","results":[{"span_id":s["span_id"],"name":s["name"],
                "domain":s["domain"],"kind":s["kind"],"duration_ms":s["duration_ms"]}
                for s in hits[:limit]]}
    if q == "hotpaths":
        return {"query":"hotpaths","results":_routes(spans)[:limit]}
    if q in ("descendants","ancestors"):
        if q == "ancestors":
            out, cur = [], byid.get(span_id)
            while cur and cur["parent_span_id"]:
                cur = byid.get(cur["parent_span_id"])
                if not cur: break
                out.append({"span_id":cur["span_id"],"name":cur["name"],"domain":cur["domain"]})
            return {"query":"ancestors","of":span_id,"results":out[:limit]}
        kids = {}
        for s in spans: kids.setdefault(s["parent_span_id"], []).append(s)
        out, stack = [], list(kids.get(span_id, []))
        while stack and len(out) < limit:
            s = stack.pop(0)
            out.append({"span_id":s["span_id"],"name":s["name"],"domain":s["domain"],
                        "kind":s["kind"],"status":s["status"]})
            stack.extend(kids.get(s["span_id"], []))
        return {"query":"descendants","of":span_id,"results":out}
    return {"error":"unknown query"}

def capture(spans):
    return {"format":"execviz-replay/1","spans":spans,
            "clusters":sorted({(s["domain"] or "unknown") for s in spans}),
            "hosts":sorted({(s["host_id"] or "local") for s in spans})}

# =========================================================================
# DIFF TWO CAPTURES (AGENTIC LOOP: RUN, PATCH, RE-RUN)
# =========================================================================
def _sig(s):
    return (s["domain"] or "", s["name"], s["kind"])

def diff(a_spans, b_spans):
    def prof(spans):
        p = {}
        for s in spans:
            k = _sig(s)
            e = p.setdefault(k, {"count":0,"errors":0,"total_ms":0.0,"stale":0})
            e["count"] += 1
            if s["status"] == "error": e["errors"] += 1
            e["total_ms"] += s["duration_ms"] or 0
            if s["end"] is None: e["stale"] += 1
        return p
    A, B = prof(a_spans), prof(b_spans)
    keys = set(A) | set(B)
    added, removed, changed = [], [], []
    for k in sorted(keys):
        a, b = A.get(k), B.get(k)
        rec = {"domain":k[0],"name":k[1],"kind":k[2]}
        if a is None: added.append(dict(rec, **{"count":b["count"],"errors":b["errors"]}))
        elif b is None: removed.append(dict(rec, **{"count":a["count"],"errors":a["errors"]}))
        else:
            dc, de = b["count"]-a["count"], b["errors"]-a["errors"]
            dms = round(b["total_ms"]-a["total_ms"], 2)
            ds = b["stale"]-a["stale"]
            if dc or de or ds or abs(dms) > 1e-6:
                changed.append(dict(rec, count_delta=dc, error_delta=de,
                                    total_ms_delta=dms, stale_delta=ds))
    return {"summary":{"a_spans":len(a_spans),"b_spans":len(b_spans),
                       "a_errors":sum(1 for s in a_spans if s["status"]=="error"),
                       "b_errors":sum(1 for s in b_spans if s["status"]=="error"),
                       "a_stale":sum(1 for s in a_spans if s["end"] is None),
                       "b_stale":sum(1 for s in b_spans if s["end"] is None)},
            "added":added,"removed":removed,"changed":changed}

# =========================================================================
# HTTP
# =========================================================================
def make_handler(db_path, store_writer=None):
    class H(BaseHTTPRequestHandler):
        def log_message(self, *a): pass
        def _send(self, obj, code=200):
            body = json.dumps(obj, separators=(",",":")).encode()
            self.send_response(code)
            self.send_header("Content-Type","application/json")
            self.send_header("Access-Control-Allow-Origin","*")
            self.send_header("Content-Length",str(len(body)))
            self.end_headers(); self.wfile.write(body)
        def _qs(self):
            from urllib.parse import urlparse, parse_qs
            u = urlparse(self.path)
            return u.path, {k:v[0] for k,v in parse_qs(u.query).items()}
        def do_GET(self):
            path, q = self._qs()
            try:
                if path == "/api/health": return self._send({"ok":True,"db":db_path})
                spans = load(db_path)
                if path == "/api/spans":  return self._send({"spans":spans})
                if path == "/api/view":
                    return self._send(view(spans, q.get("lod","field"), q.get("host"),
                        q.get("cluster"), q.get("family"), q.get("span")))
                if path == "/api/query":
                    return self._send(query(spans, q.get("q","stale"), q.get("span"),
                        int(q.get("limit",50)), float(q.get("min_overlap_ms",1.0))))
                if path == "/api/capture": return self._send(capture(spans))
                self._send({"error":"not found"}, 404)
            except Exception as e:
                self._send({"error":str(e)}, 500)
        def do_POST(self):
            path, q = self._qs()
            n = int(self.headers.get("Content-Length") or 0)
            payload = json.loads(self.rfile.read(n) or b"{}")
            try:
                if path == "/api/diff":
                    a = payload.get("a",{}).get("spans", payload.get("a",[]))
                    b = payload.get("b",{}).get("spans", payload.get("b",[]))
                    return self._send(diff(a, b))
                if path == "/api/ingest":
                    if store_writer is None: return self._send({"error":"ingest disabled"},400)
                    k = store_writer(payload)
                    return self._send({"ok":True,"ingested":k})
                self._send({"error":"not found"},404)
            except Exception as e:
                self._send({"error":str(e)},500)
    return H

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("db"); ap.add_argument("--serve", type=int)
    ap.add_argument("--view"); ap.add_argument("--query"); ap.add_argument("--diff")
    ap.add_argument("--host"); ap.add_argument("--cluster"); ap.add_argument("--family")
    ap.add_argument("--span"); ap.add_argument("--limit", type=int, default=50)
    ap.add_argument("--min-overlap-ms", type=float, default=1.0)
    ap.add_argument("--capture", action="store_true")
    ap.add_argument("--collect", action="store_true", help="accept POST /api/ingest from remote nodes")
    a = ap.parse_args()
    if a.serve:
        writer = None
        if a.collect:
            from store import Store
            st = Store(a.db)
            def writer(payload):
                spans = payload.get("spans", [])
                hid = payload.get("host_id", "remote")
                for sp in spans:
                    sp = dict(sp); sp["host_id"] = hid
                    st.begin(sp)
                    if sp.get("end") is not None:
                        st.finish(sp["span_id"], sp["end"], sp.get("status","ok"))
                    if sp.get("lifecycle"):
                        for l in sp["lifecycle"]:
                            st.add_lifecycle(sp["span_id"], l["type"], l.get("context"))
                    if sp.get("links"): st.add_links(sp["span_id"], sp["links"])
                return len(spans)
        srv = HTTPServer(("0.0.0.0", a.serve), make_handler(a.db, writer))
        print("execviz api on :%d" % a.serve, flush=True); srv.serve_forever(); return
    spans = load(a.db)
    if a.view:    out = view(spans, a.view, a.host, a.cluster, a.family, a.span)
    elif a.query: out = query(spans, a.query, a.span, a.limit, a.min_overlap_ms)
    elif a.diff:  out = diff(json.load(open(a.diff)).get("spans",[]), spans)
    elif a.capture: out = capture(spans)
    else: out = view(spans, "system")
    print(json.dumps(out, indent=1))

if __name__ == "__main__":
    main()
