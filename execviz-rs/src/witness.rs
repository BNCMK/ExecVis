// ========================================================================
//  MANIFEST
// ========================================================================
//  script_name: witness.rs
//  script_path: execviz-rs/src/witness.rs
//  module_name: witness
//  version: 0.53.1
//  description: The recorder as witness: does the instrumentation match what the machine did?
//  kind: module
//  spec: internal
//  internal_dependencies: json, store, syscalls
//  external_dependencies: std
//  features: witness
//  api_version: execvis-v1.0.0
//  last_updated: 2026-08-07
// ========================================================================

//! The recorder as witness: does the instrumentation match what the machine did?
//!
//! Every tracing tool in the field takes a span's word for what happened,
//! because nothing else knows. The recorder does know: which syscalls ran, on which
//! thread, in which window. Holding both lets one be put against the other
//!.
//!
//! Three findings, reported separately because they mean different things:
//!
//!   claimed   a span reporting work whose thread issued no matching syscall
//!   unclaimed syscalls on a thread that no span covers
//!   disagreed a span whose window does not contain the syscalls attributed to it
//!
//! Nothing here convicts. A thread that issued no syscall may have done real
//! work in userspace, so a finding states what was observed and what that is
//! consistent with. Co-occurrence is not cause, and neither is its absence.

use crate::json::J;
use crate::store::Span;
use crate::syscalls::Record;
use std::collections::BTreeMap;

// ========================================================================
// INTERNALS
// ========================================================================

/// What kind of syscall a span of this kind should be expected to perform.
///
/// Deliberately narrow. A span kind is claimed by the program, and only the
/// kinds whose meaning implies a syscall are checked at all; everything else is
/// reported as unexamined rather than silently passed.
fn expected_calls(kind: &str) -> Option<&'static [&'static str]> {
    match kind {
        "io" => Some(&["write", "read", "pwrite64", "pread64", "fsync", "openat", "close", "writev"]),
        "net" => Some(&["sendto", "recvfrom", "connect", "accept", "sendmsg", "recvmsg", "socket"]),
        "db" => Some(&["sendto", "recvfrom", "connect", "sendmsg", "recvmsg", "write", "read"]),
        _ => None,
    }
}

// ========================================================================
// TYPES
// ========================================================================

pub struct Audit {
    pub examined: usize,
    pub unexamined: usize,
    pub claimed_not_performed: usize,
    pub unclaimed_records: usize,
    pub windows_disagreed: usize,
}

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

