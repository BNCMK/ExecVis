<!--
===========================================================================
  MANIFEST
===========================================================================
  script_name: README.md
  script_path: execviz/README.md
  module_name: README
  version: 0.53.1
  description: # Recording a test run
  kind: document
  spec: internal
  internal_dependencies:
  external_dependencies:
  features: README
  api_version: execvis-v1.0.0
  last_updated: 2026-08-07
===========================================================================
-->

## Recording a test run

    pytest -p pytest_execviz --execviz-db run.db

Each test becomes a span, so a failing test carries the execution that produced
it rather than only a traceback:

    test session                    ok        158.9 ms
    test_demo.py::test_login        ok         11.7 ms
    test_demo.py::test_upload       error     101.0 ms

It traces **only the tests**, not pytest's own internals: `install(autotrace=False)`
records what the program declares and nothing else. Breadth is the right default
for exploring an unfamiliar program and the wrong one for a harness that already
knows its units; the first version buried four tests under thousands of frames
from pytest itself.

The plugin is inert unless asked for, because a test tool that changes behaviour
when merely installed is one nobody trusts. Values are off unless
`--execviz-values` is passed, since test arguments leak as readily as any others.
It records what produced the run; commit, build, environment from the usual CI
variables; so two runs can be compared rather than guessed at.

It composes with the rest:

    execviz across --runs ci_*.db
      10 runs, 5 had a failure
      test_flaky.py::test_intermittent   5/10 runs; intermittent

    execviz assert run.db --rules ci_rules.txt      # exits 1, gating the build
      ✗ no_errors_in tests; 1 errors in that domain ['test_demo.py::test_upload']

## Tail sampling

    capture.tail_sample(True, rate=0.1, slow_ms=1000)

Volume control that decides before knowing the outcome keeps the wrong traces.
This decides when a trace ends:

    t-fail   keep=True  weight= 1.0  a trace that failed is always kept
    t-open   keep=True  weight= 1.0  a trace still open is always kept
    t-slow   keep=True  weight= 1.0  a trace that ran long is always kept
    t-fast1  keep=False weight= 0.0  not drawn

A drawn trace carries the **weight** it stands for, so counts can be weighted
accurately instead of being read as totals. A trace kept because it was interesting
stands for itself alone; it was not drawn, so it represents nothing else. The
draw is deterministic, so a replay of the same capture decides the same way.

## State, failure, and the run

    capture.declare_run(commit='9f2c1ab', environment='staging')
    capture.capture_values(True)          # off by default
    capture.trace_step('charge', 'call', charge, order)

Three additions, and one that makes them safe.

**State.** A span may carry the values that went in and came out, which is how
*what was the input that caused this* becomes answerable:

    charge | error
      inputs : {"args": "[{sku=BAD, user=u-42}]"}
      error  : ValueError - no price for BAD

Values are rendered at the moment of the call, never referenced, so an object
that mutates afterwards cannot rewrite history; there is a test for exactly
that. Containers render their elements rather than their shape, because
`tuple(len=1)` answers nothing.

**Failure.** Type, message, frames and the cause chain beneath. The chain matters
most: the exception on top is usually the least informative one in the stack.

**The run.** Commit, build, environment, region, declared once and inherited.
Without it, comparing two runs compares two unknowns.

**Redaction, which is not optional.** It runs at capture, before anything is
stored, because redacting at display leaves the secret in the file and redacting
at export leaves it in the local store. Keys catch what a program named accurately;
patterns catch what it did not. A redacted value is marked rather than deleted,
so an absent field is never ambiguous between *nothing there* and *not shown*. A
secret nested inside a container is caught by its own key. Rules that cannot be
evaluated fail closed.

Verified: with an API key passed to every traced call, the string appears zero
times in the resulting store file. Twelve tests cover it.

# execviz; step-1 prototype (real capture → store → renderer)

The spec's the specification step-1 prototype, working end to end:

    python3 workload.py          # traces a REAL program → run.db (two-phase SQLite store)
    python3 export.py run.db out.html   # store → nested semantic-zoom renderer

