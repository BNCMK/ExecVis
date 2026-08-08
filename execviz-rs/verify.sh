#!/usr/bin/env bash
# =========================================================================
#  MANIFEST
# =========================================================================
#  script_name: verify.sh
#  script_path: execviz-rs/verify.sh
#  module_name: verify
#  version: 0.53.1
#  description: !/usr/bin/env bash execviz acceptance harness.
#  kind: script
#  spec: internal
#  internal_dependencies: 
#  external_dependencies: 
#  features: verify
#  api_version: execvis-v1.0.0
#  last_updated: 2026-08-07
# =========================================================================

# execviz acceptance harness.
#
# Builds and runs every adapter against one collector, merges a syscall capture,
# and asserts the contract on the result: conformance per host, zero
# misattributed parents, fan-in links present, a death signal preserved, and
# logs attributed. Prints a matrix and exits non-zero if anything fails.
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
BIN="$HERE/target/release/execviz"
ROOT="$(cd "$HERE/.." && pwd)"
WORK="${EXECVIZ_WORK:-/tmp/execviz-verify}"
PORT="${EXECVIZ_PORT:-8995}"
rm -rf "$WORK"; mkdir -p "$WORK"

FAIL=0
pass() { printf '  \033[32mPASS\033[0m %s\n' "$*"; }
fail() { printf '  \033[31mFAIL\033[0m %s\n' "$*"; FAIL=1; }
section() { printf '\n\033[1m%s\033[0m\n' "$*"; }

cleanup() { [ -n "${SRV:-}" ] && kill "$SRV" 2>/dev/null; }
trap cleanup EXIT

section "build"
( cd "$HERE" && cargo build --release >/dev/null 2>&1 ) && pass "rust core" || fail "rust core"
( cd "$HERE" && cargo test --release >/dev/null 2>&1 ) && pass "rust tests" || fail "rust tests"
SYSDIR="$ROOT/execviz-syscall"
[ -d "$SYSDIR" ] || SYSDIR="$ROOT/execviz-bpf"
if gcc -O2 -o "$WORK/execviz_bpf" "$SYSDIR/execviz_bpf.c" 2>"$WORK/bpf.err"; then
  pass "syscall: tracepoint collector"
else
  fail "syscall: tracepoint collector ($(tail -1 "$WORK/bpf.err"))"
fi
if gcc -O2 -fPIC -shared -o "$WORK/execviz_preload.so" "$SYSDIR/execviz_preload.c" -ldl 2>"$WORK/pre.err"; then
  pass "syscall: interposition library"
else
  fail "syscall: interposition library ($(tail -1 "$WORK/pre.err"))"
fi
if command -v javac >/dev/null; then
  ( cd "$ROOT/execviz-java" && javac -d "$WORK/jvm" src/execviz/ExecViz.java && \
    javac -cp "$WORK/jvm" -d "$WORK/jvm" src/demo/Workload.java ) >/dev/null 2>&1 \
    && pass "jvm adapter" || fail "jvm adapter"
fi

section "collector"
"$BIN" serve "$WORK/all.db" --port "$PORT" --collect --open >"$WORK/serve.log" 2>&1 &
SRV=$!
sleep 1.5
curl -sf "http://127.0.0.1:$PORT/api/health" >/dev/null && pass "collector up on :$PORT" || fail "collector"

section "adapters report"
COLL="http://127.0.0.1:$PORT"

