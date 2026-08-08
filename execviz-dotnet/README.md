<!--
===========================================================================
  MANIFEST
===========================================================================
  script_name: README.md
  script_path: execviz-dotnet/README.md
  module_name: README
  version: 0.53.1
  description: execviz capture adapter for .NET
  kind: document
  spec: internal
  internal_dependencies:
  external_dependencies:
  features: README, capture, adapter
  api_version: execvis-v1.0.0
  last_updated: 2026-08-07
===========================================================================
-->

# execviz capture adapter for .NET

The tenth runtime.

    apt-get install -y dotnet-sdk-8.0
    dotnet build && EXECVIZ_COLLECTOR=http://127.0.0.1:8900 dotnet run

    Capture.Install(null, "dotnet-1", "api");
    await Capture.WithSpan("handle", "call", async () => await Work());
    await Capture.Gather("fanin", () => A(), () => B());
    var msg = Capture.Stamp(item);              // context onto a queued value
    var (item, q) = Capture.Claim(msg);         // read back on the far side
    Capture.Release(q);

## The carrier

`AsyncLocal<T>`. Like Python's contextvars and Node's AsyncLocalStorage it flows
with the *logical call* rather than the thread, which makes it correct
across `await`: a continuation resumed on a pooled thread still sees the span its
caller started.

The failure mode is the mirror image of the JVM's. A `ThreadLocal` on a pooled
thread **leaks** a stale span into unrelated work; `AsyncLocal` cannot, because
the value is captured into the ExecutionContext at the await point rather than
living on the thread. What `AsyncLocal` does *not* do is flow back **out** of an
async method to its caller; a value set inside is invisible outside; which is
why every span here is made current around its work (`WithSpanCurrent`) rather
than merely assigned.

## Verified

    35 spans, conformant, 0 violations
    MISATTRIBUTED PARENTS ACROSS await: 0
    links: profile_fanin_join 2 ×2
    lifecycle: reconcile_lock suspended, enqueue_job claimed/released
    open (death signal): reconcile_lock
    error chain: InvalidOperationException -> [TimeoutException]

Ten runtimes in one graph, all conformant: beam, shell, dotnet, go, jvm, native
(C), node, php, python, ruby.

## No packages

`NuGet.config` clears every package source, so the build never reaches the
network. The adapter depends on nothing outside the base class library, and an
offline restore proves that rather than asserting it.

## Capturing the logs the program already writes

    Capture.CaptureLogs();

    await Capture.WithSpan("handle_request", "call", async () => {
        Console.WriteLine("loading user 42");        // captured
        Console.Error.WriteLine("downstream refused");
    });

`Console.Out` and `Console.Error` are teed,
`Microsoft.Extensions.Logging`'s console provider ends up there too, so it is
captured without knowing anything about it. `AppDomain.UnhandledException`,
`TaskScheduler.UnobservedTaskException` and `ProcessExit` are attached, because
what a program writes as it dies is the line somebody came looking for.

The writer buffers to a line boundary: a console write can arrive one character
at a time, and a log line broken into characters is not a line anybody can read.
An unobserved task exception is **not** marked observed; silencing it would
change what the program does.
