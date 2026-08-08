<!--
===========================================================================
  MANIFEST
===========================================================================
  script_name: DEPLOYING.md
  script_path: docs/DEPLOYING.md
  module_name: DEPLOYING
  version: 0.53.1
  description: Where this runs, and where it does not
  kind: document
  spec: internal
  internal_dependencies:
  external_dependencies:
  features: DEPLOYING
  api_version: execvis-v1.0.0
  last_updated: 2026-08-07
===========================================================================
-->

# Where this runs, and where it does not

Two layers with different requirements. Read this before assuming a box is
supported, because one of them is much fussier than the other.

## The collector, the map, and the adapters: portable

A single Rust binary with no dependencies outside the standard library, a
self-contained HTML page, and adapters that are ordinary source files in their
own languages. These run wherever their runtime runs, on any architecture, with
no privileges.

If a machine can run the binary, it can collect, serve the map, federate with
peers, and accept spans from instrumented programs.

## The recorder: x86_64 Linux, kernel 5.8 or newer, with CAP_BPF

Everything below is a hard requirement and each one is checked before anything
is loaded.

| requirement | why | what happens otherwise |
|---|---|---|
| **x86_64** | the register offsets and syscall numbers are architecture-specific | **refuses at startup, naming the machine** |
| **kernel 5.8+** | BPF ring buffers landed in 5.8; reading user memory from a probe in 5.5 | refuses, naming the kernel and the line |
| **CAP_BPF + CAP_PERFMON**, or root | loading a program and reading user memory are privileged | refuses, printing the `setcap` line |
| **not in kernel lockdown** | secure boot commonly enables it, and it blocks BPF outright | refuses, and says lockdown is a possible cause |

### aarch64 is not a recompile

The pt_regs offsets and the syscall numbers differ. Built for aarch64 without a
second table, the program would load cleanly and report incorrect values reported without error: a
"file descriptor" read from whatever sits at offset 112, filtered on a number
that means something else there. That differs from not running, so the build
refuses at compile time and the binary refuses at startup.

Given how much of the cloud is now Graviton and Ampere, this is a real gap and
not a footnote. It is a second offset table and a second syscall constant, which
is a day of work and a machine to test on, not an architecture port.

### Distributions below the line

RHEL 8 and CentOS 8 (4.18), Ubuntu 18.04 (4.15) and Debian 10 (4.19) are all
older than 5.8. RHEL 9, Ubuntu 20.04 with a HWE kernel, Ubuntu 22.04 and later,
Debian 12 and current Fedora are above it.

### Containers and managed platforms

A container needs the capabilities in its own set, not just on the host. Most
managed platforms; shared hosting, serverless, and many VPS products; do not
grant them and cannot be made to. On those, the adapters still work and the syscall recorder
does not.

## What has been tested

**One machine.** x86_64, kernel 6.18. Everything in this repository has been
exercised there and nowhere else. The requirements above are read from the
sources of each feature rather than from a compatibility matrix somebody ran, and
until this has run on a second kernel and a second distribution, treat that table
as a well-informed expectation rather than a test result.

The honest sentence for a release announcement is: *runs on Linux with kernel
5.8 or newer, on x86_64 and aarch64; the x86_64 table is proved against the
kernel it runs on; the aarch64 table
builds and proves itself with `--selfcheck` but has not yet been run on aarch64
hardware; Windows is on the roadmap and not in this release.*

## Before anyone can open it

Reaching an instance over a network requires an account, and there is no route
that creates one. Make it on the machine, from a shell that arrived over SSH or
one sitting at the keyboard:

    execviz account run.db create alice --password <password>
    execviz account run.db authorize alice --key ~/.ssh/id_ed25519.pub

An instance with no accounts serves nobody, which is the safe direction to fail.
If you are demonstrating it and want it open, pass `--open`, which says so on
start.

## The console

Once signed in, the map's console runs the read-only analyses against the
capture without leaving the page. It cannot administer the instance: that is done
from a shell on the machine, which is the same boundary accounts sit behind.
