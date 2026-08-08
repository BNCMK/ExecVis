#!/usr/bin/env bash
# =========================================================================
#  MANIFEST
# =========================================================================
#  script_name: execviz.sh
#  script_path: execviz-sh/execviz.sh
#  module_name: execviz
#  version: 0.53.1
#  description: !/usr/bin/env bash execviz capture adapter for the shell.
#  kind: script
#  spec: internal
#  internal_dependencies: 
#  external_dependencies: 
#  features: execviz, capture, adapter
#  api_version: execvis-v1.0.0
#  last_updated: 2026-08-07
# =========================================================================

# execviz capture adapter for the shell.
#
# Build scripts and data pipelines are execution, and until now nobody could see
# them. A shell has no runtime to hook, so this is a function a script sources
# and calls around the work it wants recorded: cooperation at boundaries, applied
# to a language with no other option.
#
#   source execviz.sh
#   execviz_init http://127.0.0.1:8900 build-host pipeline
#   execviz_span compile make -j4
#   execviz_span test ./run-tests.sh
#   execviz_flush
#
# Two properties come free and matter. A command's exit status becomes the span's
# status, so a failing build is a failing span without anyone writing that down.
# And a command that never returns leaves an OPEN span, which is an unfinished span
# doing its job exactly where people most often lose a hung build.

EXECVIZ_COLLECTOR="${EXECVIZ_COLLECTOR:-}"
EXECVIZ_HOST="${EXECVIZ_HOST:-shell}"
EXECVIZ_DOMAIN="${EXECVIZ_DOMAIN:-shell}"
EXECVIZ_TRACE=""
EXECVIZ_PARENT=""
_EXECVIZ_BUF=""
_EXECVIZ_REFUSED=0
_EXECVIZ_REPORTED=""
# The most spans buffered before a flush is forced. A build script that never
# reaches its exit trap; killed, or a CI step timing out; would otherwise hold
# everything and deliver none of it.
: "${EXECVIZ_MAX_BUFFERED:=2000}"
_EXECVIZ_COUNT=0

_execviz_id() {
  # /dev/urandom is present everywhere this runs; od avoids needing python
  od -An -N6 -tx1 /dev/urandom 2>/dev/null | tr -d ' \n' || echo "$RANDOM$RANDOM$$"
}

_execviz_now() {
  # a float second, which is what every other adapter sends
  date +%s.%N 2>/dev/null || date +%s
}

_execviz_json_escape() {
  printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g' -e 's/\t/\\t/g' | tr -d '\n'
}

execviz_init() {
  EXECVIZ_COLLECTOR="${1:-$EXECVIZ_COLLECTOR}"
  EXECVIZ_HOST="${2:-$EXECVIZ_HOST}"
  EXECVIZ_DOMAIN="${3:-$EXECVIZ_DOMAIN}"
  EXECVIZ_TRACE="$(_execviz_id)"
  EXECVIZ_PARENT=""
  _EXECVIZ_BUF=""
  # a script that inherits a parent span joins that trace instead of starting
  # its own, which is how a build step called from another build step nests
  if [ -n "${EXECVIZ_PARENT_SPAN:-}" ]; then
    EXECVIZ_PARENT="$EXECVIZ_PARENT_SPAN"
    [ -n "${EXECVIZ_TRACE_ID:-}" ] && EXECVIZ_TRACE="$EXECVIZ_TRACE_ID"
  fi
}

_execviz_emit() {
  # one span object, appended to the buffer
  [ -n "$_EXECVIZ_BUF" ] && _EXECVIZ_BUF="$_EXECVIZ_BUF,"
  _EXECVIZ_BUF="$_EXECVIZ_BUF$1"
  _EXECVIZ_COUNT=$((_EXECVIZ_COUNT + 1))
  # A long build that never reaches its exit trap; killed, or a CI step timing
  # out; would otherwise hold every span and deliver none of them. Flushing on
  # a bound turns a lost run into a partial one.
  if [ "$_EXECVIZ_COUNT" -ge "$EXECVIZ_MAX_BUFFERED" ]; then
    execviz_flush
    _EXECVIZ_COUNT=0
  fi
}

# execviz_span NAME COMMAND...
# Runs the command inside a span. The exit status is the span's status and is
# passed through unchanged, so wrapping a command never changes what the script
# sees; an observer that alters the thing it observes is not an observer.
execviz_span() {
  local name="$1"; shift
  local id start end rc kind="call"
  id="$(_execviz_id)"
  start="$(_execviz_now)"
  local saved_parent="$EXECVIZ_PARENT"
  EXECVIZ_PARENT="$id"
  EXECVIZ_PARENT_SPAN="$id" EXECVIZ_TRACE_ID="$EXECVIZ_TRACE" "$@"
  rc=$?
  EXECVIZ_PARENT="$saved_parent"
  end="$(_execviz_now)"
  local status="ok"
  [ "$rc" -ne 0 ] && status="error"
  _execviz_emit "{\"span_id\":\"$id\",\"trace_id\":\"$EXECVIZ_TRACE\",\
\"parent_span_id\":$( [ -n "$saved_parent" ] && echo "\"$saved_parent\"" || echo null ),\
\"name\":\"$(_execviz_json_escape "$name")\",\"kind\":\"$kind\",\
\"start\":$start,\"end\":$end,\"status\":\"$status\",\
\"host_id\":\"$EXECVIZ_HOST\",\"domain\":\"$EXECVIZ_DOMAIN\",\
\"clock_source\":\"date +%s\",\"origin\":\"semantic\",\"attributes\":{\"exit_code\":$rc,\"command\":\"$(_execviz_json_escape "$1")\"}}"
  return $rc
}

