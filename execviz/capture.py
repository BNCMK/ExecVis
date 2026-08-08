# =========================================================================
#  MANIFEST
# =========================================================================
#  script_name: capture.py
#  script_path: execviz/capture.py
#  module_name: capture
#  version: 0.53.1
#  description: The async carrier. A contextvar survives coroutine suspension and is copied into a task at creation, which is the lifetime a parent link needs. The frame stack is not valid across an await and is never consulted for
#  kind: module
#  spec: internal
#  internal_dependencies: 
#  external_dependencies: asyncio, push_store, redact, resource, store, sys, sysconfig, traceback
#  features: capture
#  api_version: execvis-v1.0.0
#  last_updated: 2026-08-07
# =========================================================================

"""execviz.capture; semantic capture layer + emitter API.

- sys.setprofile / threading.setprofile: every Python call/return → span.
  Semantic spans own identity. origin=semantic.
- kind inference via C-call events: time.sleep → wait (+suspended/resumed
  lifecycle), file/socket ops → io. Python frames default to call.
- the specification explosion control: frame filter, depth cap, loop aggregation
  (>LOOP_N same-fn-same-parent calls collapse to ONE kind=loop span).
- context: thread-local trace/stack/domain; queue helpers stamp
  context on the crossing (sender) and read it back (receiver). The
  receiver's already-running span gets a LINK to the queue span.
- reentrancy guard: emitter/store internals are never themselves traced.
"""
import sys, threading, time, os, uuid, contextvars, logging

from store import Store

_store = None
_tls = threading.local()

# The async carrier. A contextvar survives coroutine suspension and
# is copied into a task at creation, which is the lifetime a parent link
# needs. The frame stack is not valid across an await and is never consulted for
# the parent when the carrier holds one.
_active = contextvars.ContextVar("execviz_active", default=None)

def _task_id():
    """Identity of the running task, or None outside a loop. The carrier is only
    trusted when the frame being recorded belongs to the task that set it."""
    try:
        import asyncio
        t = asyncio.current_task()
        return id(t) if t is not None else None
    except Exception:
        return None
HOST = os.uname().nodename
LOOP_N = 8
DEPTH_CAP = 24
_SKIP_PREFIX = ("<frozen", )
# Runtime machinery is not the program under test. Capturing the
# event loop's own frames buries the traced program in scheduler noise.
_SKIP_MODULES = ("importlib", "threading", "queue", "sqlite3", "json",
                 "encodings", "codecs", "genericpath", "posixpath",
                 "_bootstrap", "uuid", "tempfile", "random",
                 "asyncio", "selectors", "concurrent", "contextvars", "logging",
                 "enum", "functools", "types", "typing", "abc", "inspect",
                 "linecache", "traceback", "warnings", "weakref")

WAIT_CFUNCS = {"sleep"}
IO_CFUNCS = {"read", "write", "open", "close", "send", "recv", "connect",
             "flush", "readline", "readlines", "urlopen"}

def _sid(): return uuid.uuid4().hex[:12]

def _tid():
    """OS thread id. The syscall stream reports this, so the semantic stream
    must record it for the two to be correlated by observation rather than
    inference."""
    try: return threading.get_native_id()
    except Exception: return 0

def _ctx():
    if not hasattr(_tls, "stack"):
        _tls.stack = []
        _tls.trace_id = None
        _tls.domain = threading.current_thread().name
        _tls.counts = {}
        _tls.aggs = {}
        _tls.ambient = None
        _tls.busy = False
    return _tls

def _guarded(fn):
    def wrap(*a, **kw):
        c = _ctx()
        if c.busy: return fn(*a, **kw)
        c.busy = True
        try: return fn(*a, **kw)
        finally: c.busy = False
    return wrap

def set_domain(d):
    """An explicit domain wins over the derived one: some boundaries are
    semantic and invisible to code structure."""
    c = _ctx(); c.domain = d; c.domain_explicit = True

