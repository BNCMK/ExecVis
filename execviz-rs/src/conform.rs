// ========================================================================
//  MANIFEST
// ========================================================================
//  script_name: conform.rs
//  script_path: execviz-rs/src/conform.rs
//  module_name: conform
//  version: 0.53.1
//  description: Conformance checks. Every failure mode in the adapter contract produces a tree that still looks plausible, so the checks are structural and run against a recorded capture rather than against the adapter’s claims.
//  kind: module
//  spec: internal
//  internal_dependencies: json, store
//  external_dependencies: std
//  features: conform, capture, adapter
//  api_version: execvis-v1.0.0
//  last_updated: 2026-08-07
// ========================================================================

//! Conformance checks. Every failure mode in the adapter contract
//! produces a tree that still looks plausible, so the checks are structural and
//! run against a recorded capture rather than against the adapter's claims.
use crate::json::J;
use crate::store::Span;
use std::collections::{BTreeMap, BTreeSet};

// ========================================================================
// CONSTANTS
// ========================================================================

const ONTOLOGY: [&str; 9] = ["call", "branch", "loop", "io", "wait", "queue",
                             "spawn", "error", "external"];

const NON_DERIVABLE: [&str; 6] = ["suspended", "resumed", "claimed", "released",
                                  "cancelled", "migrated"];

const SKEW_TOLERANCE_S: f64 = 0.010;

// ========================================================================
// TYPES
// ========================================================================

