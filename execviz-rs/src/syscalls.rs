// ========================================================================
//  MANIFEST
// ========================================================================
//  script_name: syscalls.rs
//  script_path: execviz-rs/src/syscalls.rs
//  module_name: syscalls
//  version: 0.53.1
//  description: Merging the syscall stream into the semantic one (spec 2.4, 5.3).
//  kind: module
//  spec: internal
//  internal_dependencies: json, store
//  external_dependencies: std
//  features: syscalls
//  api_version: execvis-v1.0.0
//  last_updated: 2026-08-07
// ========================================================================

//! Merging the syscall stream into the semantic one (spec 2.4, 5.3).
//!
//! Syscall spans enrich the semantic span that was active when they occurred.
//! Attribution is by observation: a record carries a thread id and a timestamp,
//! and is attached to the innermost semantic span running on that thread at that
//! instant. A record with no span around it is kept against the host rather than
//! attached to whatever happened to be nearby.
use crate::json::J;
use crate::store::{Span, Store};
use std::collections::BTreeMap;

// ========================================================================
// TYPES
// ========================================================================

pub struct Record {
    pub t: f64, pub dur: f64, pub tid: i64, pub name: String,
    /// The descriptor the call acted on, when the recorder knew it. This is what
    /// ties a read, the handler that ran between, and the write back together,
    /// however far apart in time they happen to sit.
    pub fd: Option<i64>,
    /// The program that wrote it, when the recorder knew. A region named
    /// `postgres` is one a reader recognises; `tid 4021` is not.
    pub comm: Option<String>,
}

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

/// x86_64 numbers for the calls worth naming. Anything else keeps its number,
/// because inventing a name for a syscall the adapter does not know is exactly
/// the kind of guess this design refuses to make.
/// Name a syscall number for the architecture the record came from.
///
/// The same number names a different call on each architecture, so a table
/// applied to the wrong one does not fail, it reports a plausible wrong name:
/// arm64's `socket` is 198, which on x86_64 is `sched_setaffinity`. Records
/// carry their `arch` for exactly this reason, and a reader that ignores it will
/// quietly mis-describe every capture taken on the other machine.
pub fn name_of_arch(nr: i64, arch: &str) -> String {
    if arch == "aarch64" { return arm64_name(nr); }
    syscall_name(nr)
}

pub fn name_of(nr: i64) -> String { syscall_name(nr) }

// ========================================================================
// INTERNALS
// ========================================================================

/// The asm-generic table, which arm64 uses. Only the calls worth naming, on the
/// same principle as the x86_64 table below: a number with no name keeps its
/// number rather than being given a guess.
fn arm64_name(nr: i64) -> String {
    match nr {
        63 => "read", 64 => "write", 56 => "openat", 57 => "close", 62 => "lseek",
        222 => "mmap", 215 => "munmap", 29 => "ioctl", 67 => "pread64", 68 => "pwrite64",
        59 => "pipe2", 72 => "pselect6", 73 => "ppoll", 23 => "dup", 101 => "nanosleep",
        172 => "getpid", 198 => "socket", 203 => "connect", 206 => "sendto",
        207 => "recvfrom", 211 => "sendmsg", 212 => "recvmsg", 200 => "bind",
        201 => "listen", 202 => "accept", 242 => "accept4", 210 => "shutdown",
        220 => "clone", 221 => "execve", 93 => "exit", 94 => "exit_group",
        25 => "fcntl", 82 => "fsync", 35 => "unlinkat", 61 => "getdents64",
        79 => "newfstatat", 80 => "fstat", 98 => "futex", 22 => "epoll_pwait",
        20 => "epoll_create1", 21 => "epoll_ctl", 65 => "readv", 66 => "writev",
        _ => return format!("syscall_{}", nr),
    }.to_string()
}

fn syscall_name(nr: i64) -> String {
    match nr {
        0 => "read", 1 => "write", 2 => "open", 3 => "close", 8 => "lseek",
        9 => "mmap", 11 => "munmap", 16 => "ioctl", 17 => "pread64", 18 => "pwrite64",
        22 => "pipe", 23 => "select", 32 => "dup", 35 => "nanosleep", 39 => "getpid",
        41 => "socket", 42 => "connect", 44 => "sendto", 45 => "recvfrom",
        56 => "clone", 60 => "exit", 72 => "fcntl", 74 => "fsync", 87 => "unlink",
        202 => "futex", 228 => "clock_gettime", 230 => "clock_nanosleep",
        231 => "exit_group", 232 => "epoll_wait", 233 => "epoll_ctl",
        257 => "openat", 262 => "newfstatat", 318 => "getrandom",
        _ => return format!("syscall_{}", nr),
    }.to_string()
}

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

