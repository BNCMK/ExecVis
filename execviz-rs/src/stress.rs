// ========================================================================
//  MANIFEST
// ========================================================================
//  script_name: stress.rs
//  script_path: execviz-rs/src/stress.rs
//  module_name: stress
//  version: 0.53.1
//  description: Stress derived from observed shape.
//  kind: module
//  spec: internal
//  internal_dependencies: decode, json, store, syscalls
//  external_dependencies: std
//  features: stress
//  api_version: execvis-v1.0.0
//  last_updated: 2026-08-07
// ========================================================================

//! Stress derived from observed shape.
//!
//! Every fault-injection tool in the field asks the operator to author the
//! fault: pick the syscall, pick the failure, write the scenario. That only ever
//! tests the failures somebody already imagined, which are by construction not
//! the ones that take a service down.
//!
//! The recorder has already watched the program. It knows, without being told and
//! without the program being instrumented, which syscall families were used,
//! whether reads came back short, how many descriptors were open, whether
//! anything was written to a socket, whether the process blocked and on what.
//! A stress plan can therefore be DERIVED from what the program did
//! rather than from what a test author guessed it might do.
//!
//! This module derives and reports the plan. It injects nothing. That split is
//! deliberate: the derivation is the novel part and it is worth being able to
//! read, argue with and correct before anything is allowed to interfere with a
//! running process.
//!
//! The honesty rules that govern the rest of the tool govern this too:
//!
//! - Every proposed stressor carries the EVIDENCE that produced it, with counts.
//!   A proposal no reader can check against the capture is a guess wearing a
//!   uniform.
//! - Stressors that do NOT apply are reported, with the evidence that was
//!   absent. "This program never opened a socket, so no socket faults are
//!   proposed" is the same negative-space claim the rest of the tool makes, and
//!   it is the half that tells a reader the plan was derived rather than
//!   recited.
//! - A capture too thin to characterise produces NO plan and reports it. Deriving
//!   six confident stressors from nine records would be the worst thing this
//!   command could do.
//! - Nothing here judges the program. The plan says what would be exercised and
//!   what that would demonstrate, never that the program is correct or broken.

use crate::json::J;
use std::collections::{BTreeMap, BTreeSet};

// ========================================================================
// TYPES
// ========================================================================

/// What the capture showed, before any interpretation.
pub struct Shape {
    pub records: usize,
    pub inbound: usize,
    pub outbound: usize,
    pub truncated: usize,
    pub small_writes: usize,
    pub large_writes: usize,
    pub sockets: usize,
    pub files: usize,
    pub std_streams: usize,
    pub blocking: usize,
    pub binary: usize,
    pub distinct_pids: usize,
    pub distinct_fds: usize,
    pub max_fd: i64,
    pub protocols: BTreeSet<String>,
    pub comms: BTreeSet<String>,
    pub calls: BTreeMap<String, usize>,
    /// How many calls happened before this program first produced output of its
    /// own. That is its startup, and a fault landing there kills it before its
    /// code runs.
    pub calls_before_first_output: usize,
}

// ========================================================================
// INTERNALS
// ========================================================================

/// Syscalls that mean the program stopped and waited for something. Which
/// architecture's numbers these are matters, so the caller passes the names the
/// records already carry rather than this module guessing from a number.
fn is_blocking(name: &str) -> bool {
    matches!(name,
        "poll" | "ppoll" | "select" | "pselect6" | "epoll_wait" | "epoll_pwait"
        | "futex" | "nanosleep" | "clock_nanosleep" | "accept" | "accept4")
}

fn is_socket_call(name: &str) -> bool {
    matches!(name,
        "socket" | "connect" | "accept" | "accept4" | "sendto" | "sendmsg"
        | "recvfrom" | "recvmsg" | "bind" | "listen" | "shutdown")
}

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

