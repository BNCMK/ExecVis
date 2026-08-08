# =========================================================================
#  MANIFEST
# =========================================================================
#  script_name: live.py
#  script_path: execviz/live.py
#  module_name: live
#  version: 0.53.1
#  description: live follows the edge unless the user scrubs
#  kind: module
#  spec: internal
#  internal_dependencies: 
#  external_dependencies: http, json
#  features: live
#  api_version: execvis-v1.0.0
#  last_updated: 2026-08-07
# =========================================================================

"""execviz.live; live mode (spec the specification: poll-on-interval live feed).

Serves the nested semantic-zoom renderer at / and the CURRENT store contents
at /spans. The renderer polls /spans and re-ingests: the map updates as the
traced program executes. The store is the recording; both server and renderer
only READ it. Run a traced workload in another process writing run.db.
"""
import json, os, sqlite3, hashlib, sys
from http.server import HTTPServer, BaseHTTPRequestHandler

USAGE = "usage: python3 live.py [CAPTURE.db] [PORT]"


def _args(argv):
    """Reads the arguments, or explains.

    A wrong argument answered with a Python traceback, which is the same defect
    the Rust binary was corrected for: a tool that reports a user's mistake as an
    internal error teaches the user to distrust every other message it prints.
    Exit 2 for usage, matching the rest of the project.
    """
    if any(a in ("-h", "--help") for a in argv[1:]):
        print(USAGE)
        raise SystemExit(0)
    db = argv[1] if len(argv) > 1 else "run.db"
    port = 8765
    if len(argv) > 2:
        try:
            port = int(argv[2])
        except ValueError:
            sys.stderr.write("live.py: '%s' is not a port number\n%s\n" % (argv[2], USAGE))
            raise SystemExit(2)
        if not (1 <= port <= 65535):
            sys.stderr.write("live.py: port %d is out of range\n%s\n" % (port, USAGE))
            raise SystemExit(2)
    if not os.path.exists(db):
        sys.stderr.write("live.py: no capture at '%s'\n"
                         "  a capture is created by `execviz serve <db> --collect` or by an adapter\n" % db)
        raise SystemExit(2)
    return db, port


DB, PORT = _args(sys.argv)
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

ROLE = {"gateway":"entry", "MainThread":"entry", "edge-agent":"entry",
        "sensors":"data", "inference":"logic", "uplink":"logic",
        "auth":"logic", "orders":"logic", "worker":"logic", "worker-1":"logic",
        "billing":"logic", "billing-1":"logic"}
COLS = ["span_id","trace_id","parent_span_id","links","name","kind","start",
        "end","status","lifecycle","origin","host_id","clock_source","domain",
        "attributes","events"]

def read_spans():
    db = sqlite3.connect("file:%s?mode=ro" % DB, uri=True, timeout=2.0)
    rows = db.execute("SELECT "+",".join(COLS)+" FROM spans ORDER BY start").fetchall()
    db.close()
    spans = []
    for r in rows:
        d = dict(zip(COLS, r))
        for jf in ("links","lifecycle","attributes","events"):
            d[jf] = json.loads(d[jf])
        spans.append(d)
    if not spans: return {"spans": [], "clusters": []}
    t0 = min(s["start"] for s in spans)
    t1 = max([s["end"] for s in spans if s["end"]] + [s["start"] for s in spans])
    scale = 1000.0 / max(1e-9, (t1 - t0))
    for s in spans:
        s["start"] = round((s["start"] - t0) * scale, 2)
        s["end"] = round((s["end"] - t0) * scale, 2) if s["end"] else None
        for l in s["lifecycle"]: l["t"] = round((l["t"] - t0) * scale, 2)
        for e in s["events"]: e["t"] = round((e["t"] - t0) * scale, 2)
    pairs = sorted({((s["host_id"] or "local"), (s["domain"] or "unknown")) for s in spans})
    slots, clusters = {}, []
    for hostid, d in pairs:
        region = ROLE.get(d) or ["entry","logic","data"][
            int(hashlib.md5(d.encode()).hexdigest(),16)%3]
        key = (hostid, region)
        slot = slots.get(key, 0); slots[key] = slot + 1
        clusters.append({"id": hostid+"/"+d, "label": d, "region": region,
                         "slot": slot, "host": hostid})
    return {"spans": spans, "clusters": clusters}

