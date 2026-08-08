<!--
===========================================================================
  MANIFEST
===========================================================================
  script_name: README.md
  script_path: README.md
  module_name: README
  version: 0.53.1
  description: ExecVis
  kind: document
  spec: internal
  internal_dependencies:
  external_dependencies:
  features: README
  api_version: execvis-v1.0.0
  last_updated: 2026-08-07
===========================================================================
-->

# ExecVis

Records what every process on a machine did, draws it as one map with
one clock, and tells you what it could not see.

Free, AGPL-3.0, no account, no telemetry, no cloud.

**v0.53.1, beta.** `docs/COMPATIBILITY.md` lists the kernels and distributions
confirmed to run the recorder. If yours is not there, `execviz doctor --report`
answers the question in one command and is safe to paste in public.

## Why this exists

A trace covers the code somebody instrumented. A log covers the lines somebody
wrote. When either is incomplete, nothing in the system reports that: the gap
looks the same as an absence of work.

The recorder records syscalls for every process on the machine, so the record does
not depend on anyone having anticipated the question beforehand.

Spans and syscalls are held in one data model, not joined across two tools. That
makes one comparison possible: a span can be put against the syscalls its own
thread made, and the difference between what was claimed and what the kernel saw
is a number rather than an argument. The same model is what lets total work be
differenced against instrumented work, and lets a fault be derived from a
capture and then injected against it.

## What makes it different

**It checks your instrumentation against the kernel.** `witness` puts every span
against what the kernel observed on that thread in that window and reports three
things separately: work claimed but not performed, work performed but not
claimed, and windows that disagree. It exits 1 on the first. This is a comparison
inside one data model, not an integration between two products.

**It draws what nothing instrumented.** `unclaimed` shows the work no span
accounts for, named by program rather than by thread id, with the covered
fraction beside it.

**It derives its own stress tests.** `stress` reads a capture and reports which
faults the program's shape implies and which it excludes, then `execviz-stress`
carries one out below libc against an unmodified program, then the same map
reports what changed.

**It watches from below the C library.** A log line ends in `write(2)` whatever
language wrote it, so a kernel probe sees it without the program cooperating,
being recompiled, or knowing. `printf` cannot escape it.

**It shows where the time went, two ways, and says which is which.** `flame`
folds the span tree by measured self time: exact for instrumented work, blind to
the rest. `execviz-cpu` samples the machine on a timer with call chains, so a
slow function nobody wrapped appears there and nowhere else, and `cpu` folds
those samples. Both emit the standard folded format, so they open in speedscope
or flamegraph.pl. They are reported separately rather than merged, because
merging an exact measurement with a statistical one produces a number that is
neither. `critical` walks the chain that set the duration, since adding up
everything slow answers nothing when the work overlapped.

**It reads the wire, and reports what it could not read.** `decode` handles HTTP
requests and responses, the HTTP/2 preface, gRPC, DNS, MySQL, PostgreSQL,
Cassandra, Redis RESP, bare SQL and JSON, and prints the fraction of bytes it
could not parse beside the fraction it could. `iouring` counts submissions that
bypass the syscall boundary and reports them as work the capture does not
contain.

**It checks your instrumentation against the kernel.** `witness` puts every span
against what the kernel observed on that thread in that window, and reports three
things separately: work claimed but not performed, work performed but not
claimed, and windows that disagree. It exits 1 on the first.

## Against the alternatives

`●` has it, `◐` partly, `○` does not.

| capability | sysdig | Pixie | Datadog | Parca | ExecVis |
|---|---|---|---|---|---|
| System-wide syscall capture, no target named | ● | ◐ | ○ | ○ | ● |
| Capture from below libc, no source change | ● | ● | ◐ agent | ● | ● |
| Every descriptor, not just stdout and stderr | ● | ◐ | ◐ | ○ | ● |
| Protocol decoding at the kernel | ◐ | ● | ● | ○ | ● |
| Reports the fraction it could **not** decode | ○ | ○ | ○ | ○ | ● |
| Sampled CPU profiling with call chains | ○ | ● | ● | ● | ● |
| Symbol resolution built in | ○ | ● | ● | ● | ○ |
| Flamegraph from measured time, not samples | ○ | ○ | ○ | ○ | ● |
| Critical path through concurrent work | ○ | ○ | ◐ | ○ | ● |
| One map from fleet to single call | ○ | ◐ service map | ◐ dashboards | ○ | ● |
| Draws the work nobody instrumented | ○ | ○ | ○ | ○ | ● |
| Checks instrumentation against the kernel | ○ | ○ | ○ | ○ | ● |
| Derives its own stress tests from a capture | ○ | ○ | ○ | ○ | ● |
| Behavioural identity, no label needed | ○ | ○ | ○ | ○ | ● |
| Blocks as well as observes | ○ | ○ | ○ | ○ | ○ |
| Years of production behind it | ● | ● | ● | ● | ○ |
| Free, no tier above, no telemetry | ● | ● | ○ | ● | ● |

