// ========================================================================
//  MANIFEST
// ========================================================================
//  script_name: expect.rs
//  script_path: execviz-rs/src/expect.rs
//  module_name: expect
//  version: 0.53.1
//  description: Expected shape, counterfactual, and reading many runs at once.
//  kind: module
//  spec: internal
//  internal_dependencies: find, json, store
//  external_dependencies: std
//  features: expect
//  api_version: execvis-v1.0.0
//  last_updated: 2026-08-07
// ========================================================================

//! Expected shape, counterfactual, and reading many runs at once.
use crate::json::J;
use crate::store::Span;
use std::collections::{BTreeMap, BTreeSet};

// ========================================================================
// EXPECTED SHAPE
// ========================================================================

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

/// Proposes a shape from captures. A person freezes it; the tool never does.
///
/// Learning a shape from one run and enforcing it immediately turns whatever
/// happened to run that day into law. The machine measures, the person decides,
/// because this returns a proposal rather than installing one.
pub fn propose_shape(spans: &[Span]) -> J {
    let mut domains: BTreeSet<&str> = BTreeSet::new();
    let mut names: BTreeSet<&str> = BTreeSet::new();
    for s in spans {
        domains.insert(s.domain.as_deref().unwrap_or("unknown"));
        names.insert(s.name.as_str());
    }
    let mut o = J::obj();
    o.set("note", J::s("a proposal, not a rule: freeze it deliberately, or one day's behaviour becomes law"));
    o.set("from_spans", J::n(spans.len() as f64));
    o.set("domains", J::Arr(domains.iter().map(|d| J::s(d)).collect()));
    o.set("names", J::Arr(names.iter().take(500).map(|n| J::s(n)).collect()));
    o
}

// ========================================================================
// TYPES
// ========================================================================

pub struct Shape { pub domains: BTreeSet<String>, pub names: BTreeSet<String> }

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

/// Parses a frozen shape: `domain X` and `name Y` lines, `#` comments.
pub fn parse_shape(text: &str) -> Shape {
    let (mut domains, mut names) = (BTreeSet::new(), BTreeSet::new());
    for l in text.lines() {
        let l = l.split('#').next().unwrap_or("").trim();
        if l.is_empty() { continue; }
        match l.split_once(char::is_whitespace) {
            Some(("domain", v)) => { domains.insert(v.trim().to_string()); }
            Some(("name", v)) => { names.insert(v.trim().to_string()); }
            _ => {}
        }
    }
    Shape { domains, names }
}

/// Compares a run against the shape, in both directions.
///
/// The absence matters as much as the surprise: a request that quietly stopped
/// touching a service is exactly the change nobody notices, and it shows up
/// here as a domain that was expected and never appeared.
pub fn check_shape(spans: &[Span], shape: &Shape) -> J {
    let seen_d: BTreeSet<String> = spans.iter()
        .map(|s| s.domain.clone().unwrap_or_else(|| "unknown".into())).collect();
    let seen_n: BTreeSet<String> = spans.iter().map(|s| s.name.clone()).collect();

    let missing_d: Vec<&String> = shape.domains.difference(&seen_d).collect();
    let extra_d: Vec<&String> = seen_d.difference(&shape.domains).collect();
    let missing_n: Vec<&String> = shape.names.difference(&seen_n).collect();
    let extra_n: Vec<&String> = seen_n.difference(&shape.names).collect();

    let matches = missing_d.is_empty() && extra_d.is_empty()
        && missing_n.is_empty() && extra_n.is_empty();
    let arr = |v: Vec<&String>| J::Arr(v.iter().take(50).map(|x| J::s(x)).collect());
    let mut o = J::obj();
    o.set("matches", J::Bool(matches));
    o.set("expected_but_absent_domains", arr(missing_d));
    o.set("unexpected_domains", arr(extra_d));
    o.set("expected_but_absent_names", arr(missing_n));
    o.set("unexpected_names", arr(extra_n));
    o.set("note", J::s("absence matters as much as surprise: work that quietly stopped happening is the change nobody notices"));
    o
}

// ========================================================================
// COUNTERFACTUAL
// ========================================================================

