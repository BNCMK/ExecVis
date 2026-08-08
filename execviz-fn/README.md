<!--
===========================================================================
  MANIFEST
===========================================================================
  script_name: README.md
  script_path: execviz-fn/README.md
  module_name: README
  version: 0.53.1
  description: execviz for serverless functions
  kind: document
  spec: internal
  internal_dependencies:
  external_dependencies:
  features: README
  api_version: execvis-v1.0.0
  last_updated: 2026-08-07
===========================================================================
-->

# execviz for serverless functions

    import execviz_fn
    execviz_fn.install(domain="orders")

    @execviz_fn.handler(name="POST /orders")
    def create_order(event, context): ...

Two assumptions the ordinary delivery model makes are false here.

**The process may be killed the instant a handler returns**, so anything
buffered for a later flush is lost; worst precisely where it matters most,
because a function that died mid-flight is the one someone is looking for.
Delivery is synchronous at the boundary, and the cost is stated rather than
hidden: the handler waits for the recording.

**A sandbox is frozen between invocations, not destroyed.** Wall time keeps
running while nothing executes, so a span left open across a freeze reports
minutes of duration for microseconds of work. Wall time and execution time are
different quantities here, and a capture that conflates them is lying with
arithmetic.

    execviz functions run.db

    1 sandbox, 4 invocations, 1 cold start, 470.4ms frozen
     #1 ok     wall 30.7ms  cpu 0.448ms  waiting 30.28ms  cold=True
     #3 ok     wall 30.7ms  cpu 0.443ms  waiting 30.24ms  frozen 403ms before

## What the adapter learned from its own checker

The first version recorded `cold_start` and `resumed_after_freeze` as lifecycle
events, and `execviz check` rejected the capture with three derivability
violations. It was right: both follow from timestamps, and a capture that states
a derived fact invites it to disagree with the data it came from.

What is **not** derivable is which sandbox an invocation ran in,
concurrent sandboxes interleave and nothing in the timestamps separates them. So
the adapter records the sandbox identity and its start, and `execviz functions`
derives the rest. The capture is conformant with zero violations.
