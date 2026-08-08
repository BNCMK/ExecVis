<!--
===========================================================================
  MANIFEST
===========================================================================
  script_name: README.md
  script_path: execviz-rs/README.md
  module_name: README
  version: 0.53.1
  description: execviz core
  kind: document
  spec: internal
  internal_dependencies:
  external_dependencies:
  features: README
  api_version: execvis-v1.0.0
  last_updated: 2026-08-07
===========================================================================
-->

# execviz core

One binary, no runtime, no interpreter on the target. Build:

    cargo build --release        # target/release/execviz, ~2.1 MB static

## Commands

    execviz serve   <db> [--port N] [--collect] [--ui FILE]
    execviz node    --collector URL --db FILE [--host-id ID] [--interval S] [--once]
    execviz view    <db> --lod system|field|cluster|channel|span [--host H] [--cluster C] [--family F] [--span ID]
    execviz query   <db> --q stale|errors|races|slowest|hotpaths|descendants|ancestors [--span ID] [--limit N]
    execviz diff    <db> --against capture.json
    execviz logs    <db> [--host H] [--domain D] [--span S] [--under SPAN_ID]
                         [--level info|warning|error] [--contains TEXT]
                         [--sort time|level|domain|span]
                         [--group span|domain|host|level] [--errors] [--limit N] [--json]
    execviz syscalls <db> --records FILE.ndjson [--apply]
    execviz fingerprint <db> [--against a.db,b.db,...]
    execviz account <db> create <name> [--password P]
    execviz account <db> add-key <name> --file id_ed25519.pub
    execviz account <db> api-key <name> [--label L]
    execviz account <db> revoke <key_id> | list
    execviz cost    <db> [--limit N]
    execviz correlate <db> [--min-support N]
    execviz concurrency <db>
    execviz watch   <db> --rules FILE [--interval S] [--once]
    execviz sampling <db> [--declare RULE --rate R]
    execviz backup  <db> --to FILE             exits 1 if the copy does not verify
    execviz egress  <db> [--allowed FILE] [--fail-on-unexpected]
    execviz attempts <db>
    execviz integrity <db>                     exits 1 if the file is not sound
    execviz shape   <db> [--against FILE] [--fail-on-departure]
    execviz whatif  <db> --span NAME [--faster 0.5]
    execviz across  --runs a.db,b.db,c.db
    execviz stats   <db> [--min-count N]
    execviz assert  <db> --rules FILE          exits 1 on failure
    execviz coverage <db> --expected FILE
    execviz skew    <db>
    execviz regress <db> --against earlier.db [--fail-on-regression]
    execviz export  <db> [--format chrome|folded]
    execviz seal    <db> [--verify HASH]       exits 1 if the seal is broken
    execviz audit   <db> [--limit N]
    execviz find    <db> <text|key=value> [--limit N]
    execviz selftime <db> [--limit N]
    execviz critpath <db> [--span ID]
    execviz trim    <db> [--older-than-secs S] [--keep-last-traces N] [--apply]
    execviz sync    <db> --with URL [--api-key K] [--depth N]
    execviz rollup  <db> [--node ID] [--depth N]
    execviz check   <db>
    execviz capture <db>

## Tests

    cargo test --release          # 19 tests, ~0.1s

Each test is written so that removing the behaviour it covers makes it fail. The
suite was checked by mutation rather than assumed sound:

| mutation | test that failed |
|---|---|
| upsert becomes `DO NOTHING`, so a re-sent completion duplicates | two_phase_completion_updates_in_place_rather_than_duplicating |
| orphaned work reclassified as a violation | work_outliving_its_parent_is_an_observation_not_a_violation |
| stale redefined as merely unfinished | stale_means_running_past_threshold_not_merely_unfinished |

In each case exactly one test failed, and it was the one claiming to cover that
behaviour. A fixture that cannot express the failure it is meant to catch is a
rubber stamp, so every conformance test builds a capture that violates
the rule under test.

## Layout

- `store.rs` - the span store. Two-phase writes as a single upsert: a span
  arriving with an end updates its open row instead of duplicating it, which is
  what lets a remote node re-send a completed span safely. A span that never
  receives its second phase keeps end NULL, holding the stale-running death
  signal as a stored fact.
