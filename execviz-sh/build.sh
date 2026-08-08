#!/usr/bin/env bash
# =========================================================================
#  MANIFEST
# =========================================================================
#  script_name: build.sh
#  script_path: execviz-sh/build.sh
#  module_name: build
#  version: 0.53.1
#  description: !/usr/bin/env bash a build script that records itself
#  kind: script
#  spec: internal
#  internal_dependencies: 
#  external_dependencies: 
#  features: build
#  api_version: execvis-v1.0.0
#  last_updated: 2026-08-07
# =========================================================================

# a build script that records itself
set -uo pipefail
source "$(dirname "$0")/execviz.sh"
execviz_init "${EXECVIZ_COLLECTOR:-}" "build-1" "pipeline"

execviz_span fetch_sources sleep 0.05
execviz_span compile bash -c 'sleep 0.12; exit 0'
execviz_span run_tests bash -c 'sleep 0.08; exit 1'    # a failing step
execviz_span package sleep 0.03
execviz_open HUNG deploy_waiting_for_approval wait   # never closed, and never lost to a subshell
execviz_flush
echo "build finished"