The last three rows are the honest ones. This does not block, it has no
production history, and Pixie and Parca resolve symbols where this emits
addresses for a later pass. sysdig has shipped system-wide capture since 2014 on
fleets larger than anything this has run on.

## Running it

Requires x86_64 Linux, kernel 5.8 or newer. Nothing is installed and nothing is
copied into a system directory.

From a release archive, extract and run:

    tar xf execvis.tar.gz
    cd execvis
    ./execviz doctor

From a clone there is no binary yet, so build the two parts. Rust and Node are
the only prerequisites, and both builds write inside the tree:

    git clone https://github.com/BNCMK/ExecVis.git && cd ExecVis
    cargo build --release --manifest-path execviz-rs/Cargo.toml
    cc -O2 -static -o execviz-record execviz-syscall/execviz_bpf.c
    cc -O2 -static -o execviz-cpu    execviz-syscall/execviz_cpu.c
    ( cd execviz-ui && npm install && npm run build )
    ./execviz-rs/target/release/execviz doctor

Some archives and clones lose the executable bit on the shell scripts. `bash
verify.sh` works regardless; to restore them:

    find . -name '*.sh' -exec chmod +x {} \;

`doctor` reports whether this machine can run the syscall recorder and, if it cannot, names
what is missing and how to fix it. It installs nothing and changes nothing.

    ./execviz serve --collect --ui execviz-ui/dist/index.html

That serves the map on port 8900. Create an account first, since reaching it over
a network requires one:

    ./execviz account run.db create alice --password <password>

The recorder is the only part that needs privileges, and only because reading the
syscalls of other processes does. Run it with `sudo`, or grant the two
capabilities once so it does not need root again:

    sudo setcap cap_bpf,cap_perfmon+ep ./execviz-record

The collector and the map need no privileges and no particular architecture, so
they run anywhere, including where the recorder cannot. `docs/DEPLOYING.md` has the
requirement table; `docs/COMPATIBILITY.md` lists which distributions have been
confirmed rather than assumed.

## Five minutes

**Start the collector and open the map.**

    execviz serve capture.db --port 8900 --collect --ui /usr/local/share/execviz/ui.html

Open `http://localhost:8900`. Scroll to zoom, drag to pan, double-click to dive
in, right-double-click to come out. It is one world: hosts contain regions,
regions contain clusters, clusters resolve into spans. The layout does not move
when the data changes.

**Record the machine.**

    execviz-record --host $(hostname) > syscalls.ndjson

Every process, every descriptor, nothing opted in. Writes to stdout, stderr and
to files are all captured, each classified as text, binary, signal or blank, with
its true byte count.

**Ask what it saw.**

    execviz unclaimed capture.db --records syscalls.ndjson    # what nothing accounts for
    execviz decode --records syscalls.ndjson                  # protocols, and the residue
    execviz identity --records syscalls.ndjson                # who these processes are, by behaviour

**Check the instrumentation against reality.**

    execviz witness capture.db --records syscalls.ndjson

Exits 1 if a span claimed work its thread never performed. Suitable for CI.

**Query the capture directly.**

    execviz ask capture.db --q "from spans group by kind show count max(duration) sort by count desc"

It refuses `median` and any percentile, at parse time, with the reason: they are
not monoids, and a tier built from tiers would be wrong.

## Instrumenting your own code

Optional. The recorder works without it; adapters add **which unit of work** a line
belongs to, which the kernel cannot know.

Attach without touching your source:

| runtime | how |
|---|---|
| Python | `PYTHONPATH=/path/to/execviz-attach` |
| Node | `NODE_OPTIONS="--import file:///path/to/execviz-attach/attach.mjs"` |
| Ruby | `RUBYOPT="-r/path/to/execviz-attach/attach"` |
| PHP | `-d auto_prepend_file=/path/to/execviz-attach/attach.php` |

Then `EXECVIZ_COLLECTOR=http://host:8900 ./your-program`. Go and the browser need
a line of code, because neither has a startup hook.

## Three ways in