- `views.rs` - progressive summarisation and the query surface. Each tier
  returns aggregates rather than the tier below it. `races` reads both edge sets
  at once: causal siblings that overlapped in time.
- `http.rs` - threaded HTTP server and the client the node agent uses. The
  surface is small and fixed, so no async runtime is pulled in.
- `json.rs` - the wire format, hand-rolled to keep the binary dependency-light.
- `conform.rs` - the conformance checks (5.2), split into violations, which
  mean the adapter is wrong, and observations, which mean the program did
  something worth seeing.
- `tests.rs` - the suite described above.
- `main.rs` - subcommand dispatch.

Only dependency: `rusqlite` (bundled SQLite).

## Endpoints

    GET  /                 the map, when --ui is given
    GET  /spans            renderer feed: normalised 0..1000 clock + cluster list
    GET  /events           the same feed as server-sent events (push)
    GET  /api/health
    GET  /api/spans        raw spans
    GET  /api/view?lod=... progressive summarisation
    GET  /api/query?q=...  queries over both edge sets
    GET  /api/capture      replay capture
    GET  /api/check        conformance of the capture
    GET  /api/logs         the log console, same filters as the CLI
    POST /api/ingest       node push, merged by host_id
    POST /api/diff         {"a": capture, "b": capture}

## Peering

Every instance is the same program. There is no collector build and no agent
build: what separates two installations is configuration and consent.

    # on each machine, the same command
    execviz serve site.db --port 8900 --identity site-a

    # offer to peer, from the other end
    execviz peer local.db add http://site-a:8900 --identity site-b --self-url http://site-b:8900

    # nothing crosses until the far end agrees
    execviz peer site.db list                       # shows site-b pending
    execviz peer site.db approve site-b             # now it may read

A pending peer is refused with a reason rather than silently ignored. Approval is
per direction, so a device that reports upward without taking anything back is a
one-way link and is the normal case. Either side may revoke, and the change lands
on the next exchange without needing the other side to cooperate.

When the far end requires an account, the near end presents a key it issued:

    # on the far end
    execviz account far.db api-key ops --label 'peer near'     # shown once
    # on the near end
    execviz peer near.db add http://far:8900 --api-key execviz_...

Both the handshake and the pull carry it, because an instance that requires an
account requires it of everyone, including a peer introducing itself. The link
records that a credential is held and never lists the credential. Consent and
identity are separate and both must hold: approval says this peer may read, the
key says this really is that peer.

The link is a pull with a cursor: a peer asks for what changed since the position
it last saw, and applies it with the same upsert as any local write, so a span
that completes later updates its row rather than duplicating. A pending outbound
link keeps retrying, because the far end approves on its own schedule and has no
way to tell us; the next successful exchange is how it is discovered, and a
revocation is noticed the same way in reverse.

`host_id` travels with the span, so a capture forwarded through a third instance
still belongs to the machine that recorded it. That makes a chain of
peers produce one graph instead of a telephone game.

## Distributed capture

    # collector
    execviz serve collector.db --port 8900 --collect --ui ui.html

    # remote device
    execviz node --collector http://collector:8900 --db node.db --host-id edge-1

The node forwards its local capture store and the collector merges by `host_id`.
Each host renders as its own container at the system tier.

## Capture adapters

Capture must run inside the traced process, so it is a thin per-runtime adapter
that writes a local store and speaks the wire format. The Python adapter
(`../execviz/capture.py`, `store.py`) is the reference implementation. Adapters
hold no logic beyond capture and batching.

## Live delivery

`/spans` is the poll endpoint and `/events` is the push upgrade. Both return the
same snapshot, so the renderer's ingest path is identical either way. The page
opens an EventSource and falls back to polling if the stream does not arrive
within a few seconds, which also covers a collector that was started without
streaming.

## Syscall stream

    execviz syscalls run.db --records syscalls.ndjson --apply

Merges a syscall capture into the semantic one, attributing each record to the
innermost span running on its thread at that instant. See ../execviz-syscall for
the two adapters that produce those records.

## Fingerprint

    execviz fingerprint run.db                          # the signature
    execviz fingerprint today.db --against a.db,b.db    # read against a baseline

Six invariants, each a deterministic function of the captured spans, normalised
onto one scale so they can be read together: branching, concentration, loop
density, jitter, io ratio, depth.

