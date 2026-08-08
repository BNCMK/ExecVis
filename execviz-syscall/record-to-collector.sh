#!/bin/sh
# =========================================================================
#  MANIFEST
# =========================================================================
#  script_name: record-to-collector.sh
#  script_path: execviz-syscall/record-to-collector.sh
#  module_name: record-to-collector
#  version: 0.53.1
#  description: !/bin/sh Feeds the recorder’s NDJSON into a collector, and keeps the two apart.
#  kind: script
#  spec: internal
#  internal_dependencies: 
#  external_dependencies: 
#  features: record-to-collector
#  api_version: execvis-v1.0.0
#  last_updated: 2026-08-07
# =========================================================================

# Feeds the recorder's NDJSON into a collector, and keeps the two apart.
#
# The capture writes to a file and this reads it, deliberately: a collector that
# is slow or down must never be able to stall the processes being observed. The
# file is the buffer, and the machine keeps running whatever happens here.
set -eu
FILE="${1:-/var/lib/execviz/syscalls.ndjson}"
COLLECTOR="${EXECVIZ_COLLECTOR:-http://127.0.0.1:8900}"
HOST="${EXECVIZ_HOST:-$(hostname)}"

tail -n0 -F "$FILE" | while IFS= read -r line; do
  case "$line" in
    *'"log"'*) ;;
    *) continue ;;
  esac
  printf '%s\n' "$line"
done | execviz-record-feed --collector "$COLLECTOR" --host "$HOST" 2>/dev/null || true
