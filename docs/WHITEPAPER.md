<!--
===========================================================================
  MANIFEST
===========================================================================
  script_name: WHITEPAPER.md
  script_path: docs/WHITEPAPER.md
  module_name: WHITEPAPER
  version: 0.53.1
  description: ExecVis: one capture, one map, and an honest account of what it could not see
  kind: document
  spec: internal
  internal_dependencies:
  external_dependencies:
  features: WHITEPAPER, capture
  api_version: execvis-v1.0.0
  last_updated: 2026-08-07
===========================================================================
-->

# ExecVis: one capture layer, one map, and a stated account of what it missed

## Summary

A working observability stack is usually six or seven products: a syscall
recorder, an auto-instrumentation agent, a tracing SDK, a profiler, a rules
engine, and a query layer. Each ships its own agent and its own data model, and
each is billed separately.

ExecVis covers the same ground with one capture layer and one model. It records
what every process on a machine wrote, from below libc, with nothing installed
into the programs it watches. The map, the conformance audit, the decoder, the
fingerprint, the query language and the detection engine all read that one data
set.

The consequence is a comparison the assembled version cannot make. Spans and
syscalls sit in the same model, so a span can be checked against the syscalls
its thread made, and work no span covers can be drawn rather than inferred.
Across seven products that is an integration project; here it is a subtraction.

Two sentences carry the difference. The first is what most tools claim in some
form: it records what every process wrote. The second is the one no other tool
says: it then tells you what it could not see.

It is released free, under AGPL-3.0, with no tier above it and no telemetry
inside it. That is not a growth tactic, and
it is addressed at the end rather than the front.

## Who this is for

Network and infrastructure engineers first. The capture is system-wide and
takes no target, so the unit it reasons about is the machine and, through
digest-synced peering, the fleet. The questions it was shaped to answer are
network-and-host questions: what is every process on this box doing on
the wire and on disk, which of that is accounted for by something someone is
monitoring, and what changed on a host after a deploy without a release to
explain it.

Below that, in descending order of fit:

- **SRE and platform teams**, for a live fleet map that moves kilobytes rather
  than gigabytes, and for detections that fire on the shape of a failure rather
  than on a single metric crossing a line.
- **Application and backend engineers**, for a trace that can be checked against
  reality, and for logs and protocol traffic read from the kernel without an
  agent in the request path.
- **Security and supply-chain teams**, for identity derived from behaviour
  rather than from a label, so an unlabelled process can be named by what it
  does and a binary that changed shape without a release becomes a question.
- **Anyone debugging a machine they did not prepare**, because the syscall recorder needs
  no import, no flag, no config file, and no restart of the programs it records.

## The problem with an assembled stack

A conventional stack is a chain of derivations. The service map is built from
spans, the dashboard from the map, the alert from the dashboard. Each layer takes
the one below it as given, because it has nothing else to compare against. If the
span at the bottom describes work that never happened, every layer above it
reports that work faithfully, and the reporting looks the same as it does when
the span is correct.

Two failures follow, and both are common:

1. **The instrumentation lies and nothing catches it.** A span reports a
   database call that was served from cache and issued no socket write. A span
   declares a duration that does not contain the work attributed to it. The
   trace looks complete. It is complete and wrong, and every tool downstream
   inherits the error.

2. **The instrumentation is silent and the silence looks like health.** A
   process nobody instrumented runs, writes files, opens sockets, and appears
   nowhere. A decoder fails on a protocol variant and the traffic it can no
   longer read looks identical to a service that went quiet. Absence and
   healthy-quiet are indistinguishable, an ambiguity a monitoring
   tool must not have.

An assembled stack cannot fix either, because fixing them requires a source of
truth outside the instrumentation and a discipline that treats absence as a
reported fact rather than a blank. execviz has both by construction: the kernel
is the source of truth, and the honesty rules are enforced by the tests.

## How it works, in one paragraph

A recorder records syscalls system-wide using eBPF, reading the write buffer out of
user memory and classifying every write by kind (text, binary, signal, blank,
empty) with its true byte count, never dropping one because it did not look
useful. Optional per-runtime adapters add the one thing the kernel cannot know:
which span was running when a line was written. Everything above the recorder,
the map, the audit, the decoder, the fingerprint, the query language, the
detection engine, the export, is a read over that single capture. No part
re-collects the data. That is what lets the parts agree, and what lets one part
check another.

## The capability teardown

By capability, against the tools each capability is usually bought from.

Legend: ● full · ◐ partial or adjacent · ○ absent.