Comparison is the operation this exists for. Several baseline captures give a
band per axis; a candidate is read against it and the answer names the axis that
moved rather than only reporting that something did:

    branching      baseline 0.80 ±0.03   value 0.72  <-- outside
    io_ratio       baseline 0.15 ±0.03   value 0.37  <-- outside
    depth          baseline 0.62 ±0.03   value 0.36  <-- outside

Measured on repeated captures of real programs: two runs of one program sit
0.0005 apart, two different programs 0.1565 apart, so separation is roughly
three hundred times the run-to-run spread with no overlap and every run
identifying its own program by nearest neighbour.

One invariant was redefined rather than dropped along the way. `io_ratio` was
duration-weighted, and durations move with the machine, so it was the least
stable axis. Removing it improved every measure, which was the trap: it is the
most meaningful signal in the set. Counting spans instead of weighting time
makes it a property of the program rather than the hardware.

## Serving the map, behind an account

    execviz account run.db create bob --password '...'
    execviz account run.db add-key bob --file ~/.ssh/id_ed25519.pub
    execviz account run.db api-key bob --label ci      # shown once
    execviz serve run.db --port 8900 --ui dist/index.html

The instance serves the map at its own address, so the machine can be watched
from anywhere it is reachable. If no account exists access is open and the
server reports it on startup; the moment one exists, every route but sign-in
requires a credential, including the feeds, because the feed is the data.

Three credentials, because there are three kinds of caller:

| credential | for | stored as |
|---|---|---|
| password | a person at a browser | PBKDF2-HMAC-SHA256, 210k iterations |
| SSH public key | a person whose key this machine already trusts | the public key only |
| API key | a program, a script, or a peer | hash of the key |

The SSH path is a challenge, not an upload. The server issues a nonce, the
caller signs it with the key it already holds, and `ssh-keygen -Y verify` checks
it against the account's registered keys. The private key never moves, and
someone who can already reach the box by SSH needs no second secret. Verification
is left to the SSH tooling deliberately: a hand-rolled signature checker is
exactly the wrong thing to hand-roll.

Verified end to end: no credential gets 401 on the feed and the sign-in page on
the map; a wrong password is refused; an API key in `X-Execviz-Key` is accepted;
a password issues a session that works; a signed challenge issues one too; a
replayed nonce is refused as already used; and a signature over a different
nonce is refused by `ssh-keygen` itself.

SHA-256, HMAC and PBKDF2 are implemented in `sha256.rs` rather than pulled in,
and the tests check them against the published FIPS and RFC vectors rather than
against their own output. That is the one place writing the primitive is
defensible: a fixed, fully specified algorithm with public test vectors.

**What this does not do.** Over plain HTTP the password and the capture both
cross the network in the clear. The account stops someone who finds the port; it
does not stop someone reading the wire. On anything but a trusted network this
belongs behind TLS or an SSH tunnel, and the sign-in page reports it rather than
implying a protection it does not provide.

## Retention

    execviz trim run.db --keep-last-traces 200            # dry run, always
    execviz trim run.db --older-than-secs 86400 --apply

Three rules, each because the obvious alternative breaks something already
relied on.

**Trims whole traces, never single spans.** Removing a span whose children
remain leaves them pointing at a parent that no longer exists, which is a
parent-integrity violation the tool would have manufactured itself. On a real
capture, trimming 38,266 spans left `execviz check` reporting conformant with
zero violations, which is reported of the rule.

**Never trims an open span.** An open span is an unfinished span, and age is not
evidence of irrelevance: the oldest open span in a store is usually the most
interesting row in it. The report says how many traces were kept for this
reason rather than doing it silently. On that same capture, 169 traces were
retained because something in them never finished.

**A trace's age is its newest activity**, not its start, so a long-running trace
still being written to is young.

**The recorder is reported, so a stale reader learns of the gap.** After trimming,
the feed carries the earliest position still present, and a delivery to a cursor
below it is marked:

    floor 1700000000.92 | gap true | spans older than the retention floor were trimmed and are gone; reset and re-read

Delivering what remains and letting the receiver believe it has everything is
the one dishonest option. This makes retention safe alongside peering: a
peer that was offline while the far end trimmed learns that it was, rather than
silently carrying a hole in its copy.

## Syncing by digest

    execviz sync local.db --with http://far:8900 --api-key execviz_...