def clear_domain():
    c = _ctx(); c.domain_explicit = False

import sysconfig as _sysconfig
_STDLIB_PATHS = tuple(filter(None, {
    _sysconfig.get_paths().get("stdlib"),
    _sysconfig.get_paths().get("platstdlib"),
    _sysconfig.get_paths().get("purelib"),
    _sysconfig.get_paths().get("platlib"),
}))

def _is_foreign(frame):
    """Code that is not the program under test. The skip list governs what gets
    captured; this governs what gets NAMED, which is a different question: a
    stdlib frame reached from application code is still worth recording, but it
    is not a domain of the system being read."""
    if frame is None: return False
    fn = frame.f_globals.get("__file__", "")
    if not fn:
        # generated code (a namedtuple's methods, an exec'd snippet) has no file
        # and is not a domain of anything
        return True
    return fn.startswith(_STDLIB_PATHS)

def _derived_domain(frame):
    """Top-level module or package of the frame. This is the unit a codebase
    already organises itself into, so it needs no configuration and survives
    edits within a package."""
    mod = frame.f_globals.get("__name__", "") if frame is not None else ""
    if mod and mod != "__main__":
        return mod.split(".")[0]
    fn = frame.f_globals.get("__file__", "") if frame is not None else ""
    if fn:
        base = os.path.basename(fn)
        if base.endswith(".py"): base = base[:-3]
        if base: return base
    return threading.current_thread().name

def _domain_for(frame):
    c = _ctx()
    if getattr(c, "domain_explicit", False):
        return c.domain
    # Walk out to the nearest frame that belongs to the program. Library work
    # done on behalf of a domain belongs to that domain; naming a cluster after
    # someone else's module describes the interpreter, not the system.
    f, hops = frame, 0
    while f is not None and hops < 12 and _is_foreign(f):
        f = f.f_back; hops += 1
    return _derived_domain(f if f is not None else frame)

@_guarded
def set_trace(trace_id=None):
    c = _ctx(); c.trace_id = trace_id or _sid(); return c.trace_id

def current_span():
    a = _active.get()
    # A context copy can outlive the task that made it, and the profiler fires on
    # frames belonging to whichever task the loop is stepping. Trusting the
    # carrier across a task boundary parents work under an unrelated task's span.
    if a is not None and a[1] == _task_id():
        return a[0]
    c = _ctx()
    for entry in reversed(c.stack):
        if entry is not None:
            return entry["span_id"]
    if getattr(c, "explicit", None):
        return c.explicit[-1]
    return c.ambient

# =========================================================================
# MANUAL EMITTER API
# =========================================================================
@_guarded
def span_start(name, kind, parent=None, links=None, attributes=None, domain=None, inputs=None):
    c = _ctx()
    _cost_open = _read_cost()
    if c.trace_id is None: c.trace_id = _sid()
    sid = _sid()
    _store.begin({"span_id": sid, "trace_id": c.trace_id,
        "parent_span_id": parent if parent is not None else current_span(),
        "links": links or [], "name": name, "kind": kind, "start": time.time(),
        "origin": "semantic", "host_id": HOST, "clock_source": "monotonic-ish",
        "domain": domain or (c.domain if getattr(c,"domain_explicit",False)
                             else _derived_domain(sys._getframe(2))),
        "attributes": dict(attributes or {}, tid=_tid()),
        # rendered and redacted now, at the moment of the call
        "inputs": render_values(inputs) if inputs is not None else None,
        "run": run_identity() or None})
    if _cost_open is not None:
        _COST_OPEN[sid] = (_cost_open, time.time())
    if not hasattr(c, "explicit"): c.explicit = []
    c.explicit.append(sid)
    return sid