| capability | sysdig | Pixie | Datadog | rr / Pernosco | ExecVis |
|---|---|---|---|---|---|
| System-wide syscall capture, no target | ● | ◐ | ○ | ○ | ● |
| Capture from below libc, no source change | ● | ● | ◐ agent | ○ | ● |
| Every descriptor, not just stdout/stderr | ● | ◐ | ◐ | ○ | ● |
| Classifies writes rather than filtering them | ○ | ○ | ○ | ○ | ● |
| Protocol decoding at the kernel | ◐ | ● | ● | ○ | ◐ decoders + residue |
| Reports the fraction it could **not** decode | ○ | ○ | ○ | ○ | ● |
| One continuous map: host→cluster→family→span | ○ | ◐ service map | ◐ dashboards | ○ | ● |
| Draws the work nobody instrumented | ○ | ○ | ○ | ○ | ● |
| Checks instrumentation against the kernel | ○ | ○ | ○ | ○ | ● |
| Behavioural identity, no label needed | ○ | ○ | ○ | ○ | ● |
| Drift / substitution detection from behaviour | ○ | ○ | ◐ anomaly | ○ | ● |
| Detection on shape, not on a syscall or metric | ○ | ○ | ◐ monitors | ○ | ● |
| Query language with honesty enforced by the grammar | ◐ filters | ● PxL | ● | ○ | ● |
| Record / replay of execution | ◐ syscalls | ○ | ○ | ● full program | ◐ the record, stated |
| Fleet federation that moves only diverging digests | ○ | ○ | ○ | ○ | ● |
| Standard export (OpenTelemetry), losses named | ◐ | ◐ | ● | ○ | ● |
| Names its own limits in its output | ○ | ○ | ○ | ◐ | ● |
| Free, AGPL-3.0, no telemetry, no tier above | ◐ CE | ◐ CNCF | ○ | ○ | ● |

Read across any single row and several tools tie. Read down the execviz column
and the pattern is reported: the rows only execviz fills are the ones that
require both a kernel-level source of truth and a discipline of reporting
absence. No assembled stack has that pairing, because the pairing is an
architectural property, not a feature that can be added.

A fair reading of the table also concedes where others lead. rr and Pernosco
replay the actual program and can evaluate an unrecorded expression; execviz
replays the record and reports it. Pixie and Datadog ship more protocol decoders
today. Datadog is a managed product with support and retention that a
free tool does not offer. None of that is the axis execviz competes on.

## Two flamegraphs, kept apart

`flame` folds the span tree by measured self time. Overlapping children are
merged before subtracting, so a parent with two concurrent children reports the
time it spent in itself rather than a negative number. The result is exact for
instrumented work and empty for everything else.

`execviz-cpu` samples with `perf_event_open` on the software CPU clock, one
event per CPU, with `PERF_SAMPLE_CALLCHAIN`. A function nobody wrapped appears
here. `execviz cpu` folds those samples. Both emit the standard folded format,
so both open in speedscope or flamegraph.pl.

They are never merged. Averaging a measured duration with a sample count
produces a figure that is neither, and a reader cannot tell afterwards which
half a frame came from.

Sampled frames are addresses. Resolving one needs the symbol table of whatever
mapped it, and the tool does not carry symbolisation. Parca and Pyroscope do.

`critical` walks the chain that set a request's duration: from each span, the
child that finished last. Work that overlapped that chain cost no wall time, and
a list of slow spans cannot express the difference.

## What the decoder reads

HTTP requests and responses, the HTTP/2 connection preface, gRPC, DNS, MySQL,
PostgreSQL, Cassandra, Redis RESP, bare SQL, and JSON.

Each binary decoder checks a declared length against the bytes present, and a
body against the command that declared it: a query carries text, a ping carries
nothing. The recorder bounds what it copies, so a captured buffer is usually
shorter than the packet declares; what is checkable is that it is never longer
than the whole message.

Random bytes are a weak test for this. Real binary has structure, and a five
byte prefix followed by a legal protobuf tag occurs throughout ELF headers and
library data. On a capture containing no gRPC, a structural match claimed 88
buffers. gRPC is now claimed only where its wire marker is present, and a bare
protobuf message with no marker is left undecoded. On a capture of real DNS
traffic and ordinary binary, the decoders produce zero false matches.

HTTP/2 bodies are not reassembled. That needs HPACK state tracked across frames,
and the recorder sees what crossed the syscall boundary unframed. The preface is
recognised and the rest counts as undecoded.

## What only the combination produces

Four capabilities exist because both layers exist over one model. None can be
bolted onto a single-layer tool.

### 1. The recorder as witness

Spans are supplied by an adapter. `execviz-attach` provides them for Python,
Node, Ruby and PHP through an environment variable, with no change to the
program. On a machine with nothing installed, `drift` asks a narrower version of
the same question by comparing a process against its own stored behavioural
fingerprint, and reports a shape that moved with no corresponding release.

Every tracing product in the field takes a span's word for what happened. The
floor knows what the kernel recorded: which syscalls occurred, on which thread, in
which window. Cross-checking the two answers a question nothing else can ask.

- **Work claimed but not performed.** A span reporting a database call whose
  thread issued no socket write in that window. Either the instrumentation is wrong, or
  the call was cached; either is reported.
- **Work performed but unclaimed.** Syscalls on a thread no span covers. The
  trace is not wrong, it is incomplete, and it says where.
- **Timing that disagrees.** A span whose declared duration does not contain the
  syscalls attributed to it.

