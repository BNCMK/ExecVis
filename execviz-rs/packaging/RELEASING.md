<!--
===========================================================================
  MANIFEST
===========================================================================
  script_name: RELEASING.md
  script_path: execviz-rs/packaging/RELEASING.md
  module_name: RELEASING
  version: 0.53.1
  description: Building a release
  kind: document
  spec: internal
  internal_dependencies:
  external_dependencies:
  features: RELEASING
  api_version: execvis-v1.0.0
  last_updated: 2026-08-07
===========================================================================
-->

# Building a release

There is almost nothing to fetch, which is deliberate. The whole tree has **one**
third-party dependency: SQLite, compiled in, so no system library is needed at
runtime. The BPF program is hand-assembled rather than compiled, precisely so a
BPF toolchain is not a requirement for anyone building or running this.

## What a build machine needs

    rustc + cargo        the collector and the map server
    gcc                  the recorder, which is one C file
    node + npm           the page, built once with tsc and esbuild

Nothing else, and none of it is needed to *run* the result.

## Build the binary statically, or it will not run where you send it

A default build links against the glibc of the machine that built it, and a
binary built on a current distribution fails on an older one with
`GLIBC_2.xx not found`. That single detail decides whether one download works
everywhere or generates a support thread per distribution.

    rustup target add x86_64-unknown-linux-musl
    apt-get install -y musl-tools
    cargo build --release --target x86_64-unknown-linux-musl

    gcc -O2 -static -o execviz-record execviz-syscall/execviz_bpf.c

Check it:

    ldd target/x86_64-unknown-linux-musl/release/execviz

Expect either "not a dynamic executable" or, for a static-PIE, "statically
linked". Both mean the same thing here. What disqualifies a release is a line
containing an arrow, which is a dependency on a shared object.

## What ships

Two binaries, one HTML page, one service file, and the adapters as source. No
installer that fetches anything, no runtime, no package manager, no SDK. An
operator can read the install script in full before running it, which for a tool
that watches other programs is the minimum courtesy.

## What to publish alongside

- a checksum for every artifact, and a signature over the checksums
- the exact toolchain versions used, so a third party can rebuild and compare
- `execviz doctor` output from a clean machine of each supported distribution,
  which is the compatibility matrix stated as evidence rather than as a promise