@_guarded
def span_end(sid, status="ok", attributes=None, output=None, error=None):
    c = _ctx()
    if sid in getattr(c, "explicit", []):
        c.explicit.remove(sid)
    cost = None
    opened = _COST_OPEN.pop(sid, None)
    if opened is not None:
        before, t0 = opened
        cost = _cost_delta(before, _read_cost(), time.time() - t0)
    if cost:
        attributes = dict(attributes or {}, cost=cost)
    _store.finish(sid, time.time(), status, attributes,
                  output=render_values({'return': output}) if output is not None else None,
                  error=describe_error(error) if error is not None else None)

@_guarded
def span_lifecycle(sid, ltype, context=None):
    _store.add_lifecycle(sid, ltype, context)

def trace_step(name, kind, fn, *a, **kw):
    """Runs fn inside a span, recording what went in and what came out.

    The failure path records the exception itself rather than only a status, so
    a reader learns what went wrong and not merely that something did.
    """
    sid = span_start(name, kind, inputs={'args': a, **kw} if (a or kw) else None)
    try:
        r = fn(*a, **kw); span_end(sid, "ok", output=r); return r
    except Exception as e:
        span_end(sid, "error", error=e); raise

# =========================================================================
# QUEUE CONTEXT PROPAGATION
# =========================================================================
@_guarded
def q_put(q, item, name="queue_item"):
    c = _ctx()
    if c.trace_id is None: c.trace_id = _sid()
    sid = _sid()
    _store.begin({"span_id": sid, "trace_id": c.trace_id,
        "parent_span_id": current_span(), "links": [], "name": name,
        "kind": "queue", "start": time.time(), "origin": "semantic",
        "host_id": HOST, "clock_source": "monotonic-ish", "domain": c.domain})
    q.put({"__ctx__": {"trace_id": c.trace_id, "queue_span": sid}, "item": item})
    return sid

@_guarded
def q_get(q, timeout=None):
    msg = q.get(timeout=timeout)
    ctx = msg["__ctx__"]; c = _ctx()
    c.trace_id = ctx["trace_id"]
    _store.add_lifecycle(ctx["queue_span"], "claimed",
                         {"worker": threading.current_thread().name})
    receiver = current_span()
    if receiver is not None:
        _store.add_links(receiver, [ctx["queue_span"]])
    c.ambient = ctx["queue_span"]
    return msg["item"], ctx["queue_span"]

@_guarded
def q_done(queue_span):
    c = _ctx()
    if c.ambient == queue_span: c.ambient = None
    _store.add_lifecycle(queue_span, "released",
                         {"worker": threading.current_thread().name})
    _store.finish(queue_span, time.time(), "ok")

# =========================================================================
# LOG ATTACHMENT
# =========================================================================
# The capture layer already knows the active span, so a line written while that
# span runs belongs to it. Nothing is injected at the call site and no request id
# is threaded through the program.

_unattributed = []          # lines emitted with no span active, kept per host
MAX_EVENTS_PER_SPAN = 40    # volume control: an aggregated span keeps a sample

_event_counts = {}

@_guarded
def log_event(level, msg):
    sid = current_span()
    if sid is None:
        _unattributed.append({"t": time.time(), "level": level, "msg": msg,
                              "host": HOST, "domain": _ctx().domain})
        return None
    n = _event_counts.get(sid, 0)
    if n >= MAX_EVENTS_PER_SPAN:
        if n == MAX_EVENTS_PER_SPAN:
            _store.add_event(sid, "meta", "further lines suppressed for this span")
            _event_counts[sid] = n + 1
        return sid
    _event_counts[sid] = n + 1
    _store.add_event(sid, level, msg)
    return sid

def unattributed():
    return list(_unattributed)

def _skip(frame):
    # At interpreter teardown a module's globals are set to None, so
    # `.get("__name__", "")` returns None rather than the default; a default
    # only applies when the key is MISSING, not when its value is None. The
    # profiler is still installed at that point, so the last frames of a dying
    # process crashed on `None.startswith`, which killed the capture and left a
    # store with no spans in it.
    fn = frame.f_code.co_filename or ""
    if fn.startswith(_SKIP_PREFIX): return True
    base = os.path.basename(fn)
    if base in ("capture.py", "store.py"): return True
    mod = frame.f_globals.get("__name__") or ""
    return any(mod.startswith(m) for m in _SKIP_MODULES)

