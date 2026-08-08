<!--
===========================================================================
  MANIFEST
===========================================================================
  script_name: README.md
  script_path: execviz-erl/README.md
  module_name: README
  version: 0.53.1
  description: execviz capture adapter for the BEAM
  kind: document
  spec: internal
  internal_dependencies:
  external_dependencies:
  features: README, capture, adapter
  api_version: execvis-v1.0.0
  last_updated: 2026-08-07
===========================================================================
-->

# execviz capture adapter for the BEAM

Erlang and Elixir. The tenth runtime, and the one whose concurrency model fits
this design best.

    erlc -o . execviz.erl workload.erl
    EXECVIZ_COLLECTOR=http://127.0.0.1:8900 erl -noshell -pa . -s workload main

    execviz:install("http://127.0.0.1:8900", <<"beam-1">>, <<"api">>),
    execviz:with_span(handle, call, fun() -> work() end),
    {Pid, Id} = execviz:spawn_span(worker, fun() -> loop() end),
    Msg = execviz:stamp(Item),          % context onto a message
    {Item, Q} = execviz:claim(Msg),     % read back on the far side
    execviz:release(Q).

## The carrier

The cleanest of any runtime here, and the least forgiving. A process owns its
heap and its process dictionary, so **the dictionary is a correct per-process
carrier**: nothing leaks between concurrent processes because nothing is shared.
No task-id check is needed, unlike Python's contextvars.

What it does not do is cross a `spawn` or a message send; a spawned process
starts with an empty dictionary, which is right, because it is a different unit
of execution. The parent span is therefore handed across explicitly, exactly as
at a Ruby thread boundary or a PHP fiber boundary. Inheriting it silently would
be the same mistake as using a frame stack across an `await`.

A message send is the BEAM's real boundary, so `stamp` and `claim` are explicit
there rather than hidden: the runtime makes the crossing visible and the adapter
should not pretend otherwise.

Buffering is one process holding every span, which is how the BEAM has shared
state without a lock. Delivery never crashes the program being observed: if the
buffer is gone, recording is a no-op.

## Verified

    35 spans, conformant, 0 violations
    MISATTRIBUTED PARENTS: 0        (across processes, not just across calls)
    links: profile_fanin_join 2 ×3
    lifecycle: reconcile_lock suspended, enqueue_job claimed/released
    open (death signal): reconcile_lock
    logs attributed: 7

Nine runtimes in one graph, all conformant: beam, build (shell), go, jvm, native
(C), node, php, python, ruby.

## See also

**.NET** is built too; see `execviz-dotnet/`. An earlier version of this file
said it was unreachable from this environment; that was wrong. `dotnet-sdk-8.0`
is in Ubuntu's main archive, and the claim came from reading a truncated
`apt-cache policy` rather than from testing.

## Capturing the logs the program already writes

    execviz:install(Collector, <<"beam-1">>, <<"api">>),
    execviz_logger_h:install(),

    execviz:with_span(handle_request, call, fun() ->
        logger:info("loading user ~p", [42]),      %% captured, no execviz call
        logger:error(#{event => downstream_refused, code => 503})
    end).

**The cleanest seam of any runtime here.** Every OTP library, and every
application built on one, already reports through `logger`. There is nothing to
patch, no stream to tee and no format to guess: adding a handler is the supported
way to observe logging, so this adds one. It is additive, so the program's own
handlers keep receiving everything they received before.

All three message shapes OTP delivers are ordinary here; a format string with
arguments, a plain string, and a structured report rendered through its own
`report_cb` when it has one:

    info     'loading user 42'
    warning  'cache miss'
    error    '#{code => 503,event => downstream_refused}'

**The primary level filters before any handler runs.** OTP defaults it to
`notice`, so `logger:info(...)` is discarded before this could ever see it; the
common case is losing exactly the lines somebody wanted attributed. The level is
widened deliberately and said so here rather than done quietly, and a program
that has chosen its level on purpose declines with
`execviz_logger_h:install(#{raise_primary => false})`.

**A handler that raises is removed by OTP.** A recorder that gets itself removed
for misbehaving has failed twice: once at the line it dropped, and once at every
line after it. The callback catches everything.