pub fn parse(ndjson: &str) -> Vec<Record> {
    let mut out = Vec::new();
    for line in ndjson.lines() {
        let line = line.trim();
        if line.is_empty() { continue; }
        let v = match crate::json::parse(line) { Ok(v) => v, Err(_) => continue };
        let t = match v.get("t").and_then(|x| x.as_f64()) { Some(t) => t, None => continue };
        let tid = v.get("tid").and_then(|x| x.as_f64()).unwrap_or(0.0) as i64;
        let dur = v.get("dur").and_then(|x| x.as_f64()).unwrap_or(0.0);
        let name = match v.get("call").and_then(|x| x.as_str()) {
            Some(c) => c.to_string(),
            None => syscall_name(v.get("nr").and_then(|x| x.as_f64()).unwrap_or(-1.0) as i64),
        };
        let comm = v.get("comm").and_then(|x| x.as_str()).map(|c| c.to_string());
        // The descriptor comes from the record's own field where the recorder
        // emitted one. Older captures carry it only inside `where`, as a
        // resolved path or as `fd7 (unresolved)`, so the leading digits are
        // taken and the suffix ignored.
        let fd = v.get("fd").and_then(|x| x.as_f64()).map(|n| n as i64).or_else(|| {
            v.get("where").and_then(|x| x.as_str())
                .and_then(|w| w.strip_prefix("fd"))
                .map(|r| r.chars().take_while(|c| c.is_ascii_digit()).collect::<String>())
                .and_then(|d| d.parse::<i64>().ok())
        });
        out.push(Record { t, dur, tid, name, comm, fd });
    }
    out
}

// ========================================================================
// TYPES
// ========================================================================

pub struct Merge { pub attributed: usize, pub unattributed: usize, pub enriched: usize }

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

pub fn merge(store: &Store, spans: &[Span], recs: &[Record], write: bool) -> (J, Merge) {
    // innermost span on a thread at an instant: among the spans that contain t,
    // the one that started last
    let mut by_tid: BTreeMap<i64, Vec<&Span>> = BTreeMap::new();
    for s in spans {
        let tid = s.attributes.get("tid").and_then(|x| x.as_f64()).unwrap_or(-1.0) as i64;
        by_tid.entry(tid).or_default().push(s);
    }
    for v in by_tid.values_mut() { v.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap()); }

    let mut counts: BTreeMap<String, BTreeMap<String, i64>> = BTreeMap::new();
    let mut times: BTreeMap<String, f64> = BTreeMap::new();
    let mut m = Merge { attributed: 0, unattributed: 0, enriched: 0 };
    let mut unattributed_names: BTreeMap<String, i64> = BTreeMap::new();

    for r in recs {
        let cands = match by_tid.get(&r.tid) { Some(c) => c, None => {
            m.unattributed += 1;
            *unattributed_names.entry(r.name.clone()).or_insert(0) += 1;
            continue; } };
        let mut best: Option<&Span> = None;
        for s in cands {
            if s.start > r.t { break; }
            let e = s.end.unwrap_or(f64::MAX);
            if r.t <= e {
                match best { Some(b) if b.start >= s.start => {}, _ => best = Some(s) }
            }
        }
        match best {
            Some(s) => {
                m.attributed += 1;
                *counts.entry(s.span_id.clone()).or_default().entry(r.name.clone()).or_insert(0) += 1;
                *times.entry(s.span_id.clone()).or_insert(0.0) += r.dur;
            }
            None => {
                m.unattributed += 1;
                *unattributed_names.entry(r.name.clone()).or_insert(0) += 1;
            }
        }
    }

    // enrichment: the semantic span keeps its identity and gains what the
    // syscall stream saw. It never gets redefined by it.
    if write {
        for s in spans {
            if let Some(c) = counts.get(&s.span_id) {
                let mut sp = s.clone();
                let mut sc = J::obj();
                for (k, v) in c { sc.set(k, J::n(*v as f64)); }
                sp.attributes.set("syscalls", sc);
                sp.attributes.set("syscall_count", J::n(c.values().sum::<i64>() as f64));
                if let Some(t) = times.get(&s.span_id) {
                    if *t > 0.0 { sp.attributes.set("syscall_ms", J::n((t * 1000.0 * 100.0).round() / 100.0)); }
                }
                if store.upsert(&sp).is_ok() { m.enriched += 1; }
            }
        }
    }

    let mut top: Vec<(&String, i64)> = Vec::new();
    for (sid, c) in &counts { top.push((sid, c.values().sum())); }
    top.sort_by(|a, b| b.1.cmp(&a.1));
    let name_of = |id: &str| spans.iter().find(|s| s.span_id == id)
        .map(|s| s.name.clone()).unwrap_or_default();

    let mut o = J::obj();
    o.set("records", J::n(recs.len() as f64));
    o.set("attributed", J::n(m.attributed as f64));
    o.set("unattributed", J::n(m.unattributed as f64));
    o.set("spans_enriched", J::n(counts.len() as f64));
    o.set("busiest", J::Arr(top.iter().take(8).map(|(sid, n)| {
        let mut e = J::obj();
        e.set("span", J::s(&name_of(sid)));
        e.set("syscalls", J::n(*n as f64));
        if let Some(c) = counts.get(*sid) {
            let mut b = J::obj();
            for (k, v) in c { b.set(k, J::n(*v as f64)); }
            e.set("breakdown", b);
        }
        if let Some(t) = times.get(*sid) { if *t > 0.0 {
            e.set("syscall_ms", J::n((t * 1000.0 * 100.0).round() / 100.0)); } }
        e
    }).collect()));
    let mut un = J::obj();
    for (k, v) in unattributed_names.iter().take(10) { un.set(k, J::n(*v as f64)); }
    o.set("unattributed_by_call", un);
    (o, m)
}
