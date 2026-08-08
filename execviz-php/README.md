<!--
===========================================================================
  MANIFEST
===========================================================================
  script_name: README.md
  script_path: execviz-php/README.md
  module_name: README
  version: 0.53.1
  description: execviz capture adapter for PHP
  kind: document
  spec: internal
  internal_dependencies:
  external_dependencies:
  features: README, capture, adapter
  api_version: execvis-v1.0.0
  last_updated: 2026-08-07
===========================================================================
-->

# execviz capture adapter for PHP

Reports spans from a PHP request to a collector, using the same wire
format every other runtime uses.

    php workload.php        # EXECVIZ_COLLECTOR=http://127.0.0.1:8900

    $ev = Execviz::install(null, 'php-1', 'api');

    $ev->span('GET /profile', 'call', function () use ($ev) {
        $ev->gather('fanin', [fn() => fetchUser($ev), fn() => fetchOrders($ev)]);
    });

    $worker = $ev->fiber('worker', fn() => work());   // inherits its creator

    $jobs[] = $ev->stamp($item);                      // stamp the crossing
    [$item, $qid] = $ev->claim(array_shift($jobs));   // read it back
    $ev->release($qid);

## The carrier

PHP's carrier problem is the opposite of everyone else's. A classic request is
one synchronous flow in one process, so a plain stack is a valid parent chain
and this adapter keeps one. Fibers change that: a fiber suspends and resumes
with its own stack, so a span opened inside one belongs to that fiber rather
than to whatever resumed it. Each fiber therefore gets its own stack, keyed by
the fiber object, and the main flow keeps its own.

Delivery survives a dying request. A shutdown handler flushes whatever is
pending, and spans that never completed stay open, which makes a fatal
error visible rather than silent.

## Coverage

Without an extension PHP offers no per-call hook, so this records explicitly
instrumented work plus the propagation around it. That is a property of the
runtime, stated rather than glossed over.

## Verification

`workload.php` runs three requests that fan in, a fiber worker claiming stamped
jobs off a queue, one failing request, and a lock that never releases:

    MISATTRIBUTED PARENTS: 0
    links: profile_fanin_join 2 ×3
    lifecycle: reconcile_lock suspended, enqueue_job claimed/released
    open spans: reconcile_lock
    conformant: true

## Capturing the logs the program already writes

    Execviz::i()->captureLogs();

    Execviz::i()->span('handle_request', 'call', function () {
        echo "a plain echo\n";                        // captured
        trigger_error('cache miss', E_USER_WARNING);   // captured
    });

PHP has no root logger, and what a program writes ends up in three places: the
error handler (`trigger_error`, and everything the engine raises), the exception
handler, and the output buffer. All three are attached, plus a shutdown hook,
a fatal error reaches neither handler, and that is the only seam left for it.

**Handlers are chained, not replaced.** Whatever the program had installed still
runs and still decides what the engine does next; the error handler returns
false so PHP's own reporting proceeds exactly as before.

**The output buffer uses chunk size 1.** With 0 the callback fires only when the
buffer flushes; at shutdown, by which time every span has ended and the line has
nothing to attach to. Size 1 makes it fire as the program writes, while the span
that wrote it is still running. The callback returns false, so the buffer passes
through untouched and the page renders identically.
