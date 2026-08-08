<!--
===========================================================================
  MANIFEST
===========================================================================
  script_name: README.md
  script_path: execviz-syscall/README.md
  module_name: README
  version: 0.53.1
  description: execviz syscall adapters
  kind: document
  spec: internal
  internal_dependencies:
  external_dependencies:
  features: README, adapter
  api_version: execvis-v1.0.0
  last_updated: 2026-08-07
===========================================================================
-->

# execviz syscall adapters

The syscall stream sees what the semantic stream cannot: work that leaves the
runtime. Two mechanisms, chosen by what the host permits.

    gcc -O2 -o execviz_bpf execviz_bpf.c
    gcc -O2 -fPIC -shared -o execviz_preload.so execviz_preload.c -ldl

## Kernel tracepoints

    ./execviz_bpf <pid> --host dev-a > syscalls.ndjson

Attaches a BPF program to the raw syscall entry tracepoint, filtered to one
process, writing records into a ring buffer this process drains. Sees every
syscall, including ones issued by native code the runtime never learns about.
Requires privilege the host may not grant.

The program is hand-assembled rather than compiled from C, because a BPF
compiler is not a reasonable dependency for an adapter that has to run wherever
the traced program runs.

## Library interposition

    EXECVIZ_SYSCALL_OUT=syscalls.ndjson EXECVIZ_HOST=dev-a \
      LD_PRELOAD=./execviz_preload.so ./your-program

Wraps the call sites ahead of libc, forwards to the real implementation, and
records around it. Needs no privilege. Sees only what goes through libc: a
static binary or a direct syscall is invisible to it. It does carry a duration
per call, which the tracepoint path does not, since it observes both sides.

## Merging

    execviz syscalls run.db --records syscalls.ndjson --apply

Each record carries a thread id and a timestamp and is attributed to the
innermost semantic span running on that thread at that instant. Attribution is
by observation rather than inference, and a record with no span around it is
counted against the host instead of being attached to whatever was nearby.

Enrichment only: the semantic span keeps its identity and gains `syscalls`,
`syscall_count` and, where the mechanism measured it, `syscall_ms`. It is never
redefined by the syscall stream.

A syscall a mechanism does not have a name for keeps its number rather than
being given an invented one.

## Coverage, stated

A capture records which mechanism produced it, because a gap in interposition
coverage and a gap in the program are different findings.

One consequence reported: the store writes to SQLite in the traced process,
so a tracepoint capture attributes those writes to whichever span was running.
They are real syscalls that really happened, but they belong to the recorder
rather than the program. Running the store out of process removes them.

## Capturing a program's output with no source at all

    EXECVIZ_SYSCALL_OUT=out.ndjson LD_PRELOAD=./preload.so ./any-program

A write to fd 1 or 2 is not merely a syscall; it is a log line, and this library
is already standing where it is written. `write` and `writev` are both wrapped,
multi-line writes are split, and the text is escaped through the same helper the
host name uses, because a program's output is arbitrary bytes and arbitrary bytes
in a JSON string is how a record becomes unparseable.

This is the only place in the suite where log capture needs no cooperation
whatsoever: no handler to install, no stream to replace, no code to change, and
it works on a binary nobody has the source to.

### What it does not catch, measured

A C program emitting five lines by `puts`, `fputs`, `fwrite`, `write` and
`printf` had exactly **one** captured; the direct `write`. glibc resolves its
own stdio internally, so those calls never pass through the dynamic symbol table
where `LD_PRELOAD` can reach them. Wrappers for them were written, measured, and
removed: code that does not do its job differs from no code, because the next
reader assumes it works.

So this captures **direct `write` and `writev`**; which covers shell scripts,
Go programs, and anything writing to a descriptor itself. A C program that wants
its stdio output attributed calls `execviz_log` from `execviz-c/execviz.h`, which
is one line and always works.

