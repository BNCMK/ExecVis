// ========================================================================
//  MANIFEST
// ========================================================================
//  script_name: egress.rs
//  script_path: execviz-rs/src/egress.rs
//  module_name: egress
//  version: 0.53.1
//  description: Egress, retries and store integrity.
//  kind: module
//  spec: internal
//  internal_dependencies: json, store
//  external_dependencies: std
//  features: egress, store
//  api_version: execvis-v1.0.0
//  last_updated: 2026-08-07
// ========================================================================

//! Egress, retries and store integrity.
use crate::json::J;
use crate::store::{Span, Store};
use std::collections::{BTreeMap, BTreeSet};

// ========================================================================
// EGRESS
// ========================================================================

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

/// Every destination the program reached.
///
/// The capture already holds this: it is the same data the latency views use,
/// read with a different question. Destinations come from what the program
/// recorded; an attribute naming a host, or the name of an external span; and
/// never from guessing.
pub fn destinations(spans: &[Span]) -> BTreeMap<String, usize> {
    let mut out: BTreeMap<String, usize> = BTreeMap::new();
    for s in spans {
        if !matches!(s.kind.as_str(), "external" | "io") { continue; }
        let mut found = None;
        if let J::Obj(m) = &s.attributes {
            for key in ["host", "peer", "url", "address", "endpoint", "target", "server"] {
                if let Some(v) = m.get(key) {
                    if let Some(t) = v.as_str() { found = Some(t.to_string()); break; }
                }
            }
        }
        let dest = found.unwrap_or_else(|| s.name.clone());
        *out.entry(dest).or_insert(0) += 1;
    }
    out
}

/// Compares where the program went against where it was expected to go.
///
/// This is not intrusion detection and must not be presented as such: it reports
/// what happened and a person decides whether it should have. The case it serves
/// well is the dull, valuable one; a dependency that started calling somewhere
/// new after an upgrade.
pub fn egress(spans: &[Span], allowed: &[String]) -> J {
    let seen = destinations(spans);
    let allow: BTreeSet<&str> = allowed.iter().map(|s| s.as_str()).collect();
    let mut unexpected: Vec<(&String, &usize)> = seen.iter()
        .filter(|(d, _)| !allow.iter().any(|a| d.contains(a)))
        .collect();
    unexpected.sort_by(|a, b| b.1.cmp(a.1));
    let unreached: Vec<&String> = allowed.iter()
        .filter(|a| !seen.keys().any(|d| d.contains(a.as_str()))).collect();

    let mut o = J::obj();
    o.set("note", J::s("this reports where the program went; whether it should have is a judgement for a person, and calling this intrusion detection would overstate it"));
    o.set("destinations_seen", J::n(seen.len() as f64));
    o.set("all_expected", J::Bool(unexpected.is_empty()));
    o.set("unexpected", J::Arr(unexpected.iter().take(50).map(|(d, n)| {
        let mut e = J::obj();
        e.set("destination", J::s(d));
        e.set("calls", J::n(**n as f64));
        e
    }).collect()));
    // an expected destination never reached is reported too: a dependency
    // that silently stopped being used is a change nobody notices
    o.set("expected_but_never_reached", J::Arr(unreached.iter().map(|u| J::s(u)).collect()));
    o
}

// ========================================================================
// RETRIES
// ========================================================================

/// Counts declared attempts at one operation.
///
/// The relation is declared by the program, never inferred from names: two
/// traces running the same code are not attempts at the same thing, and guessing
/// they are would invent a causal claim the capture does not support.
pub fn attempts(spans: &[Span]) -> J {
    let mut groups: BTreeMap<String, Vec<&Span>> = BTreeMap::new();
    let mut declared = 0usize;
    for s in spans {
        if s.parent_span_id.is_some() { continue; }          // trace roots only
        let of = match &s.run {
            J::Obj(m) => m.get("retry_of").and_then(|v| v.as_str()).map(|x| x.to_string()),
            _ => None,
        }.or_else(|| match &s.attributes {
            J::Obj(m) => m.get("retry_of").and_then(|v| v.as_str()).map(|x| x.to_string()),
            _ => None,
        });
        match of {
            Some(root) => { declared += 1; groups.entry(root).or_default().push(s); }
            None => { groups.entry(s.trace_id.clone()).or_default().push(s); }
        }
    }
    let mut rows: Vec<(&String, &Vec<&Span>)> = groups.iter().filter(|(_, v)| v.len() > 1).collect();
    rows.sort_by(|a, b| b.1.len().cmp(&a.1.len()));

    let mut o = J::obj();
    o.set("declared_relations", J::n(declared as f64));
    o.set("operations_retried", J::n(rows.len() as f64));
    o.set("note", J::s("attempts are grouped by a relation the program declared; two traces running the same code are not attempts at the same thing"));
    o.set("operations", J::Arr(rows.iter().take(30).map(|(key, v)| {
        let failed_first = v.first().map(|s| s.status == "error").unwrap_or(false);
        let eventually_ok = v.iter().any(|s| s.status == "ok");
        let mut e = J::obj();
        e.set("operation", J::s(key));
        e.set("attempts", J::n(v.len() as f64));
        e.set("first_attempt_failed", J::Bool(failed_first));
        e.set("eventually_succeeded", J::Bool(eventually_ok));
        e
    }).collect()));
    o
}

