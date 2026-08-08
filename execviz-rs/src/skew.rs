// ========================================================================
//  MANIFEST
// ========================================================================
//  script_name: skew.rs
//  script_path: execviz-rs/src/skew.rs
//  module_name: skew
//  version: 0.53.1
//  description: Clock skew across hosts (spec 5.5, gap 7).
//  kind: module
//  spec: internal
//  internal_dependencies: json, store
//  external_dependencies: std
//  features: skew
//  api_version: execvis-v1.0.0
//  last_updated: 2026-08-07
// ========================================================================

//! Clock skew across hosts (spec 5.5, gap 7).
//!
//! Every host stamps with its own clock. A capture spanning machines can
//! therefore show a child starting before its parent, which inverts causality
//! *visually* while the recorded parentage stays correct.
//!
//! The rule this module follows: **detect, estimate, report; never silently
//! rewrite a timestamp.** A recorded time is what the machine said. Correcting
//! it in place would destroy the evidence that the clocks disagree, which is
//! itself a finding worth surfacing.
use crate::json::J;
use crate::store::Span;
use std::collections::BTreeMap;

// ========================================================================
// TYPES
// ========================================================================

pub struct Pair {
    pub parent_host: String,
    pub child_host: String,
    /// Which clock stamped each side. Two hosts using different clocks disagree
    /// for a different reason than two hosts using the same one, and a reader
    /// deciding whether an offset is drift or a wrong clock needs to know which.
    pub parent_clock: String,
    pub child_clock: String,
    pub crossings: usize,
    pub violations: usize,
    /// Seconds to add to the child host's clock to make causality hold. The
    /// estimate is the worst violation, because a smaller shift would leave
    /// some crossing still impossible.
    pub estimate: f64,
    pub example: String,
}

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

pub fn analyse(spans: &[Span]) -> Vec<Pair> {
    let by_id: BTreeMap<&str, &Span> = spans.iter().map(|s| (s.span_id.as_str(), s)).collect();
    let mut pairs: BTreeMap<(String, String), Pair> = BTreeMap::new();

    for s in spans {
        let p = match &s.parent_span_id { Some(p) => match by_id.get(p.as_str()) {
            Some(p) => *p, None => continue }, None => continue };
        if p.host_id == s.host_id { continue; }               // one clock, no question
        let key = (p.host_id.clone(), s.host_id.clone());
        let e = pairs.entry(key).or_insert(Pair {
            parent_host: p.host_id.clone(), child_host: s.host_id.clone(),
            parent_clock: p.clock_source.clone().unwrap_or_else(|| "unstated".into()),
            child_clock: s.clock_source.clone().unwrap_or_else(|| "unstated".into()),
            crossings: 0, violations: 0, estimate: 0.0, example: String::new() });
        e.crossings += 1;
        // a child cannot begin before the parent that caused it
        let gap = p.start - s.start;
        if gap > 0.0 {
            e.violations += 1;
            if gap > e.estimate {
                e.estimate = gap;
                e.example = format!("{} began {:.1}ms before its parent {}",
                                    s.name, gap * 1000.0, p.name);
            }
        }
    }
    let mut out: Vec<Pair> = pairs.into_values().collect();
    out.sort_by(|a, b| b.estimate.partial_cmp(&a.estimate).unwrap());
    out
}

pub fn to_json(pairs: &[Pair]) -> J {
    let total: usize = pairs.iter().map(|p| p.violations).sum();
    let mut o = J::obj();
    o.set("note", J::s("detected and estimated, never applied: a recorded time is what that machine said, and correcting it in place would destroy the evidence that the clocks disagree"));
    o.set("host_pairs", J::n(pairs.len() as f64));
    o.set("impossible_crossings", J::n(total as f64));
    o.set("clocks_agree", J::Bool(total == 0));
    o.set("pairs", J::Arr(pairs.iter().map(|p| {
        let mut e = J::obj();
        e.set("parent_host", J::s(&p.parent_host));
        e.set("child_host", J::s(&p.child_host));
        e.set("parent_clock", J::s(&p.parent_clock));
        e.set("child_clock", J::s(&p.child_clock));
        if p.parent_clock != p.child_clock && p.parent_clock != "unstated" && p.child_clock != "unstated" {
            e.set("different_clocks", J::Bool(true));
        }
        e.set("crossings", J::n(p.crossings as f64));
        e.set("violations", J::n(p.violations as f64));
        e.set("estimated_offset_ms", J::n((p.estimate * 1000.0 * 100.0).round() / 100.0));
        if !p.example.is_empty() { e.set("example", J::s(&p.example)); }
        e
    }).collect()));
    o
}
