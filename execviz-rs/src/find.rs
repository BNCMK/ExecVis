// ========================================================================
//  MANIFEST
// ========================================================================
//  script_name: find.rs
//  script_path: execviz-rs/src/find.rs
//  module_name: find
//  version: 0.53.1
//  description: Search, self time and the critical path (spec 5.5, gaps 1, 2, 4, 5).
//  kind: module
//  spec: internal
//  internal_dependencies: json, store
//  external_dependencies: std
//  features: find
//  api_version: execvis-v1.0.0
//  last_updated: 2026-08-07
// ========================================================================

//! Search, self time and the critical path (spec 5.5, gaps 1, 2, 4, 5).
use crate::json::J;
use crate::store::Span;
use std::collections::BTreeMap;

// ========================================================================
// INTERNALS
// ========================================================================

fn hay(s: &Span) -> String {
    format!("{} {} {} {} {}", s.name, s.kind, s.status,
            s.domain.clone().unwrap_or_default(), s.host_id).to_lowercase()
}

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

/// Free-text search over the things a person remembers: the name, the
/// kind, the status, the domain, the host, and the attributes the program
/// attached. Attributes matter most; a capture that recorded a user id should
/// be answerable by that id, which until now it was not.
pub fn search<'a>(spans: &'a [Span], q: &str, limit: usize) -> Vec<&'a Span> {
    let needle = q.trim().to_lowercase();
    if needle.is_empty() { return Vec::new(); }
    // key=value searches an attribute exactly; bare text searches everything
    let kv: Option<(String, String)> = needle.split_once('=')
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()));
    let mut out: Vec<&Span> = spans.iter().filter(|s| {
        if let Some((k, v)) = &kv {
            if let Some(av) = s.attributes.get(k.as_str()) {
                return av.dump().to_lowercase().trim_matches('"').contains(v.as_str());
            }
            return false;
        }
        if hay(s).contains(&needle) { return true; }
        // recorded values and failures are searchable too: "which call had this
        // input" and "which span raised this error" are the same question a
        // person arrives with, and the answer now lives in these fields
        for field in [&s.inputs, &s.output, &s.error] {
            if !matches!(field, crate::json::J::Null)
                && field.dump().to_lowercase().contains(&needle) { return true; }
        }
        // attributes are a JSON object; searching their rendering covers both
        // the keys a program chose and the values it recorded
        match &s.attributes {
            crate::json::J::Obj(m) => m.iter().any(|(k, v)| {
                k.to_lowercase().contains(&needle) || v.dump().to_lowercase().contains(&needle)
            }),
            _ => false,
        }
    }).collect();
    // most recent first: a person searching a live capture means "now"
    out.sort_by(|a, b| b.start.partial_cmp(&a.start).unwrap());
    out.truncate(limit);
    out
}

/// Self time: total minus the time covered by children.
///
/// `slowest` credits a parent for everything its children did, which answers a
/// different question than the one being asked. Children are merged before
/// subtracting, so concurrent children are not double-counted; subtracting the
/// sum of overlapping intervals can drive a real duration negative and has.
pub fn self_ms(spans: &[Span]) -> BTreeMap<&str, f64> {
    let mut kids: BTreeMap<&str, Vec<(f64, f64)>> = BTreeMap::new();
    for s in spans {
        if let (Some(p), Some(e)) = (&s.parent_span_id, s.end) {
            kids.entry(p.as_str()).or_default().push((s.start, e));
        }
    }
    let mut out = BTreeMap::new();
    for s in spans {
        let total = match s.duration_ms() { Some(d) => d, None => continue };
        let covered = match kids.get(s.span_id.as_str()) {
            None => 0.0,
            Some(iv) => {
                let mut v = iv.clone();
                v.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
                let (mut acc, mut cur) = (0.0, v[0]);
                for w in v.iter().skip(1) {
                    if w.0 <= cur.1 { if w.1 > cur.1 { cur.1 = w.1; } }
                    else { acc += cur.1 - cur.0; cur = *w; }
                }
                acc += cur.1 - cur.0;
                acc * 1000.0
            }
        };
        out.insert(s.span_id.as_str(), (total - covered).max(0.0));
    }
    out
}