pub fn observe(ndjson: &str) -> Shape {
    let mut s = Shape {
        records: 0, inbound: 0, outbound: 0, truncated: 0,
        small_writes: 0, large_writes: 0, sockets: 0, files: 0, std_streams: 0,
        blocking: 0, binary: 0, distinct_pids: 0, distinct_fds: 0, max_fd: -1,
        protocols: BTreeSet::new(), comms: BTreeSet::new(), calls: BTreeMap::new(),
        calls_before_first_output: 0,
    };
    let mut pids: BTreeSet<i64> = BTreeSet::new();
    let mut calls_so_far = 0usize;
    let mut seen_output = false;
    let mut fds: BTreeSet<String> = BTreeSet::new();

    for line in ndjson.lines() {
        let line = line.trim();
        if line.is_empty() { continue; }
        let v = match crate::json::parse(line) { Ok(v) => v, Err(_) => continue };
        s.records += 1;

        if let Some(p) = v.get("pid").and_then(|x| x.as_f64()) { pids.insert(p as i64); }
        if let Some(c) = v.get("comm").and_then(|x| x.as_str()) {
            if !c.is_empty() && c != "unknown" { s.comms.insert(c.to_string()); }
        }

        // A record naming a syscall is a call record; one carrying a payload is
        // a write or a read. They are counted separately because they answer
        // different questions.
        if let Some(nr) = v.get("nr").and_then(|x| x.as_f64()) {
            // The record says which table its numbers came from; assuming would
            // name arm64's socket as sched_setaffinity and count it as neither.
            let arch = v.get("arch").and_then(|x| x.as_str()).unwrap_or("x86_64");
            let name = crate::syscalls::name_of_arch(nr as i64, arch);
            if is_blocking(&name) { s.blocking += 1; }
            if is_socket_call(&name) { s.sockets += 1; }
            *s.calls.entry(name).or_insert(0) += 1;
            if !seen_output { calls_so_far += 1; }
        }

        if let Some(log) = v.get("log").and_then(|x| x.as_str()) {
            if !seen_output { seen_output = true; s.calls_before_first_output = calls_so_far; }
            let bytes = v.get("bytes").and_then(|x| x.as_f64()).unwrap_or(log.len() as f64) as u64;
            let dir = v.get("direction").and_then(|x| x.as_str()).unwrap_or("out");
            if dir == "in" { s.inbound += 1; } else { s.outbound += 1; }
            if matches!(v.get("truncated"), Some(J::Bool(true))) { s.truncated += 1; }
            if v.get("kind").and_then(|x| x.as_str()) == Some("binary") { s.binary += 1; }
            if bytes > 0 && bytes <= 64 { s.small_writes += 1; }
            if bytes >= 4096 { s.large_writes += 1; }
            if let Some(w) = v.get("where").and_then(|x| x.as_str()) {
                fds.insert(w.to_string());
                if w == "stdout" || w == "stderr" { s.std_streams += 1; }
                else if w.starts_with('/') { s.files += 1; }
                // `where` is a resolved path, or a descriptor the recorder could
                // not resolve, written as `fd7 (unresolved)`. Take the leading
                // digits so the number survives the suffix.
                if let Some(rest) = w.strip_prefix("fd") {
                    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
                    if let Ok(n) = digits.parse::<i64>() { if n > s.max_fd { s.max_fd = n; } }
                }
            }
            if let Some(d) = crate::decode::sniff(log) { s.protocols.insert(d.protocol.to_string()); }
        }
    }
    s.distinct_pids = pids.len();
    s.distinct_fds = fds.len();
    s
}

// ========================================================================
// INTERNALS
// ========================================================================

/// Count the records the recorder marked as error level, and the last moment
/// anything was recorded. Both are needed to tell "handled it and carried on"
/// from "stopped early".
fn errors_and_end(ndjson: &str) -> (usize, f64) {
    let (mut errs, mut last) = (0usize, 0.0f64);
    for line in ndjson.lines() {
        let v = match crate::json::parse(line) { Ok(v) => v, Err(_) => continue };
        if v.get("level").and_then(|x| x.as_str()) == Some("error") { errs += 1; }
        if let Some(t) = v.get("t").and_then(|x| x.as_f64()) { if t > last { last = t; } }
    }
    (errs, last)
}

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