/// What the total would be if one span on the critical path were faster.
///
/// It never promises the saving. Shortening a link on the critical path usually
/// promotes a different chain to critical, so the answer is the ceiling; the
/// total would fall to X, at which point Y becomes the constraint; rather than
/// a subtraction that will not happen.
pub fn counterfactual(spans: &[Span], target: &str, factor: f64) -> J {
    let root = spans.iter().filter(|s| s.parent_span_id.is_none())
        .max_by(|a, b| a.duration_ms().unwrap_or(0.0)
            .partial_cmp(&b.duration_ms().unwrap_or(0.0)).unwrap());
    let root = match root { Some(r) => r, None => return J::obj() };

    let path = crate::find::critical_path(spans, &root.span_id);
    let on_path: Vec<&Span> = path.iter().filter(|s| s.name.contains(target)).cloned().collect();

    let span_of = |v: &[&Span]| -> f64 {
        let lo = v.iter().map(|s| s.start).fold(f64::INFINITY, f64::min);
        let hi = v.iter().map(|s| s.end.unwrap_or(s.start)).fold(f64::NEG_INFINITY, f64::max);
        if lo.is_finite() && hi > lo { (hi - lo) * 1000.0 } else { 0.0 }
    };
    let before = span_of(&path);
    let saved: f64 = on_path.iter()
        .map(|s| (s.end.unwrap_or(s.start) - s.start) * 1000.0 * (1.0 - factor)).sum();

    // the next constraint: the longest work NOT on this chain, which is what the
    // total would rest on once this chain stops being the longest
    let path_ids: BTreeSet<&str> = path.iter().map(|s| s.span_id.as_str()).collect();
    let next = spans.iter().filter(|s| !path_ids.contains(s.span_id.as_str()))
        .max_by(|a, b| a.duration_ms().unwrap_or(0.0)
            .partial_cmp(&b.duration_ms().unwrap_or(0.0)).unwrap());

    let mut o = J::obj();
    o.set("target", J::s(target));
    o.set("factor", J::n(factor));
    o.set("on_critical_path", J::n(on_path.len() as f64));
    o.set("total_now_ms", J::n((before * 100.0).round() / 100.0));
    if on_path.is_empty() {
        o.set("verdict", J::s("that work is not on the critical path, so making it faster changes nothing"));
        return o;
    }
    let floor = next.and_then(|s| s.duration_ms()).unwrap_or(0.0);
    let naive = (before - saved).max(0.0);
    let ceiling = naive.max(floor);
    o.set("total_would_fall_to_ms", J::n((ceiling * 100.0).round() / 100.0));
    o.set("saving_ms", J::n(((before - ceiling).max(0.0) * 100.0).round() / 100.0));
    if let Some(n) = next {
        o.set("next_constraint", J::s(&n.name));
        o.set("next_constraint_ms", J::n((floor * 100.0).round() / 100.0));
    }
    o.set("note", J::s("a ceiling, not a subtraction: shortening a link on the critical path usually promotes a different chain to critical"));
    o
}

// ========================================================================
// MANY RUNS
// ========================================================================

/// Reads a set of captures together.
///
/// Every other view assumes one capture, which makes flakiness invisible by
/// construction: a test failing one time in fifty produces forty-nine boring
/// captures and one nobody is looking at.
pub fn across_runs(runs: &[(String, Vec<Span>)]) -> J {
    let n_runs = runs.len().max(1);
    let mut fail_runs: BTreeMap<&str, usize> = BTreeMap::new();
    let mut seen_runs: BTreeMap<&str, usize> = BTreeMap::new();
    let mut durations: BTreeMap<&str, Vec<f64>> = BTreeMap::new();
    let mut failing_runs = 0usize;

    for (_, spans) in runs {
        let mut here: BTreeSet<&str> = BTreeSet::new();
        let mut failed_here: BTreeSet<&str> = BTreeSet::new();
        for s in spans {
            here.insert(s.name.as_str());
            if s.status == "error" { failed_here.insert(s.name.as_str()); }
            if let Some(d) = s.duration_ms() { durations.entry(s.name.as_str()).or_default().push(d); }
        }
        if !failed_here.is_empty() { failing_runs += 1; }
        for n in here { *seen_runs.entry(n).or_insert(0) += 1; }
        for n in failed_here { *fail_runs.entry(n).or_insert(0) += 1; }
    }

    let mut rows: Vec<(&str, usize, usize, f64)> = fail_runs.iter().map(|(name, f)| {
        let seen = *seen_runs.get(name).unwrap_or(&0);
        (*name, *f, seen, *f as f64 / seen.max(1) as f64)
    }).collect();
    rows.sort_by(|a, b| b.3.partial_cmp(&a.3).unwrap());

    let mut o = J::obj();
    // a rate without its denominator is a rumour
    o.set("runs", J::n(n_runs as f64));
    o.set("runs_with_a_failure", J::n(failing_runs as f64));
    o.set("note", J::s("every rate carries the number of runs behind it; a rate stated without its denominator is a rumour"));
    o.set("flaky", J::Arr(rows.iter().take(30).map(|(name, f, seen, rate)| {
        let mut e = J::obj();
        e.set("name", J::s(name));
        e.set("failed_in_runs", J::n(*f as f64));
        e.set("appeared_in_runs", J::n(*seen as f64));
        e.set("failure_rate", J::n((rate * 1000.0).round() / 1000.0));
        e.set("verdict", J::s(if *f == *seen { "fails every time it runs" }
            else if *rate > 0.5 { "fails more often than not" }
            else { "intermittent" }));
        e
    }).collect()));
    o
}
