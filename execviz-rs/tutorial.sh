#!/usr/bin/env bash
# =========================================================================
#  MANIFEST
# =========================================================================
#  script_name: tutorial.sh
#  script_path: execviz-rs/tutorial.sh
#  module_name: tutorial
#  version: 0.53.1
#  description: !/usr/bin/env bash execviz tutorial. Runs every capability end to end, including a second device faked as a separate host, and narrates what each step is showing.
#  kind: script
#  spec: internal
#  internal_dependencies: 
#  external_dependencies: 
#  features: tutorial
#  api_version: execvis-v1.0.0
#  last_updated: 2026-08-07
# =========================================================================

# execviz tutorial. Runs every capability end to end, including a second device
# faked as a separate host, and narrates what each step is showing.
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
BIN="$HERE/target/release/execviz"
PY="${EXECVIZ_PY:-$HERE/../execviz}"
NODE_ADAPTER="${EXECVIZ_NODE:-$HERE/../execviz-node}"
WORK="${EXECVIZ_WORK:-/tmp/execviz-tutorial}"
PORT="${EXECVIZ_PORT:-8950}"
rm -rf "$WORK"; mkdir -p "$WORK"; cd "$WORK"

say() { printf '\n\033[1m== %s ==\033[0m\n' "$*"; }
note() { printf '   %s\n' "$*"; }
pause() { [ "${EXECVIZ_PAUSE:-0}" = "1" ] && read -rp "   [enter] " _ || true; }

cleanup() { [ -n "${SRV:-}" ] && kill "$SRV" 2>/dev/null; }
trap cleanup EXIT

say "1. Capture a running program"
note "A normal Python program. The only added line turns capture on."
( cd "$PY" && EXECVIZ_DB="$WORK/device-a.db" timeout 60 python3 logging_workload.py >/dev/null 2>&1 )
note "captured: $($BIN view "$WORK/device-a.db" --lod system | head -c 200)"
pause

say "2. Logs are attached to spans, not written to a file"
note "The program used the standard logging module and print. Nothing was injected"
note "at the log sites, and no request id was threaded through anything."
$BIN logs "$WORK/device-a.db" --limit 6
pause

say "3. Sort and group the noise"
note "Every field a log line normally has to carry in its text is already structure."
$BIN logs "$WORK/device-a.db" --group span --limit 30 | head -20
note "Only what went wrong, worst first:"
$BIN logs "$WORK/device-a.db" --level warning --sort level
pause

say "4. The death signal"
note "A span that never completed stays open. Absence becomes a stored fact."
$BIN query "$WORK/device-a.db" --q stale --limit 5
pause

say "5. Progressive summarisation"
note "Each tier answers with aggregates, so a large trace is read a level at a time."
$BIN view "$WORK/device-a.db" --lod field
pause

say "6. Queries over both edge sets"
note "Causal siblings that overlapped in time. The causal graph permits it; the"
note "temporal graph shows it happened."
$BIN query "$WORK/device-a.db" --q races --min-overlap-ms 5 --limit 3
pause

say "7. A second device"
note "Start a collector, then trace a different program as another host. The"
note "second device is faked here with a separate store and host id; on real"
note "hardware only the --collector address changes."
"$BIN" serve "$WORK/collector.db" --port "$PORT" --collect --open --ui "$HERE/ui.html" >"$WORK/serve.log" 2>&1 &
SRV=$!
sleep 1.5
note "collector up on :$PORT"
( cd "$PY" && EXECVIZ_DB="$WORK/device-b.db" timeout 60 python3 async_workload.py >/dev/null 2>&1 )
"$BIN" node --collector "http://127.0.0.1:$PORT" --db "$WORK/device-a.db" --host-id device-a --interval 0.4 --once 2>&1 | tail -1
"$BIN" node --collector "http://127.0.0.1:$PORT" --db "$WORK/device-b.db" --host-id device-b --interval 0.4 --once 2>&1 | tail -1
ROOT="$(cd "$HERE/.." && pwd)"
if command -v node >/dev/null 2>&1; then
  note "a third, in a different language, pushing straight to the collector"
  ( cd "$NODE_ADAPTER" && EXECVIZ_COLLECTOR="http://127.0.0.1:$PORT" timeout 60 node workload.mjs >/dev/null 2>&1 )
fi
if command -v go >/dev/null 2>&1 && [ -d "$ROOT/execviz-go" ]; then
  note "a fourth, in Go, where the carrier is context.Context"
  ( cd "$ROOT/execviz-go" && GOFLAGS=-mod=mod GOCACHE=/tmp/gocache \
      EXECVIZ_COLLECTOR="http://127.0.0.1:$PORT" timeout 120 go run ./cmd/workload >/dev/null 2>&1 )