/// What changed when the fault was injected.
///
/// This reports OBSERVATIONS, never a verdict. The tool cannot know what the
/// program was supposed to do under a fault it was never designed for, so
/// saying "handled correctly" or "failed" would be inventing a standard nobody
/// stated. What it can do is put the two captures side by side and name every
/// difference, which is what a reader needs in order to judge.
pub fn compare(baseline: &str, stressed: &str) -> J {
    let (b, s) = (observe(baseline), observe(stressed));
    let (berr, bend) = errors_and_end(baseline);
    let (serr, send) = errors_and_end(stressed);

    let mut out = J::obj();
    let mut before = J::obj();
    before.set("records", J::n(b.records as f64));
    before.set("error_records", J::n(berr as f64));
    before.set("processes", J::n(b.distinct_pids as f64));
    before.set("blocking_calls", J::n(b.blocking as f64));
    before.set("last_record_at", J::n(bend));
    out.set("baseline", before);

    let mut after = J::obj();
    after.set("records", J::n(s.records as f64));
    after.set("error_records", J::n(serr as f64));
    after.set("processes", J::n(s.distinct_pids as f64));
    after.set("blocking_calls", J::n(s.blocking as f64));
    after.set("last_record_at", J::n(send));
    out.set("under_stress", after);

    let mut obs: Vec<J> = Vec::new();

    if serr > berr {
        obs.push(J::s("the program wrote more to its error stream under stress, so the fault \
              reached code that had something to say about it"));
    } else if serr == berr && berr == 0 {
        obs.push(J::s("the program wrote nothing to its error stream under stress; either the \
              fault was handled silently or it was not noticed, and these look the same \
              from outside"));
    }
    if s.distinct_pids < b.distinct_pids {
        obs.push(J::s("fewer processes were seen under stress than without it, so something that \
              ran before did not run or did not survive"));
    }
    if s.blocking > b.blocking * 2 && b.blocking > 0 {
        obs.push(J::s("blocking calls more than doubled, which is the shape of retrying or backing \
              off after a failure"));
    }
    if s.records * 2 < b.records && b.records > 0 {
        obs.push(J::s("far fewer records were produced under stress, so the program did less work; \
              worth checking whether it finished at all"));
    }
    if obs.is_empty() {
        obs.push(J::s("no difference large enough to name; the fault either did not fire often \
              enough to matter or the program absorbed it without changing what it did"));
    }
    out.set("observations", J::Arr(obs));
    out.set("this_does_not_say", J::s(
        "whether the program behaved CORRECTLY under the fault. Nothing here knows what \
         it was supposed to do when the fault it was never designed for arrived. These \
         are the differences between the two captures, for a reader to judge."));
    out
}

// ========================================================================
// INTERNALS
// ========================================================================

fn stressor(id: &str, why: &str, what: &str, proves: &str, how: &str, evidence: f64) -> J {
    let mut o = J::obj();
    o.set("stressor", J::s(id));
    o.set("derived_from", J::s(why));
    o.set("evidence_count", J::n(evidence));
    o.set("would_do", J::s(what));
    o.set("would_demonstrate", J::s(proves));
    o.set("injection", J::s(how));
    o
}

fn excluded(id: &str, why: &str) -> J {
    let mut o = J::obj();
    o.set("stressor", J::s(id));
    o.set("not_proposed_because", J::s(why));
    o
}

// ========================================================================
// CONSTANTS
// ========================================================================

/// The minimum a capture must contain before its shape means anything. Below
/// this the command refuses rather than guessing, for the same reason the
/// fingerprint refuses a thin sample.
pub const MIN_RECORDS: usize = 200;

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

