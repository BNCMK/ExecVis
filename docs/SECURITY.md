<!--
===========================================================================
  MANIFEST
===========================================================================
  script_name: SECURITY.md
  script_path: docs/SECURITY.md
  module_name: SECURITY
  version: 0.53.1
  description: What this sends, and how to check rather than trust
  kind: document
  spec: internal
  internal_dependencies:
  external_dependencies:
  features: SECURITY
  api_version: execvis-v1.0.0
  last_updated: 2026-08-07
===========================================================================
-->

# What this sends, and how to check rather than trust

This runs privileged and reads what every process on the machine writes. Nobody
should take that on faith, including from this document. What follows is a claim,
the way to check it yourself, and the things the check cannot cover.

## The claim

**Nothing leaves this machine unless you configure a collector.** There is no
telemetry, no update check, no crash reporting, and no analytics. The recorder
writes NDJSON to a file. A separate reader sends it wherever you point it, and if
you point it nowhere, it goes nowhere.

## Checking it with the tool itself

The recorder records every process, and that includes the syscall recorder. Run it and look at
what it says about itself:

    execviz-record --host $(hostname) > syscalls.ndjson
    execviz ask capture.db --records syscalls.ndjson \
      --q "from floor where comm = execviz-record group by call show count"

Its own syscalls are there. On an idle run they number in single figures, and
**none of them are socket calls**.

The one exemption is narrow and stated: this recorder suppresses its own
observation overhead, which is reads, opens, waits and the record output itself.
Left in, one polling loop produced 300,000 records about the recorder in six
seconds and buried every other process on the machine. The suppressed count is
printed on exit. **`socket`, `connect`, `sendto`, `sendmsg`, and any write to a
descriptor other than the record output are never suppressed**, because those are
exactly what an auditor came to look at.

## Did it watch itself the *same way*?

Watching itself is not enough on its own: an exemption applied quietly to its own
records would look exactly like honesty. So each record carries a **policy
digest**, a hash over the decisions that produced it rather than over what it
says. Whether it was suppressed, how it was classified, whether it was truncated,
whether the descriptor was resolved, whether the bytes were hexed. **Who the
record is about is deliberately not in there**; if it were, every self record
would differ by construction and the question would answer itself falsely.

    execviz scrutiny --records syscalls.ndjson

    policies_on_others   5
    policies_on_recorder 3
    shared_treatment     2
    only_on_recorder     1   (declared: observation overhead)
    undeclared           0
    merkle_root          625754780ae757c1e1bbc61dc5ec39a1...

**This does special-case itself, and reports it.** The recorder suppresses its own
reads, opens, waits and record output, because a poll loop left in produced three
hundred thousand records about the recorder in six seconds and buried every other
process on the machine. That exemption is emitted as a record of its own, with
its count and its reason.

The point is not that no exemption exists. It is that **every exemption must be
declared, and an undeclared one is detectable from the output alone** by anybody,
without reading the source. A decision path that applies to the recorder and to
nothing else, and is not declared, exits 1 and names itself. Verified by planting
one.

The distinct policies are combined into a Merkle root, so a run reduces to one
comparable number and a single record's treatment can be proven against it
without handing over the rest of the capture.

## Why that is corroboration and not proof

A recorder reporting on itself is not independent evidence. A dishonest build
could omit its own traffic and the output would look identical. So the argument
here is not "trust the self-report"; it is "the self-report is one of three
things, and the other two do not come from us".

**Watch it from outside.** The check that does not depend on this software:

    sudo tcpdump -i any -n 'not port 22' -w outside.pcap &
    execviz-record --host $(hostname) > syscalls.ndjson
    # do work, stop both, then read outside.pcap

An egress firewall that denies by default is stronger still, and costs nothing to
apply to a process that is not supposed to talk.

The policy digest has the same ceiling: a dishonest build could compute it
disaccurately. It is evidence against accidental divergence, and against a modified
binary whose author did not think about it, and it is not proof against one whose
author did.

**Read the build, not the binary.** Auditing source only means something if the
binary you run came from it. Releases are reproducible and signed, and the build
inputs are published, so a third party can rebuild and compare hashes. Until you
have done that or trust someone who has, "the source is open" is a statement
about a different artifact than the one on your disk.

## What this cannot see

Following the rule this project applies everywhere else: name the limit in the
output rather than in a footnote.

- **io_uring bypasses the syscall boundary by design.** A program submitting
  reads and writes through an io_uring ring performs no `write` syscall, and a
  probe on syscall entry sees nothing. Data can move this way and the recorder will
  not record it. This is a real hole and it is not closed.
- **Kernel-side and hardware paths are out of reach.** A kernel module, a
  compromised kernel, DMA, or anything below the syscall boundary is not
  observable from here.
- **A 176-byte slice is carried from each write.** Longer writes are recorded
  with `truncated` set and their true `bytes`, so the record says what it did not
  carry rather than appearing complete.
- **Capture is not prevention.** Recording that something was sent is not the
  same as stopping it. This is not an intrusion detection system and does not
  enforce anything.

## What it captures, and how to capture less

The recorder reads what processes write, which on a real machine includes passwords
in log lines, API keys, tokens and customer data. Treat a capture as being as
sensitive as the most sensitive thing your software logs, because it is.

- Redaction runs **at capture**, marks rather than deletes, and **fails closed**:
  a value that cannot be evaluated is withheld rather than passed through.
- Payload capture can be turned off entirely, leaving syscall metadata with no
  bytes. This is the right default for a shared or regulated machine.
- Peering requires explicit approval on both sides, and a peer listing shows that
  a credential exists, never the credential.

## What the CPU sampler sees

`execviz-cpu` is a separate program with a separate privilege, and it reads
something the recorder does not: instruction pointers and return addresses from
every process on the machine.

- It records addresses, never memory contents. A stack frame reveals which code
  was executing, not the data it was working on.
- Addresses expose ASLR layout for the sampled processes while the capture is
  held. Treat sampler output as sensitive against local attackers who could use
  it to defeat address randomisation.
- It samples the whole machine unless given `--pid`. On a shared host that means
  neighbours are sampled too.
- It needs CAP_PERFMON, or `kernel.perf_event_paranoid` at 2 or lower. Lowering
  that sysctl grants the same visibility to every other program on the machine,
  so granting the capability to the one binary is narrower.

## Reporting something

Security reports go to the address in this repository's metadata rather than to
the public issue tracker. A report that turns out to be a misunderstanding is
still worth sending; the cost of reading one is far lower than the cost of the
one nobody sent.

## Accounts are made on the machine

There is no route that creates an account, so the only way to get one is a shell
on the host. Whoever can already run commands there can grant access, and nobody
else can, however the instance is exposed.

Reaching it over a network requires an account, always, and the check is made
per request rather than once at startup. Deciding it at boot means an account
created while the server runs changes nothing until somebody restarts, and an
instance that started with no accounts stays open after it has some.

An instance with no accounts refuses every request. The alternative would leave
an instance published to a network with its capture readable by whoever finds the
port. Serving without
an account requires `--open`, which announces itself on start.

## The console carries a name, not a command line

The map's console runs analyses against the capture on screen. It sends the name
of one, and the collector matches that name against a list and calls the matching
function in its own process. No shell is involved, no process is started, and no
argument is interpreted, so there is nothing for an argument to become.

Administration is absent from that list, `account` first among them. A console
able to create an account would grant over the network the one thing that is
deliberately only grantable from a shell on the machine. Asking for it is refused
with the reason, because a capability withheld on purpose and a capability broken
look the same when the answer is silence.