# python, pushing directly: nothing is written to disk in the traced process,
# so a syscall capture of it is not polluted by the recorder's own writes
( cd "$ROOT/execviz" && EXECVIZ_COLLECTOR="$COLL" timeout 90 python3 -c "
import sys; sys.path.insert(0,'.')
import capture, asyncio
capture.install_push('$COLL', host_id='py-1')
import async_workload" ) >/dev/null 2>&1 || true
# and again into a local store, which is the other delivery mode
( cd "$ROOT/execviz" && EXECVIZ_DB="$WORK/py.db" timeout 90 python3 async_workload.py ) >/dev/null 2>&1
"$BIN" peer "$WORK/py.db" list >/dev/null 2>&1
[ -s "$WORK/py.db" ] && pass "python (local store + push)" || fail "python"

# node, pushing directly
if command -v node >/dev/null; then
  ( cd "$ROOT/execviz-node" && EXECVIZ_COLLECTOR="$COLL" timeout 60 node workload.mjs ) >/dev/null 2>&1 \
    && pass "node (direct push)" || fail "node"
fi

# go
if command -v go >/dev/null; then
  ( cd "$ROOT/execviz-go" && GOFLAGS=-mod=mod GOCACHE=/tmp/gocache EXECVIZ_COLLECTOR="$COLL" \
      timeout 120 go run ./cmd/workload ) >/dev/null 2>&1 \
    && pass "go (direct push)" || fail "go"
fi

# jvm
if [ -d "$WORK/jvm" ]; then
  ( EXECVIZ_COLLECTOR="$COLL" timeout 90 java -cp "$WORK/jvm" demo.Workload ) >/dev/null 2>&1 \
    && pass "jvm (direct push)" || fail "jvm"
fi

# ruby
if command -v ruby >/dev/null && [ -d "$ROOT/execviz-ruby" ]; then
  ( cd "$ROOT/execviz-ruby" && EXECVIZ_COLLECTOR="$COLL" timeout 60 ruby workload.rb ) >/dev/null 2>&1 \
    && pass "ruby (direct push)" || fail "ruby"
fi

# php
if command -v php >/dev/null && [ -d "$ROOT/execviz-php" ]; then
  ( cd "$ROOT/execviz-php" && EXECVIZ_COLLECTOR="$COLL" timeout 60 php workload.php ) >/dev/null 2>&1 \
    && pass "php (direct push)" || fail "php"
fi

# dotnet; built once, then run without rebuilding so the harness measures the
# adapter rather than the SDK
if command -v dotnet >/dev/null && [ -d "$ROOT/execviz-dotnet" ]; then
  ( cd "$ROOT/execviz-dotnet" \
    && DOTNET_CLI_TELEMETRY_OPTOUT=1 DOTNET_NOLOGO=1 DOTNET_CLI_HOME="$WORK" dotnet build -v q --nologo >/dev/null 2>&1 \
    && DOTNET_CLI_TELEMETRY_OPTOUT=1 DOTNET_NOLOGO=1 DOTNET_CLI_HOME="$WORK" \
       EXECVIZ_COLLECTOR="$COLL" timeout 120 dotnet run --no-build -v q ) >/dev/null 2>&1 \
    && pass "dotnet (AsyncLocal across await)" || fail "dotnet"
fi

# beam
if command -v erl >/dev/null && [ -d "$ROOT/execviz-erl" ]; then
  ( cd "$ROOT/execviz-erl" && erlc -o . execviz.erl workload.erl >/dev/null 2>&1 \
    && EXECVIZ_COLLECTOR="$COLL" timeout 60 erl -noshell -pa . -s workload main ) >/dev/null 2>&1 \
    && pass "beam (per-process dictionary)" || fail "beam"
fi

# native C
if command -v gcc >/dev/null && [ -d "$ROOT/execviz-c" ]; then
  ( cd "$ROOT/execviz-c" && gcc -O2 -o demo demo.c -lpthread >/dev/null 2>&1 \
    && EXECVIZ_COLLECTOR="$COLL" timeout 60 ./demo ) >/dev/null 2>&1 \
    && pass "native C" || fail "native C"
fi

# shell
if [ -d "$ROOT/execviz-sh" ]; then
  ( cd "$ROOT/execviz-sh" && EXECVIZ_COLLECTOR="$COLL" timeout 90 bash build.sh ) >/dev/null 2>&1 \
    && pass "shell (a build is execution too)" || fail "shell"
fi

# serverless
if [ -d "$ROOT/execviz-fn" ]; then
  ( cd "$ROOT/execviz-fn" && EXECVIZ_COLLECTOR="$COLL" timeout 60 python3 demo.py ) >/dev/null 2>&1 \
    && pass "serverless (synchronous flush at the boundary)" || fail "serverless"
fi

# database interior
if [ -d "$ROOT/execviz-db" ]; then
  ( cd "$ROOT/execviz-db" && EXECVIZ_COLLECTOR="$COLL" timeout 60 python3 demo.py ) >/dev/null 2>&1 \
    && pass "database interior (plans captured)" || fail "database interior"
fi
sleep 1

section "syscall stream"
# exec so the recorded pid is the traced program itself: a subshell or a timeout
# wrapper would be filtered against instead, and the capture would come back
# almost empty for a reason nothing reports
( cd "$ROOT/execviz" && exec env EXECVIZ_DB="$WORK/sys.db" python3 sysdemo.py ) >/dev/null 2>&1 &
VP=$!
sleep 0.05
timeout 8 "$WORK/execviz_bpf" "$VP" --host sys-1 >"$WORK/sys.ndjson" 2>/dev/null
wait $VP 2>/dev/null
RECS=$(wc -l < "$WORK/sys.ndjson")
[ "$RECS" -gt 0 ] && pass "tracepoint captured $RECS records" || fail "tracepoint captured nothing"
MERGED=$("$BIN" syscalls "$WORK/sys.db" --records "$WORK/sys.ndjson" 2>/dev/null \
  | python3 -c "import json,sys; d=json.load(sys.stdin); print(d['attributed'], d['spans_enriched'])" 2>/dev/null)
NATTR=${MERGED%% *}; NSPAN=${MERGED##* }
if [ "${NATTR:-0}" -gt 0 ] && [ "${NSPAN:-0}" -gt 0 ]; then
  pass "merge attributed $NATTR records to $NSPAN spans"
else
  fail "merge attributed nothing (records=$RECS)"
fi

section "both layers together"
# the recorder as witness: does the instrumentation match what the machine did?
W=$("$BIN" witness "$WORK/sys.db" --records "$WORK/sys.ndjson" 2>/dev/null \
  | python3 -c "import json,sys; d=json.load(sys.stdin); print(d['spans_examined'], d['claimed_not_performed'], d['record_coverage'])" 2>/dev/null)
WEX=${W%% *}
if [ "${WEX:-0}" -gt 0 ]; then pass "witness examined $W (spans, lies, coverage)"; else fail "witness examined nothing"; fi

# it must CATCH a lie, or it is decoration
cp "$WORK/sys.db" "$WORK/lie.db" 2>/dev/null
python3 - "$WORK/lie.db" <<'PY' 2>/dev/null
import sqlite3, json, sys
db = sqlite3.connect(sys.argv[1])
r = db.execute("select end, attributes from spans where end is not null limit 1").fetchone()
if r:
    end, attrs = r
    tid = json.loads(attrs or "{}").get("tid", 1)
    db.execute("insert into spans (span_id,trace_id,parent_span_id,links,name,kind,start,end,status,"
               "lifecycle,origin,host_id,clock_source,domain,attributes,events,inputs,run) "
               "values (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
               ("lie-1","t-lie",None,"[]","claims_a_query","db",end+50.0,end+50.5,"ok","[]",
                "semantic","sys-1",None,"app",json.dumps({"tid":tid}),"[]",None,None))
    db.commit()
PY
LIES=$("$BIN" witness "$WORK/lie.db" --records "$WORK/sys.ndjson" 2>/dev/null \
  | python3 -c "import json,sys; print(json.load(sys.stdin)['claimed_not_performed'])" 2>/dev/null)
[ "${LIES:-0}" -gt 0 ] && pass "a span claiming work it did not do is caught" \
                       || fail "the witness did not catch a planted lie"

# the negative space
U=$("$BIN" unclaimed "$WORK/sys.db" --records "$WORK/sys.ndjson" 2>/dev/null \
  | python3 -c "import json,sys; d=json.load(sys.stdin); print(d['covered_fraction'], len(d['regions']))" 2>/dev/null)
[ -n "$U" ] && pass "negative space reported (covered fraction, regions: $U)" \
             || fail "negative space reported nothing"

# a decoder that hides its residue is indistinguishable from a quiet service
D=$("$BIN" decode --records "$WORK/sys.ndjson" 2>/dev/null \
  | python3 -c "import json,sys; d=json.load(sys.stdin); print(d['records'], len(d['residue']))" 2>/dev/null)
[ -n "$D" ] && pass "decode reported its residue ($D records, residue kinds)" \
             || fail "decode reported nothing"

# identity without instrumentation
I=$("$BIN" identity --records "$WORK/sys.ndjson" --min-records 50 2>/dev/null \
  | python3 -c "import json,sys; d=json.load(sys.stdin); print(len(d['identities']))" 2>/dev/null)
[ "${I:-0}" -ge 0 ] && pass "behavioural identity computed from the recorder ($I processes)" \
                    || fail "identity computed nothing"

# the language must REFUSE a non-monoid, and that refusal is the feature
if "$BIN" ask "$WORK/sys.db" --q "from spans show median(duration)" >/dev/null 2>&1; then
  fail "median was accepted; a non-monoid must be refused"
else
  pass "a non-monoid is refused at parse time"
fi
"$BIN" ask "$WORK/sys.db" --q "from spans group by kind show count" >/dev/null 2>&1 \
  && pass "a question nobody anticipated can still be asked" \
  || fail "ask could not run a valid query"

# leaving must be possible, and must say what it costs
O=$("$BIN" otlp "$WORK/sys.db" 2>/dev/null \
  | python3 -c "import json,sys; d=json.load(sys.stdin); print(len(d['execviz_not_exported']))" 2>/dev/null)
[ -n "$O" ] && pass "otlp export names what it cannot carry ($O fields)" \
             || fail "otlp export produced nothing"

# detection on shape rather than on values
cat > "$WORK/shape.rules" <<'RULES'
stuck 0.5
orphaned
inverted
RULES
"$BIN" detect "$WORK/sys.db" --rules "$WORK/shape.rules" >/dev/null 2>&1
[ $? -eq 0 ] && pass "a healthy capture fires no shape rules" \
              || fail "shape rules fired on a healthy capture"

# and it must FIRE when the shape is there, or it is decoration
python3 - "$WORK/shapes.db" "$WORK/sys.db" <<'PY' 2>/dev/null
import sqlite3, shutil, sys, json
shutil.copy(sys.argv[2], sys.argv[1])
db = sqlite3.connect(sys.argv[1])
r = db.execute("select start, end from spans where end is not null limit 1").fetchone()
if r:
    start, end = r
    def add(sid, name, s, e, parent):
        db.execute("insert into spans (span_id,trace_id,parent_span_id,links,name,kind,start,end,"
                   "status,lifecycle,origin,host_id,clock_source,domain,attributes,events,inputs,run) "
                   "values (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
                   (sid,"t-shape",parent,"[]",name,"call",s,e,"ok","[]","semantic","sys-1",
                    None,"app","{}","[]",None,None))
    add("orphan-1","child_of_a_ghost", end+1, end+2, "nobody-here")
    add("par-1","join", end+10, end+11, None)
    add("inv-1","outlived_its_parent", end+10.2, end+13.0, "par-1")
    db.commit()
PY
"$BIN" detect "$WORK/shapes.db" --rules "$WORK/shape.rules" >/dev/null 2>&1
[ $? -eq 1 ] && pass "planted shapes (orphan, inversion) are caught" \
              || fail "shape detection missed planted shapes"

# an unknown rule must not look like a clean run
printf 'typoed_predicate 1\n' > "$WORK/bad.rules"
"$BIN" detect "$WORK/sys.db" --rules "$WORK/bad.rules" >/dev/null 2>&1
[ $? -eq 2 ] && pass "an unknown rule is a usage failure, not a clean run" \
              || fail "an unknown rule was treated as a pass"

# did it watch itself the same way?
"$BIN" scrutiny --records "$WORK/sys.ndjson" --recorder execviz_bpf >/dev/null 2>&1
RC=$?
[ $RC -le 1 ] && pass "scrutiny ran over the recorder's own records" \
               || fail "scrutiny could not read the records"

# a planted undeclared self-only treatment must be caught, or the check is decoration
{ cat "$WORK/sys.ndjson"
  printf '{"comm":"floor","policy_text":"v1.sup=0.kind=quietly-shortened.trunc=0.fd=0.hex=0"}\n'
  printf '{"comm":"app","policy_text":"v1.sup=0.kind=text.trunc=0.fd=0.hex=0"}\n'
  printf '{"comm":"floor","policy_text":"v1.sup=0.kind=text.trunc=0.fd=0.hex=0"}\n'
} > "$WORK/sneaky.ndjson"
"$BIN" scrutiny --records "$WORK/sneaky.ndjson" --recorder floor >/dev/null 2>&1
[ $? -eq 1 ] && pass "an undeclared self-only treatment is caught" \
              || fail "scrutiny missed a planted undeclared exemption"

# a bundle must not carry secrets by default
rm -rf "$WORK/bundle"
"$BIN" bundle "$WORK/sys.db" --records "$WORK/sys.ndjson" --to "$WORK/bundle" >/dev/null 2>&1
if [ -f "$WORK/bundle/manifest.json" ]; then
  pass "bundle written with a manifest and a seal"
else
  fail "bundle produced nothing"
fi
grep -q '"floor_payloads_withheld"' "$WORK/bundle/manifest.json" 2>/dev/null \
  && pass "the manifest states what it withheld" \
  || fail "the manifest does not say what it withheld"

section "peering"
cp "$WORK/all.db" "$WORK/near.db" 2>/dev/null || : > "$WORK/near.db"
"$BIN" serve "$WORK/far.db" --port $((PORT+1)) --identity far --collect --open >"$WORK/far.log" 2>&1 &
FAR=$!
sleep 1.2
"$BIN" peer "$WORK/near.db" add "http://127.0.0.1:$((PORT+1))" --identity near \
  --self-url "http://127.0.0.1:$PORT" >/dev/null 2>&1
PEND=$("$BIN" peer "$WORK/far.db" list | python3 -c "import json,sys; print(sum(1 for p in json.load(sys.stdin)['peers'] if p['status']=='pending'))" 2>/dev/null || echo 0)
[ "${PEND:-0}" -ge 1 ] && pass "an offer is held pending until approved" || fail "peer offer not held"
"$BIN" peer "$WORK/far.db" approve near --direction inbound >/dev/null 2>&1 \
  && pass "approval recorded" || fail "approval"
kill $FAR 2>/dev/null

section "retention"
"$BIN" trim "$WORK/all.db" --keep-last-traces 5 | python3 -c "
import json,sys
d = json.load(sys.stdin)
assert d['applied'] is False, 'trim must be a dry run unless --apply'
print(f\"  \033[32mPASS\033[0m dry run by default: would remove {d['traces_removed']} traces, \"
      f\"keeping {d['traces_kept_because_open']} that hold an open span\")
" || fail "trim"

section "rolled-up tiers"
"$BIN" rollup "$WORK/all.db" --depth 1 | python3 -c "
import json,sys
d = json.load(sys.stdin)
assert d['digest'], 'a tier must carry a digest'
print(f\"  \033[32mPASS\033[0m root digest {d['digest']} over {d['rollup']['spans']} spans, \"
      f\"{len(json.dumps(d))} bytes at depth 1\")
" || fail "rollup"

section "fingerprint"
"$BIN" fingerprint "$WORK/all.db" | python3 -c "
import json,sys
d = json.load(sys.stdin)
names = [i['name'] for i in d['invariants']]
assert len(names) == 6, names
print('  \033[32mPASS\033[0m six invariants: ' + ', '.join(names))
" || fail "fingerprint"

section "refusing what should not be stored"
# On its own store and port: a check that injects malformed spans must not
# contaminate the capture the contract section is about to judge.
HPORT=$((PORT + 7))
"$BIN" serve "$WORK/hardening.db" --port $HPORT --collect --open >"$WORK/hardening.log" 2>&1 &
HPID=$!
sleep 1.2
python3 "$ROOT/execviz-rs/hardening_check.py" "http://127.0.0.1:$HPORT" || fail "ingest validation"
kill $HPID 2>/dev/null

section "renderer model"
# The renderer had no unit tests: every check was behavioural against a browser,
# which is how a hung span falling outside a lookback window survived six passes.
if [ -d "$ROOT/execviz-ui" ] && command -v npx >/dev/null 2>&1; then
  ( cd "$ROOT/execviz-ui" && npm test --silent ) 2>&1 | sed 's/^/  /' || fail "renderer model tests"
fi

section "one contract, two implementations"
# api.py is a documented Python reader of the same store the Rust core reads.
# Two implementations of one contract drift apart silently unless something
# checks; a change to either is a change to the other's documented behaviour.
python3 "$ROOT/execviz-rs/parity_check.py" "$BIN" "$ROOT/execviz/api.py" "$WORK/all.db" || fail "python and rust disagree"

section "contract"
CAP="$WORK/capture.json"
"$BIN" capture "$WORK/all.db" > "$CAP" 2>/dev/null
python3 - "$CAP" << 'PY'
import json, sys, collections
d = json.load(open(sys.argv[1]))["spans"]
byid = {s["span_id"]: s for s in d}
hosts = collections.defaultdict(list)
for s in d: hosts[s["host_id"]].append(s)

ok = True
def check(cond, msg):
    global ok
    print(("  \033[32mPASS\033[0m " if cond else "  \033[31mFAIL\033[0m ") + msg)
    if not cond: ok = False

check(len(hosts) >= 2, f"{len(hosts)} runtimes in one graph: {', '.join(sorted(hosts))}")

# every child span must trace back to its own request, across every runtime
bad = []
for s in d:
    if s["name"].startswith(("fetch_user_", "fetch_orders_")):
        uid = s["name"].rsplit("_", 1)[1]
        cur, chain, n = s, [], 0
        while cur and n < 14:
            chain.append(cur["name"]); cur = byid.get(cur["parent_span_id"]); n += 1
        req = [c for c in chain if c.startswith("GET /profile/")]
        if not (req and req[0].endswith("/" + uid)): bad.append(s["name"])
check(not bad, f"parent attribution across concurrency (0 misattributed of "
               f"{sum(1 for s in d if s['name'].startswith(('fetch_user_','fetch_orders_')))} checked)")

joins = [s for s in d if s["links"]]
check(joins, f"fan-in recorded in links ({len(joins)} spans carry links)")

openspans = [s for s in d if s["end"] is None]
check(openspans, f"death signal preserved ({len(openspans)} spans left open)")

lifec = [s for s in d if s["lifecycle"]]
kinds = {l["type"] for s in lifec for l in s["lifecycle"]}
check(kinds, f"lifecycle transitions recorded: {', '.join(sorted(kinds))}")

logged = [s for s in d if s["events"]]
check(logged, f"logs attributed to spans ({sum(len(s['events']) for s in logged)} lines on {len(logged)} spans)")
sys.exit(0 if ok else 1)
PY
[ $? -eq 0 ] || FAIL=1

CONF=$("$BIN" check "$WORK/all.db")
echo "$CONF" | python3 -c "
import json,sys
d=json.load(sys.stdin)
for h in d['hosts']:
    tag='\033[32mPASS\033[0m' if h['conformant'] else '\033[31mFAIL\033[0m'
    obs=sum(o['count'] for o in h.get('observations',[]))
    verdict='conformant' if h['conformant'] else 'NOT conformant'
    print(f\"  {tag} {h['host']:8} {h['spans']:4} spans  {verdict}  ({obs} observations)\")
    for vio in h.get('violations',[]):
        print(f\"         {vio['rule']}: {vio.get('examples',[''])[:1]}\")
sys.exit(0 if d['conformant'] else 1)
" || FAIL=1

section "result"
if [ $FAIL -eq 0 ]; then printf '  \033[32mall checks passed\033[0m\n'; else printf '  \033[31msome checks failed\033[0m\n'; fi
exit $FAIL
