<!--
===========================================================================
  MANIFEST
===========================================================================
  script_name: README.md
  script_path: docs/README.md
  module_name: README
  version: 0.53.1
  description: Documents
  kind: document
  spec: internal
  internal_dependencies:
  external_dependencies:
  features: README
  api_version: execvis-v1.0.0
  last_updated: 2026-08-07
===========================================================================
-->

# Documents

| document | what it answers |
|---|---|
| `WHITEPAPER.md` | what one capture layer and one map do that five separate tools cannot |
| `DEPLOYING.md` | will the recorder and the CPU sampler run on this machine, and what to do when they will not |
| `COMPATIBILITY.md` | which kernels and distributions are confirmed, and which need confirming |
| `SECURITY.md` | what the recorder and the sampler can each see, what they cannot, and how to check without trusting the claim |

The recorder and the CPU sampler have separate requirements. The recorder needs
eBPF, CAP_BPF and kernel 5.8 or newer. The sampler uses `perf_event_open` and
needs CAP_PERFMON, so it runs in places the recorder does not. Both documents
above cover each separately.