A cursor answers "what is new". A digest answers "what is different", and a peer
that was offline while the far end trimmed cannot recover by asking for what is
new: nothing is new, and the two copies still disagree.

The comparison walks both rollup trees top down and descends only where digests
differ. A branch whose digest matches is identical beneath it, whatever its size,
and is skipped entirely.

On the 50,006-span capture with 309 spans changed in a single cluster:

    diverging 9 of 1605 nodes
      kind  host-1/svc-7/call  differs  mine=38 theirs=38

The skeleton that crosses the wire is 132 KB against 17 MB for the capture, and
the whole comparison takes under a second. The cost is the size of the
disagreement, not the size of the capture.

It is read-only and symmetric: asking what differs changes nothing on either
side. What comes back is a list of subtrees, and reconciliation remains the
ordinary pull, so digests decide *what* to fetch while the cursor and the upsert
deliver and apply it.

One honest limit: a matching digest proves two subtrees agree **as recorded**. It
does not prove either is complete, since both could be missing the same span.
This detects divergence, not truth.

## Rolled-up tiers

    execviz rollup run.db --depth 1
    execviz rollup run.db --node vm/api --depth 2

Each node in the hierarchy carries two things, and they answer different
questions. A **digest** over its children's digests answers *did anything below
this change*, in constant time and without reading below. A **rollup** carries
the summary itself and is what a tier renders from. A hash alone tells a reader
nothing; a summary alone cannot be compared cheaply.

On the 50,006-span capture the top tier is **1,893 bytes** against 17 MB of
leaves, and the whole tree builds in 789 ms.

The rollup is a monoid, and that is the constraint rather than a detail: a
parent must be computable from its children alone, associatively, with an
identity for the empty case. Counts, sums, minima, maxima and worst-status
qualify. A median does not. A ratio does not unless numerator and denominator
travel separately and are divided at the point of reading, because
`io_share` is stored as two counts: averaging children's shares gives an average
of averages, a different and wrong number.

Five tests cover the laws, and each was mutation-checked. Storing the ratio
pre-divided fails the ratio test; dropping the child digests from the parent
fails the propagation test. One test states the actual payoff: when a busy
subtree changes, an untouched sibling keeps its digest, so it can be skipped.
That skipping is the entire saving, and it is proportional to how much of the
system is idle.

The digest is FNV-1a, chosen because change detection between cooperating parts
of one system needs no dependency and no resistance to a forger. If it ever
guards a trust boundary it must be replaced with a cryptographic hash; that is a
different requirement, not a stronger version of this one.

## Primitive and family

`kind` is the primitive: the smallest honest statement about what a span was, and
the only one an adapter records. `family` is **derived** from it by a total
function, carried on the wire for convenience, and never accepted as input.

That distinction is reported rather than a detail. A recorded field is a claim
an adapter can be wrong about; a derived field cannot disagree with the data
because it is a function of it. If adapters could send a family, two of them
could classify the same primitive differently and the map would show a difference
the program never had.

Proven rather than asserted: with every `family` on the wire rewritten to
`fault`, the renderer still computed `{control: 108, io: 15, wait: 4}` from the
primitives. A Rust test does the same thing from the other side.

The mapping is total, so an unrecognised kind lands in `control` rather than
leaving a gap, and the conformance checker reports the unknown kind separately.
The reader learns of it from the check instead of from a hole in the picture.

## Did anything get slower?

    execviz regress after.db --against before.db --fail-on-regression

`diff` reports work that appeared, moved or disappeared. This asks the question
people have after a change, and it compares distributions rather than single
durations because a comparison without a denominator is a guess.

    db_query   slower                     23.3 ->  27.4ms  (n 30/30)
    request    too few samples to judge  1738.4 -> 1734.7ms  (n 1/1)

Two rules, each with a test. **A comparison states its sample sizes**, and
refuses to judge on too few; a tenfold move across two samples has demonstrated
nothing. **A difference smaller than the earlier run's own spread is not a
finding**, because reporting every wobble as a regression teaches people to
ignore the tool, which costs more than the wobble did.

Exits non-zero with `--fail-on-regression`, so CI can act on it.

## Opening the capture elsewhere

    execviz export run.db --format chrome > trace.json    # Perfetto, devtools
    execviz export run.db --format folded > stacks.txt    # flamegraph tooling