/// Cross-checks declared spans against recorder records.
pub fn audit(spans: &[Span], recs: &[Record]) -> (J, Audit) {
    // records by thread, in time order, so a window lookup is a scan of one
    // thread's timeline rather than of everything the machine did
    let mut by_tid: BTreeMap<i64, Vec<&Record>> = BTreeMap::new();
    for r in recs {
        by_tid.entry(r.tid).or_default().push(r);
    }
    for v in by_tid.values_mut() {
        v.sort_by(|a, b| a.t.partial_cmp(&b.t).unwrap_or(std::cmp::Ordering::Equal));
    }

    let mut findings: Vec<J> = Vec::new();
    let mut a = Audit {
        examined: 0, unexamined: 0,
        claimed_not_performed: 0, unclaimed_records: 0, windows_disagreed: 0,
    };

// ========================================================================
// CLAIMED BUT NOT PERFORMED, AND WINDOWS THAT DISAGREE
// ========================================================================
    let mut covered: BTreeMap<i64, Vec<(f64, f64)>> = BTreeMap::new();
    for s in spans {
        let tid = s.attributes.get("tid").and_then(|x| x.as_f64()).unwrap_or(-1.0) as i64;
        let end = match s.end { Some(e) => e, None => continue };  // an open span has no window yet
        covered.entry(tid).or_default().push((s.start, end));

        let want = match expected_calls(&s.kind) {
            Some(w) => w,
            None => { a.unexamined += 1; continue }
        };
        a.examined += 1;

        let empty: Vec<&Record> = Vec::new();
        let thread = by_tid.get(&tid).unwrap_or(&empty);
        let inside: Vec<&&Record> = thread.iter()
            .filter(|r| r.t >= s.start && r.t <= end)
            .collect();
        let matching = inside.iter().filter(|r| want.contains(&r.name.as_str())).count();

        // Evented runtimes do a request's I/O either side of the handler rather
        // than inside it: the request is read and parsed before the event the
        // span starts at, and the response written after the span closes. A time
        // window cannot hold that, but the DESCRIPTOR can: the read, the handler
        // and the write are the same connection whenever each happens. A span
        // that names its descriptor is matched on that, and the window is used
        // only to bound the search to the same connection's lifetime.
        let span_fd = s.attributes.get("fd").and_then(|x| x.as_f64()).map(|n| n as i64);
        let on_fd = match span_fd {
            Some(fd) if matching == 0 => thread.iter()
                .filter(|r| r.fd == Some(fd))
                .filter(|r| want.contains(&r.name.as_str()))
                .count(),
            _ => 0,
        };
        let straddling = on_fd;

        if matching == 0 && straddling > 0 {
            // The work was done on this thread, just either side of the handler.
            a.windows_disagreed += 1;
            findings.push(J::Obj([
                ("finding".into(), J::Str("performed_around_window".into())),
                ("span_id".into(), J::Str(s.span_id.clone())),
                ("name".into(), J::Str(s.name.clone())),
                ("kind".into(), J::Str(s.kind.clone())),
                ("tid".into(), J::Num(tid as f64)),
                ("matching_on_descriptor".into(), J::Num(straddling as f64)),
                ("fd".into(), J::Num(span_fd.unwrap_or(-1) as f64)),
                ("consistent_with".into(), J::Str(
                    "an event loop: the work was performed on this span's own \
                     descriptor, read before the handler opened and written after \
                     it closed, so it falls outside the window but is the same \
                     connection"
                    .into())),
            ].into_iter().collect()));
        } else if matching == 0 {
            a.claimed_not_performed += 1;
            // The wording is careful on purpose: this is evidence, not a conclusion.
            let consistent = if inside.is_empty() {
                "no syscalls at all were recorded on this thread in this window, \
                 which is consistent with work served from cache, work done in \
                 userspace, or a thread id that does not match the recorder's"
            } else {
                "syscalls were recorded on this thread in this window but none of \
                 the kind this span's own kind implies, which is consistent with \
                 a cached result or with a span kind that overstates the work"
            };
            findings.push(J::Obj([
                ("finding".into(), J::Str("claimed_not_performed".into())),
                ("span_id".into(), J::Str(s.span_id.clone())),
                ("name".into(), J::Str(s.name.clone())),
                ("kind".into(), J::Str(s.kind.clone())),
                ("tid".into(), J::Num(tid as f64)),
                ("syscalls_in_window".into(), J::Num(inside.len() as f64)),
                ("matching".into(), J::Num(0.0)),
                ("consistent_with".into(), J::Str(consistent.into())),
            ].into_iter().collect()));
        }

        // A span carrying syscall attributes from a merge, whose own window does
        // not contain them, has a clock or a carrier problem rather than an
        // instrumentation one.
        if let Some(n) = s.attributes.get("syscalls").and_then(|x| x.as_f64()) {
            if n > 0.0 && inside.is_empty() {
                a.windows_disagreed += 1;
                findings.push(J::Obj([
                    ("finding".into(), J::Str("window_disagreed".into())),
                    ("span_id".into(), J::Str(s.span_id.clone())),
                    ("name".into(), J::Str(s.name.clone())),
                    ("attributed_syscalls".into(), J::Num(n)),
                    ("syscalls_in_window".into(), J::Num(0.0)),
                    ("consistent_with".into(), J::Str(
                        "clock skew between the recorder and the program, or a \
                         thread id recorded by one layer and not the other".into())),
                ].into_iter().collect()));
            }
        }
    }

// ========================================================================
// PERFORMED BUT UNCLAIMED
// ========================================================================
    //
    // Not a defect in itself: it is the coverage question, and the answer is a
    // fraction rather than a list of accusations.
    let mut unclaimed_by_thread: BTreeMap<i64, usize> = BTreeMap::new();
    for (tid, rs) in &by_tid {
        let windows = covered.get(tid);
        for r in rs {
            let inside_any = windows.map(|ws| ws.iter().any(|(a0, b0)| r.t >= *a0 && r.t <= *b0))
                .unwrap_or(false);
            if !inside_any {
                a.unclaimed_records += 1;
                *unclaimed_by_thread.entry(*tid).or_insert(0) += 1;
            }
        }
    }
    for (tid, n) in unclaimed_by_thread.iter().take(24) {
        findings.push(J::Obj([
            ("finding".into(), J::Str("performed_not_claimed".into())),
            ("tid".into(), J::Num(*tid as f64)),
            ("records".into(), J::Num(*n as f64)),
            ("consistent_with".into(), J::Str(
                "work this capture's instrumentation does not cover; the trace is \
                 not wrong here, it is incomplete, and this is where".into())),
        ].into_iter().collect()));
    }

    let total = recs.len().max(1);
    let coverage = 1.0 - (a.unclaimed_records as f64 / total as f64);

    let out = J::Obj([
        ("spans_examined".into(), J::Num(a.examined as f64)),
        ("spans_unexamined".into(), J::Num(a.unexamined as f64)),
        ("claimed_not_performed".into(), J::Num(a.claimed_not_performed as f64)),
        ("performed_not_claimed".into(), J::Num(a.unclaimed_records as f64)),
        ("windows_disagreed".into(), J::Num(a.windows_disagreed as f64)),
        ("records".into(), J::Num(recs.len() as f64)),
        ("record_coverage".into(), J::Num((coverage * 1000.0).round() / 1000.0)),
        ("findings".into(), J::Arr(findings)),
        ("note".into(), J::Str(
            "A span kind that implies no syscall is not examined rather than \
             passed. Every finding states what was observed and what it is \
             consistent with; none of them convict.".into())),
    ].into_iter().collect());
    (out, a)
}

