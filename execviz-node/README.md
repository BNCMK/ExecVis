<!--
===========================================================================
  MANIFEST
===========================================================================
  script_name: README.md
  script_path: execviz-node/README.md
  module_name: README
  version: 0.53.1
  description: execviz capture adapter for Node.js
  kind: document
  spec: internal
  internal_dependencies:
  external_dependencies:
  features: README, capture, adapter
  api_version: execvis-v1.0.0
  last_updated: 2026-08-07
===========================================================================
-->

# execviz capture adapter for Node.js

Reports spans from a Node process to a collector. The wire format is the same
one every other runtime uses, so a Node service and a Python service appear as
two hosts in one graph.

    import * as ev from './execviz.mjs';

    ev.install({ collector: 'http://collector:8900', hostId: 'node-1', domain: 'api' });

    await ev.withSpan('GET /profile', 'call', async () => {
      await ev.gatherSpan('fanin', [() => fetchUser(), () => fetchOrders()]);
      await ev.awaitSpan(sleep(20), 'template', 'wait');
    });

    const task = ev.spawn(() => worker(), 'worker_loop');   // inherits its creator

    queue.push(ev.stamp({ job: 'invoice-1' }));             // stamp the crossing
    const { item, spanId } = ev.claim(queue.shift());       // read it back
    ev.release(spanId);

## The carrier

`AsyncLocalStorage` is the runtime's native equivalent of `contextvars`. It
survives an await and is inherited by anything the body schedules, so the parent
link stays correct while requests interleave.

## Capture completeness

Node offers no cheap per-call hook equivalent to `sys.setprofile`, so this
adapter records explicitly instrumented work plus the context propagation around
it. Coverage is therefore narrower than the Python adapter's, which is a property
of the runtime rather than a defect, and it is stated rather than hidden.

## Delivery

Spans are pushed directly to the collector, so no local store or disk is needed.
A span is queued at phase one and re-sent once its second phase lands, and the
collector upserts on `span_id`, so completion updates a row rather than
duplicating it. A failed flush is retried on the next tick.

## Verification

`workload.mjs` runs three concurrent requests, one of which fails, a worker that
claims stamped jobs off a queue, and a lock that never releases:

    MISATTRIBUTED PARENTS: 0
    links (fan-in): worker_loop 2, profile_fanin_join 1, profile_fanin_join 1
    lifecycle: GET /profile/0 claimed/released, db_user suspended/resumed
    open spans: reconcile_lock

Every `fetch_user_N` and `fetch_orders_N` traces back to `GET /profile/N`, never
to a sibling request that was running at the same moment.

Both runtimes into one collector:

    execviz serve multi.db --port 8900 --collect --ui ui.html
    node workload.mjs                                   # pushes directly
    execviz node --collector http://127.0.0.1:8900 --db python.db --host-id py-1

    execviz view multi.db --lod system
    -> node-1: 35 spans, 4 errors
    -> py-async: 128 spans, 5 errors, 2 stale-running

## Capturing the logs the program already writes

    execviz.install({ collector: '...' });
    execviz.captureLogs();

    await execviz.withSpan('handle_request', 'call', async () => {
      console.log('loading user 42');     // captured, no execviz call
      console.warn('cache miss');
    });

Three seams, because Node has no single logger: `console.*`, the raw
`process.stdout`/`stderr` writes everything else funnels into, and the hooks that
fire when the program is least able to instrument itself,
`uncaughtException`, `unhandledRejection` and `process.on('warning')`.

Every line still goes where it was going. `releaseLogs()` puts back what was
replaced.

**A console line is recorded once.** `console.log` writes to the stream
underneath, and that write is hooked too; so the guard is held across the tee.
Without it one line arrived twice, once with the level the program chose and once
with the level the stream implies, and the two disagreed. Verified: four lines
written, four recorded.