def _profiler(frame, event, arg):
    c = _ctx()
    if c.busy:
        return
    if event == "call":
        if _skip(frame) or len(c.stack) >= DEPTH_CAP:
            c.stack.append(None); return
        if c.trace_id is None: c.trace_id = _sid()
        name = frame.f_code.co_name
        parent = current_span()
        key = (parent, name)
        n = c.counts.get(key, 0) + 1; c.counts[key] = n
        if n > LOOP_N:
            agg = c.aggs.get(key)
            if agg is None:
                sid = _sid()
                _store.begin({"span_id": sid, "trace_id": c.trace_id,
                    "parent_span_id": parent, "name": name + " ×loop",
                    "kind": "loop", "start": time.time(), "origin": "semantic",
                    "host_id": HOST, "clock_source": "monotonic-ish",
                    "domain": _domain_for(frame),
                    "attributes": {"iterations": n - LOOP_N,
                                   "sampled_individually": LOOP_N}})
                agg = {"span_id": sid, "count": n}; c.aggs[key] = agg
            agg["count"] = n
            c.stack.append({"span_id": agg["span_id"], "name": name, "agg": True})
            return
        sid = _sid()
        _store.begin({"span_id": sid, "trace_id": c.trace_id,
            "parent_span_id": parent, "name": name, "kind": "call",
            "start": time.time(), "origin": "semantic", "host_id": HOST,
            "clock_source": "monotonic-ish", "domain": _domain_for(frame),
            "attributes": {"file": os.path.basename(frame.f_code.co_filename),
                           "line": frame.f_code.co_firstlineno}})
        c.stack.append({"span_id": sid, "name": name, "agg": False})
    elif event == "return":
        if not c.stack: return
        top = c.stack.pop()
        if top is None: return
        if top["agg"]:
            for k, v in c.aggs.items():
                if v["span_id"] == top["span_id"]:
                    _store.finish(top["span_id"], time.time(), "ok",
                                  {"iterations": v["count"] - LOOP_N,
                                   "sampled_individually": LOOP_N})
                    break
            return
        status = "error" if sys.exc_info()[0] is not None else "ok"
        _store.finish(top["span_id"], time.time(), status)
    elif event == "c_call":
        cname = getattr(arg, "__name__", "")
        parent = current_span()
        if parent is None: c.stack.append(None); return
        if cname in WAIT_CFUNCS:
            sid = _sid()
            _store.begin({"span_id": sid, "trace_id": c.trace_id,
                "parent_span_id": parent, "name": cname, "kind": "wait",
                "start": time.time(), "origin": "semantic", "host_id": HOST,
                "clock_source": "monotonic-ish", "domain": c.domain,
                "attributes": {"tid": _tid()}})
            _store.add_lifecycle(sid, "suspended")
            c.stack.append({"span_id": sid, "name": cname, "agg": False,
                            "wait": True})
        elif cname in IO_CFUNCS:
            sid = _sid()
            _store.begin({"span_id": sid, "trace_id": c.trace_id,
                "parent_span_id": parent, "name": cname, "kind": "io",
                "start": time.time(), "origin": "semantic", "host_id": HOST,
                "clock_source": "monotonic-ish", "domain": c.domain,
                "attributes": {"tid": _tid()}})
            c.stack.append({"span_id": sid, "name": cname, "agg": False})
        else:
            c.stack.append(None)
    elif event in ("c_return", "c_exception"):
        if not c.stack: return
        top = c.stack.pop()
        if top is None: return
        if top.get("wait"):
            _store.add_lifecycle(top["span_id"], "resumed",
                                 {"thread": threading.current_thread().name})
        _store.finish(top["span_id"], time.time(),
                      "error" if event == "c_exception" else "ok")

