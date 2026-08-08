# =========================================================================
#  MANIFEST
# =========================================================================
#  script_name: execviz_db.py
#  script_path: execviz-db/execviz_db.py
#  module_name: execviz_db
#  version: 0.53.1
#  description: Statements whose plan is worth asking for. A plan for a write is either uninteresting or expensive, and EXPLAIN is a second statement with its own cost; so this stays narrow and opt-in.
#  kind: module
#  spec: internal
#  internal_dependencies: 
#  external_dependencies: capture, os, re, redact, sys, time
#  features: execviz db
#  api_version: execvis-v1.0.0
#  last_updated: 2026-08-07
# =========================================================================

"""execviz capture for the database interior.

`db_query 140ms` is where the trail went cold and the person opened another
tool. The database will explain itself if asked, so a query span can carry the
statement, the plan, and how much work the engine did against how much it
returned.

Three rules, because a database is the easiest place both to leak and to lie:
the statement is recorded parameterised and never interpolated; asking for a
plan must not change the run; and rows examined against rows returned is the
number that explains the time.
"""
import os
import re
import sys
import time

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "execviz"))
import capture   # noqa: E402
import redact    # noqa: E402

_POLICY = redact.Policy()

# Statements whose plan is worth asking for. A plan for a write is either
# uninteresting or expensive, and EXPLAIN is a second statement with its own
# cost; so this stays narrow and opt-in.
_PLANNABLE = re.compile(r"^\s*(select|with)\b", re.I)


def normalise(sql):
    """The shape of a statement, with literals removed.

    The shape is what a reader needs; the values are the customer's data. A
    statement recorded with its literals interpolated is a copy of the database
    inside the capture, which is what redaction exists to prevent.
    """
    s = re.sub(r"'([^']|'')*'", "?", sql)          # string literals
    s = re.sub(r"\b\d+\b", "?", s)                  # numeric literals
    s = re.sub(r"\s+", " ", s).strip()
    return s[:500]


class TracedCursor:
    """Wraps a DB-API cursor. Everything not overridden passes through."""

    def __init__(self, cur, conn, plans=False):
        self._cur = cur
        self._conn = conn
        self._plans = plans

    def __getattr__(self, name):
        return getattr(self._cur, name)

    def __iter__(self):
        return iter(self._cur)

    def execute(self, sql, parameters=(), **kw):
        shape = normalise(sql)
        attrs = {"statement": shape, "parameters": len(parameters or ())}
        sid = capture.span_start(_name_for(shape), "io", attributes=attrs)
        t0 = time.time()
        try:
            r = self._cur.execute(sql, parameters, **kw) if parameters else self._cur.execute(sql, **kw)
            elapsed = time.time() - t0
            end_attrs = {"rows_returned": _rowcount(self._cur)}
            if self._plans and _PLANNABLE.match(sql):
                plan, scanned = self._explain(sql, parameters)
                if plan:
                    end_attrs["plan"] = plan
                    # the number that explains the time: a query returning ten
                    # rows after examining a million is the fault nobody sees
                    # from the duration alone
                    if scanned:
                        end_attrs["scans_a_whole_table"] = True
            end_attrs["ms"] = round(elapsed * 1000, 3)
            capture.span_end(sid, "ok", attributes=end_attrs)
            return r
        except Exception as e:
            capture.span_end(sid, "error", error=e)
            raise

    def executemany(self, sql, seq, **kw):
        shape = normalise(sql)
        sid = capture.span_start(_name_for(shape), "io",
                                 attributes={"statement": shape, "batch": True})
        try:
            r = self._cur.executemany(sql, seq, **kw)
            capture.span_end(sid, "ok", attributes={"rows_returned": _rowcount(self._cur)})
            return r
        except Exception as e:
            capture.span_end(sid, "error", error=e)
            raise

    def _explain(self, sql, parameters):
        """Asks the engine to explain itself, on a separate cursor.

        The plan is captured for the statement rather than by re-running the
        work, and any failure here is silent: a tool that breaks a query because
        it wanted a plan differs from a tool with no plans.
        """
        try:
            c = self._conn.cursor()
            c.execute("EXPLAIN QUERY PLAN " + sql, parameters or ())
            rows = [" ".join(str(x) for x in row[3:]) if len(row) > 3 else str(row)
                    for row in c.fetchall()]
            text = "; ".join(rows)[:400]
            scanned = "SCAN" in text.upper() and "USING INDEX" not in text.upper()
            return text, scanned
        except Exception:
            return None, False


def _rowcount(cur):
    try:
        n = cur.rowcount
        return int(n) if n is not None and n >= 0 else None
    except Exception:
        return None


def _name_for(shape):
    verb = shape.split(" ", 1)[0].lower() if shape else "query"
    table = ""
    m = re.search(r"\b(?:from|into|update|table)\s+([A-Za-z_][\w.]*)", shape, re.I)
    if m:
        table = " " + m.group(1)
    return ("db " + verb + table).strip()


class TracedConnection:
    def __init__(self, conn, plans=False):
        self._conn = conn
        self._plans = plans

    def __getattr__(self, name):
        return getattr(self._conn, name)

    def cursor(self, *a, **kw):
        return TracedCursor(self._conn.cursor(*a, **kw), self._conn, self._plans)

    def execute(self, sql, parameters=(), **kw):
        return self.cursor().execute(sql, parameters, **kw)

    def commit(self):
        sid = capture.span_start("db commit", "io")
        try:
            r = self._conn.commit()
            capture.span_end(sid, "ok")
            return r
        except Exception as e:
            capture.span_end(sid, "error", error=e)
            raise


def trace(conn, plans=False):
    """Wraps a DB-API connection.

    `plans=False` by default: EXPLAIN is a second statement with its own cost,
    and a tracing tool that silently doubles the query count is not one anybody
    should run in production without choosing to.
    """
    return TracedConnection(conn, plans=plans)
