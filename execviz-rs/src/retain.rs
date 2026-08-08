// ========================================================================
//  MANIFEST
// ========================================================================
//  script_name: retain.rs
//  script_path: execviz-rs/src/retain.rs
//  module_name: retain
//  version: 0.53.1
//  description: Retention. What is removed matters more than how much.
//  kind: module
//  spec: internal
//  internal_dependencies: json, store
//  external_dependencies: rusqlite, std
//  features: retain
//  api_version: execvis-v1.0.0
//  last_updated: 2026-08-07
// ========================================================================

//! Retention. What is removed matters more than how much.
use crate::json::J;
use crate::store::{Span, Store};
use rusqlite::params;
use std::collections::BTreeMap;

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

pub fn now_secs() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

// ========================================================================
// TYPES
// ========================================================================

pub struct Plan {
    pub traces_removed: Vec<String>,
    pub spans_removed: usize,
    pub traces_kept_open: Vec<String>,
    pub floor_before: f64,
    pub floor_after: f64,
}

// ========================================================================
// INTERNALS
// ========================================================================

fn floor_of(spans: &[Span]) -> f64 {
    spans.iter().map(|s| s.start).fold(f64::INFINITY, f64::min)
}

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

/// Decides what may go, without removing anything.
///
/// The unit is the trace, because removing a span whose children remain would
/// manufacture the orphan the conformance checker exists to catch. A trace with
/// any open span is retained whatever its age: an old open span is usually the
/// most interesting row in the store, not the least.
pub fn plan(spans: &[Span], older_than: f64, keep_last: usize, now: f64) -> Plan {
    let mut by_trace: BTreeMap<&str, Vec<&Span>> = BTreeMap::new();
    for s in spans { by_trace.entry(s.trace_id.as_str()).or_default().push(s); }

    // newest activity in a trace decides its age; a trace touched a second ago
    // is young even if it started an hour before
    let mut ages: Vec<(&str, f64, bool, usize)> = by_trace.iter().map(|(t, list)| {
        let last = list.iter().map(|s| s.end.unwrap_or(s.start)).fold(f64::NEG_INFINITY, f64::max);
        let has_open = list.iter().any(|s| s.end.is_none());
        (*t, last, has_open, list.len())
    }).collect();
    ages.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());   // newest first

    let mut removed = Vec::new();
    let mut kept_open = Vec::new();
    let mut spans_removed = 0;
    for (i, (trace, last, has_open, count)) in ages.iter().enumerate() {
        let too_old = older_than > 0.0 && (now - last) > older_than;
        let beyond_cap = keep_last > 0 && i >= keep_last;
        if !(too_old || beyond_cap) { continue; }
        if *has_open {
            // an unfinished span outlives the policy, and the policy reports it
            kept_open.push(trace.to_string());
            continue;
        }
        removed.push(trace.to_string());
        spans_removed += count;
    }

    let before = floor_of(spans);
    let survivors: Vec<&Span> = spans.iter()
        .filter(|s| !removed.iter().any(|t| t == &s.trace_id)).collect();
    let after = survivors.iter().map(|s| s.start).fold(f64::INFINITY, f64::min);

    Plan { traces_removed: removed, spans_removed, traces_kept_open: kept_open,
           floor_before: if before.is_finite() { before } else { 0.0 },
           floor_after: if after.is_finite() { after } else { 0.0 } }
}

pub fn apply(store: &Store, plan: &Plan) -> rusqlite::Result<usize> {
    let mut n = 0;
    for t in &plan.traces_removed {
        n += store.conn.execute("DELETE FROM spans WHERE trace_id=?1", params![t])?;
    }
    // The recorder is remembered, not recomputed from what survived: a reader whose
    // position is below it has missed spans that no longer exist, and that has
    // to stay true even after the store is emptied and refilled.
    store.conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS retention (id INTEGER PRIMARY KEY CHECK (id=1), floor REAL NOT NULL);")?;
    store.conn.execute(
        "INSERT INTO retention (id,floor) VALUES (1,?1) \
         ON CONFLICT(id) DO UPDATE SET floor=max(retention.floor, excluded.floor)",
        params![plan.floor_after])?;
    Ok(n)
}

/// The earliest position still guaranteed present. Zero means nothing was ever
/// trimmed, so no reader can have a gap.
pub fn floor(store: &Store) -> f64 {
    store.conn.query_row("SELECT floor FROM retention WHERE id=1", [], |r| r.get(0)).unwrap_or(0.0)
}

pub fn to_json(p: &Plan, applied: bool) -> J {
    let mut o = J::obj();
    o.set("applied", J::Bool(applied));
    o.set("traces_removed", J::n(p.traces_removed.len() as f64));
    o.set("spans_removed", J::n(p.spans_removed as f64));
    o.set("traces_kept_because_open", J::n(p.traces_kept_open.len() as f64));
    if !p.traces_kept_open.is_empty() {
        o.set("kept_open", J::Arr(p.traces_kept_open.iter().take(8).map(|t| J::s(t)).collect()));
        o.set("note", J::s("a trace with an open span is retained whatever its age: an unfinished span is the finding"));
    }
    o.set("floor_before", J::n(p.floor_before));
    o.set("floor_after", J::n(p.floor_after));
    o
}
