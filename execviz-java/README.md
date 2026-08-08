<!--
===========================================================================
  MANIFEST
===========================================================================
  script_name: README.md
  script_path: execviz-java/README.md
  module_name: README
  version: 0.53.1
  description: execviz capture adapter for the JVM
  kind: document
  spec: internal
  internal_dependencies:
  external_dependencies:
  features: README, capture, adapter
  api_version: execvis-v1.0.0
  last_updated: 2026-08-07
===========================================================================
-->

# execviz capture adapter for the JVM

Reports spans from a JVM program to a collector, using the same wire
format every other runtime uses.

    javac -d out src/execviz/ExecViz.java
    javac -cp out -d out src/demo/Workload.java
    EXECVIZ_COLLECTOR=http://127.0.0.1:8900 java -cp out demo.Workload

    ExecViz.install(new ExecViz.Config(collector, "jvm-1", "api", 300));

    ExecViz.in("GET /profile", "call", "api", () -> {
        ExecViz.gather("fanin", pool, List.of(
            () -> ExecViz.in("fetch_user", "call", "users", () -> { ... }),
            () -> ExecViz.in("fetch_orders", "call", "orders", () -> { ... })));
        return null;
    });

## The carrier, and where it stops

An `InheritableThreadLocal` is the closest thing the platform offers to an
automatic carrier: a thread started inside a span inherits it, including a
virtual thread started per task. It does **not** cross a pooled executor,
because a pool thread was created long before the work it later runs.

That is a limit of the runtime rather than of this adapter, so the adapter
carries the span across a submission explicitly:

    ExecutorService pool = ExecViz.decorate(Executors.newFixedThreadPool(4));

`decorate` wraps everything submitted through it, so call sites do not have to
remember. `ExecViz.wrap(Runnable)` and `wrap(Callable)` are there for pools that
are not owned by the caller. An unwrapped submission to a pool loses the parent
link, and that is stated rather than papered over.

## Coverage

Without bytecode instrumentation the JVM offers no cheap per-call hook, so this
records explicitly instrumented work plus the propagation around it.

## Verification

`demo.Workload` runs three requests concurrently on a four-thread pool, a worker
claiming stamped jobs off a queue, one failing request, and a lock that never
releases:

    MISATTRIBUTED PARENTS: 0
    links (fan-in): profile_fanin_join 2, ×3
    lifecycle: reconcile_lock suspended, enqueue_job claimed/released
    open spans: reconcile_lock
    conformant: true

Every `fetch_user_N` and `fetch_orders_N` traces back to `GET /profile/N`, even
though all six ran on shared pool threads that were created before any request
existed. That is the decorated executor doing its job.

## Capturing the logs the program already writes

    ExecViz.captureLogs();

    ExecViz.in("handle_request", "call", "api", () -> {
        LOG.info("loading user 42");        // captured, no ExecViz call
        System.out.println("a plain println");
        return null;
    });

Three seams: `java.util.logging`; which the JDK itself uses and which SLF4J,
Log4j and Logback can all be bridged onto; the two standard streams, and the
default uncaught-exception handler, which fires when the program can least
instrument itself.

    info     jul     'loading user 42'
    warning  jul     'cache miss'
    error    jul     'downstream refused'
    info     stdout  'a plain println'
    error    stderr  'and a plain stderr line'

The handler is **added**, not substituted, and the streams are teed rather than
replaced, so every line still goes where it was going. `releaseLogs()` puts back
what was replaced.

**The root logger's own level filters before any handler runs**, so it is widened
to INFO if it was narrower; a visible change to the program's logging, stated
here rather than done quietly.

Message parameters are formatted through `MessageFormat` as the record intended,
and a `LogRecord` carrying a throwable has it appended, because the exception is
usually the reason the line exists.