def install(db_path, autotrace=True):
    """Local store mode: the traced process writes the capture to disk itself.

    `autotrace=False` records only what the program declares explicitly. Breadth
    is the right default for exploring an unfamiliar program and the wrong one
    for a harness that already knows the units it cares about; a test runner
    tracing its own internals buries the tests among them.
    """
    global _store
    _store = Store(db_path)
    if autotrace:
        threading.setprofile(_profiler)
        sys.setprofile(_profiler)
    return _store

def install_push(collector, host_id=None, flush_secs=0.7, autotrace=True):
    """Push mode: nothing is written to disk in the traced process, so a syscall
    capture is not polluted by the recorder's own writes (spec 5.1 allows either
    delivery; this is the one that keeps the measurement clean)."""
    global _store
    from push_store import PushStore
    _store = PushStore(collector, host_id or HOST, flush_secs)
    if autotrace:
        threading.setprofile(_profiler)
        sys.setprofile(_profiler)
    return _store

def uninstall():
    sys.setprofile(None); threading.setprofile(None)
    if hasattr(_store, "close"):
        _store.close()


# =========================================================================
# ASYNC SUPPORT
# =========================================================================
def _set_active(span_id):
    """Bind the active span into the carrier along with the task that owns it."""
    return _active.set((span_id, _task_id()))

def _reset_active(token):
    try: _active.reset(token)
    except ValueError: pass          # reset in a different context: ignore

class async_span:
    """Async context manager. The span rides the contextvar carrier, so it stays
    the parent across awaits and inside anything the body spawns."""
    def __init__(self, name, kind="call", links=None, attributes=None, domain=None):
        self.name, self.kind = name, kind
        self.links, self.attributes, self.domain = links, attributes, domain
        self.sid = None; self.token = None
    async def __aenter__(self):
        self.sid = span_start(self.name, self.kind, None, self.links,
                              self.attributes, self.domain)
        self.token = _set_active(self.sid)
        return self.sid
    async def __aexit__(self, exc_type, exc, tb):
        span_end(self.sid, "error" if exc_type else "ok")
        _reset_active(self.token)
        return False

def spawn(coro, name=None, kind="spawn"):
    """Create a task. Task creation is a crossing: the child inherits the current
    span as its parent because asyncio copies the context at creation time."""
    import asyncio
    parent = current_span()
    sid = span_start(name or getattr(coro, "__name__", "task"), kind, parent)
    async def _wrapped():
        tok = _set_active(sid)
        try:
            r = await coro
            span_end(sid, "ok"); return r
        except Exception:
            span_end(sid, "error"); raise
        finally:
            _reset_active(tok)
    return asyncio.ensure_future(_wrapped())

async def await_span(awaitable, name, kind="wait"):
    """Await something, recording suspended and resumed. The resume may land on a
    different task; that context change is what the lifecycle events carry."""
    import asyncio
    sid = span_start(name, kind)
    span_lifecycle(sid, "suspended")
    tok = _set_active(sid)
    try:
        r = await awaitable
        span_lifecycle(sid, "resumed",
                       {"task": getattr(asyncio.current_task(), "get_name", lambda: "?")()})
        span_end(sid, "ok"); return r
    except Exception:
        span_end(sid, "error"); raise
    finally:
        _reset_active(tok)

async def gather_span(name, *awaitables):
    """A gather is a fan-in: the continuation records every child that fed it in
    links, not as extra parents."""
    import asyncio
    parent = current_span()
    child_ids = []
    async def run(a, i):
        sid = span_start("%s[%d]" % (name, i), "call", parent)
        child_ids.append(sid)
        tok = _set_active(sid)
        try:
            r = await a; span_end(sid, "ok"); return r
        except Exception:
            span_end(sid, "error"); raise
        finally: _reset_active(tok)
    results = await asyncio.gather(*[run(a, i) for i, a in enumerate(awaitables)])
    # The join is contained by the scope that called gather, so that is its
    # parent; every child that fed it is a secondary cause and goes in links.
    # Parenting the join to a child would place it outside its parent in time.
    join = span_start(name + "_join", "call", parent, links=child_ids)
    span_end(join, "ok")
    return results


