# =========================================================================
#  MANIFEST
# =========================================================================
#  script_name: push_store.py
#  script_path: execviz/push_store.py
#  module_name: push_store
#  version: 0.53.1
#  description: The store interface the capture layer already speaks
#  kind: module
#  spec: internal
#  internal_dependencies: 
#  external_dependencies: json, sys
#  features: push store, capture, store
#  api_version: execvis-v1.0.0
#  last_updated: 2026-08-07
# =========================================================================

"""A store that keeps nothing on disk in the traced process.

The local SQLite store is the other delivery mode and it is fine, but
it writes inside the program being measured, so a syscall capture attributes the
recorder's own pwrite64 and fcntl to whatever span happened to be running. Those
are real syscalls with the wrong owner.

This buffers in memory and posts to an execviz instance instead, so the only
syscalls the recorder makes are the periodic sends, on its own thread, and the
capture stops contaminating the thing it is capturing.
"""
import json
import sys, threading, time, urllib.request


class PushStore:
    def __init__(self, collector, host_id="local", flush_secs=0.7, batch=500):
        self.collector = collector.rstrip("/")
        self.host_id = host_id
        self.flush_secs = flush_secs
        self.batch = batch
        self._lock = threading.Lock()
        self._spans = {}          # span_id -> span, held until its phase two lands
        self._sent = {}           # span_id -> (has_end, status) last delivered
        self.dropped = 0            # spans lost to a full buffer, reported not hidden
        self.dropped_traces = 0     # the unit of loss: whole traces, never spans
        self.dropped_abnormal = 0   # traces holding an error or a stuck span
        self.oversized_traces = 0   # a single trace larger than the whole buffer
        self.refused_by_collector = 0
        self._reported = set()      # one message per distinct reason, not per batch
        self._stop = threading.Event()
        self._t = threading.Thread(target=self._loop, name="execviz-flush", daemon=True)
        self._t.start()

# =========================================================================
# THE STORE INTERFACE THE CAPTURE LAYER ALREADY SPEAKS
# =========================================================================

    def begin(self, span):
        s = dict(span)
        s.setdefault("links", [])
        s.setdefault("lifecycle", [])
        s.setdefault("attributes", {})
        s.setdefault("events", [])
        s["end"] = None
        s["status"] = "running"
        with self._lock:
            self._spans[s["span_id"]] = s
        # the bound is enforced where the buffer grows, not on a timer: a burst
        # between two flushes is exactly when it would otherwise run away
        self._evict()


    def finish(self, span_id, end, status, attributes=None, output=None, error=None):
        with self._lock:
            s = self._spans.get(span_id)
            if s is None: return
            s["end"] = end
            s["status"] = status
            if attributes: s["attributes"].update(attributes)

    def add_lifecycle(self, span_id, ltype, context=None):
        ev = {"t": time.time(), "type": ltype}
        if context: ev["context"] = context
        with self._lock:
            s = self._spans.get(span_id)
            if s is not None: s["lifecycle"].append(ev)

    def add_links(self, span_id, links):
        with self._lock:
            s = self._spans.get(span_id)
            if s is None: return
            for l in links:
                if l not in s["links"]: s["links"].append(l)

    def add_event(self, span_id, level, msg, t=None):
        ev = {"t": t if t is not None else time.time(), "level": level, "msg": msg}
        with self._lock:
            s = self._spans.get(span_id)
            if s is None: return False
            s["events"].append(ev)
            return True

    def dump(self):
        with self._lock:
            return [dict(s) for s in self._spans.values()]