pub fn plan(ndjson: &str, min_records: usize) -> (J, i32) {
    let s = observe(ndjson);
    let mut out = J::obj();
    out.set("records_read", J::n(s.records as f64));

    if s.records < min_records {
        // Absent values are reported as absent, not as zero, and a thin capture is not a program with no shape.
        out.set("plan", J::s("none"));
        out.set("reason", J::s(
            "too few records to characterise this program's shape; a plan derived from \
             this little would be a guess presented as a derivation"));
        out.set("records_needed", J::n(min_records as f64));
        out.set("what_to_do", J::s(
            "capture for longer, or under real load, then run this again"));
        return (out, 1);
    }

    let mut proposed: Vec<J> = Vec::new();
    let mut skipped: Vec<J> = Vec::new();

    // Reads that came back with something: does the caller handle a SHORT one?
    if s.inbound > 0 {
        proposed.push(stressor(
            "short_read",
            "the program reads data in, so every read it makes has a length it \
             asked for and a length it got, and those are allowed to differ",
            "return fewer bytes than requested on a fraction of reads, without an error",
            "whether the code treats a partial read as a complete one, which is the \
             most common way a parser silently corrupts a message under load",
            "seccomp user notification on the read family, returning a shortened count",
            s.inbound as f64));
    } else {
        skipped.push(excluded("short_read",
            "no inbound payloads were captured; this program was not observed reading data in"));
    }

    // Sockets: backpressure and a peer that goes away.
    if s.sockets > 0 || s.protocols.iter().any(|p| p.starts_with("http") || p == "postgres" || p == "resp") {
        proposed.push(stressor(
            "slow_consumer",
            "socket calls were observed, so this program has a peer whose speed it \
             does not control",
            "delay and partially drain the receiving side so writes block and buffers fill",
            "whether backpressure is handled or  waited on, and whether a stalled \
             peer turns into a stuck span rather than an error",
            "seccomp user notification delaying the send family; no change to the program",
            s.sockets as f64));
        proposed.push(stressor(
            "peer_disappears",
            "the same socket activity implies a connection that can end at any point",
            "close the far end mid-exchange, so a send fails and a read returns zero",
            "whether an ended connection is distinguished from an idle one, which is the \
             ambiguity this project refuses to tolerate elsewhere",
            "seccomp user notification returning ECONNRESET, then zero, on the connection",
            s.sockets as f64));
    } else {
        skipped.push(excluded("slow_consumer",
            "no socket calls and no network protocol were observed; this program was not \
             seen talking to a peer"));
        skipped.push(excluded("peer_disappears",
            "no socket calls were observed, so there is no connection to end"));
    }

    // Descriptor pressure, sized from what was open.
    if s.max_fd >= 8 || s.distinct_fds >= 8 {
        proposed.push(stressor(
            "descriptor_exhaustion",
            "the program worked with many descriptors at once",
            "lower the descriptor limit until open and accept begin to fail",
            "whether an exhausted descriptor table produces a handled error or a crash, \
             and whether the failure is attributed to the right operation",
            "RLIMIT_NOFILE on the target, no interception needed",
            s.distinct_fds.max(s.max_fd.max(0) as usize) as f64));
    } else {
        skipped.push(excluded("descriptor_exhaustion",
            "few descriptors were seen open at once; exhausting the table would test the \
             kernel rather than this program"));
    }

    // Waiting is where interruption lives.
    if s.blocking > 0 {
        proposed.push(stressor(
            "interrupted_wait",
            "the program blocks and waits, so its waits can be interrupted",
            "deliver signals so blocking calls return EINTR rather than completing",
            "whether interrupted waits are retried or mistaken for completion, which \
             turns into work silently skipped",
            "signal delivery to the target while it is in a blocking call",
            s.blocking as f64));
    } else {
        skipped.push(excluded("interrupted_wait",
            "no blocking calls were observed; there is no wait here to interrupt"));
    }

    // Big payloads mean the boundary cases live at the edges of a buffer.
    if s.truncated > 0 || s.large_writes > 0 {
        proposed.push(stressor(
            "payload_boundary",
            "payloads larger than a single buffer were observed",
            "split writes and reads at awkward offsets, including inside a multi-byte \
             character and inside a protocol header",
            "whether message framing survives being cut at a boundary the code did not \
             choose, which is the defect this project's own record stream had",
            "seccomp user notification splitting the transfer, no change to the program",
            (s.truncated + s.large_writes) as f64));
    } else {
        skipped.push(excluded("payload_boundary",
            "no oversized or truncated payloads were observed; every transfer fitted"));
    }

    // Many small writes are a latency profile, not a throughput one.
    if s.small_writes > s.large_writes * 4 && s.small_writes > 20 {
        proposed.push(stressor(
            "write_latency",
            "output is dominated by many small writes rather than few large ones",
            "add per-call latency to the write path",
            "whether throughput depends on writes being cheap, which is the assumption \
             that breaks when the same code meets a slower disk or a busier host",
            "seccomp user notification delaying the write family",
            s.small_writes as f64));
    } else {
        skipped.push(excluded("write_latency",
            "output was not dominated by small writes, so per-call latency would not be \
             the dominant cost here"));
    }

    // More than one process means the interesting failures are between them.
    if s.distinct_pids > 1 {
        proposed.push(stressor(
            "child_dies",
            "more than one process was observed, so this program's work depends on \
             processes other than itself",
            "kill a participating process partway through its work",
            "whether the survivor notices, and whether the map shows the missing work \
             as unfinished rather than as never having existed",
            "signal to a selected child; the recorder already records every process",
            s.distinct_pids as f64));
    } else {
        skipped.push(excluded("child_dies",
            "only one process was observed; there is no second participant to remove"));
    }

    out.set("observed_shape", {
        let mut o = J::obj();
        o.set("processes", J::n(s.distinct_pids as f64));
        o.set("programs", J::Arr(s.comms.iter().map(|c| J::s(c)).collect()));
        o.set("inbound_payloads", J::n(s.inbound as f64));
        o.set("outbound_payloads", J::n(s.outbound as f64));
        o.set("socket_calls", J::n(s.sockets as f64));
        o.set("blocking_calls", J::n(s.blocking as f64));
        o.set("distinct_descriptors", J::n(s.distinct_fds as f64));
        o.set("truncated_payloads", J::n(s.truncated as f64));
        o.set("protocols", J::Arr(s.protocols.iter().map(|p| J::s(p)).collect()));
        o
    });
    // WHAT THE SUPERVISOR SHOULD BE TOLD, worked out from the capture rather
    // than guessed at the command line.
    //
    // A fault injected from the first call lands on process startup: the loader
    // reading shared libraries, the runtime reading its own files. That kills
    // the program before its own code runs and demonstrates nothing about it.
    // The number of calls that belongs to startup is program-specific, but it is
    // plainly visible here: it is the calls that happened before the program
    // first produced any output of its own.
    //
    // The rate is chosen so a run injects enough faults to be worth reading
    // without turning every call into one, which stops being a test of the
    // program and becomes a test of whether it can run at all.
    let startup = s.calls_before_first_output;
    let readish = s.inbound.max(1);
    let suggested_rate = if readish > 20 { (readish / 15).max(2) } else { 2 };
    out.set("suggested", {
        let mut o = J::obj();
        o.set("after", J::n(startup as f64));
        o.set("after_derived_from", J::s(
            "calls made before this program first produced output of its own, which is \
             its startup; faults landing there kill it before its code runs"));
        o.set("rate", J::n(suggested_rate as f64));
        o.set("rate_derived_from", J::s(
            "chosen to inject enough faults to read without failing so many calls that \
             the run tests whether the program can start rather than how it copes"));
        o
    });

    out.set("proposed", J::Arr(proposed));
    out.set("not_proposed", J::Arr(skipped));
    out.set("note", J::s(
        "This plan is derived from what the program was observed doing, not from a \
         catalogue of faults. Nothing here has been executed and nothing here judges \
         the program: each entry says what would be exercised and what that would \
         demonstrate. The excluded list is part of the answer, because it says which \
         failures this program cannot have."));
    (out, 0)
}

