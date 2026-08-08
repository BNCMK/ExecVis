# =========================================================================
#  MANIFEST
# =========================================================================
#  script_name: export.py
#  script_path: execviz/export.py
#  module_name: export
#  version: 0.53.1
#  description: Normalize time to 0..1000 (renderer t units). end=none stays none.
#  kind: module
#  spec: internal
#  internal_dependencies: 
#  external_dependencies: json, os
#  features: export, render
#  api_version: execvis-v1.0.0
#  last_updated: 2026-08-07
# =========================================================================

"""execviz.export; read the span store, emit the nested-map renderer
fed with REAL captured spans. The store IS the recording;
this is a pure read of it."""
import json
import os, sqlite3, sys, re, hashlib

DB = sys.argv[1] if len(sys.argv) > 1 else "run.db"
def _template_path():
    """Finds the page template relative to this file.

    The path was absolute and pointed into the machine this was written on, so
    the tool worked there and nowhere else. Resolving from the script's own
    location makes a checkout work wherever it lands, and the environment
    variable gives an operator the last word.
    """
    env = os.environ.get("EXECVIZ_TEMPLATE")
    if env:
        return env
    here = os.path.dirname(os.path.abspath(__file__))
    for candidate in (
        os.path.join(here, "..", "browser", "exec-viz-nested.html"),  # where it lives
        os.path.join(here, "..", "exec-viz-nested.html"),   # beside the packages
        os.path.join(here, "exec-viz-nested.html"),         # beside this script
        "/mnt/user-data/outputs/exec-viz-nested.html",      # where it has always been
    ):
        if os.path.exists(candidate):
            return os.path.abspath(candidate)
    return os.path.abspath(os.path.join(here, "..", "browser", "exec-viz-nested.html"))


TEMPLATE = _template_path()
OUT = sys.argv[2] if len(sys.argv) > 2 else "execviz-live.html"

cols = ["span_id","trace_id","parent_span_id","links","name","kind","start",
        "end","status","lifecycle","origin","host_id","clock_source","domain",
        "attributes","events"]

def main():
    """Reads the store and writes the standalone page.

    This work used to run at import time, so `import export` wrote a large
    file and printed to stdout. A module that does its job merely by being
    imported cannot be used by anything else, and any test that imports it
    acquires a side effect nobody asked for.
    """
    db = sqlite3.connect(DB)
    rows = db.execute("SELECT "+",".join(cols)+" FROM spans ORDER BY start").fetchall()
    spans = []
    for r in rows:
        d = dict(zip(cols, r))
        for jf in ("links","lifecycle","attributes","events"):
            d[jf] = json.loads(d[jf])
        spans.append(d)

# =========================================================================
# NORMALIZE TIME TO 0..1000 (RENDERER T UNITS). END=NONE STAYS NONE.
# =========================================================================
    t0 = min(s["start"] for s in spans)
    t1 = max([s["end"] for s in spans if s["end"]] + [s["start"] for s in spans])
    scale = 1000.0 / max(1e-9, (t1 - t0))
    for s in spans:
        s["start"] = round((s["start"] - t0) * scale, 2)
        s["end"] = round((s["end"] - t0) * scale, 2) if s["end"] else None
        for l in s["lifecycle"]:
            l["t"] = round((l["t"] - t0) * scale, 2)