A recording only one program can read is a lock-in dressed as a format. The point
is not generosity toward other tools: a reader who can check a conclusion
somewhere else is a reader who can trust this one.

The folded stacks carry **self** time along each causal path, so a frame's width
is what that frame spent rather than what it contained. An unfinished span is
omitted from the Chrome export rather than given an invented duration.

## Working, or waiting?

    execviz cost run.db

A duration says how long something took and nothing about what it cost. Two
ten-millisecond spans; one burning a core, one asleep on a socket; are the two
cases a person is trying to tell apart, and a duration cannot tell them apart at
all.

    hash_a_lot    1380.4ms  cpu 1351.3ms  ratio 0.996  working
    sleep_on_it    624.3ms  cpu    2.9ms  ratio 0.005  waiting

**What the runtime cannot measure is absent, never zero.** Zero is a
measurement; a capture with no cost recorded reports 127 spans unmeasured rather
than 127 spans that used nothing. The waterfall shows the same figure per row.

Preemption counts are recorded because they are the visible edge of off-CPU
time: the span was ready and something else had the machine. The full split
between running, runnable and blocked needs the scheduler, and where it is
unavailable it is reported as unknown rather than guessed; a guessed
decomposition sends a person to optimise work that was never the problem.

## What co-occurs with failure

    execviz correlate run.db --min-support 10
    errors are 2.8x more common where node=host-3 in this capture (n=40)

The discipline here is entirely in the wording. This computes **co-occurrence**,
and co-occurrence is not cause. The report states a fact about the recording and
never a claim about the world, because a capture cannot support one. A tool that
blurs the two produces confident people who are wrong; and there is a test
asserting the word "cause" never appears in a finding.

A lift computed over three spans is noise with a decimal point, so nothing is
reported below a minimum support.

## How much ran at once

    execviz concurrency run.db
    peak parallelism: 3 | time at peak: 614.81ms | idle: 0ms

A sweep over starts and ends. A level held for a long time suggests a limit, and
a limit is only a finding if work was waiting behind it; so both figures are
reported and neither is called a fault. An unfinished span has no interval and is
excluded, since counting it would leave the level permanently raised.

## Walking the record

    execviz step run.db

       0  -> POST /checkout {"cart":"[A1, BAD]"}
       1    -> price {"args":"[A1]"}
       2    <- price {"return":"9.99"} [0.5ms]
       5    -> price {"args":"[BAD]"}
       6    <- price  [0.5ms]  !! ValueError no price for BAD
       7  <- POST /checkout {"return":"{ok=False}"} [4.4ms]

**This replays the record, not the program**, and the difference matters enough
to be in the output. Time-travel debugging usually means re-executing from a
recording of every source of nondeterminism, so a person can evaluate new
expressions at an old moment. This tool records observations. It cannot evaluate
an expression that was never recorded, enter a function nobody instrumented, or
show a variable that was not captured; and a person who believes otherwise will
eventually ask it a question it cannot answer.

What it does do is real: both edges of every span in causal order, forwards or
backwards at the same cost since the record is complete before reading begins,
with **absence stated** rather than shown as a blank:

       0  -> __init__ (values not recorded)

## Watching

    execviz watch live.db --rules invariants.txt

The same rules assertions use, pointed at a live store. Live mode showed what was
happening and never said that something *had* happened, and nobody watches a map
indefinitely.

    fired: max_duration_ms db_ 100; saw: 4 spans exceeded the limit ['db_user at 137.6ms']

Two rules keep it from becoming noise, which is how alerting usually fails. **It
fires on a transition, not on a condition**: a rule true for an hour is one event,
not three thousand, and it fires again only after recovering. And **it says what
it saw**; an alert that sends a person back to the map has done none of the work.
Recovery is reported too, so a reader learns when it ended.

## Sampling metadata

    execviz sampling run.db --declare 'tail: keep errors, 1 in 100 otherwise' --rate 0.01

A capture holding one span in a hundred used to look exactly like one holding
everything, so every count taken from it was wrong by a factor nobody could
recover. A capture now declares its rule and rate:

    observed: 127 → projected estimate: 12700
    counts are estimates: multiply by 1/rate to project, and say that you did

Numbers are never silently scaled. An estimate labelled as one is useful; a
scaled-up number wearing the clothes of a measurement is not.

