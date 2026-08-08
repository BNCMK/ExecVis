<!--
===========================================================================
  MANIFEST
===========================================================================
  script_name: README.md
  script_path: execviz-sh/README.md
  module_name: README
  version: 0.53.1
  description: execviz capture adapter for the shell
  kind: document
  spec: internal
  internal_dependencies:
  external_dependencies:
  features: README, capture, adapter
  api_version: execvis-v1.0.0
  last_updated: 2026-08-07
===========================================================================
-->

# execviz capture adapter for the shell

    source execviz.sh
    execviz_init http://collector:8900 build-1 pipeline
    execviz_span compile make -j4
    execviz_span run_tests ./test.sh
    execviz_open HUNG deploy_waiting_for_approval wait
    execviz_flush

Build scripts and data pipelines are execution, and nobody could see them. A
shell has no runtime to hook, so this is cooperation at boundaries applied to a
language with no other option.

## Two properties come free and matter

**An exit status is a status.** A failing step is a failing span without anyone
writing that down, and the status is passed through unchanged; an observer that
alters the thing it observes is not an observer.

**A command that never returns leaves an open span**, which is an unfinished span
doing its job exactly where people most often lose a hung build:

    fetch_sources                ok       53ms
    compile                      ok      125ms
    run_tests                    error    85ms
    package                      ok       33ms
    deploy_waiting_for_approval  running  OPEN

## One trap reported

`execviz_open` assigns into a variable you name rather than printing an id.
Printing would force `id=$(execviz_open ...)`, and command substitution runs in a
**subshell**; the span would be buffered in a shell that exits immediately and
the record would vanish. That cost a debugging cycle here, and it is exactly the
class of bug this tool exists to make visible.

A script that exits without flushing has still recorded something, so an EXIT
trap delivers whatever is buffered.