# =========================================================================
# STATE, FAILURE, RUN
# =========================================================================
# Spec 3.2. All of this is OFF by default: recording values can cost more than
# the work it observes and can leak more than the program intended.

import redact as _redact

_RUN = {}
_VALUES = {'enabled': False, 'policy': _redact.DEFAULT}


def declare_run(**facts):
    """What produced this capture.

    Without it, comparing two runs compares two unknowns. Recorded once and
    inherited by every span written afterwards.
    """
    _RUN.update({k: v for k, v in facts.items() if v is not None})
    return dict(_RUN)


def run_identity():
    return dict(_RUN)


def capture_values(enabled=True, policy=None):
    """Turn on input/output capture, which is off by default."""
    _VALUES['enabled'] = bool(enabled)
    if policy is not None:
        _VALUES['policy'] = policy
    return _VALUES['enabled']


def render_values(mapping):
    """Renders and redacts a mapping of values at the moment of the call.

    Rendered, never referenced: an object that mutates afterwards must not
    silently rewrite history. Absence is stated rather than implied.
    """
    if not _VALUES['enabled']:
        return {'recorded': False, 'why': 'value capture is off'}
    pol = _VALUES['policy']
    out, withheld = {}, {}
    for k, v in (mapping or {}).items():
        text, why = pol.scrub(k, v)
        out[str(k)] = text
        if why:
            withheld[str(k)] = why
    res = {'recorded': True, 'values': out}
    if withheld:
        # a redacted value is marked, not deleted: the reader learns one existed
        res['withheld'] = withheld
    return res


def describe_error(exc, limit=12):
    """Type, message, frames and the cause chain beneath.

    The chain matters most: the exception on top is usually the least
    informative one in the stack.
    """
    import traceback
    if exc is None:
        return None
    pol = _VALUES['policy']
    msg, _ = pol.scrub('message', str(exc))
    frames = []
    tb = getattr(exc, '__traceback__', None)
    for fr in traceback.extract_tb(tb)[-limit:] if tb else []:
        frames.append({'file': fr.filename, 'line': fr.lineno,
                       'func': fr.name, 'text': (fr.line or '')[:120]})
    chain = []
    seen, cur = set(), exc
    while True:
        nxt = getattr(cur, '__cause__', None) or getattr(cur, '__context__', None)
        if nxt is None or id(nxt) in seen:
            break
        seen.add(id(nxt))
        cmsg, _ = pol.scrub('message', str(nxt))
        chain.append({'type': type(nxt).__name__, 'message': cmsg})
        cur = nxt
        if len(chain) > 8:
            break
    err = {'type': type(exc).__name__, 'message': msg, 'frames': frames}
    if chain:
        err['caused_by'] = chain
    return err


# =========================================================================
# TAIL SAMPLING
# =========================================================================
# Spec 4.5.2. Volume control that decides before knowing the outcome keeps the
# wrong traces. This decides when a trace ends: everything that failed or ran
# long is kept, the rest is sampled, and the decision is recorded so no count
# taken from the result is read as a total by mistake.

_SAMPLING = {'enabled': False, 'rate': 0.1, 'slow_ms': 1000.0, 'seed': 12345}


def tail_sample(enabled=True, rate=0.1, slow_ms=1000.0):
    """Keep every interesting trace, sample the rest, decide at the end."""
    _SAMPLING.update(enabled=bool(enabled), rate=float(rate), slow_ms=float(slow_ms))
    return dict(_SAMPLING)


def sampling_policy():
    return dict(_SAMPLING)