- **The map**, over HTTP, at whatever address the collector is bound to
- **The command line**, 50 subcommands, `execviz --help`
- **The HTTP API**, the same endpoints the map uses

Exit codes throughout: **0** success, **1** the command ran and the answer was
no, **2** usage.

## Linking machines

    execviz peer capture.db request --url https://other-host:8900
    execviz peer capture.db approve <id>          # on the other side

Peers exchange rolled-up digests rather than spans, so a subtree that has not
changed is never transferred. The overview draws a whole system while holding no
spans at all.

## Sending a finding to somebody

    execviz bundle capture.db --records syscalls.ndjson --to ./finding

Payloads are **withheld by default** and the manifest says how many, because a
bundle is the thing people attach to public issues and captures contain whatever
your programs write. `--with-payloads` includes them once you have decided that
is safe.

Press `G` in the map to save the replay as a GIF for an issue or a chat.

## What it cannot do

- **io_uring bypasses the syscall boundary by design.** Data can move that way and
  the recorder will not record it.
- **x86_64 and aarch64** for the recorder, each with its own register table. The
  table is not trusted, it is proved: `execviz-record --selfcheck` performs a
  write it knows exactly and requires the captured record to agree on
  descriptor, length and syscall number. It has been proved on x86_64; the
  aarch64 table cross-builds and awaits one run on aarch64 hardware. Any other
  architecture refuses by name.
- **176 bytes per write** are carried; longer writes are marked `truncated` with
  their true size.
- **io_uring work is counted, not read.** Operations submitted through io_uring
  do not cross the syscall boundary. The submission calls do, so every capture
  reports per process how much was submitted that way and states that its content
  is not represented here.
- **`witness` needs spans.** `execviz-attach` supplies them for Python, Node,
  Ruby and PHP with no source change: each wraps the entry point its ecosystem
  actually goes through, so a request served or made becomes a span with its own
  timing and status without the program being edited or aware. On a machine with
  nothing installed at all, `drift` compares a process against its own stored
  fingerprint instead.
- **Capture is not prevention.** This records; it does not block anything.
- **It replays the record, not the program.** It cannot evaluate an expression
  that was never recorded.

## Is it sending your data anywhere

No, and `docs/SECURITY.md` tells you how to check without trusting that sentence:
`tcpdump` beside it, a default-deny egress rule, and `execviz scrutiny`, which
proves the recorder applied the same rules to itself as to everything else.

## Stressing a program with faults derived from what it does

The stress plan is derived from a capture rather than authored.

    execviz stress --records capture.ndjson

reports which stressors the program's shape implies, each with the evidence count
behind it, and which stressors are excluded, each with the evidence that was
absent. A capture too thin to characterise produces no plan and reports it, and
exits 1.

    execviz-stress --from-plan plan.json -- ./your-program

carries out the first stressor in that plan, using seccomp user notification. The
program is not modified, not relinked and not aware. The injection rate and the
number of startup calls to leave alone are read from the plan; `--rate` and
`--after` override them.

Implemented stressors: `short_read`, `interrupted_wait` (EINTR), `peer_disappears`
(ECONNRESET) and `descriptor_exhaustion` (EMFILE). `short_read` performs the read,
returns fewer bytes than were asked for, and writes those bytes into the
program's own buffer; the bytes not taken stay in the file or socket.

Every run reports how many calls were intercepted, how many were failed and how
many were allowed through, plus any that could not be injected. A run that
intercepted nothing reports that and exits 1.

    execviz stress --records stressed.ndjson --baseline before.ndjson

names the differences between the two captures: error records, process count,
blocking calls, total records and the last moment anything was recorded. It does
not state whether the program behaved correctly under the fault.

## Saying what your own output means

A profile is where a project records what its own output means. A line reading
"connection reset by peer" is a fault in one service and the normal end of a
polling loop in another.

    execviz profile --records capture.ndjson --profile execviz.profile.json

`execviz.profile.json` in this repository is the suite's own profile, labelling
every indicator execviz itself emits, and doubles as a worked example. Each
indicator is a label, a meaning (`fault`, `warning`, `informational`) and
something to match on. The command exits 1 if anything the project itself calls
a fault occurred, so continuous integration can gate on it.

Indicators that matched nothing are reported as silent rather than omitted.
Output that no indicator matched is counted separately.

A summary is around a kilobyte against a capture of half a megabyte, so one per
week for a year is a few hundred kilobytes and the captures need not be kept:

    execviz profile --baseline week01.json --summary week32.json

