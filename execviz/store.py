# =========================================================================
#  MANIFEST
# =========================================================================
#  script_name: store.py
#  script_path: execviz/store.py
#  module_name: store
#  version: 0.53.1
#  description: phase 1: span begins
#  kind: module
#  spec: internal
#  internal_dependencies: 
#  external_dependencies: json
#  features: store
#  api_version: execvis-v1.0.0
#  last_updated: 2026-08-07
# =========================================================================

"""execviz.store; the span store.

Two-phase writes: a span is INSERTed with status=running at start, and
UPDATEd with end + final status at completion. A span that never gets its
second phase stays status=running with end=NULL; that is the stale-running
death signal, stored as a first-class fact (mutable span state is a
deliberate design choice; see spec the specification).
"""
import json, sqlite3, threading, time

SCHEMA = """
CREATE TABLE IF NOT EXISTS spans (
  span_id        TEXT PRIMARY KEY,
  trace_id       TEXT NOT NULL,
  parent_span_id TEXT,
  links          TEXT NOT NULL DEFAULT '[]',   -- JSON [span_id]
  name           TEXT NOT NULL,
  kind           TEXT NOT NULL,                -- activity only
  start          REAL NOT NULL,                -- epoch seconds (float)
  end            REAL,                         -- NULL while running
  status         TEXT NOT NULL,                -- running | ok | error
  lifecycle      TEXT NOT NULL DEFAULT '[]',   -- JSON [{t,type,context?}]
  origin         TEXT NOT NULL,                -- semantic | syscall
  host_id        TEXT NOT NULL,
  clock_source   TEXT,
  domain         TEXT,                         -- bounded execution domain (cluster)
  attributes     TEXT NOT NULL DEFAULT '{}',   -- JSON object
  events         TEXT NOT NULL DEFAULT '[]'    -- JSON [{t,msg}]
);
CREATE INDEX IF NOT EXISTS idx_trace ON spans(trace_id);
"""

class Store:
    def __init__(self, path):
        self._lock = threading.Lock()
        self._db = sqlite3.connect(path, check_same_thread=False)
        self._db.executescript(SCHEMA)
        self._migrate()
        self._db.commit()

    # phase 1: span begins
    def _migrate(self):
        """Columns added after the first schema. A store written by an
        older build stays readable and gains the new fields empty."""
        for col in ("inputs", "output", "error", "run"):
            try:
                self._db.execute("ALTER TABLE spans ADD COLUMN %s TEXT" % col)
            except Exception:
                pass          # already present
        self._db.commit()

    def begin(self, span):
        with self._lock:
            self._db.execute(
                "INSERT OR REPLACE INTO spans "
                "(span_id,trace_id,parent_span_id,links,name,kind,start,end,status,"
                " lifecycle,origin,host_id,clock_source,domain,attributes,events,"
                " inputs,run) "
                "VALUES (?,?,?,?,?,?,?,NULL,'running',?,?,?,?,?,?,?,?,?)",
                (span["span_id"], span["trace_id"], span.get("parent_span_id"),
                 json.dumps(span.get("links", [])), span["name"], span["kind"],
                 span["start"], json.dumps(span.get("lifecycle", [])),
                 span.get("origin", "semantic"), span.get("host_id", "local"),
                 span.get("clock_source"), span.get("domain"),
                 json.dumps(span.get("attributes", {})),
                 json.dumps(span.get("events", [])),
                 json.dumps(span["inputs"]) if span.get("inputs") else None,
                 json.dumps(span["run"]) if span.get("run") else None))
            self._db.commit()

    # phase 2: span completes (never called for a crashed/hung span → stale running)
    def finish(self, span_id, end, status, attributes=None, output=None, error=None):
        with self._lock:
            if attributes:
                self._db.execute(
                    "UPDATE spans SET end=?, status=?, "
                    "attributes=json_patch(attributes, ?) WHERE span_id=?",
                    (end, status, json.dumps(attributes), span_id))
            else:
                self._db.execute(
                    "UPDATE spans SET end=?, status=? WHERE span_id=?",
                    (end, status, span_id))
            # phase two also carries what came out and what went wrong
            if output is not None:
                self._db.execute("UPDATE spans SET output=? WHERE span_id=?",
                                 (json.dumps(output), span_id))
            if error is not None:
                self._db.execute("UPDATE spans SET error=? WHERE span_id=?",
                                 (json.dumps(error), span_id))
            self._db.commit()

    def add_lifecycle(self, span_id, ltype, context=None):
        ev = {"t": time.time(), "type": ltype}
        if context: ev["context"] = context
        with self._lock:
            row = self._db.execute(
                "SELECT lifecycle FROM spans WHERE span_id=?", (span_id,)).fetchone()
            if row is None: return
            lc = json.loads(row[0]); lc.append(ev)
            self._db.execute("UPDATE spans SET lifecycle=? WHERE span_id=?",
                             (json.dumps(lc), span_id))
            self._db.commit()

    def add_event(self, span_id, level, msg, t=None):
        """A point-in-time fact inside a span's lifetime. This is where a log
        line lands: the span already identifies the work, so the line carries no
        correlation id of its own."""
        ev = {"t": t if t is not None else time.time(), "level": level, "msg": msg}
        with self._lock:
            row = self._db.execute(
                "SELECT events FROM spans WHERE span_id=?", (span_id,)).fetchone()
            if row is None: return False
            evs = json.loads(row[0])
            evs.append(ev)
            self._db.execute("UPDATE spans SET events=? WHERE span_id=?",
                             (json.dumps(evs), span_id))
            self._db.commit()
            return True

    def add_links(self, span_id, links):
        with self._lock:
            row = self._db.execute(
                "SELECT links FROM spans WHERE span_id=?", (span_id,)).fetchone()
            if row is None: return
            cur = json.loads(row[0])
            for l in links:
                if l not in cur: cur.append(l)
            self._db.execute("UPDATE spans SET links=? WHERE span_id=?",
                             (json.dumps(cur), span_id))
            self._db.commit()

    def dump(self):
        with self._lock:
            cols = ["span_id","trace_id","parent_span_id","links","name","kind",
                    "start","end","status","lifecycle","origin","host_id",
                    "clock_source","domain","attributes","events"]
            rows = self._db.execute(
                "SELECT " + ",".join(cols) + " FROM spans ORDER BY start").fetchall()
        out = []
        for r in rows:
            d = dict(zip(cols, r))
            for jf in ("links","lifecycle","attributes","events"):
                d[jf] = json.loads(d[jf])
            out.append(d)
        return out