- **store.py**; two-phase span store. INSERT status=running at start,
  UPDATE end+status at completion. A span that never completes stays
  running/end=NULL; the stale-running death signal, stored as a fact.
- **capture.py**; sys.setprofile/threading.setprofile semantic capture
  + manual emitter API. Kind inference from C-call events (sleep→wait
  with suspended/resumed lifecycle; file ops→io). the specification loop aggregation
  (hot loops collapse to one kind=loop span with iteration count).
  Queue context propagation: stamped on the crossing by the sender,
  read back by the receiver; receiver span linked to the queue span;
  claimed/released lifecycle. Reentrancy guard: the capture layer never
  traces itself.
- **workload.py**; the traced demo: multi-domain request handling, real file
  IO, waits, a 400-iteration hot loop, a cross-thread queue handoff, a
  failing request, and a hang (→ 3 genuine stale-running spans).
- **export.py**; reads the store, splices the real spans into the nested
  semantic-zoom map (execviz-live.html). The store is the recording; the
  renderer only reads it.

Captured on last run: 102 spans; 3 stale-running, 3 aggregated loops,
14 waits, 11 io, 1 queue (claimed+released), 2 errors, 1 fan-in link.

## Live mode (the specification live feed; poll on interval)

    python3 live.py run.db 8765 &        # serves renderer at / and store at /spans
    python3 slow_workload.py             # ~35s of real traced activity

Open http://127.0.0.1:8765/; the map updates as spans arrive (verified:
span count grew 61 → 117 → 167 in-browser during a real run). The view
follows the live edge until you scrub; scrubbing switches to review.

**Stale-running semantics (live):** in-flight spans have end=NULL too, so
"stale" means running PAST THRESHOLD, not merely unfinished; the
renderer age-gates the death-signal styling (dotted, pulsing, fading rail)
and young open spans render as normal running work. Bonus proof: killing
the workload process mid-run leaves exactly the in-flight spans
(service_run, worker, the hang chain) stale in the store; the death
signal records what a dead process was doing.

Not yet here (honest gaps): syscall stream (no eBPF/LD_PRELOAD in this
environment), async-context carrier, cross-process traces, push feed
(SSE/WebSocket; the poll seam is where it swaps in).


## Machine-facing API (headless; no browser)

    python3 api.py run.db --view system          # hosts, span/error/stale counts
    python3 api.py run.db --view field           # clusters + routes
    python3 api.py run.db --view cluster --cluster orders
    python3 api.py run.db --view channel --cluster orders --family control
    python3 api.py run.db --view span --span <id>
    python3 api.py run.db --query races --min-overlap-ms 5
    python3 api.py run.db --query stale|errors|slowest|hotpaths
    python3 api.py run.db --query descendants --span <id>
    python3 api.py run.db --diff earlier-capture.json
    python3 api.py run.db --serve 8900 [--collect]

`--view` is progressive summarisation: each tier returns aggregates, not
the tier below, so a huge trace is consumed a level at a time instead of all at
once. `--query races` is the two-edge-set idea made queryable: causal
siblings that overlapped in time. `--diff` compares two captures by
(domain, name, kind) signature; run, patch, re-capture, compare.

## Distributed capture; a node on another device

    # collector (machine A)
    python3 api.py collector.db --serve 8900 --collect
    python3 live.py collector.db 8765          # the map, all hosts in one graph

    # remote device (machine B)
    python3 node.py --collector http://A:8900 --host-id edge-1 -- python3 myapp.py

The node traces locally into its own store and forwards spans to the collector,
which merges them by `host_id`. Two-phase spans are re-sent when their second
phase lands, so a completed span updates rather than duplicating; a failed flush
is retried on the next tick. Verified: a remote workload's 71 spans joined the
collector's 102 and both hosts appear in `--view system`.

**System tier (new top LOD).** Hosts are the tier above the field, following the
same rules as every other tier: deterministic fixed position, contains the tier
below it, resolves by LOD, labels crossfade. Each host is a disc; its Regions,
Clusters, Channels and Spans nest inside it. Cross-host routes bend harder than
in-host transport; they are a different species again.

