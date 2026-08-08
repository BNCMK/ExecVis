<!--
===========================================================================
  MANIFEST
===========================================================================
  script_name: README.md
  script_path: browser/README.md
  module_name: README
  version: 0.53.1
  description: browser
  kind: document
  spec: internal
  internal_dependencies:
  external_dependencies:
  features:
  api_version: execvis-v1.0.0
  last_updated: 2026-08-07
===========================================================================
-->

# browser

Standalone HTML. The product's map is `execviz-ui/`, which builds to
`execviz-ui/dist/index.html`.

`exec-viz-nested.html` is a live dependency: `execviz/export.py` and
`execviz/live.py` read it as their page template. Changing its markers changes
what those tools produce. `EXECVIZ_TEMPLATE` overrides the path.

`execviz-live.html` is generated output.

The rest are earlier studies the specification refers to.
