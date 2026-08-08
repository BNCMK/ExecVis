<!--
===========================================================================
  MANIFEST
===========================================================================
  script_name: README.md
  script_path: execviz-c/README.md
  module_name: README
  version: 0.53.1
  description: execviz capture for native code
  kind: document
  spec: internal
  internal_dependencies:
  external_dependencies:
  features: README, capture
  api_version: execvis-v1.0.0
  last_updated: 2026-08-07
===========================================================================
-->

# execviz capture for native code

Single header, no dependencies beyond libc.

    #define EXECVIZ_IMPLEMENTATION
    #include "execviz.h"

    execviz_init("http://collector:8900", "svc-1", "orders");
    execviz_span s = execviz_begin("charge", "io", 0);
    execviz_end(s, EXECVIZ_OK);
    execviz_flush();

C, C++ and Rust have no managed runtime to interpose on, so there is nothing to
attach to automatically. The recorder still sees every write. What these three
calls add is **semantics**: the syscall layer sees a write, and only the program
knows it was a checkout.

Three calls on purpose; begin, end, fail. Anything larger would not survive
contact with three languages and every build system they use.

## What it will not do

**It starts no threads.** A capture layer that spawns a thread inside the program
it is measuring has changed the thing it observes.

**It never grows into the program's memory.** The span table is fixed and bounded;
past the bound it records nothing rather than allocating without limit.

**It copies, never references.** A message is copied at the call, so a buffer
freed afterwards cannot rewrite history.

A frame stack per thread is a valid carrier here, unlike in the async runtimes,
because C has no await to break it.

## Verified

    decode_frame        call   ok       65ms
      parse_header        call   ok       20ms
      read_block          io     ok       35ms
      checksum            call   error    10ms   checksum mismatch at offset 4096
      awaiting_device     wait   running  OPEN

Compiled with `-Wall -Wextra`, conformant with zero violations, and the span that
was never ended is still open; an unfinished span in a language with no
exceptions.