# execviz_open VAR NAME [KIND]
#
# Assigns the new span id into VAR rather than printing it. Printing would force
# the caller to write `id=$(execviz_open ...)`, and command substitution runs in
# a SUBSHELL; the span would be buffered in a shell that exits immediately and
# the record would vanish. This cost a debugging cycle and is the class
# of bug this tool exists to make visible.
#
# A span never closed is an unfinished span: a build that hangs leaves it open.
execviz_open() {
  local __var="$1"; shift
  local id start
  id="$(_execviz_id)"; start="$(_execviz_now)"
  _execviz_emit "{\"span_id\":\"$id\",\"trace_id\":\"$EXECVIZ_TRACE\",\
\"parent_span_id\":$( [ -n "$EXECVIZ_PARENT" ] && echo "\"$EXECVIZ_PARENT\"" || echo null ),\
\"name\":\"$(_execviz_json_escape "$1")\",\"kind\":\"${2:-wait}\",\
\"start\":$start,\"end\":null,\"status\":\"running\",\
\"host_id\":\"$EXECVIZ_HOST\",\"domain\":\"$EXECVIZ_DOMAIN\",\"clock_source\":\"date +%s\",\"origin\":\"semantic\"}"
  printf -v "$__var" '%s' "$id"
}

execviz_close() {
  local id="$1" status="${2:-ok}"
  _execviz_emit "{\"span_id\":\"$id\",\"trace_id\":\"$EXECVIZ_TRACE\",\
\"name\":\"\",\"kind\":\"wait\",\"start\":0,\"end\":$(_execviz_now),\"status\":\"$status\",\
\"host_id\":\"$EXECVIZ_HOST\"}"
}

# Reads what the collector said about the batch.
#
# It names every span it refused and why. Discarding the reply leaves whoever
# instrumented the script with nothing to fix, and a build script is exactly
# where a malformed span goes unnoticed for months.
#
# Reported once per distinct reason: a bug repeats on every run, and a message
# that repeats with it is one nobody reads.
_execviz_report_refusals() {
  local reply="$1"
  case "$reply" in
    *'"rejected"'*) ;;
    *) return 0 ;;
  esac
  # tolerant of whatever spacing the peer chose: assuming a compact serialiser
  # is an assumption about someone else's formatter
  local count
  count=$(printf '%s' "$reply" | sed -n 's/.*"rejected"[[:space:]]*:[[:space:]]*\([0-9][0-9]*\).*/\1/p')
  [ -z "$count" ] && return 0
  [ "$count" -eq 0 ] && return 0
  _EXECVIZ_REFUSED=$((_EXECVIZ_REFUSED + count))
  local reasons key line
  reasons=$(printf '%s' "$reply" \
    | sed -n 's/.*"reasons"[[:space:]]*:[[:space:]]*\[\(.*\)\].*/\1/p')
  [ -z "$reasons" ] && return 0
  # Split on the quote-comma-quote between entries rather than on every comma:
  # a reason reads "name is empty, and a nameless span cannot be read", and
  # splitting on the comma inside it printed two half-sentences.
  #
  # A `while read` after a pipe runs in a subshell, so every update to the
  # seen-list was discarded and the same reason printed on every flush. The
  # substitution is fed in without a pipe for exactly that reason.
  reasons=$(printf '%s' "$reasons" | sed 's/"[[:space:]]*,[[:space:]]*"/\n/g; s/^[[:space:]]*"//; s/"[[:space:]]*$//')
  while IFS= read -r line; do
    [ -z "$line" ] && continue
    key="${line#*: }"
    case "$_EXECVIZ_REPORTED" in
      *"|$key|"*) continue ;;
    esac
    _EXECVIZ_REPORTED="$_EXECVIZ_REPORTED|$key|"
    printf 'execviz: the collector refused a span; %s\n' "$line" >&2
    printf '  (further spans refused for this reason will not be reported again)\n' >&2
  done <<EOF
$reasons
EOF
}

execviz_flush() {
  [ -z "$_EXECVIZ_BUF" ] && return 0
  [ -z "$EXECVIZ_COLLECTOR" ] && { _EXECVIZ_BUF=""; return 0; }
  local body="{\"host_id\":\"$EXECVIZ_HOST\",\"spans\":[$_EXECVIZ_BUF]}"
  local reply=""
  if command -v curl >/dev/null 2>&1; then
    reply=$(printf '%s' "$body" | curl -s -X POST -H 'Content-Type: application/json' \
      --data-binary @- "$EXECVIZ_COLLECTOR/api/ingest" 2>/dev/null)
  elif command -v wget >/dev/null 2>&1; then
    reply=$(printf '%s' "$body" | wget -q -O - --post-file=- \
      --header='Content-Type: application/json' "$EXECVIZ_COLLECTOR/api/ingest" 2>/dev/null)
  fi
  _EXECVIZ_BUF=""
  _execviz_report_refusals "$reply"
}

# A script that exits without flushing has still recorded something, so the exit
# trap delivers whatever is buffered rather than losing the whole run.
trap 'execviz_flush' EXIT
