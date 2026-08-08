// ========================================================================
//  MANIFEST
// ========================================================================
//  script_name: shapes_rules.rs
//  script_path: execviz-rs/src/shapes_rules.rs
//  module_name: shapes_rules
//  version: 0.53.1
//  description: Detection on shape, not on values.
//  kind: module
//  spec: internal
//  internal_dependencies: finger, json, store, syscalls, witness
//  external_dependencies: std
//  features: shapes rules, detect
//  api_version: execvis-v1.0.0
//  last_updated: 2026-08-07
// ========================================================================

//! Detection on shape, not on values.
//!
//! A rules engine over a syscall stream fires when a value matches a pattern.
//! That is right for a stream and wrong for a map, because the map holds what a
//! stream does not: causality, identity, and coverage.
//!
//! Every predicate here is a relation between things rather than a property of
//! one thing, which is the whole argument for holding a map. None of them could
//! be written against a syscall.
//!
//! A rule that fires states what it saw. A detection no reader can check against
//! the evidence is an alarm, and this project has no use for alarms.

use crate::json::J;
use crate::store::Span;
use crate::syscalls::Record;
use std::collections::{BTreeMap, BTreeSet};

// ========================================================================
// TYPES
// ========================================================================

pub struct Rule {
    pub name: String,
    pub threshold: f64,
}

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

/// Reads a rules file. One rule per line: `<predicate> [threshold]`.
///
/// An unknown predicate is a failure, not a pass: a rules file with a typo that
/// silently matches nothing differs from no rules file, because it looks like
/// a system with no problems.
pub fn parse_rules(text: &str) -> (Vec<Rule>, Vec<String>) {
    let known = ["stuck", "orphaned", "inverted", "drifted", "unwitnessed", "dark"];
    let mut rules = Vec::new();
    let mut unknown = Vec::new();
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() { continue }
        let mut parts = line.split_whitespace();
        let name = parts.next().unwrap_or("").to_string();
        let threshold = parts.next().and_then(|t| t.parse().ok()).unwrap_or(0.0);
        if !known.contains(&name.as_str()) {
            unknown.push(name);
            continue;
        }
        rules.push(Rule { name, threshold });
    }
    (rules, unknown)
}

// ========================================================================
// INTERNALS
// ========================================================================

fn finding(rule: &str, subject: &str, saw: J, why: &str) -> J {
    J::Obj([
        ("rule".to_string(), J::Str(rule.to_string())),
        ("subject".to_string(), J::Str(subject.to_string())),
        ("saw".to_string(), saw),
        ("why".to_string(), J::Str(why.to_string())),
    ].into_iter().collect())
}

// ========================================================================
// TYPES
// ========================================================================

pub struct Outcome { pub fired: usize, pub unknown: usize }

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

