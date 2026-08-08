<!--
===========================================================================
  MANIFEST
===========================================================================
  script_name: COMPATIBILITY.md
  script_path: docs/COMPATIBILITY.md
  module_name: COMPATIBILITY
  version: 0.53.1
  description: Confirming where the recorder runs
  kind: document
  spec: internal
  internal_dependencies:
  external_dependencies:
  features: COMPATIBILITY
  api_version: execvis-v1.0.0
  last_updated: 2026-08-07
===========================================================================
-->

# Confirming where the recorder runs

## The variable is the kernel, not the distribution

Ubuntu, Debian and Fedora with the same kernel version behave identically for the
floor, because what it needs are kernel features. What differs by
distribution is two things, and only one of them is hard.

**The C library.** A binary built against one glibc fails on an older one with
`GLIBC_2.xx not found`. This is solved outright rather than tested around: the
release is built against musl and statically linked, so there is one binary and
no distribution variance at all. CI fails the build if `ldd` does not say
`not a dynamic executable`.

**The kernel version.** This cannot be tested in a container, because a container
shares the host's kernel. An Ubuntu 20.04 container running on a 6.x host proves
nothing about whether the recorder works on Ubuntu 20.04's own kernel. There is no
SDK for this and no way around it: it needs a real kernel, which means a runner
or a virtual machine.

## What CI confirms, free

`.github/workflows/compat.yml` builds statically, runs the whole acceptance
harness, and tries to load the recorder on the `ubuntu-22.04` and `ubuntu-24.04`
runner images, printing the kernel it got. A runner that will not grant
`CAP_BPF` is reported rather than hidden, because that is a fact about the runner
and the rest of the suite still has to pass.

That covers the Ubuntu versions most servers run. Extending it is a line in the
matrix.

## What still needs a machine

| distribution | kernel | how it gets confirmed |
|---|---|---|
| Ubuntu 22.04, 24.04 | 5.15, 6.8 | **CI, every push** |
| Ubuntu 24.04 | 6.18 | confirmed by hand |
| Debian 12 | 6.1 | needs a VM or a volunteer |
| RHEL 9, Rocky 9, Alma 9 | 5.14 | needs a VM; 5.14 is above the line but close to it |
| Fedora current | 6.x | needs a VM or a volunteer |
| Amazon Linux 2023 | 6.1 | needs a VM |
| Alpine | varies | needs a VM; musl userland, which the static build suits |
| Ubuntu 20.04 | 5.4 default | **below the line**; the HWE kernel is 5.15 and is above it |
| RHEL 8, Ubuntu 18.04, Debian 10 | 4.x | **below the line, and will refuse by name** |

A distribution nobody has run it on is listed as needing confirmation, not as
supported. Guessing here would be the one thing this project does not do
anywhere else.

## Letting the people with the machines fill this in

    execviz doctor --report

Prints the distribution, the kernel, the linkage, and each requirement with what
was found. It carries no hostname, user, path or process name, so it is safe to
paste in public without reading it first.

For a tool given away, this is the only way the table ever gets filled in
accurately: far more people have machines than we do, and a report costs them one
command. Every row above that says "needs a volunteer" is a row somebody can
close in ten seconds.