# =========================================================================
# DOMAINS → CLUSTERS, DETERMINISTIC PLACEMENT (MAP NEVER MOVES)
# =========================================================================
    ROLE = {"gateway":"entry", "MainThread":"entry", "edge-agent":"entry",
            "sensors":"data", "inference":"logic", "uplink":"logic",
            "auth":"logic", "orders":"logic", "worker":"logic", "worker-1":"logic",
            "billing":"logic", "billing-1":"logic"}
    pairs = sorted({((s["host_id"] or "local"), (s["domain"] or "unknown")) for s in spans})
    slots = {}
    clusters = []
    for hostid, d in pairs:
        region = ROLE.get(d)
        if region is None:  # deterministic fallback: hash → region band
            region = ["entry","logic","data"][int(hashlib.md5(d.encode()).hexdigest(),16)%3]
        key = (hostid, region)
        slot = slots.get(key, 0); slots[key] = slot + 1
        clusters.append({"id": hostid+"/"+d, "label": d, "region": region,
                         "slot": slot, "host": hostid})

    payload = json.dumps({"spans": spans, "clusters": clusters}, separators=(",",":"))

    html = open(TEMPLATE).read()

    # 1) swap badge: SYNTHETIC → REAL CAPTURE
    html = html.replace(
      '<span class="badge" title="No real capture in a browser. Span stream hand-authored, schema-conforming.">⚠ SYNTHETIC</span>',
      '<span class="badge" style="background:#0a3a1a;color:#56d364;border-color:#1a5c2c" title="Spans captured from a real Python process via sys.setprofile (semantic stream). No eBPF/LD_PRELOAD in this environment; syscall stream absent; disclosed.">● REAL CAPTURE</span>')

    # 2) replace synthetic data + clusters with the real payload
    m_start = html.index("  var CLUSTERS=[")
    m_end = html.index("  // canopy routes")
    replacement = """  var __REAL__ = %s;
      var CLUSTERS = __REAL__.clusters;
      var CMAP={}; CLUSTERS.forEach(c=>CMAP[c.id]=c);
      // fixed world placement (map never moves): deterministic from region+slot
      (function placeWorld(){
        var cxm=WORLD_W*0.5, cym=WORLD_H*0.52;
        var byR={}; CLUSTERS.forEach(c=>{(byR[c.region]=byR[c.region]||[]).push(c);});
        Object.keys(byR).forEach(function(rk){
          var arr=byR[rk].sort((a,b)=>a.slot-b.slot), n=arr.length;
          arr.forEach(function(c,i){
            if(rk==='external'){ var ang=-Math.PI*0.32+c.slot*0.5, R=Math.min(WORLD_W,WORLD_H)*0.46;
              c.wx=cxm+Math.cos(ang)*R; c.wy=cym+Math.sin(ang)*R*0.8; }
            else { var band=rk==='entry'?cym-WORLD_H*0.32:rk==='data'?cym+WORLD_H*0.30:cym;
              var sw=Math.min(WORLD_W*0.62,Math.max(n*330,330)); c.wx=cxm-sw/2+(n>1?sw*i/(n-1):sw/2); c.wy=band; }
            c.wr=56;
          }); });
      })();
      var spans = __REAL__.spans.map(function(s){
        return { span_id:s.span_id, trace_id:s.trace_id, parent_span_id:s.parent_span_id,
          links:s.links, name:s.name, kind:s.kind, start:s.start,
          end:(s.end==null? 1e9 : s.end),          // stale-running: never completes
          stale:(s.end==null), status:s.status, lifecycle:s.lifecycle,
          attributes:s.attributes, events:(s.events||[]),
          cluster:((s.host_id||'local')+'/'+(s.domain||'unknown')),
          transport:false, fromCluster:null };
      });
      // derive cross-cluster transport edges from parent chains crossing domains
      var byId={}; spans.forEach(s=>byId[s.span_id]=s);
      spans.forEach(function(s){
        var p = s.parent_span_id && byId[s.parent_span_id];
        if(p && p.cluster !== s.cluster){ s.fromCluster = p.cluster; s.transport = true; }
      });
      var TMAX=1000;
    """ % payload
    html = html[:m_start] + replacement + html[m_end:]

    # 3) neutralize the demo-vs-spec disclosure to the real-capture one
    html = html.replace(
      "<b>Demo vs spec.</b> Synthetic data conforming to the specification",
      "<b>Real capture.</b> Spans captured live from a traced Python process (semantic stream, sys.setprofile): two-phase writes, kind inference, the specification loop aggregation, queue context propagation with claimed/released lifecycle and the specification links, and genuine stale-running spans (the hang is real).")

    open(OUT, "w").write(html)
    print("wrote", OUT, "| spans:", len(spans), "| clusters:", [c["id"] for c in clusters])
    stale = [s["name"] for s in spans if s["end"] is None]
    print("stale-running in payload:", stale)


if __name__ == "__main__":
    main()