def build_html():
    html = open(TEMPLATE).read()
    html = html.replace(
      '<span class="badge" title="No real capture in a browser. Span stream hand-authored, schema-conforming.">⚠ SYNTHETIC</span>',
      '<span class="badge" style="background:#0a2a3a;color:#58c6ff;border-color:#1a4c5c">◉ LIVE; polling real store</span>')
    m_start = html.index("  var CLUSTERS=[")
    m_end = html.index("  function clusterState(c,t){")
    replacement = """  var CLUSTERS=[], CMAP={}, spans=[], byId={}, routeList=[], maxCount=1;
  var TMAX=1000;
  function ingest(data){
    CLUSTERS=data.clusters; CMAP={}; CLUSTERS.forEach(c=>CMAP[c.id]=c);
    placeWorld();
    spans=data.spans.map(function(s){
      return { span_id:s.span_id, trace_id:s.trace_id, parent_span_id:s.parent_span_id,
        links:s.links, name:s.name, kind:s.kind, start:s.start,
        end:(s.end==null? 1e9 : s.end), stale:(s.end==null),
        status:s.status, lifecycle:s.lifecycle, attributes:s.attributes,
        events:(s.events||[]),
        duration_ms:s.duration_ms,
        cluster:((s.host_id||'local')+'/'+(s.domain||'unknown')), transport:false, fromCluster:null }; });
    byId={}; spans.forEach(s=>byId[s.span_id]=s);
    spans.forEach(function(s){ var p=s.parent_span_id&&byId[s.parent_span_id];
      if(p&&p.cluster!==s.cluster){ s.fromCluster=p.cluster; s.transport=true; } });
    var routes={};
    spans.forEach(function(s){ if(!s.fromCluster||s.fromCluster===s.cluster)return;
      var key=s.fromCluster+'>'+s.cluster, r=routes[key]||(routes[key]={from:s.fromCluster,to:s.cluster,count:0,durs:[],err:false,transport:true,spans:[]});
      r.count++; r.durs.push(s.end-s.start); if(s.status==='error')r.err=true; r.spans.push(s); });
    routeList=Object.values(routes);
    maxCount=Math.max(1,Math.max.apply(null,routeList.map(r=>r.count).concat([1])));
    routeList.forEach(function(r){ var fin=r.durs.filter(d=>d<1e8);
      var m=fin.length?fin.reduce((a,b)=>a+b,0)/fin.length:1;
      r.variance=fin.length?Math.sqrt(fin.reduce((a,b)=>a+(b-m)*(b-m),0)/fin.length)/(m||1):0; });
    CLUSTERS.forEach(function(c){
      var mine=spans.filter(s=>s.cluster===c.id);
      var byFam={}; mine.forEach(s=>{var f=famOf(s.kind);(byFam[f]=byFam[f]||[]).push(s);});
      Object.keys(byFam).forEach(f=>byFam[f].sort((a,b)=>a.start-b.start));
      c._byFam=byFam; c._spans=mine; });
    window.__spanCount=spans.length;
    window.__staleCount=spans.filter(s=>s.stale).length;
  }
  ingest({spans:[],clusters:[]});
  function poll(){ fetch('/spans').then(r=>r.json()).then(function(d){ ingest(d);
      var s=document.getElementById('scrub'); if(!window.__scrubbed){ T=1000; s.value=1000; }
    }).catch(function(){}); }
  setInterval(poll, 700); poll();
"""
    html = html[:m_start] + replacement + html[m_end:]
    # live follows the edge unless the user scrubs
    html = html.replace(
      "scrub.addEventListener('input',function(){playing=false;playBtn.textContent='▶';T=+scrub.value;});",
      "scrub.addEventListener('input',function(){window.__scrubbed=true;playing=false;playBtn.textContent='▶';T=+scrub.value;});")
    html = html.replace(
      "if(playing){T+=dt*speed*0.28;if(T>=TMAX){T=TMAX;setTimeout(function(){if(playing)T=0;},800);}scrub.value=T;}",
      "if(playing&&!window.__scrubbed){T=TMAX;scrub.value=T;} else if(playing){T+=dt*speed*0.28;if(T>=TMAX)T=TMAX;scrub.value=T;}")
    # the specification stale-running styling: dotted, slowly pulsing, fading token + dashed rail
    def must(h, a, b, lab):
        assert a in h, "live.py anchor lost: "+lab
        return h.replace(a, b, 1)
    html = must(html,
      "        if(t>=s.start&&t<=s.end&&!dim){ var f=(t-s.start)/Math.max(1,(s.end-s.start)),susp=false;",
      """        if(s.stale&&t-s.start>60){   // stale = running PAST THRESHOLD, not merely in-flight
          var pu=0.35+0.25*Math.sin(t*0.02+s.start); var srad=Math.max(3,R*0.022);
          ctx.setLineDash([3,4]); ctx.strokeStyle=sc('run',0.6); ctx.globalAlpha=pu;
          ctx.lineWidth=1.5; ctx.beginPath(); ctx.arc(x1,y1,srad+4,0,7); ctx.stroke();
          ctx.setLineDash([]); ctx.fillStyle=sc('run',0.6); ctx.globalAlpha=pu*0.8;
          ctx.beginPath(); ctx.arc(x1,y1,srad*0.7,0,7); ctx.fill(); ctx.globalAlpha=1;
        } else if(t>=s.start&&t<=s.end&&!dim){ var f=(t-s.start)/Math.max(1,(s.end-s.start)),susp=false;""", "stale token")
    html = html.replace(
      "        ctx.strokeStyle=reached?col:'rgba(90,103,115,0.3)';ctx.globalAlpha=reached?0.78:0.3;ctx.lineWidth=Math.max(2,R*0.018);ctx.lineCap='round';",
      "        ctx.strokeStyle=reached?col:'rgba(90,103,115,0.3)';ctx.globalAlpha=reached?0.78:0.3;ctx.lineWidth=Math.max(2,R*0.018);ctx.lineCap='round';if(s.stale&&t-s.start>60)ctx.setLineDash([4,5]);")
    html = must(html,
      "        ctx.beginPath();ctx.moveTo(x0,y0);ctx.quadraticCurveTo(cc.cx,cc.cy,x1,y1);ctx.stroke();\n        // the string: lineage thread",
      "        ctx.beginPath();ctx.moveTo(x0,y0);ctx.quadraticCurveTo(cc.cx,cc.cy,x1,y1);ctx.stroke();ctx.setLineDash([]);\n        // the string: lineage thread",
      "dash reset")
    return html

HTML = build_html()

class H(BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def do_GET(self):
        if self.path == "/spans":
            body = json.dumps(read_spans(), separators=(",",":")).encode()
            ct = "application/json"
        else:
            body = HTML.encode(); ct = "text/html; charset=utf-8"
        self.send_response(200)
        self.send_header("Content-Type", ct)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers(); self.wfile.write(body)

if __name__ == "__main__":
    open("execviz-live-mode.html", "w").write(HTML)   # written beside the caller
    print("serving on http://127.0.0.1:%d (db=%s)" % (PORT, DB), flush=True)
    HTTPServer(("127.0.0.1", PORT), H).serve_forever()