def decide_trace(trace_id, spans):
    """Returns (keep, reason, weight).

    `weight` is how many traces this one stands for, so a count can be weighted
    accurately instead of being read as a total. A kept-because-interesting trace
    stands for itself alone: it was not drawn, so it represents nothing else.
    """
    if not _SAMPLING['enabled']:
        return True, 'sampling off', 1.0
    if any(s.get('status') == 'error' for s in spans):
        return True, 'a trace that failed is always kept', 1.0
    if any(s.get('end') is None for s in spans):
        return True, 'a trace still open is always kept', 1.0
    span_ms = 0.0
    starts = [s['start'] for s in spans if s.get('start') is not None]
    ends = [s['end'] for s in spans if s.get('end') is not None]
    if starts and ends:
        span_ms = (max(ends) - min(starts)) * 1000.0
    if span_ms >= _SAMPLING['slow_ms']:
        return True, 'a trace that ran long is always kept', 1.0
    # deterministic draw: the same trace decides the same way on every replay,
    # which keeps a rerun of the same capture reproducible
    h = 0
    for ch in str(trace_id):
        h = (h * 131 + ord(ch)) & 0xFFFFFFFF
    drawn = (h % 10000) / 10000.0 < _SAMPLING['rate']
    if drawn:
        return True, 'drawn by the sampler', 1.0 / max(_SAMPLING['rate'], 1e-9)
    return False, 'not drawn', 0.0


# =========================================================================
# COST (SPEC 3.3)
# =========================================================================
# A duration says how long; it says nothing about what was spent. Two 10ms spans,
# one burning a core and one asleep on a socket, are the two cases a person is
# trying to tell apart, and a duration cannot tell them apart at all.

import resource as _resource

_COST = {'enabled': False}
_COST_OPEN = {}


def capture_cost(enabled=True):
    """Record processor time and allocation alongside duration. Off by default."""
    _COST['enabled'] = bool(enabled)
    return _COST['enabled']


def _read_cost():
    """A reading now, to be differenced against a reading later.

    What this runtime cannot measure is ABSENT, never zero: zero is a
    measurement, and a reader seeing it will conclude the program allocated
    nothing rather than that nobody counted.
    """
    if not _COST['enabled']:
        return None
    out = {}
    try:
        t = time.process_time()          # processor time for this process
        out['cpu'] = t
    except Exception:
        pass
    try:
        ru = _resource.getrusage(_resource.RUSAGE_SELF)
        out['maxrss_kb'] = ru.ru_maxrss
        out['vol_ctx'] = ru.ru_nvcsw          # gave up the processor willingly
        out['invol_ctx'] = ru.ru_nivcsw       # was taken off it
    except Exception:
        pass
    return out or None


def _cost_delta(before, after, wall_s):
    """The change across the span, measured the same way at both ends.

    A number read once and reported as though it were a change is a different
    quantity wearing the same name, so both readings are required.
    """
    if not before or not after:
        return None
    d = {}
    if 'cpu' in before and 'cpu' in after:
        cpu_ms = max(0.0, (after['cpu'] - before['cpu']) * 1000.0)
        d['cpu_ms'] = round(cpu_ms, 3)
        wall_ms = max(wall_s * 1000.0, 1e-9)
        # the ratio the whole section exists for: near 1 means it was working,
        # near 0 means it was waiting
        d['cpu_ratio'] = round(min(1.0, cpu_ms / wall_ms), 4)
        d['spent'] = 'working' if d['cpu_ratio'] > 0.5 else 'waiting'
    if 'maxrss_kb' in before and 'maxrss_kb' in after:
        grew = after['maxrss_kb'] - before['maxrss_kb']
        if grew > 0:
            d['rss_growth_kb'] = grew
    for k, label in (('vol_ctx', 'yielded'), ('invol_ctx', 'preempted')):
        if k in before and k in after:
            n = after[k] - before[k]
            if n > 0:
                d[label] = n
    # preemption is the visible edge of off-CPU time: the span was ready and
    # something else had the machine. The full split needs the scheduler, so
    # what is not measured here stays absent rather than being guessed
    if d.get('preempted'):
        d['note'] = 'taken off the processor while ready; the full off-CPU split needs the scheduler'
    return d or None