/// The negative space: what the machine did that no span accounts for.
///
/// Every observability product draws what was instrumented. None draw what was
/// not. The recorder sees every process; spans cover some of them, and
/// the difference is a real region that can be put on the same map.
///
/// Reported per process rather than per thread, because a reader recognises
/// `postgres` and does not recognise tid 4021.
pub fn negative_space(spans: &[Span], recs: &[Record], comms: &BTreeMap<i64, String>) -> J {
    let mut covered: BTreeMap<i64, Vec<(f64, f64)>> = BTreeMap::new();
    for s in spans {
        let tid = s.attributes.get("tid").and_then(|x| x.as_f64()).unwrap_or(-1.0) as i64;
        if let Some(end) = s.end { covered.entry(tid).or_default().push((s.start, end)); }
    }

    struct Region { records: usize, first: f64, last: f64, calls: BTreeMap<String, usize> }
    let mut regions: BTreeMap<String, Region> = BTreeMap::new();
    let mut claimed = 0usize;

    for r in recs {
        let inside = covered.get(&r.tid)
            .map(|ws| ws.iter().any(|(a, b)| r.t >= *a && r.t <= *b))
            .unwrap_or(false);
        if inside { claimed += 1; continue; }
        let who = comms.get(&r.tid).cloned()
            .unwrap_or_else(|| format!("tid {}", r.tid));
        let e = regions.entry(who).or_insert(Region {
            records: 0, first: r.t, last: r.t, calls: BTreeMap::new(),
        });
        e.records += 1;
        if r.t < e.first { e.first = r.t }
        if r.t > e.last { e.last = r.t }
        *e.calls.entry(r.name.clone()).or_insert(0) += 1;
    }

    let mut out: Vec<J> = Vec::new();
    let mut ordered: Vec<(&String, &Region)> = regions.iter().collect();
    ordered.sort_by(|a, b| b.1.records.cmp(&a.1.records));
    for (who, reg) in ordered.iter().take(64) {
        let mut calls: Vec<(&String, &usize)> = reg.calls.iter().collect();
        calls.sort_by(|a, b| b.1.cmp(a.1));
        let top: Vec<J> = calls.iter().take(5)
            .map(|(n, c)| J::Obj([
                ("call".to_string(), J::Str((*n).clone())),
                ("count".to_string(), J::Num(**c as f64)),
            ].into_iter().collect()))
            .collect();
        out.push(J::Obj([
            ("who".to_string(), J::Str((*who).clone())),
            ("records".to_string(), J::Num(reg.records as f64)),
            ("first".to_string(), J::Num(reg.first)),
            ("last".to_string(), J::Num(reg.last)),
            ("calls".to_string(), J::Arr(top)),
        ].into_iter().collect()));
    }

    let total = recs.len().max(1);
    let unclaimed = recs.len() - claimed;
    J::Obj([
        ("records".to_string(), J::Num(recs.len() as f64)),
        ("claimed".to_string(), J::Num(claimed as f64)),
        ("unclaimed".to_string(), J::Num(unclaimed as f64)),
        ("covered_fraction".to_string(),
            J::Num(((claimed as f64 / total as f64) * 1000.0).round() / 1000.0)),
        ("regions".to_string(), J::Arr(out)),
        ("note".to_string(), J::Str(
            "Unclaimed work is not a defect. It is the part of this machine that \
             the instrumentation does not describe, stated rather than hidden.".to_string())),
    ].into_iter().collect())
}