## Async capture

The frame stack is not a valid parent chain across an `await`: the interpreter
unwinds frames on suspension and resumes them on whatever task the scheduler
picks. Causality therefore rides a `contextvars` carrier, which is copied into a
task at creation and restored on resume.

    async with capture.async_span("GET /profile", "call"):
        await capture.gather_span("fanin", fetch_user(), fetch_orders())
        await capture.await_span(asyncio.sleep(0.02), "template", "wait")

    task = capture.spawn(worker(), "worker")     # child inherits its creator

`await_span` records `suspended` and `resumed`; the resume may land on a
different task, and that context change is what the lifecycle events carry.
`gather_span` treats a gather as a fan-in: the continuation records the extra
children in `links` rather than inventing extra parents.

`async_workload.py` is the proof. Three concurrent requests interleave in time,
and the check walks every child span back to its originating request:

    spans: 128
      OK  fetch_user_0    <- profile_fanin[0] <- GET /profile/0 <- service
      OK  fetch_orders_2  <- profile_fanin[1] <- GET /profile/2 <- service
      ...
    fan-in joins with links: 3
    stale-running: watchdog, never_returns
    MISATTRIBUTED PARENTS: 0

Event-loop machinery is excluded from capture, which is what takes that program
from 834 spans of scheduler noise to 128 spans of the program itself.

## Task-scoped carrier

The carrier holds the active span together with the identity of the task that
set it, and is trusted only when the frame being recorded belongs to that same
task. A context copy outlives the task that made it, and the profiler fires on
frames belonging to whichever task the loop is currently stepping, so an
unqualified carrier parents work under an unrelated task's span. Conformance
caught exactly that: request frames were being attributed to another request's
`template` wait.

## Coroutine frames end early

An auto-captured coroutine frame reports a return at every suspension point,
because that is what the interpreter does. The explicitly instrumented span
continues across those suspensions, so `execviz check` reports the logical span
as outliving its frame. That is an accurate description of coroutine mechanics,
not a defect, and it is why the explicit async API is authoritative for causality
while auto-capture supplies breadth.

## Log capture

    capture.capture_logs()          # the only line a program adds

Intercepts the standard logging module and the standard streams. A line written
while a span is running becomes an event on that span, so a program that already
logs gains correlation without changing a call site and without threading a
request id through anything. A line written with no span active is retained as
unattributed rather than being attached to a nearby span.

Two things this changes about a program, stated rather than done quietly: the
root logger's level is lowered to INFO if it was higher, since the root level
filters before any handler runs, and `sys.stdout` and `sys.stderr` are wrapped
with tees that still write through to the real streams.

The `logging` module's own frames are excluded from capture. Without that, lines
attach to `Logger.handle` instead of the caller's span.

## Self-observation

    python3 self_trace.py

The tool's Python side is a program like any other, so it can be traced by the
tool: the store, the emitter, log attachment and a read-back all run under
capture, and what fires while the debugger works is then visible in the debugger.

## Delivery: local store or push

    capture.install("run.db")                                  # local store
    capture.install_push("http://127.0.0.1:8900", "host-1")     # nothing on disk

Both are allowed by the adapter contract. The difference matters when a syscall
capture is running: the local store writes SQLite inside the traced process, so
the recorder's own `pwrite64` and `fcntl` are attributed to whichever span was
running. They are real syscalls with the wrong owner.

Measured on the same workload under a kernel tracepoint capture:

| delivery | syscall records | SQLite-shaped calls attributed to the program |
|---|---|---|
| local store | 5432 | hundreds per span (pwrite64 ×178, fcntl ×153, …) |
| push | 1314 | **0** |

Push buffers in memory and posts on its own thread, re-sending a span once its
second phase lands so completion updates rather than duplicates, and retrying a
failed flush on the next tick. Use the local store when the collector may be
unreachable and the capture has to survive on disk; use push when the
measurement must not contaminate what it measures.