fi
if command -v ruby >/dev/null 2>&1 && [ -d "$ROOT/execviz-ruby" ]; then
  note "one in Ruby, where the carrier is fiber storage and a Thread does not inherit it"
  ( cd "$ROOT/execviz-ruby" && EXECVIZ_COLLECTOR="http://127.0.0.1:$PORT" timeout 60 ruby workload.rb >/dev/null 2>&1 )
fi
if command -v php >/dev/null 2>&1 && [ -d "$ROOT/execviz-php" ]; then
  note "one in PHP, where a plain stack is valid until a Fiber suspends with its own"
  ( cd "$ROOT/execviz-php" && EXECVIZ_COLLECTOR="http://127.0.0.1:$PORT" timeout 60 php workload.php >/dev/null 2>&1 )
fi
if command -v javac >/dev/null 2>&1 && [ -d "$ROOT/execviz-java" ]; then
  note "and a fifth, on the JVM, where the carrier cannot cross a thread pool"
  note "on its own and the pool has to be decorated to carry it"
  ( cd "$ROOT/execviz-java" && javac -d "$WORK/jvm" src/execviz/ExecViz.java 2>/dev/null && \
    javac -cp "$WORK/jvm" -d "$WORK/jvm" src/demo/Workload.java 2>/dev/null && \
    EXECVIZ_COLLECTOR="http://127.0.0.1:$PORT" timeout 90 java -cp "$WORK/jvm" demo.Workload >/dev/null 2>&1 )
fi
sleep 1
note "one graph, several hosts:"
$BIN view "$WORK/collector.db" --lod system
pause

say "8. Logs across devices"
note "The same console, now spanning every host that reported."
$BIN logs "$WORK/collector.db" --group host --limit 40 | head -24
pause

say "9. The syscall stream"
note "The semantic stream sees what the runtime knows. The syscall stream sees"
note "what leaves it. Two mechanisms: kernel tracepoints where the host permits"
note "them, library interposition where it does not."
SYSDIR="$ROOT/execviz-syscall"; [ -d "$SYSDIR" ] || SYSDIR="$ROOT/execviz-bpf"
if gcc -O2 -o "$WORK/execviz_bpf" "$SYSDIR/execviz_bpf.c" 2>/dev/null; then
  # exec so the pid belongs to the traced program, not a subshell or a wrapper
  ( cd "$PY" && exec env EXECVIZ_DB="$WORK/sys.db" python3 sysdemo.py ) >/dev/null 2>&1 &
  VP=$!
  sleep 0.05
  timeout 8 "$WORK/execviz_bpf" "$VP" --host device-a > "$WORK/sys.ndjson" 2>/dev/null
  wait $VP 2>/dev/null
  note "captured $(wc -l < "$WORK/sys.ndjson") syscall records"
  note "attributed to the span that was running on that thread at that instant:"
  $BIN syscalls "$WORK/sys.db" --records "$WORK/sys.ndjson" | python3 -c "