struct Finding { host: String, rule: &'static str, detail: String }

// Violations mean the adapter is wrong. Observations mean the program did
// something worth seeing. Conflating them makes a checker that cries wolf.

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

pub fn check(spans: &[Span]) -> J {
    let byid: BTreeMap<&str, &Span> = spans.iter().map(|s| (s.span_id.as_str(), s)).collect();
    let mut f: Vec<Finding> = Vec::new();
    let mut o: Vec<Finding> = Vec::new();
    macro_rules! note { ($h:expr, $r:expr, $d:expr) => {
        f.push(Finding { host: $h.to_string(), rule: $r, detail: $d }) } }
    macro_rules! obs { ($h:expr, $r:expr, $d:expr) => {
        o.push(Finding { host: $h.to_string(), rule: $r, detail: $d }) } }

    for s in spans {
        // schema and ontology
        if s.name.is_empty() { note!(&s.host_id, "schema", format!("{} has empty name", s.span_id)); }
        if !ONTOLOGY.contains(&s.kind.as_str()) {
            note!(&s.host_id, "schema", format!("{} kind '{}' is outside the ontology", s.name, s.kind));
        }
        // two-phase honesty
        match (s.end.is_some(), s.status.as_str()) {
            (true, "running") => note!(&s.host_id, "two_phase",
                format!("{} has an end but reports running", s.name)),
            (false, st) if st != "running" => note!(&s.host_id, "two_phase",
                format!("{} has no end but reports {}", s.name, st)),
            _ => {}
        }
        // parent integrity and causal time
        if let Some(p) = &s.parent_span_id {
            match byid.get(p.as_str()) {
                None => note!(&s.host_id, "parent_integrity",
                    format!("{} points at missing parent {}", s.name, p)),
                Some(par) => {
                    if s.start + SKEW_TOLERANCE_S < par.start {
                        note!(&s.host_id, "causal_time",
                            format!("{} starts before its parent {}", s.name, par.name));
                    }
                    if let (Some(ce), Some(pe)) = (s.end, par.end) {
                        if ce > pe + SKEW_TOLERANCE_S {
                            // Not an adapter defect: a parent that aborted or was
                            // cancelled leaves real work still running behind it.
                            // Reported as an observation about the program.
                            obs!(&s.host_id, "orphaned_work",
                                format!("{} outlived its parent {}", s.name, par.name));
                        }
                    }
                }
            }
        }
        // link integrity
        for l in &s.links {
            if l == &s.span_id {
                note!(&s.host_id, "link_integrity", format!("{} links to itself", s.name));
            }
            if !byid.contains_key(l.as_str()) {
                note!(&s.host_id, "link_integrity",
                    format!("{} links to missing span {}", s.name, l));
            }
            if Some(l) == s.parent_span_id.as_ref() {
                note!(&s.host_id, "link_integrity",
                    format!("{} duplicates its parent in links", s.name));
            }
        }
        // derivability: only transitions a timestamp cannot express
        if let J::Arr(lc) = &s.lifecycle {
            for e in lc {
                if let Some(t) = e.get("type").and_then(|x| x.as_str()) {
                    if !NON_DERIVABLE.contains(&t) {
                        note!(&s.host_id, "derivability",
                            format!("{} records '{}' as lifecycle; it is derivable from timestamps", s.name, t));
                    }
                }
            }
        }
        // self-tracing
        // Adapter machinery only. A traced program's own flush or write is io
        // and must never be mistaken for the adapter tracing itself.
        let n = s.name.as_str();
        let machinery = matches!(n, "span_start" | "span_end" | "span_lifecycle"
            | "_sid" | "_ctx" | "_guarded" | "_profiler" | "upsert")
            || n.starts_with("execviz");
        if machinery && s.kind != "io" {
            note!(&s.host_id, "self_tracing", format!("{} is adapter machinery", s.name));
        }
    }

    // cycles in the causal graph
    let mut colour: BTreeMap<&str, u8> = BTreeMap::new();
    for s in spans {
        let mut path: Vec<&str> = Vec::new();
        let mut cur = Some(s.span_id.as_str());
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        while let Some(id) = cur {
            if colour.get(id) == Some(&2) { break; }
            if !seen.insert(id) {
                let host = byid.get(id).map(|x| x.host_id.clone()).unwrap_or_default();
                f.push(Finding { host, rule: "no_cycles",
                    detail: format!("cycle through {}", path.join(" -> ")) });
                break;
            }
            path.push(id);
            cur = byid.get(id).and_then(|x| x.parent_span_id.as_deref());
        }
        for id in path { colour.insert(id, 2); }
    }

    // report per host: a capture may carry several adapters at once
    let mut hosts: BTreeMap<String, Vec<&Finding>> = BTreeMap::new();
    for s in spans { hosts.entry(s.host_id.clone()).or_default(); }
    for x in &f { hosts.entry(x.host.clone()).or_default().push(x); }

    let mut out = J::obj();
    let total = f.len();
    out.set("conformant", J::Bool(total == 0));
    out.set("findings", J::n(total as f64));
    let mut obs_by_host: BTreeMap<String, Vec<&Finding>> = BTreeMap::new();
    for x in &o { obs_by_host.entry(x.host.clone()).or_default().push(x); }
    out.set("observations", J::n(o.len() as f64));
    out.set("hosts", J::Arr(hosts.into_iter().map(|(h, items)| {
        let mut rules: BTreeMap<&str, Vec<&Finding>> = BTreeMap::new();
        for i in &items { rules.entry(i.rule).or_default().push(i); }
        let mut o = J::obj();
        o.set("host", J::s(&h));
        o.set("spans", J::n(spans.iter().filter(|s| s.host_id == h).count() as f64));
        o.set("conformant", J::Bool(items.is_empty()));
        let empty: Vec<&Finding> = Vec::new();
        let obs_items = obs_by_host.get(&h).unwrap_or(&empty);
        let mut obs_rules: BTreeMap<&str, Vec<&&Finding>> = BTreeMap::new();
        for i in obs_items { obs_rules.entry(i.rule).or_default().push(i); }
        o.set("observations", J::Arr(obs_rules.into_iter().map(|(rule, list)| {
            let mut r = J::obj();
            r.set("rule", J::s(rule));
            r.set("count", J::n(list.len() as f64));
            r.set("examples", J::Arr(list.iter().take(3).map(|x| J::s(&x.detail)).collect()));
            r
        }).collect()));
        o.set("violations", J::Arr(rules.into_iter().map(|(rule, list)| {
            let mut r = J::obj();
            r.set("rule", J::s(rule));
            r.set("count", J::n(list.len() as f64));
            r.set("examples", J::Arr(list.iter().take(4)
                .map(|x| J::s(&x.detail)).collect()));
            r
        }).collect()));
        o
    }).collect()));
    out
}