## Backup

    execviz backup run.db --to backup.db

Copying the file is not safe under a live writer, and an inconsistent copy is
worse than none because it looks like one, so this uses SQLite's own consistent
copy. It then **verifies what it wrote**; sound, and carrying the same seal,
because an unverified backup is a belief rather than a copy. It never overwrites,
and exits non-zero if verification fails.

## Where did it go

    execviz egress run.db --allowed destinations.txt --fail-on-unexpected

The capture already holds every boundary the program crossed. This is the same
data read with a different question: did it talk to anything it was not supposed
to? Destinations come from what the program recorded, never from guessing.

    all expected: false
    unexpected  : ['close', 'flush', 'write']
    expected but never reached: ['expected_service_we_never_call']

Both directions again: a dependency that silently stopped being used is as
interesting as a new one that appeared.

**This is not intrusion detection and the output reports it.** It reports where the
program went; whether it should have is a judgement for a person. A test asserts
that disclaimer stays in the output, because overstating it would invite someone
to rely on it as a control.

## Retries

    execviz attempts run.db
    invoice-7: 3 attempts, first failed=true, eventually ok=true

Links join spans inside one trace; retries are a relation *between* traces, so
five attempts at one operation used to look like five unrelated events. The
relation is **declared by the program, never inferred from names**: two traces
running the same code are not attempts at the same thing, and guessing they are
would invent a causal claim the capture cannot support.

## Is this file sound

    execviz integrity run.db

SQLite's own `quick_check`, duplicate span identities, references that point
nowhere, and spans that end before they start. Against a store with one bad row
inserted it reports `negative_duration` and exits non-zero.

Soundness is not the same as untampered: a store can be undamaged and still have
been edited deliberately, which is what `seal` answers. Span ids are random per
host, so a collision across a federation is unlikely rather than impossible; and
an unlikely event nobody checks for is one that gets debugged as something else.

## Expected shape

    execviz shape run.db                       # propose one
    execviz shape run.db --against shape.txt --fail-on-departure

A capture can propose the shape a system had; its domains and span names; and a
person freezes it deliberately. The tool never promotes a proposal to a rule on
its own: learning a shape from one run and enforcing it turns whatever happened
to run that day into law.

The check reports **both directions**, and the absence matters as much as the
surprise. A request that quietly stopped touching a service is exactly the change
nobody notices, and it appears here as a domain that was expected and never came.

## What if this were faster

    execviz whatif run.db --span service --faster 0.5

    total now  : 506.97 ms
    would fall : 412.50 ms  (saving 94.47 ms)
    then the constraint becomes: GET /profile/2 at 412.50 ms

A **ceiling, not a subtraction.** Halving a 501 ms span looks like it saves
250 ms; it saves 94, because shortening a link on the critical path usually
promotes a different chain to critical. Work that is not on the path at all is
told plainly that making it faster changes nothing.

## Many runs at once

    execviz across --runs run1.db,run2.db,...

Every other view assumes one capture, which makes flakiness invisible by
construction: a test failing one time in five produces four boring captures and
one nobody is looking at.

    10 runs, 9 had a failure
      test_upload      failed in 9/10 runs  (90%) ; fails more often than not
      test_checkout    failed in 2/10 runs  (20%) ; intermittent

Every rate carries the number of runs behind it, because a rate stated without
its denominator is a rumour.

## Statistics

    execviz stats run.db --min-count 20

Count, median, p90, p95, p99, max and error rate per span name, because "is this
slow" cannot be answered by one duration. Percentiles are computed from the
values, never folded out of a rollup; a percentile is not a monoid. Each row
carries `percentiles_meaningful`, because a p99 over eleven samples is the
maximum wearing a costume.

## Assertions, and failing a build

    execviz assert run.db --rules invariants.txt

    no_orphans
    max_duration_ms  checkout  250
    no_errors_in     billing
    max_error_rate   charge    0.01
    must_run         reconcile

Conformance checks that an *adapter* is honest; it cannot check that a *program*
is behaving, because only the project knows what behaving means. A failure names
the spans that broke it; a red light without a location is an alarm, not a
finding; and exits non-zero so CI can act on it. An unrecognised rule is a
failure rather than a pass: silently ignoring a rule reports success for an
invariant nobody checked.