reports what appeared, what stopped, what moved by more than a factor of two, and
what changed in the profile itself. It does not state whether a change is an
improvement.

## Layout

    execviz-rs/       the core, one binary
    execviz-ui/       the map, TypeScript, builds to dist/index.html
    execviz-syscall/  the recorder and the stress supervisor
    execviz-attach/   spans with no source change, by environment variable
    execviz-*/        one adapter per runtime
    browser/          standalone HTML, including the page template
    docs/             specification and everything written about it

## Using the map

The map is one surface at every scale. Zoom changes how much of a node is drawn,
never which nodes exist.

At the fleet scale the map is drawn from the collector's rollup and holds no
spans at all: a subtree whose digest has not changed costs nothing to redraw.
Spans are fetched only for what you descend into. The mode follows the camera,
and the footer states which one is in use.

Right click a node, or press `ACTIONS` in the top bar, for what can be done to
it: isolate it so only routes touching it are drawn, mute its inbound or
outbound routes or both, open the log console scoped to it, or centre the camera
on it. Muted routes are drawn faintly rather than removed, so a silenced path
stays distinguishable from one that never existed.

At the rails tier a flipbook opens on the largest family in view. Drag the rows
sideways to step through them, or use the bar at the bottom. Dragging anywhere
else pans.

`INFO` opens a panel with what the current node is, the controls, and the
capture's contents: hosts, services, spans in the map, routes, errored, started
with no end, and what was drawn this frame. The panel overlays the map rather
than resizing it.

The log console can be dragged by its header. The strip above the timeline
selects a time window; the bar itself seeks.

## Reading further

`docs/WHITEPAPER.md` covers what one capture layer plus one map does that an
assembled stack of five tools cannot, a capability-by-capability comparison
against the tools each part is usually bought from, and the limits.

The rest, by what you need:

| document | what it answers |
|---|---|
| `docs/DEPLOYING.md` | will the recorder run on this machine, and what to do when it will not |
| `docs/COMPATIBILITY.md` | which kernels and distributions are confirmed, and which need confirming |
| `docs/SECURITY.md` | what it can see, what it cannot, and how to check without trusting the claim |

## The console

Along the bottom of the map, `console` opens a command bar. It runs the same
analyses the terminal runs, against the capture on screen: `stats`, `check`,
`concurrency`, `skew`, `stress`, `fingerprint` and the rest. `help` lists them.

It sends a name, not a command line. The collector matches the name against a
list and calls the matching function in its own process, so there is no shell
behind it and no argument that can become one.

Administration is absent by design. `account` above all: accounts are made on
the machine, and a console that could create one would hand out over the network
exactly what that rule exists to keep off it. Asking for it says so rather than
failing blankly.

## Who can read a capture

Reaching an instance over a network requires an account, always. There is no
route that creates one, so the only way to get an account is a shell on the
machine, whether that shell arrived over SSH or is sitting at the keyboard:

    execviz account run.db create alice --password <password>
    execviz account run.db authorize alice --key ~/.ssh/id_ed25519.pub

An instance with no accounts refuses every request. Signing out ends the session
on the server, not just in the browser.

For a demo, `--open` serves without an account and says so on start. It is a
decision made out loud, not a default that happens when nothing is configured.

## Contributing

Compatibility reports are the most useful thing you can send.

    execviz doctor --report

carries no hostname, user, path or process name, so it is safe to paste in public
without reading it first. Every distribution in `docs/COMPATIBILITY.md` marked
"needs confirmation" is one somebody can close in ten seconds, and the issue
template takes that output directly.

To verify a change:

    cd execviz-rs && bash verify.sh

The harness builds the core, runs every test, exercises each runtime adapter and
checks the renderer. It plants failures and fails if any goes uncaught: a lying
span for `witness`, an orphan and an inversion for `detect`. A change that passes
without planting anything new has not been tested against the thing it changed.

Exit codes throughout are 0 for success, 1 for the answer being no, and 2 for
usage. A capability absent from `--help` is not finished. Anything that reports a
measurement reports what it could not measure beside it.

Some archives do not preserve the executable bit, and git stores the mode
permanently once it is set:

    chmod +x $(find . -name '*.sh')

## Licence

AGPL-3.0. Free to use, including commercially. Changes stay free: that applies
whether you ship a modified copy or run one as a service over a network.
Apache-2.0 has no network clause, which is why it is not used here.

Copyright 2026 BNCMK LLC. The copyright holder is not bound by these terms and
may license the same code differently elsewhere.