/// A plan derived from the spans a collector holds, rather than from floor
/// records.
///
/// The collector stores spans, not syscalls, so the shape available here is
/// coarser: kinds, statuses, unfinished work and the routes between services.
/// It is reported as what it is. A plan derived from recorder records sees the
/// syscalls themselves and is the better one where a capture exists.
pub fn plan_from_spans(spans: &[crate::store::Span]) -> J {
    let (mut io, mut wait, mut queue, mut errors, mut open, mut ext) = (0usize, 0, 0, 0, 0, 0);
    let mut hosts: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    let mut cross = 0usize;
    let by_id: std::collections::HashMap<&str, &crate::store::Span> =
        spans.iter().map(|s| (s.span_id.as_str(), s)).collect();
    for s in spans {
        hosts.insert(s.host_id.as_str());
        match s.kind.as_str() {
            "io" => io += 1,
            "external" => { io += 1; ext += 1; }
            "wait" => wait += 1,
            "queue" => queue += 1,
            _ => {}
        }
        if s.status == "error" { errors += 1; }
        if s.end.is_none() { open += 1; }
        if let Some(p) = s.parent_span_id.as_deref() {
            if let Some(par) = by_id.get(p) { if par.host_id != s.host_id { cross += 1; } }
        }
    }

    let mut proposed: Vec<J> = Vec::new();
    let mut skipped: Vec<J> = Vec::new();
    let mut add = |v: &mut Vec<J>, id: &str, why: &str, n: usize, does: &str| {
        let mut o = J::obj();
        o.set("stressor", J::s(id));
        o.set("derived_from", J::s(why));
        o.set("evidence_count", J::n(n as f64));
        o.set("would_do", J::s(does));
        v.push(o);
    };

    if ext > 0 || cross > 0 {
        add(&mut proposed, "peer_disappears",
            "spans crossing between hosts, or marked external", ext.max(cross),
            "fail reads from the peer as if the connection ended");
        add(&mut proposed, "slow_consumer",
            "the same cross-host work implies a peer whose speed this side does not control",
            ext.max(cross), "delay the sending side so buffers fill");
    } else {
        skipped.push({ let mut o = J::obj(); o.set("stressor", J::s("peer_disappears"));
            o.set("not_proposed_because", J::s("no external or cross-host spans in this capture")); o });
    }
    if io > 0 {
        add(&mut proposed, "short_read", "spans of kind io or external", io,
            "return fewer bytes than were asked for, with the read performed");
    } else {
        skipped.push({ let mut o = J::obj(); o.set("stressor", J::s("short_read"));
            o.set("not_proposed_because", J::s("no io spans in this capture")); o });
    }
    if wait > 0 {
        add(&mut proposed, "interrupted_wait", "spans of kind wait", wait,
            "return EINTR from blocking calls");
    } else {
        skipped.push({ let mut o = J::obj(); o.set("stressor", J::s("interrupted_wait"));
            o.set("not_proposed_because", J::s("no wait spans in this capture")); o });
    }
    if queue > 0 {
        add(&mut proposed, "queue_backpressure", "spans of kind queue", queue,
            "hold the queue so producers block");
    }
    if open > 0 {
        add(&mut proposed, "child_dies", "spans started with no end recorded", open,
            "kill a participating process partway through its work");
    }

    let mut out = J::obj();
    out.set("derived_from", J::s("spans"));
    out.set("spans_read", J::n(spans.len() as f64));
    let mut shape = J::obj();
    shape.set("hosts", J::n(hosts.len() as f64));
    shape.set("io_spans", J::n(io as f64));
    shape.set("wait_spans", J::n(wait as f64));
    shape.set("queue_spans", J::n(queue as f64));
    shape.set("external_spans", J::n(ext as f64));
    shape.set("cross_host_spans", J::n(cross as f64));
    shape.set("errored", J::n(errors as f64));
    shape.set("started_no_end", J::n(open as f64));
    out.set("observed_shape", shape);
    out.set("proposed", J::Arr(proposed));
    out.set("not_proposed", J::Arr(skipped));
    out.set("note", J::s(
        "Derived from spans. A plan derived from recorder records sees the syscalls \
         themselves and is more precise. Nothing here has been executed; run one \
         with execviz-stress --from-plan."));
    out
}