/// The critical path: the chain of spans that set the total.
///
/// In concurrent work the total is decided by one chain, not by the sum, so a
/// list of the slowest spans includes work that cost nothing because it
/// overlapped something longer. This walks down from a root, at each step taking
/// the child that finishes last, which is the child the parent was waiting for.
pub fn critical_path<'a>(spans: &'a [Span], root: &str) -> Vec<&'a Span> {
    let by_id: BTreeMap<&str, &Span> = spans.iter().map(|s| (s.span_id.as_str(), s)).collect();
    let mut children: BTreeMap<&str, Vec<&Span>> = BTreeMap::new();
    for s in spans {
        if let Some(p) = &s.parent_span_id { children.entry(p.as_str()).or_default().push(s); }
    }
    let mut path = Vec::new();
    let mut cur = match by_id.get(root) { Some(s) => *s, None => return path };
    let _ = &by_id;
    path.push(cur);
    loop {
        let kids = match children.get(cur.span_id.as_str()) { Some(k) if !k.is_empty() => k, _ => break };
        let next = kids.iter().max_by(|a, b| {
            let ae = a.end.unwrap_or(a.start); let be = b.end.unwrap_or(b.start);
            ae.partial_cmp(&be).unwrap()
        });
        match next { Some(n) => { cur = n; path.push(cur); } None => break }
        if path.len() > 512 { break; }
    }
    path
}

pub fn search_json(hits: &[&Span], q: &str, total: usize) -> J {
    let mut o = J::obj();
    o.set("query", J::s(q));
    o.set("searched", J::n(total as f64));
    o.set("hits", J::n(hits.len() as f64));
    o.set("spans", J::Arr(hits.iter().map(|s| {
        let mut e = J::obj();
        e.set("span_id", J::s(&s.span_id));
        e.set("name", J::s(&s.name));
        e.set("kind", J::s(&s.kind));
        e.set("status", J::s(&s.status));
        e.set("host", J::s(&s.host_id));
        e.set("domain", J::s(&s.domain.clone().unwrap_or_default()));
        e.set("duration_ms", match s.duration_ms() { Some(d) => J::n(d), None => J::Null });
        e
    }).collect()));
    o
}

pub fn self_json(spans: &[Span], limit: usize) -> J {
    let selfs = self_ms(spans);
    let mut rows: Vec<(&Span, f64)> = spans.iter()
        .filter_map(|s| selfs.get(s.span_id.as_str()).map(|v| (s, *v))).collect();
    rows.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
    rows.truncate(limit);
    let mut o = J::obj();
    o.set("note", J::s("self time is total minus the time covered by children, merged so concurrent children are not double-counted"));
    o.set("spans", J::Arr(rows.iter().map(|(s, v)| {
        let mut e = J::obj();
        e.set("name", J::s(&s.name));
        e.set("span_id", J::s(&s.span_id));
        e.set("self_ms", J::n((v * 100.0).round() / 100.0));
        e.set("total_ms", J::n(s.duration_ms().unwrap_or(0.0)));
        e.set("domain", J::s(&s.domain.clone().unwrap_or_default()));
        e
    }).collect()));
    o
}

pub fn path_json(path: &[&Span]) -> J {
    // The span of the path itself, not the root's own duration. A child can
    // outlive its parent when work is handed off asynchronously, and reporting
    // the root's duration there would claim a total smaller than something
    // inside it; which is the number a reader would not question.
    let first = path.iter().map(|s| s.start).fold(f64::INFINITY, f64::min);
    let last = path.iter().map(|s| s.end.unwrap_or(s.start)).fold(f64::NEG_INFINITY, f64::max);
    let handoff = path.windows(2).any(|w| {
        match (w[0].end, w[1].end) { (Some(pe), Some(ce)) => ce > pe + 1e-9, _ => false }
    });
    let mut o = J::obj();
    o.set("length", J::n(path.len() as f64));
    o.set("total_ms", J::n(if first.is_finite() && last > first {
        ((last - first) * 1000.0 * 100.0).round() / 100.0 } else { 0.0 }));
    o.set("contains_async_handoff", J::Bool(handoff));
    if handoff {
        o.set("note", J::s("a child outlives its parent here, so this chain is a causal path rather than a chain of waiting; the parent did not block on what follows"));
    }
    o.set("path", J::Arr(path.iter().map(|s| {
        let mut e = J::obj();
        e.set("name", J::s(&s.name));
        e.set("span_id", J::s(&s.span_id));
        e.set("kind", J::s(&s.kind));
        e.set("duration_ms", match s.duration_ms() { Some(d) => J::n(d), None => J::Null });
        e
    }).collect()));
    o
}