import json,sys
d=json.load(sys.stdin)
print(f\"   {d['attributed']} of {d['records']} attributed, {d['spans_enriched']} spans enriched\")
for b in d['busiest'][:3]:
    top=sorted(b['breakdown'].items(), key=lambda kv:-kv[1])[:4]
    print(f\"   {b['span']:16} {b['syscalls']:5} calls  \" + ', '.join(f'{k}×{v}' for k,v in top))
"
else
  note "(tracepoints unavailable on this host; interposition would be used instead)"
fi
pause

cp "$WORK/collector.db" "$WORK/near.db" 2>/dev/null || true
say "10. Peering: two instances of the same program, and consent"
note "There is no collector build and no agent build. What separates two"
note "installations is configuration and consent: one offers, the other accepts,"
note "and nothing crosses until it does."
"$BIN" serve "$WORK/far.db" --port $((PORT+2)) --identity far --collect --open >"$WORK/far.log" 2>&1 &
FARPID=$!
sleep 1.3
"$BIN" peer "$WORK/near.db" add "http://127.0.0.1:$((PORT+2))" --identity near \
  --self-url "http://127.0.0.1:$PORT" 2>/dev/null | head -c 120
echo
note "before approval, reading is refused with a reason:"
curl -s "http://127.0.0.1:$((PORT+2))/api/peer/spans?peer=near&since=0" | head -c 90
echo
note "the far end approves, and only then does the link work:"
"$BIN" peer "$WORK/far.db" approve near --direction inbound
kill $FARPID 2>/dev/null
pause

say "11. Rolled-up tiers, and comparing by digest"
note "Each tier carries a summary and a digest over its children. The digest"
note "answers 'did anything below change' without reading below."
"$BIN" rollup "$WORK/collector.db" --depth 1 | python3 -c "
import json,sys
d=json.load(sys.stdin)
print(f\"   root digest {d['digest']} over {d['rollup']['spans']} spans\")
print(f\"   the whole top tier is {len(json.dumps(d))} bytes, whatever is beneath it\")
"
pause

say "12. Retention: what is removed matters more than how much"
note "Whole traces, never single spans, or a surviving child would point at a"
note "parent that no longer exists. And never an open span, whatever its age:"
note "the oldest unfinished span is usually the most interesting row in the store."
"$BIN" trim "$WORK/collector.db" --keep-last-traces 3
pause

say "13. Conformance"
note "Adapters are checked against the contract rather than trusted. Violations"
note "mean an adapter is wrong; observations mean the program did something."
$BIN check "$WORK/collector.db"
pause

say "14. Compare two runs"
note "Run, change something, run again, and diff the captures by signature."
$BIN capture "$WORK/device-a.db" > "$WORK/before.json"
( cd "$PY" && EXECVIZ_DB="$WORK/device-a2.db" timeout 60 python3 logging_workload.py >/dev/null 2>&1 )
$BIN diff "$WORK/device-a2.db" --against "$WORK/before.json" | head -c 600
echo
pause

say "15. execviz watching itself"
note "The tool's own machinery is a program, so it can be traced by the tool."
( cd "$PY" && EXECVIZ_DB="$WORK/self.db" EXECVIZ_SUBJECT="$WORK/subject.db" timeout 60 python3 self_trace.py 2>&1 | tail -1 )
$BIN logs "$WORK/self.db" --group domain --limit 20
pause

say "16. Stress derived from what the program did"
note "Every fault-injection tool asks you to name the fault. This reads one"
note "out of the capture instead: reads that came back short imply a"
note "short-read test, socket calls imply a peer that stops answering, many"
note "descriptors imply exhaustion."
run "$BIN stress --records $WORK/sys.ndjson --min-records 20 || true"
note "The excluded list is half the answer: it names the faults this program"
note "gives no evidence for, so the plan was derived rather than recited."
note ""
note "To carry one out, against a program that is not modified or aware:"
note "  execviz-stress --from-plan plan.json -- ./your-program"
note "and then compare the stressed capture against the unstressed one:"
note "  execviz stress --records stressed.ndjson --baseline before.ndjson"

say "17. Work submitted outside the syscall boundary"
note "io_uring does not cross the boundary the recorder watches. Its setup and"
note "submission calls do, so the quantity is counted and reported rather"
note "than left as a silent gap."
run "$BIN iouring --records $WORK/sys.ndjson || true"

say "18. Saying what your own output means"
note "A line reading 'connection reset by peer' is a fault in one service and"
note "the normal end of a polling loop in another. A profile is where a"
note "project settles that, in its own words."
run "$BIN profile --records $WORK/sys.ndjson --profile $ROOT/execviz.profile.json || true"
note "Indicators that matched nothing are reported as silent rather than"
note "omitted, and output no indicator matched is counted, so the profile's"
note "own coverage is visible. Keep each summary: they are about a kilobyte"
note "against a capture of hundreds, so one per week for a year is cheap, and"
note "  execviz profile --baseline week01.json --summary week32.json"
note "then says what appeared, what stopped, and what moved."

say "19. Drift, with nothing instrumented"
note "witness needs spans. On a machine with none, the same question is asked"
note "of a program against its own past behaviour: identity derives a"
note "fingerprint from recorder records alone, and drift compares two of them."
run "$BIN identity --records $WORK/sys.ndjson --min-records 50 > $WORK/fp.json; echo wrote $WORK/fp.json"
note "  execviz drift --records now.json --baseline before.json"
note "reports the invariants that moved and by how much. It does not say the"
note "binary was substituted: a release or a different workload moves the"
note "shape the same way."

say "20. Where the time went, two ways"
note "A span tree says where measured time went, exactly, for work somebody"
note "instrumented. It cannot say anything about a slow function nobody"
note "wrapped, because no span exists to carry it."
run "$BIN flame $WORK/collector.db | head -c 400 || true"
note "That is folded from spans: exact, and blind to the rest."
note ""
note "The other half is sampled. execviz-cpu interrupts the machine on a timer"
note "and records where it was standing, call chain and all:"
note "  execviz-cpu --freq 99 --seconds 10 > cpu.ndjson"
note "  execviz cpu --records cpu.ndjson"
note "Frames are addresses. Resolving one needs the symbol table of whatever"
note "mapped it, and a name that could not be verified would be invented."
note ""
note "The two are never merged. An exact measurement averaged with a"
note "statistical one is a number that is neither."

say "21. Which chain set the duration"
note "Adding up everything slow in a request answers nothing when the work"
note "overlapped: the total is set by one chain and the rest cost no wall time."
run "$BIN critical $WORK/collector.db | head -c 400 || true"

say "22. Who can read this"
note "The tutorial served with --open, which is a decision made out loud: this"
note "instance answers anyone who reaches the port. Without it, reaching an"
note "instance over a network requires an account, always."
note ""
note "There is no route that creates one. The only way to get an account is a"
note "shell on the machine, whether that shell arrived over SSH or is sitting"
note "at the keyboard:"
note "  execviz account run.db create alice --password <password>"
note "  execviz account run.db authorize alice --key ~/.ssh/id_ed25519.pub"
note ""
note "An instance with no accounts serves nobody. The absence of accounts is"
note "not permission: it means nobody can sign in yet. Signing out ends the"
note "session on the server rather than only in the browser."

say "23. The console"
note "Along the bottom of the map, the console runs these same analyses against"
note "the capture on screen, so a reader does not have to leave the map to ask"
note "a question the map does not answer."
note ""
note "It sends a name, not a command line: the collector matches the name"
note "against a list and calls the matching function in its own process. There"
note "is no shell behind it and no argument that can become one."
note ""
note "Administration is deliberately absent. Asking the console to create an"
note "account is refused with the reason, because accounts are made on the"
note "machine and a console that could make one would hand out over the"
note "network exactly what that rule keeps off it."

say "24. The map"
say "Fault injection derived from what the program did"
note "Every chaos tool asks you to name the fault. This reads one out of a"
note "capture instead: reads that came back short imply a short-read test,"
note "socket calls imply a peer that stops answering, many descriptors imply"
note "exhaustion. Stressors the shape gives no evidence for are excluded and"
note "say which evidence was missing."
"$BIN" stress --records "$WORK/sys.ndjson" 2>/dev/null | head -c 600 || true
echo
note "execviz-stress --from-plan plan.json -- ./your-program carries one out"
note "below libc with seccomp user notification. The program is not modified,"
note "not relinked and not aware. The injection rate and the number of startup"
note "calls to leave alone are read from the plan."
pause

say "What this project says its own output means"
note "A line reading connection reset by peer is a fault in one service and the"
note "normal end of a polling loop in another. A profile is where a project"
note "settles that, in its own words. execviz.profile.json in the repository"
note "root is this suite's own profile and the worked example."
"$BIN" profile --records "$WORK/sys.ndjson" --profile "$ROOT/execviz.profile.json" 2>/dev/null | head -c 500 || true
echo
note "Indicators that matched nothing are reported as silent rather than"
note "dropped: a fault that stopped happening and an indicator that no longer"
note "matches are different facts. Output no indicator matched is counted too."
note "A summary is about a kilobyte against a capture of half a megabyte, so"
note "keep one per week and compare any two with --baseline long after the"
note "captures are gone."
pause

say "A program whose behaviour moved, with nothing instrumented"
note "identity fingerprints each program from its syscall shape alone. drift"
note "compares those fingerprints against a stored baseline and reports which"
note "invariants moved and by how much. It does not say a binary was"
note "substituted: a release or a different workload moves the shape the same"
note "way."
"$BIN" identity --records "$WORK/sys.ndjson" --min-records 50 > "$WORK/fp.json" 2>/dev/null || true
"$BIN" drift --records "$WORK/fp.json" --baseline "$WORK/fp.json" 2>/dev/null | head -c 320 || true
echo
pause

say "Work submitted where the syscall boundary cannot see it"
note "io_uring does not cross the syscall boundary, so the recorder cannot read"
note "what was submitted through it. The submission calls do cross, so they are"
note "counted per program and reported as work this capture does not represent."
note "A gap that is measured is different from a gap that is silent."
"$BIN" iouring --records "$WORK/sys.ndjson" 2>/dev/null | head -c 320 || true
echo
pause

note "The map also carries a log console, a flipbook for stepping one row at a"
note "time, a fingerprint panel, and an overview mode that draws the whole"
note "system from the rollup while holding no spans at all."
note ""
note "Open http://127.0.0.1:$PORT/ to see all of it: hosts contain regions,"
note "regions contain clusters, clusters resolve into family compasses, and"
note "rails resolve into spans. Scroll to zoom, drag to pan."
if [ -n "${EXECVIZ_TUTORIAL_NONINTERACTIVE:-}" ]; then
  # The tutorial ends by serving the map, which is right for a person and
  # impossible for a script: nothing could verify that all twenty-four sections ran
  # without waiting forever on a keypress. This exits instead, so the tutorial
  # is checkable the same way everything else here is.
  note "EXECVIZ_TUTORIAL_NONINTERACTIVE is set, so the tutorial stops here"
  note "rather than serving the map."
  exit 0
fi
note "Press ctrl-c when finished."
wait "$SRV"
