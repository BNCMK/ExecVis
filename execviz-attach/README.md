<!--
===========================================================================
  MANIFEST
===========================================================================
  script_name: README.md
  script_path: execviz-attach/README.md
  module_name: README
  version: 0.53.1
  description: Attaching without touching the program
  kind: document
  spec: internal
  internal_dependencies:
  external_dependencies:
  features: README
  api_version: execvis-v1.0.0
  last_updated: 2026-08-07
===========================================================================
-->

# Attaching without touching the program

The adapters ship with this repository; nothing to download. But shipping is
not attaching: until now you still had to add a line to your source, and a line
added to your source is the injection this was supposed to avoid.

Every runtime here can load code from an environment variable. That is the
supported way to attach to a program you are not editing, and it is what these
shims are:

| runtime | how |
|---|---|
| Python | `PYTHONPATH=/path/to/execviz-attach`; CPython imports `sitecustomize` at startup |
| Node | `NODE_OPTIONS="--import file:///path/to/execviz-attach/attach.mjs"` |
| Ruby | `RUBYOPT="-r/path/to/execviz-attach/attach"` |
| PHP | `-d auto_prepend_file=/path/to/execviz-attach/attach.php`, or the same line in `php.ini` |

Then:

    EXECVIZ_COLLECTOR=http://host:8900 python3 app.py

`app.py` imports nothing, is not rebuilt, and does not know this exists.

    span  py     app.py     ok
          info     'python: loading user 42'
          stdout   'python: a plain print'
    span  node   app.js     ok
          info     'node: loading user 42'
          warning  'node: cache miss'
    --- 4 spans, 6 lines, zero source changes

## A process is a unit of execution

The first version of these shims captured nothing, and the reason was the
project's own rule: with no source changes there are no spans, so every line was
dropped for having no parent.

Each shim therefore opens one span for the run itself, closed at exit. That is
not an invented parent; the program really did run, and the run is a true unit
of execution. Lines written inside work the program *does* instrument still
attach to the nearer span; everything else attaches to the process.

## What each runtime made us learn

**Node needs `--import`, not `--require`.** The adapter is an ES module, so a
`--require` shim can only reach it through a dynamic `import`; which resolves
*after* the main module has started. The hooks installed too late to see
anything the program logged. `--import` awaits the file before the program
begins.

**`process.on('exit')` cannot flush.** Exit handlers may not run asynchronous
work and delivery is a fetch, so a flush started there never leaves.
`beforeExit` still allows it.

**Attaching never breaks the program.** Every shim wraps its work in a
try/catch and reports to stderr on failure. A program must not fail to start
because a recorder could not attach.

**Tracing is off unless asked for.** `EXECVIZ_TRACE=1` enables Python's
per-call profile hook. Attaching to a program nobody asked to instrument should
not silently change its performance profile; log capture is nearly free, a
profile hook is not.

## Two runtimes cannot do this

**Go** is compiled: there is no startup hook and no preload, so the adapter must
be called from the source. **The browser** needs a script tag. For both, and for
any binary you have no source to, use the kernel layer in `execviz-syscall/`,
it needs nothing from the program at all.

## What each one wraps

Attaching opens a span for the process, and then wraps the entry point its
ecosystem passes through, so the work becomes visible without the program being
edited or aware of it.

| runtime | inbound | outbound |
|---|---|---|
| Node | `http.Server` request event, with the connection descriptor | `http.request` and `https.request` |
| Python | the WSGI handler, which Flask, Django and Bottle all pass through | `http.client`, which `requests` and `urllib` end in |
| Ruby | Rack, which Rails, Sinatra and Hanami all pass through | `Net::HTTP#request` |
| PHP | the request itself: one process serves one request, so the process span is named and classified as that request, and its status comes from the response code | |

Where the server exposes the connection's descriptor it is recorded on the span.
That is what lets `witness` tie the read that arrived before the handler and the
write that leaves after it back to the same request, which a time window cannot
do for an event loop.
