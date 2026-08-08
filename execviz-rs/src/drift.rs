// ========================================================================
//  MANIFEST
// ========================================================================
//  script_name: drift.rs
//  script_path: execviz-rs/src/drift.rs
//  module_name: drift
//  version: 0.53.1
//  description: Drift without instrumentation, and io_uring visibility (4.11j).
//  kind: module
//  spec: internal
//  internal_dependencies: json
//  external_dependencies: std
//  features: drift
//  api_version: execvis-v1.0.0
//  last_updated: 2026-08-07
// ========================================================================

//! Drift without instrumentation, and io_uring visibility (4.11j).
//!
//! `witness` needs spans. On a machine with nothing installed, a narrower form of
//! the same question is available: a process compared against its own past
//! behaviour, from recorder records alone.

use crate::json::J;
use std::collections::BTreeMap;

// ========================================================================
// CONSTANTS
// ========================================================================

/// How far an invariant may move before it is worth reporting. Below this a
/// process is doing the same thing at a different volume.
const MOVED: f64 = 0.15;

// ========================================================================
// INTERNALS
// ========================================================================

fn vectors(identity_json: &str) -> Result<BTreeMap<String, Vec<(String, f64)>>, String> {
    let v = crate::json::parse(identity_json).map_err(|e| format!("not valid JSON: {}", e))?;
    let arr = v.get("identities").and_then(|x| x.as_arr())
        .ok_or_else(|| "no `identities` array; produce one with `execviz identity`".to_string())?;
    let mut out = BTreeMap::new();
    for it in arr {
        let who = it.get("who").and_then(|x| x.as_str()).unwrap_or("").to_string();
        if who.is_empty() { continue; }
        let mut inv = Vec::new();
        if let Some(list) = it.get("invariants").and_then(|x| x.as_arr()) {
            for i in list {
                let n = i.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string();
                let val = i.get("norm").and_then(|x| x.as_f64()).unwrap_or(0.0);
                if !n.is_empty() { inv.push((n, val)); }
            }
        }
        out.insert(who, inv);
    }
    Ok(out)
}

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

pub fn drift(baseline_identity: &str, now_identity: &str) -> Result<(J, i32), String> {
    let b = vectors(baseline_identity)?;
    let a = vectors(now_identity)?;

    let mut drifted: Vec<J> = Vec::new();
    let mut steady: Vec<J> = Vec::new();
    let mut appeared: Vec<J> = Vec::new();
    let mut absent: Vec<J> = Vec::new();

    for (who, now) in &a {
        match b.get(who) {
            None => {
                let mut o = J::obj();
                o.set("who", J::s(who));
                o.set("finding", J::s("no baseline fingerprint for this program"));
                appeared.push(o);
            }
            Some(was) => {
                let mut moved: Vec<J> = Vec::new();
                let mut worst = 0.0f64;
                for (name, nv) in now {
                    if let Some((_, ov)) = was.iter().find(|(n, _)| n == name) {
                        let d = (nv - ov).abs();
                        if d > worst { worst = d; }
                        if d > MOVED {
                            let mut m = J::obj();
                            m.set("invariant", J::s(name));
                            m.set("was", J::n(*ov));
                            m.set("now", J::n(*nv));
                            m.set("moved_by", J::n((d * 10000.0).round() / 10000.0));
                            moved.push(m);
                        }
                    }
                }
                let mut o = J::obj();
                o.set("who", J::s(who));
                o.set("largest_move", J::n((worst * 10000.0).round() / 10000.0));
                if moved.is_empty() {
                    o.set("finding", J::s("behavioural shape unchanged"));
                    steady.push(o);
                } else {
                    o.set("finding", J::s("unexplained_drift"));
                    o.set("moved", J::Arr(moved));
                    o.set("consistent_with", J::s(
                        "a release, a configuration change, a different workload, or a \
                         binary that is not the one measured before"));
                    drifted.push(o);
                }
            }
        }
    }
    for who in b.keys() {
        if !a.contains_key(who) {
            let mut o = J::obj();
            o.set("who", J::s(who));
            o.set("finding", J::s("present in the baseline and not seen in this capture"));
            absent.push(o);
        }
    }

    let n = drifted.len();
    let mut out = J::obj();
    out.set("unexplained_drift", J::Arr(drifted));
    out.set("unchanged", J::Arr(steady));
    out.set("no_baseline", J::Arr(appeared));
    out.set("not_seen", J::Arr(absent));
    out.set("threshold", J::n(MOVED));
    out.set("this_does_not_say", J::s(
        "that a binary was substituted. A shape moves for a release, a configuration \
         change or a different workload as readily as for a substitution, and nothing \
         here separates those."));
    Ok((out, if n > 0 { 1 } else { 0 }))
}

// ========================================================================
// CONSTANTS
// ========================================================================

/// io_uring numbers are the same on x86_64 and arm64.
const IO_URING_SETUP: i64 = 425;

const IO_URING_ENTER: i64 = 426;

const IO_URING_REGISTER: i64 = 427;

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

/// Work submitted through io_uring does not cross the syscall boundary, so the
/// floor does not see it. The submission calls do cross, so the quantity is
/// counted and reported rather than left as a silent gap.
pub fn io_uring(ndjson: &str) -> J {
    let mut per_comm: BTreeMap<String, usize> = BTreeMap::new();
    let mut total = 0usize;
    for line in ndjson.lines() {
        let v = match crate::json::parse(line) { Ok(v) => v, Err(_) => continue };
        let nr = match v.get("nr").and_then(|x| x.as_f64()) { Some(n) => n as i64, None => continue };
        if nr != IO_URING_SETUP && nr != IO_URING_ENTER && nr != IO_URING_REGISTER { continue; }
        total += 1;
        let who = v.get("comm").and_then(|x| x.as_str()).unwrap_or("unknown").to_string();
        *per_comm.entry(who).or_insert(0) += 1;
    }
    let mut out = J::obj();
    out.set("io_uring_calls", J::n(total as f64));
    let mut by: Vec<J> = Vec::new();
    for (who, n) in &per_comm {
        let mut o = J::obj();
        o.set("program", J::s(who));
        o.set("submission_calls", J::n(*n as f64));
        o.set("note", J::s(
            "work submitted through io_uring is not represented in this capture"));
        by.push(o);
    }
    out.set("by_program", J::Arr(by));
    if total == 0 {
        out.set("statement", J::s(
            "no io_uring submission calls were seen; the syscall boundary held for \
             everything in this capture"));
    } else {
        out.set("statement", J::s(
            "these programs submitted work through io_uring, which does not cross the \
             syscall boundary. The quantity is known and the content is not."));
    }
    out
}
