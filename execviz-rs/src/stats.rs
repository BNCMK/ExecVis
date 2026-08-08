// ========================================================================
//  MANIFEST
// ========================================================================
//  script_name: stats.rs
//  script_path: execviz-rs/src/stats.rs
//  module_name: stats
//  version: 0.53.1
//  description: Distributions, assertions and coverage.
//  kind: module
//  spec: internal
//  internal_dependencies: json, store
//  external_dependencies: std
//  features: stats
//  api_version: execvis-v1.0.0
//  last_updated: 2026-08-07
// ========================================================================

//! Distributions, assertions and coverage.
use crate::json::J;
use crate::store::Span;
use std::collections::BTreeMap;

// ========================================================================
// INTERNALS
// ========================================================================

/// A percentile taken from the values themselves.
///
/// Percentiles are not monoids and must never be folded into a
/// rollup as though they were: an average of percentiles is not a percentile.
fn pct(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() { return 0.0; }
    let idx = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

// ========================================================================
// TYPES
// ========================================================================

pub struct Dist {
    pub name: String,
    pub count: usize,
    pub errors: usize,
    pub median: f64,
    pub p90: f64,
    pub p95: f64,
    pub p99: f64,
    pub max: f64,
}

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

pub fn distributions(spans: &[Span], min_count: usize) -> Vec<Dist> {
    let mut by_name: BTreeMap<&str, (Vec<f64>, usize)> = BTreeMap::new();
    for s in spans {
        let e = by_name.entry(s.name.as_str()).or_insert_with(|| (Vec::new(), 0));
        if let Some(d) = s.duration_ms() { e.0.push(d); }
        // a fast failure is not a success, so errors are counted beside the shape
        if s.status == "error" { e.1 += 1; }
    }
    let mut out: Vec<Dist> = by_name.into_iter().filter(|(_, v)| v.0.len() >= min_count)
        .map(|(name, (mut v, errors))| {
            v.sort_by(|a, b| a.partial_cmp(b).unwrap());
            Dist { name: name.to_string(), count: v.len(), errors,
                   median: pct(&v, 0.50), p90: pct(&v, 0.90), p95: pct(&v, 0.95),
                   p99: pct(&v, 0.99), max: *v.last().unwrap_or(&0.0) }
        }).collect();
    out.sort_by(|a, b| b.p95.partial_cmp(&a.p95).unwrap());
    out
}

pub fn dist_json(d: &[Dist]) -> J {
    let r2 = |v: f64| (v * 100.0).round() / 100.0;
    let mut o = J::obj();
    o.set("note", J::s("a percentile is reported with its sample size: a p99 over eleven samples is the maximum wearing a costume"));
    o.set("spans", J::Arr(d.iter().map(|x| {
        let mut e = J::obj();
        e.set("name", J::s(&x.name));
        e.set("count", J::n(x.count as f64));
        e.set("errors", J::n(x.errors as f64));
        e.set("error_rate", J::n(((x.errors as f64 / x.count.max(1) as f64) * 1000.0).round() / 1000.0));
        e.set("median_ms", J::n(r2(x.median)));
        e.set("p90_ms", J::n(r2(x.p90)));
        e.set("p95_ms", J::n(r2(x.p95)));
        e.set("p99_ms", J::n(r2(x.p99)));
        e.set("max_ms", J::n(r2(x.max)));
        // the reader is told when a percentile rests on too little
        e.set("percentiles_meaningful", J::Bool(x.count >= 20));
        e
    }).collect()));
    o
}

// ========================================================================
// ASSERTIONS
// ========================================================================

// ========================================================================
// TYPES
// ========================================================================

#[derive(Debug)]
pub struct Rule { pub kind: String, pub arg: String, pub limit: f64 }

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

/// Parses an assertion file: one rule per line, `#` starts a comment.
///
///   no_orphans
///   max_duration_ms  checkout  250
///   no_errors_in     billing
///   max_error_rate   charge    0.01
///   must_run         reconcile
pub fn parse_rules(text: &str) -> Vec<Rule> {
    text.lines().filter_map(|l| {
        let l = l.split('#').next().unwrap_or("").trim();
        if l.is_empty() { return None; }
        let mut it = l.split_whitespace();
        let kind = it.next()?.to_string();
        let arg = it.next().unwrap_or("").to_string();
        let limit = it.next().and_then(|v| v.parse().ok()).unwrap_or(0.0);
        Some(Rule { kind, arg, limit })
    }).collect()
}

// ========================================================================
// TYPES
// ========================================================================

pub struct Failure { pub rule: String, pub detail: String, pub examples: Vec<String> }

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

/// Checks a capture against what the project says about itself.
///
/// A failing assertion names the spans that broke it: a red light without a
/// location is an alarm, not a finding.
pub fn assert_all(spans: &[Span], rules: &[Rule]) -> Vec<Failure> {
    let ids: std::collections::BTreeSet<&str> = spans.iter().map(|s| s.span_id.as_str()).collect();
    let mut fails = Vec::new();
    for r in rules {
        match r.kind.as_str() {
            "no_orphans" => {
                let bad: Vec<&Span> = spans.iter().filter(|s| {
                    match (&s.parent_span_id, s.end) {
                        (Some(p), Some(e)) => ids.contains(p.as_str())
                            && spans.iter().any(|q| &q.span_id == p
                                && q.end.map(|pe| e > pe + 1e-9).unwrap_or(false)),
                        _ => false,
                    }
                }).collect();
                if !bad.is_empty() {
                    fails.push(Failure { rule: "no_orphans".into(),
                        detail: format!("{} spans outlive their parent", bad.len()),
                        examples: bad.iter().take(5).map(|s| s.name.clone()).collect() });
                }
            }
            "max_duration_ms" => {
                let bad: Vec<&Span> = spans.iter()
                    .filter(|s| (r.arg.is_empty() || s.name.contains(&r.arg))
                        && s.duration_ms().map(|d| d > r.limit).unwrap_or(false))
                    .collect();
                if !bad.is_empty() {
                    fails.push(Failure { rule: format!("max_duration_ms {} {}", r.arg, r.limit),
                        detail: format!("{} spans exceeded the limit", bad.len()),
                        examples: bad.iter().take(5)
                            .map(|s| format!("{} at {:.1}ms", s.name, s.duration_ms().unwrap_or(0.0))).collect() });
                }
            }
            "no_errors_in" => {
                let bad: Vec<&Span> = spans.iter().filter(|s| s.status == "error"
                    && s.domain.as_deref().map(|d| d == r.arg).unwrap_or(false)).collect();
                if !bad.is_empty() {
                    fails.push(Failure { rule: format!("no_errors_in {}", r.arg),
                        detail: format!("{} errors in that domain", bad.len()),
                        examples: bad.iter().take(5).map(|s| s.name.clone()).collect() });
                }
            }
            "max_error_rate" => {
                let matching: Vec<&Span> = spans.iter().filter(|s| s.name.contains(&r.arg)).collect();
                let errs = matching.iter().filter(|s| s.status == "error").count();
                let rate = if matching.is_empty() { 0.0 } else { errs as f64 / matching.len() as f64 };
                if rate > r.limit {
                    fails.push(Failure { rule: format!("max_error_rate {} {}", r.arg, r.limit),
                        detail: format!("{:.1}% of {} spans failed", rate * 100.0, matching.len()),
                        examples: vec![] });
                }
            }
            "must_run" => {
                if !spans.iter().any(|s| s.name.contains(&r.arg)) {
                    fails.push(Failure { rule: format!("must_run {}", r.arg),
                        detail: "nothing by that name ran".into(), examples: vec![] });
                }
            }
            other => fails.push(Failure { rule: other.to_string(),
                detail: "unknown rule; assertions must be understood to be enforced".into(),
                examples: vec![] }),
        }
    }
    fails
}

pub fn assert_json(fails: &[Failure], checked: usize) -> J {
    let mut o = J::obj();
    o.set("rules_checked", J::n(checked as f64));
    o.set("passed", J::Bool(fails.is_empty()));
    o.set("failures", J::Arr(fails.iter().map(|f| {
        let mut e = J::obj();
        e.set("rule", J::s(&f.rule));
        e.set("detail", J::s(&f.detail));
        e.set("examples", J::Arr(f.examples.iter().map(|x| J::s(x)).collect()));
        e
    }).collect()));
    o
}

// ========================================================================
// COVERAGE
// ========================================================================

/// What never ran.
///
/// The capture knows what executed, so given a list of what exists the
/// difference is free: dead code, an untaken branch, a service nobody called.
pub fn coverage(spans: &[Span], expected: &[String]) -> J {
    let ran: std::collections::BTreeSet<&str> = spans.iter().map(|s| s.name.as_str()).collect();
    let missed: Vec<&String> = expected.iter().filter(|e| !ran.contains(e.as_str())).collect();
    let hit = expected.len() - missed.len();
    let mut o = J::obj();
    o.set("expected", J::n(expected.len() as f64));
    o.set("reached", J::n(hit as f64));
    o.set("never_ran", J::n(missed.len() as f64));
    o.set("coverage", J::n(if expected.is_empty() { 0.0 }
        else { ((hit as f64 / expected.len() as f64) * 1000.0).round() / 1000.0 }));
    o.set("names", J::Arr(missed.iter().take(200).map(|m| J::s(m)).collect()));
    o
}

// ========================================================================
// COST (SPEC 3.3)
// ========================================================================

/// Ranks spans by whether they were working or waiting.
///
/// This is the distinction a duration cannot make: two ten-millisecond spans,
/// one burning a core and one asleep on a socket, are the two cases a person is
/// trying to tell apart. Where the runtime did not report cost, the span is
/// listed as unmeasured rather than assumed idle; zero is a measurement.
pub fn cost_report(spans: &[Span], limit: usize) -> J {
    struct Row<'a> { s: &'a Span, cpu: f64, ratio: f64, spent: String }
    let mut rows: Vec<Row> = Vec::new();
    let mut unmeasured = 0usize;
    for s in spans {
        let c = s.attributes.get("cost");
        match c {
            Some(c) if !matches!(c, J::Null) => {
                let cpu = c.get("cpu_ms").and_then(|x| x.as_f64()).unwrap_or(0.0);
                let ratio = c.get("cpu_ratio").and_then(|x| x.as_f64()).unwrap_or(0.0);
                let spent = c.get("spent").and_then(|x| x.as_str()).unwrap_or("?").to_string();
                rows.push(Row { s, cpu, ratio, spent });
            }
            _ => unmeasured += 1,
        }
    }
    rows.sort_by(|a, b| b.cpu.partial_cmp(&a.cpu).unwrap());
    rows.truncate(limit);
    let mut o = J::obj();
    o.set("note", J::s("a span whose processor time approaches its duration was working; one whose processor time is a fraction of it was waiting"));
    o.set("unmeasured", J::n(unmeasured as f64));
    o.set("spans", J::Arr(rows.iter().map(|r| {
        let mut e = J::obj();
        e.set("name", J::s(&r.s.name));
        e.set("duration_ms", J::n((r.s.duration_ms().unwrap_or(0.0) * 100.0).round() / 100.0));
        e.set("cpu_ms", J::n((r.cpu * 100.0).round() / 100.0));
        e.set("cpu_ratio", J::n(r.ratio));
        e.set("spent", J::s(&r.spent));
        e
    }).collect()));
    o
}