// ========================================================================
// INTEGRITY
// ========================================================================

/// Is this store internally sound?
///
/// Distinct from sealed: a store can be undamaged and still have been edited on
/// purpose, which is what the seal answers. This answers the other question,
/// whether the file itself is intact and self-consistent.
pub fn integrity(store: &Store, spans: &[Span]) -> J {
    let mut problems: Vec<J> = Vec::new();
    let mut note = |kind: &str, detail: String, count: usize| {
        let mut e = J::obj();
        e.set("problem", J::s(kind));
        e.set("detail", J::s(&detail));
        e.set("count", J::n(count as f64));
        problems.push(e);
    };

    // SQLite's own opinion first: a truncated or half-written file fails here
    let sqlite_ok = store.conn
        .query_row("PRAGMA quick_check", [], |r| r.get::<_, String>(0))
        .map(|v| v == "ok").unwrap_or(false);
    if !sqlite_ok { note("file", "SQLite reports the file is not sound".into(), 1); }

    // duplicate identities: unlikely rather than impossible across a federation,
    // and an unlikely event nobody checks for gets debugged as something else
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut dupes = 0;
    for s in spans { if !seen.insert(s.span_id.as_str()) { dupes += 1; } }
    if dupes > 0 { note("duplicate_span_id", "the same identity appears more than once".into(), dupes); }

    // references that point nowhere
    let ids: BTreeSet<&str> = spans.iter().map(|s| s.span_id.as_str()).collect();
    let dangling = spans.iter().filter(|s| s.parent_span_id.as_deref()
        .map(|p| !ids.contains(p)).unwrap_or(false)).count();
    // a parent on another host is normal in a federated capture, so this is
    // reported as a fact rather than as damage
    let cross_host = spans.iter().filter(|s| s.parent_span_id.as_deref()
        .map(|p| !ids.contains(p)).unwrap_or(false))
        .map(|s| s.host_id.clone()).collect::<BTreeSet<_>>().len();

    let broken = spans.iter().filter(|s| match s.end {
        Some(e) => e < s.start, None => false }).count();
    if broken > 0 { note("negative_duration", "a span ends before it starts".into(), broken); }

    // A capture that is internally consistent can still be incomplete, and a
    // reader must not mistake one for the other.
    // Incompleteness is NOT unsoundness, and conflating them would be a real
    // error: a capture that lost spans in transit is still internally
    // consistent, and `integrity` exits non-zero on unsoundness. Reporting a
    // dropped span as damage would fail a build over a network hiccup.
    let losses = store.losses();
    let lost_total: i64 = losses.iter().map(|(_, n, _, _)| *n).sum();
    let traces_total: i64 = losses.iter().map(|(_, _, t, _)| *t).sum();
    let abnormal_total: i64 = losses.iter().map(|(_, _, _, a)| *a).sum();

    let mut o = J::obj();
    o.set("sound", J::Bool(problems.is_empty()));
    o.set("complete", J::Bool(lost_total == 0));
    if lost_total > 0 {
        o.set("spans_never_delivered", J::n(lost_total as f64));
        o.set("traces_never_delivered", J::n(traces_total as f64));
        if abnormal_total > 0 {
            // losing a trace that held an error or a stuck span is a worse fact
            // than losing an ordinary one, and is reported as such
            o.set("abnormal_traces_lost", J::n(abnormal_total as f64));
        }
        o.set("lost_by_host", J::Arr(losses.iter().map(|(h, n, t, a)| {
            let mut e = J::obj();
            e.set("host", J::s(h));
            e.set("spans", J::n(*n as f64));
            e.set("whole_traces", J::n(*t as f64));
            e.set("abnormal_traces", J::n(*a as f64));
            e
        }).collect()));
        o.set("completeness_note", J::s("counts are lower bounds: a sender dropped whole traces it could not deliver, so what remains is causally complete but not everything that happened"));
    }
    o.set("spans", J::n(spans.len() as f64));
    o.set("sqlite_quick_check", J::Bool(sqlite_ok));
    o.set("parents_not_in_this_store", J::n(dangling as f64));
    o.set("hosts_with_external_parents", J::n(cross_host as f64));
    o.set("problems", J::Arr(problems));
    o.set("note", J::s("soundness is not the same as untampered: a store can be undamaged and still have been edited deliberately, which is what `seal` answers"));
    o
}