Nothing here convicts. Each finding carries what it is consistent with, and the
audit exits with a failure code on a lie but not on merely incomplete coverage.
On a real capture: fifty-one spans, 90.9% coverage, a planted lying span caught.

### 2. The negative-space map

Every observability product draws what was instrumented. None draw what was not.
The recorder sees every process on the machine; spans cover some of them; the
difference is a real, drawable region. Processes running, files being written,
sockets in use, that no instrumented unit of work accounts for, named by the
program responsible rather than by a numeric thread id. For a network engineer
this is the direct answer to "what is this box doing that nobody is watching."

### 3. Identity by behaviour

The fingerprint identifies a program from six invariants of its execution shape,
producing roughly 300x separation between programs. Computed from recorder records,
it needs no instrumentation and no metadata, which yields three things: name an
unlabelled process by what it does, detect drift when a service's shape moves
after a deploy with no assertions written in advance, and notice a substitution
when a binary's behavioural shape changes without a corresponding release. The
last is a supply-chain question answered from data already being collected.

### 4. Stress derived from what the program does

Fault injection elsewhere is authored: the operator names the syscall and the
failure. Here the plan is read out of a capture. Reads that came back short imply
a short-read test; socket calls imply a peer that stops answering; many
descriptors imply exhaustion; blocking calls imply interruption. Stressors the
shape does not support are excluded, each with the evidence that was absent.

`execviz-stress` carries a plan out with seccomp user notification against an
unmodified program, taking the injection rate and the startup call count from the
capture. `short_read` performs the read and returns fewer bytes, writing those
bytes into the program's own buffer. The stressed run is captured by the recorder
and compared against the unstressed one, and `witness` and `detect` report
span-level effects.

Three stages over one capture. Assembled from separate products, the injector
does not know what the program does and the observability tool does not know a
fault was injected.

### 5. Detection on shape

A rules engine over a stream fires on a value. The map holds causality,
identity, and coverage, so the same engine fires on shapes a syscall stream
cannot express: a span that opened and never closed while its route kept moving,
a fan-in whose children outlived their join, a fingerprint that diverged from the
same service's prior run, a host whose unattributed fraction jumped after a
deploy. Every finding carries the evidence it saw, and a rule whose evidence is
absent reports it rather than passing silently. A rules file with a typo that
matches nothing exits with an error, because a system with no problems and a
rule that never ran must not look the same.

## What the tool refuses to state

The test harness plants the failures the tool claims to catch and fails if any
is missed.

- **Absent values are reported as absent, not as zero.** Unmeasured cost, unrecorded values, and missing
  coverage all say so rather than reading as data.
- **Detect, estimate, report, never silently correct.** Clock skew between hosts
  is measured and reported, never quietly adjusted away.
- **Co-occurrence is not cause.** A correlation result carries no field named
  cause, by construction.
- **Name the limit in the output.** The decoder reports the fraction it could
  not read. The export names what the model cannot carry. The replay states that
  it replays the record, not the program. The security document names io_uring
  as a boundary the syscall floor does not cross, unprompted.

Every capture reports its decoded residue: the fraction of traffic understood
against the fraction that passed through unread.

## Limits

- The recorder requires Linux kernel 5.8 or newer, with CAP_BPF and CAP_PERFMON,
  and a machine not in secure-boot lockdown. It carries register tables for
  x86_64 and aarch64 and refuses any other architecture by name. The x86_64
  table is proved against the running kernel; the aarch64 table cross-builds
  and is proved the same way by running `--selfcheck` on that hardware, which
  has not been done yet. Windows is on the roadmap, not in the first release.
  `execviz doctor` reports whether a given machine qualifies and, if not, the
  exact reason and fix.
- Confirmed on: x86_64, kernel 6.18, Ubuntu 24.04, with
  continuous integration on Ubuntu 22.04 and 24.04. Every other line of the
  compatibility table needs confirmation, and reports it.
- The collector, the map, and the adapters are portable and unprivileged; only
  the recorder is fussy. A capture produced anywhere can be read anywhere.
- io_uring bypasses the syscall boundary and is a real hole. The 176-byte
  payload slice is bounded and marks truncation rather than hiding it. Capture
  is not prevention.
- Self-observation is corroboration, not proof: the recorder records its own
  syscalls, and on an idle run that is nine records about itself and zero network
  calls, but a dishonest build could omit itself. The way to trust it is not to
  trust it: run tcpdump beside it, apply a default-deny egress rule, build it
  reproducibly.

## Why it is free

execviz exists because building complex systems required an observability suite
that did not exist. Releasing it costs little and returns
goodwill and bug reports from people running it on machines and kernels no single
author can cover. There is no paid tier withheld above it and no telemetry
inside it. The licence is AGPL-3.0, chosen so that improvements come back:
a modified copy stays free whether it is shipped or run as a service over a
network. Tips are welcomed and change nothing about what is available.

Free is the last thing said about it on purpose. The reason to adopt it is the
second sentence at the top: it records what every process wrote, and then it
tells you what it could not see. Nothing else on the market says the second
half.