# =========================================================================
# DELIVERY
# =========================================================================

    def _pending(self):
        out = []
        with self._lock:
            for sid, s in self._spans.items():
                state = (s["end"] is not None, s["status"])
                if self._sent.get(sid) != state:
                    out.append(dict(s))
                if len(out) >= self.batch: break
        return out

    #: The most spans held while delivery is failing.
    #:
    #: An unreachable collector used to mean unbounded growth inside the program
    #: being observed; a tracing tool that eventually kills the process it is
    #: watching, which is the worst failure this design could have. The bound is
    #: the same order as the collector's own batch limit.
    MAX_PENDING = 20000

    def _evict(self):
        """Drops whole traces when the buffer is full, never individual spans.

        Two invariants the specification states and the core's retention already
        honours (the specification, the specification):

        *Trace-level only.* Dropping a span whose siblings remain punches a hole
        in that trace's graph; a parent pointing at a child that no longer
        exists, or a fan-in naming a link that cannot be resolved. The unit of
        loss is the trace, so what survives is causally complete.

        *Bias toward the abnormal.* A trace holding an error or a still-running
        span is never dropped while any ordinary trace remains. Those are the
        traces someone came looking for; discarding them first would lose
        exactly the evidence the tool exists to keep.
        """
        with self._lock:
            if len(self._spans) <= self.MAX_PENDING:
                return

            traces = {}
            for sid, s in self._spans.items():
                t = s.get("trace_id") or sid          # an untraced span is its own trace
                rec = traces.setdefault(t, {"ids": [], "last": 0.0, "keep": False})
                rec["ids"].append(sid)
                rec["last"] = max(rec["last"], float(s.get("end") or s.get("start") or 0.0))
                if s.get("end") is None or s.get("status") == "error":
                    rec["keep"] = True

            # ordinary traces first, oldest by newest activity; the same age
            # rule retention uses, so a trace still being written to is young
            ordinary = sorted((r["last"], t) for t, r in traces.items() if not r["keep"])
            abnormal = sorted((r["last"], t) for t, r in traces.items() if r["keep"])

            # One trace larger than the whole buffer cannot be held at trace
            # granularity, and holding it anyway is the unbounded growth this
            # bound exists to prevent. It is dropped and reported as its own
            # fact: a single oversized trace means the buffer is too small or
            # that trace is pathological, and an operator needs to know which.
            for t, rec in traces.items():
                if len(rec["ids"]) > self.MAX_PENDING:
                    for sid in rec["ids"]:
                        if self._spans.pop(sid, None) is not None:
                            self.dropped += 1
                    self.dropped_traces += 1
                    self.oversized_traces += 1

            for _, t in ordinary + abnormal:
                if len(self._spans) <= self.MAX_PENDING:
                    break
                if t not in self._spans and not any(
                        sid in self._spans for sid in traces[t]["ids"]):
                    continue          # already removed as oversized
                rec = traces[t]
                for sid in rec["ids"]:
                    if self._spans.pop(sid, None) is not None:
                        self.dropped += 1
                self.dropped_traces += 1
                if rec["keep"]:
                    # dropping one of these means the buffer is full of nothing
                    # but abnormal traces, which is itself worth reporting
                    self.dropped_abnormal += 1

    def flush(self):
        batch = self._pending()
        if not batch: return 0
        payload = {"host_id": self.host_id, "spans": batch}
        if self.dropped:
            # the collector is told the record is incomplete, and how badly
            payload["dropped"] = self.dropped
            payload["dropped_traces"] = self.dropped_traces
            payload["dropped_abnormal"] = self.dropped_abnormal
            if self.oversized_traces:
                payload["oversized_traces"] = self.oversized_traces
        body = json.dumps(payload).encode()
        req = urllib.request.Request(self.collector + "/api/ingest", data=body,
                                     headers={"Content-Type": "application/json"})
        try:
            with urllib.request.urlopen(req, timeout=8) as r:
                self._read_reply(r.read())
        except Exception:
            return 0                       # retried on the next tick, nothing dropped
        with self._lock:
            # the loss has been reported, so the counter starts again rather
            # than double-counting it on every later delivery
            self.dropped = 0
            self.dropped_traces = 0
            self.dropped_abnormal = 0
            self.oversized_traces = 0
            for s in batch:
                sid = s["span_id"]
                self._sent[sid] = (s["end"] is not None, s["status"])
                # a completed span has been delivered and its row is now the
                # collector's; keeping it here would grow without bound
                if s["end"] is not None:
                    self._spans.pop(sid, None)
        return len(batch)

    def _read_reply(self, raw):
        """Reads what the collector said about the batch.

        The collector names every span it refused and why; a nameless span, a
        span that ends before it starts, a self-parent. That explanation existed
        and reached nobody: every adapter discarded the reply and treated any 200
        as complete success, so an adapter emitting malformed spans would go on
        emitting them with nothing to show the author.

        Reported once per distinct reason rather than per batch, because a bug in
        an adapter repeats every second and a message that repeats with it is one
        nobody reads.
        """
        try:
            reply = json.loads(raw.decode("utf-8", "replace"))
        except Exception:
            return
        if not isinstance(reply, dict) or not reply.get("rejected"):
            return
        for reason in reply.get("reasons", []):
            # the span id changes every time, so key on the explanation itself
            key = reason.split(":", 1)[-1].strip()
            if key in self._reported:
                continue
            self._reported.add(key)
            sys.stderr.write(
                "execviz: the collector refused a span; %s\n"
                "  (further spans refused for this reason will not be reported again)\n"
                % reason)
        self.refused_by_collector += int(reply.get("rejected", 0))

    def _loop(self):
        while not self._stop.wait(self.flush_secs):
            try: self.flush()
            except Exception: pass

    def close(self):
        self._stop.set()
        for _ in range(3):
            if self.flush() == 0: break