## Coverage in reverse

    execviz coverage run.db --expected all-functions.txt
    2/4 reached (50%); never ran: ['a_function_never_called', 'another_dead_one']

The capture knows what ran, so what never ran is free.

## Clock skew

    execviz skew run.db

Every host stamps with its own clock, so a capture spanning machines can show a
child starting before its parent; causality inverted visually while the recorded
parentage stays correct. Against a capture with a host deliberately set 250 ms
fast, it recovered the offset to within 2.4 ms:

    vm -> edge-2: 7/7 impossible, offset ~247.57ms
      _int_to_enum began 247.6ms before its parent getsignal

**Detected and estimated, never applied.** A recorded time is what that machine
said; correcting it in place would destroy the evidence that the clocks disagree,
which is itself the finding.

## Sealing a capture

    execviz seal run.db
    execviz seal run.db --verify 48bc38f6...

The tier digests are FNV; fast, dependency-free, right for asking a cooperating
peer whether anything changed, and useless against someone who edits a capture on
purpose. A seal answers the different question *is this the capture that was
taken*, using SHA-256 over a canonical rendering. Editing one status breaks it,
and verification exits non-zero.

## Who read it

    execviz audit run.db

A tool that records everything a program did, and nothing about who read that
recording, holds one standard for its subject and another for itself. Reads,
exports and peer exchanges are appended with the account that made them.

## Roles

    execviz account run.db create looker --password '...' --role viewer

A viewer may read a capture and may not change one. All-or-nothing access forces
every reader to be an administrator, which is how a debugging tool becomes a way
in.

## Running it

`packaging/` holds a Dockerfile, a hardened systemd unit and an install script.
The unit confines the process to one directory and drops privileges, because it
holds recordings of other programs including their inputs.

Two limits are enforced rather than assumed: a batch over 20,000 spans is refused
with a reason (an unbounded collector is a disk-filling device operated by
whichever adapter misbehaves first), and a sender declaring a newer wire version
is refused rather than half-understood.

`/api/health` reports the tool's own figures: store size, open spans, hosts,
retention floor, wire version and whether an account is required.

## Search, self time, and the critical path

    execviz find run.db fetch                 # names, kinds, hosts, domains
    execviz find run.db user_id=u-42          # and the attributes a program recorded
    execviz selftime run.db
    execviz critpath run.db

**Search** was the largest gap in the tool: on a fifty-thousand-span capture
there was no way to type a function name and go to it, though everything needed
was already indexed. It covers the things a person remembers, including
attributes, which were previously write-only; a capture that recorded a user id
could not be asked which work belonged to that user.

**Self time** is total minus the time covered by children, which is the question
`slowest` was failing to answer: a parent gets credited for everything beneath
it. Children are merged before subtracting, so two concurrent children inside a
10 ms parent leave 2 ms of self time rather than −10 ms. A test asserts both the
merge and that no duration comes out negative.

    service         self 483.63ms   total 501.24ms
    GET /profile/2  self 150.87ms   total 412.50ms

**The critical path** walks down taking the child that finishes last, because in
concurrent work the total is set by one chain and a list of slow spans includes
work that cost nothing by overlapping. It reports the span of the path itself
rather than the root's duration, and says when a child outlives its parent:

    3 spans spanning 506.97ms | async handoff: true
    a child outlives its parent here, so this chain is a causal path rather than
    a chain of waiting; the parent did not block on what follows

That distinction was a bug first: reporting the root's duration claimed a 30 ms
total for a chain containing a 501 ms child, which is the kind of number a reader
would not think to question.

## The log workspace

    execviz logs run.db --counts                 # the shape of the noise first
    execviz logs run.db --fold                   # repeated lines as one row, counted
    execviz logs run.db --sort level --group host
    execviz logs run.db --under <span_id>        # everything causally beneath

What a person does with logs is a small set of operations in different orders,
and each is a query over structure the capture already has: narrow, scope, order,
collapse, count, locate. The console and the command line express the same ones,
because the person moving between them is one person.

    $ execviz logs noisy.db --counts
    total 41 · info 40 · meta 1

    $ execviz logs noisy.db --fold
        11.2  info  batch  retrying connection ×40
       156.3  meta  batch  further lines suppressed for this span

Two rules, each with a test that fails if it is broken:

**Collapsing is reversible and always counted.** A folded group states how many
lines it stands for, folding conserves every line, and lines from different spans
never merge; the same message from two pieces of work is two facts. Removing the
count makes one test fail; merging across spans makes another.

**A sort never invents an ordering.** Lines equal on the sort key keep the order
they were recorded in, so two runs of one query read identically. A sort that
reshuffles equal rows teaches a reader to distrust the tool for no reason.

## Conformance

    execviz check <db>

Checks a recorded capture against the adapter contract, per host, because a
capture may carry several adapters at once and a failure belongs to whichever
produced it. Every failure mode in that contract yields a tree that still looks
plausible, so the checks are structural: schema and ontology, parent integrity,
absence of cycles, link integrity, two-phase honesty, causal time, the
derivability rule for lifecycle events, and self-tracing.

Violations and observations are reported separately. A violation means the
adapter is wrong. An observation means the program did something worth seeing:
`orphaned_work` is work that outlived the parent that caused it, which is what an
aborted request or a cancelled task looks like, and is a finding about the
program rather than a defect in the capture.

The checker earned its place immediately by failing both adapters on a fan-in
join parented to one of its own children, which placed the join outside its
parent in time. Both now parent the join to the enclosing scope and record every
child in `links`.

## Logs

Logs are attached to the span that was running when they were written, so the
console filters and sorts a trace rather than grepping a file.

    execviz logs run.db                        # chronological, aligned columns
    execviz logs run.db --group span           # every line under the work it came from
    execviz logs run.db --level warning --sort level
    execviz logs run.db --under <span_id>      # one request's logs, causally
    execviz logs run.db --domain pricing --contains timeout

`--under` is the one a conventional log cannot answer: it pulls everything
causally beneath a span, across domains and hosts, without any request id having
been written into a single line.

## Logs on the map

The ▤ logs button opens a console that shares the map's clock and selection.
Lines appear as the playhead reaches them, so scrubbing the trace scrubs the log.
Clicking a line flies to the span that wrote it; clicking a span node scopes the
console to that span and everything causally beneath it. Filters for warnings and
errors sit in the console header.

On the map itself a span carrying log lines grows a tick whose weight follows the
line count, coloured by the worst severity it has emitted so far. Presence reads
at distance, content resolves on zoom, which is the same contract as every other
channel.

## Acceptance harness

    ./verify.sh

Builds everything, runs every adapter against one instance, captures and merges a
syscall stream, exercises peering, retention, the rollup and the fingerprint,
then asserts the contract and exits non-zero if anything fails.

    build      rust core, rust tests, both syscall adapters, jvm adapter
    adapters   python, node, go, jvm, ruby, php
    syscall    5184 records captured, 4881 attributed to 54 spans
    peering    an offer is held pending until approved; approval recorded
    retention  dry run by default; keeps traces holding an open span
    rollup     root digest over the whole store, 1980 bytes at depth 1
    fingerprint  six invariants
    contract   6 runtimes in one graph: go-1, jvm-1, node-1, php-1, py-1, ruby-1
               parent attribution across concurrency (0 misattributed of 30 checked)
               fan-in in links · death signal preserved · lifecycle recorded
               logs attributed · every host conformant, 0 violations

The parent-attribution check is the one worth understanding. It walks every child
span across all six runtimes back up the causal chain and requires it to arrive
at its own request rather than a sibling that happened to be running at the same
moment. Six different carriers are under test at once: a contextvar, an async
local store, a context value, an inheritable thread local behind a decorated
pool, fiber storage, and per-fiber stacks.

## Tutorial

    ./tutorial.sh                  # EXECVIZ_PAUSE=1 to step through it

Sixteen sections covering capture, log attachment, sorting and grouping, the
death signal, progressive summarisation, both edge sets, additional devices in
every available runtime, logs across hosts, the syscall stream, peering and
consent, rolled-up tiers, retention, conformance, comparing two runs, the tool
tracing itself, and the map.

Section 7 brings up whichever runtimes are installed, so the same script
demonstrates one host or five. The extra devices are faked with separate stores
and host ids; on real hardware only the `--collector` address changes. A run with
everything present reports five hosts in one graph.

Section 9 captures a syscall stream against a live process and merges it, which
prints the calls each span made:

    4881 of 5100 attributed, 54 spans enriched
    write_chunk_0   696 calls  pwrite64×178, fcntl×153, newfstatat×68