## The recorder; this is the capture layer

    execviz-record --host $(hostname)          # every process on the machine
    execviz_bpf <pid> --follow                # or narrow it to one program

It takes **no target**. Naming a pid narrows it, which helps when debugging one
program, but narrowing is the exception: nothing has to opt in, ask, or know
this is here. `execviz-record.service` runs it before the programs it records.

    16 lines captured system-wide, nothing opted in
    process_api  stdout        info   '[DEBUG] Found orphan process 445'
    noise.sh     stdout        info   'shell to stdout'
    python3      stdout        info   'python to stdout'
    python3      /tmp/app.log  info   'python to a FILE'
    node         /tmp/app.log  info   'node to a FILE'
    pf           stderr        error  'via fprintf'
    pf           stdout        info   'via printf'

Shell, Python, Node and C; plus a process that had nothing to do with the test
and was captured because it happened to be running. Files as well as terminals.

**Every descriptor, not only stdout and stderr.** A service logging to
`/var/log/app.log` or through a socket to journald is how most production
software logs, and watching only fds 1 and 2 made all of it invisible.
The descriptor is resolved to a path, and the writing program is named.

**Nothing is dropped; everything is classified.** Watching every descriptor
means seeing an eventfd counter and a pipe carrying one byte to wake a loop.
Those are things a program did, so they are recorded; with a `kind` saying what
they are, because deciding they are not worth showing is deciding for the
developer what they are allowed to look at.

    43 writes captured, nothing dropped
    by kind: {'binary': 26, 'text': 16, 'signal': 1}

    binary  process_api  fd4                    8B '0100000000000000'
    text    process_api  stdout                32B '[DEBUG] Found orphan process 4'
    signal  node         pipe:[783]             1B '2a'
    binary  node         anon_inode:[eventfd]   8B '0100000000000000'
    text    pf           stderr                11B 'via fprintf'

`text` is escaped and stays readable. `binary` and `signal` are rendered as hex,
which is lossless and reversible rather than reduced to "something happened
here". Every record carries `bytes`, so a truncated payload still reports its
true size. Sort or filter by `kind` in the log workspace; together or
separately.

**The recorder does not record itself**; this process writes every line it
captures, and capturing those is a loop that ends with the machine full.

**Delivery cannot stall the machine.** The capture writes NDJSON to a file and a
separate reader feeds the collector, so a collector that is down can never apply
back-pressure to the processes being observed.

This is the one that needs nothing from the program: no adapter, no flag, no
source, no cooperation. A log line ends in `write(2)` whatever language wrote it
and whatever library it went through, so the BPF program reads the buffer out of
user memory at syscall entry; before libc buffering, before any logging
framework.

    pid    571 info   'shell: starting'
    pid    577 info   'python: loading user 42'
    pid    577 error  'python: cache miss'
    pid    578 error  'via fprintf'
    pid    578 info   'via printf'
    pid    578 info   'via puts'
    pid    579 info   'node: downstream refused'
    --- 8 lines from 4 processes, one kernel probe, zero adapters

**`printf` cannot escape it.** The `LD_PRELOAD` layer catches one line of five
from a C program, because glibc resolves its own stdio internally and never
passes through the dynamic symbol table. The kernel is below all of that: the
same three lines the preload missed arrive here without trying.

**`--follow` keeps the family.** A shell script that runs Python and Node is four
processes, and the interesting output is in the three it started. With `--follow`
the kernel filter stands down and userspace keeps the descendants, walking the
parent chain in `/proc`; a verified program has no business looping over `/proc`,
so that part is done where looping is allowed.

### What this layer cannot do

It knows the process and the thread; it does not know which span was running.
That is what the per-runtime adapters are for, and it is why both exist: **the
kernel layer catches everything, the adapters say what it belongs to.** A capture
with both has lines attributed where a span was running and lines still recorded
where none was.

Records carry `truncated` when a write was longer than the slice carried up, and
a line cut short that does not say so is a line that lies.