pub fn detect(rules: &[Rule], unknown: &[String], spans: &[Span], recs: &[Record],
              baseline: Option<&[crate::finger::Invariant]>) -> (J, Outcome) {
    let mut findings: Vec<J> = Vec::new();
    let ids: BTreeSet<&str> = spans.iter().map(|s| s.span_id.as_str()).collect();
    let by_id: BTreeMap<&str, &Span> = spans.iter().map(|s| (s.span_id.as_str(), s)).collect();

    for r in rules {
        match r.name.as_str() {
            // A span that opened and never closed, while the capture went on
            // past it. The threshold is how long past its start the capture ran
            // before it was still open.
            "stuck" => {
                let latest = spans.iter().filter_map(|s| s.end).fold(f64::NEG_INFINITY, f64::max);
                for s in spans.iter().filter(|s| s.end.is_none()) {
                    let open_for = latest - s.start;
                    if open_for >= r.threshold {
                        findings.push(finding("stuck", &s.name, J::Obj([
                            ("span_id".to_string(), J::Str(s.span_id.clone())),
                            ("open_for_secs".to_string(), J::Num((open_for * 1000.0).round() / 1000.0)),
                        ].into_iter().collect()),
                        "opened and never closed while the capture continued past it"));
                    }
                }
            }
            // A parent that is not in the capture. The trace's graph has a hole
            // and every ancestor question below it answers wrongly.
            "orphaned" => {
                for s in spans {
                    if let Some(p) = &s.parent_span_id {
                        if !ids.contains(p.as_str()) {
                            findings.push(finding("orphaned", &s.name, J::Obj([
                                ("span_id".to_string(), J::Str(s.span_id.clone())),
                                ("missing_parent".to_string(), J::Str(p.clone())),
                            ].into_iter().collect()),
                            "the declared parent is not in this capture, so the graph has a hole under it"));
                        }
                    }
                }
            }
            // A child that outlived the parent it was joined into: the parent
            // reported completion while work it owned was still running.
            "inverted" => {
                for s in spans {
                    let (Some(p), Some(cend)) = (&s.parent_span_id, s.end) else { continue };
                    let Some(parent) = by_id.get(p.as_str()) else { continue };
                    let Some(pend) = parent.end else { continue };
                    if cend > pend + 1e-6 {
                        findings.push(finding("inverted", &s.name, J::Obj([
                            ("span_id".to_string(), J::Str(s.span_id.clone())),
                            ("child_ended".to_string(), J::Num(cend)),
                            ("parent_ended".to_string(), J::Num(pend)),
                            ("overhang_secs".to_string(), J::Num(((cend - pend) * 1e6).round() / 1e6)),
                        ].into_iter().collect()),
                        "the parent reported completion while work it owned was still running"));
                    }
                }
            }
            // A shape that moved from a named baseline. Distance is reported so
            // a reader can judge; the threshold is the caller's, not the tool's.
            "drifted" => {
                let Some(base) = baseline else {
                    findings.push(finding("drifted", "(no baseline)", J::Null,
                        "this rule needs --baseline; without one there is nothing to have drifted from"));
                    continue;
                };
                let now = crate::finger::invariants(spans);
                let d = crate::finger::distance(base, &now);
                if d >= r.threshold {
                    findings.push(finding("drifted", "capture", J::Obj([
                        ("distance".to_string(), J::Num((d * 1000.0).round() / 1000.0)),
                        ("threshold".to_string(), J::Num(r.threshold)),
                    ].into_iter().collect()),
                    "the execution shape moved from the baseline by more than the stated threshold"));
                }
            }
            // A span claiming work the recorder did not observe. Only meaningful
            // with records; without them it reports it rather than passing.
            "unwitnessed" => {
                if recs.is_empty() {
                    findings.push(finding("unwitnessed", "(no records)", J::Null,
                        "this rule needs --records from the recorder; without them nothing can be witnessed"));
                    continue;
                }
                let (_, a) = crate::witness::audit(spans, recs);
                if a.claimed_not_performed as f64 > r.threshold {
                    findings.push(finding("unwitnessed", "capture", J::Obj([
                        ("claimed_not_performed".to_string(), J::Num(a.claimed_not_performed as f64)),
                        ("spans_examined".to_string(), J::Num(a.examined as f64)),
                    ].into_iter().collect()),
                    "spans claimed work the recorder did not observe on their threads"));
                }
            }
            // A host doing more than its instrumentation describes. Not a
            // defect: a coverage fact, with a threshold the operator chose.
            "dark" => {
                if recs.is_empty() {
                    findings.push(finding("dark", "(no records)", J::Null,
                        "this rule needs --records from the recorder"));
                    continue;
                }
                let mut comms: BTreeMap<i64, String> = BTreeMap::new();
                for rec in recs {
                    if let Some(c) = &rec.comm { comms.entry(rec.tid).or_insert_with(|| c.clone()); }
                }
                let ns = crate::witness::negative_space(spans, recs, &comms);
                let covered = ns.get("covered_fraction").and_then(|x| x.as_f64()).unwrap_or(1.0);
                let dark = 1.0 - covered;
                if dark >= r.threshold {
                    findings.push(finding("dark", "host", J::Obj([
                        ("unclaimed_fraction".to_string(), J::Num((dark * 1000.0).round() / 1000.0)),
                        ("threshold".to_string(), J::Num(r.threshold)),
                    ].into_iter().collect()),
                    "more of this machine's work is outside the instrumentation than the stated threshold allows"));
                }
            }
            _ => {}
        }
    }

    let unknown_j: Vec<J> = unknown.iter().map(|u| J::Obj([
        ("rule".to_string(), J::Str(u.clone())),
        ("problem".to_string(), J::Str(
            "not a predicate this understands; a rules file that silently matches \
             nothing looks exactly like a system with no problems".to_string())),
    ].into_iter().collect())).collect();

    let n = findings.len();
    let out = J::Obj([
        ("fired".to_string(), J::Num(n as f64)),
        ("rules_read".to_string(), J::Num(rules.len() as f64)),
        ("findings".to_string(), J::Arr(findings)),
        ("unknown_rules".to_string(), J::Arr(unknown_j)),
        ("note".to_string(), J::Str(
            "Each predicate compares two or more spans. None can be evaluated from a \
             syscall stream alone.".to_string())),
    ].into_iter().collect());
    (out, Outcome { fired: n, unknown: unknown.len() })
}
