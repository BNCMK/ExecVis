<!--
===========================================================================
  MANIFEST
===========================================================================
  script_name: README.md
  script_path: execviz-go/README.md
  module_name: README
  version: 0.53.1
  description: execviz capture adapter for Go
  kind: document
  spec: internal
  internal_dependencies:
  external_dependencies:
  features: README, capture, adapter
  api_version: execvis-v1.0.0
  last_updated: 2026-08-07
===========================================================================
-->

# execviz capture adapter for Go

Reports spans from a Go program to a collector, using the same wire
format every other runtime uses.

    execviz.Install(execviz.Config{HostID: "go-1", Domain: "api"})
    ctx, rootID := execviz.Start(context.Background(), "service", "call")

    err := execviz.Do(ctx, "GET /profile", "call", func(c context.Context) error {
        return execviz.Gather(c, "fanin",
            func(c2 context.Context) error { return fetchUser(c2) },
            func(c2 context.Context) error { return fetchOrders(c2) },
        )
    })

    ch := execviz.Go(ctx, "worker", worker)        // child inherits its creator

    m, _ := execviz.Send(ctx, "enqueue_job", item) // stamp the crossing
    c, item, qid := execviz.Claim(ctx, m)          // read it back
    execviz.Release(qid)

## The carrier

`context.Context` is the runtime's own mechanism for request-scoped state, and
it already crosses goroutine boundaries. A span placed in a context stays the
parent for everything that context reaches, so causality survives concurrency
without a second mechanism being invented for it.

## Coverage

Go has no per-call hook comparable to a Python profile function, so this adapter
records explicitly instrumented work plus the context propagation around it.
Coverage is narrower than the Python adapter's, which is a property of the
runtime rather than a defect.

## Verification

`cmd/workload` runs three concurrent requests, one of which fails, a worker
claiming stamped jobs off a channel, and a lock that never releases:

    MISATTRIBUTED PARENTS: 0
    links (fan-in): profile_fanin_join 2, worker_loop 2
    lifecycle: reconcile_lock suspended, enqueue_job claimed/released
    open spans: reconcile_lock, worker_loop
    conformant: true

Every `fetch_user_N` and `fetch_orders_N` traces back to `GET /profile/N`, never
to a sibling request running at the same moment. Logs land on the span that was
running when they were written, the same as in the other adapters.

The conformance run reports `enqueue_job outlived its parent` as an observation.
That is accurate: a queue span stays open until the worker releases it, which is
after the request that enqueued it has returned. Async handoff looks exactly
like that, and it is a fact about the program rather than a defect in the
capture.

## Capturing the logs the program already writes

    restore := execviz.CaptureLogs()
    defer restore()

    ctx, id := execviz.Start(context.Background(), "handle_request", "call")
    slog.InfoContext(ctx, "loading user", "id", 42)   // captured, no execviz call
    execviz.End(id, nil)

Attaches to `log/slog` and tees the standard logger. Lines still go where they
were going; a recorder that swallows a program's output has broken it, and the
breakage shows up only where somebody is reading those logs.

### Two things this runtime makes true

**Chaining onto slog's default handler deadlocks the program.** That handler
routes through the `log` package, and `log` routes back into slog's default
handler; which, once a wrapper is installed, is the wrapper. A two-line program
hangs on its second log call; it was reproduced in nine lines before the fix was
written. So when the program has not set a handler of its own, this does not
chain: it writes through a text handler aimed at the destination `log` was
already using.

**A bare `log.Println` cannot be attributed to a span.** Go has no
goroutine-local storage and the standard `log` package takes no context, so there
is no way to know which span was running when the line was written. Such lines
are teed through untouched and  not attached to anything, because an
unattributed line is a fact and a guessed parent is not. Code that wants
attribution uses the context-carrying calls; `slog.InfoContext` and friends,
which is the same context this adapter already uses as its carrier.

**Recording never blocks.** The hook runs inside the program's own logging call,
so it takes the buffer lock with `TryLock` and drops the line if it is held. A
line lost under contention is a small loss; a hung process is not a loss this
tool is entitled to cause. `execviz.LogLinesDropped()` reports the count.
