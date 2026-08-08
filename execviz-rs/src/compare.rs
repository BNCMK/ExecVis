// ========================================================================
//  MANIFEST
// ========================================================================
//  script_name: compare.rs
//  script_path: execviz-rs/src/compare.rs
//  module_name: compare
//  version: 0.53.1
//  description: Regression comparison and interoperable export.
//  kind: module
//  spec: internal
//  internal_dependencies: find, json, stats, store
//  external_dependencies: std
//  features: compare
//  api_version: execvis-v1.0.0
//  last_updated: 2026-08-07
// ========================================================================

//! Regression comparison and interoperable export.
use crate::json::J;
use crate::stats::{distributions, Dist};
use crate::store::Span;
use std::collections::BTreeMap;

// ========================================================================
// TYPES
// ========================================================================

pub struct Change {
    pub name: String,
    pub before_n: usize,
    pub after_n: usize,
    pub before_p95: f64,
    pub after_p95: f64,
    pub before_med: f64,
    pub after_med: f64,
    pub verdict: &'static str,
}

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

/// Compares two captures by what work costs rather than by what work exists.
///
/// Two rules keep the answer honest. A comparison states its sample sizes,
/// because a span that ran three times either side has demonstrated nothing
/// whatever the medians say. And a difference smaller than the run-to-run
/// spread is not reported as a regression: crying about every wobble trains
/// people to ignore the tool, which costs more than the wobble did.
pub fn regressions(before: &[Span], after: &[Span], min_n: usize, sensitivity: f64) -> Vec<Change> {
    let idx = |d: Vec<Dist>| -> BTreeMap<String, Dist> {
        d.into_iter().map(|x| (x.name.clone(), x)).collect()
    };
    let b = idx(distributions(before, 1));
    let a = idx(distributions(after, 1));
    let mut out = Vec::new();
    for (name, av) in &a {
        let bv = match b.get(name) { Some(x) => x, None => continue };
        let enough = bv.count >= min_n && av.count >= min_n;
        // the spread within the earlier run is the yardstick: a move smaller
        // than the noise the run already had is not evidence of anything
        let spread = (bv.p95 - bv.median).abs().max(bv.median * 0.05).max(0.5);
        let delta = av.median - bv.median;
        let verdict = if !enough { "too few samples to judge" }
            else if delta > spread * sensitivity { "slower" }
            else if -delta > spread * sensitivity { "faster" }
            else { "within the noise of the earlier run" };
        if verdict == "within the noise of the earlier run" && enough { continue; }
        out.push(Change {
            name: name.clone(), before_n: bv.count, after_n: av.count,
            before_p95: bv.p95, after_p95: av.p95,
            before_med: bv.median, after_med: av.median, verdict,
        });
    }
    out.sort_by(|x, y| (y.after_med - y.before_med).partial_cmp(&(x.after_med - x.before_med)).unwrap());
    out
}

pub fn regressions_json(c: &[Change]) -> J {
    let r2 = |v: f64| (v * 100.0).round() / 100.0;
    let mut o = J::obj();
    o.set("note", J::s("a difference smaller than the earlier run's own spread is not reported: every wobble called a regression teaches people to ignore the tool"));
    o.set("changed", J::n(c.iter().filter(|x| x.verdict == "slower" || x.verdict == "faster").count() as f64));
    o.set("spans", J::Arr(c.iter().map(|x| {
        let mut e = J::obj();
        e.set("name", J::s(&x.name));
        e.set("verdict", J::s(x.verdict));
        e.set("median_before_ms", J::n(r2(x.before_med)));
        e.set("median_after_ms", J::n(r2(x.after_med)));
        e.set("p95_before_ms", J::n(r2(x.before_p95)));
        e.set("p95_after_ms", J::n(r2(x.after_p95)));
        // the sample sizes travel with the claim, always
        e.set("samples_before", J::n(x.before_n as f64));
        e.set("samples_after", J::n(x.after_n as f64));
        e
    }).collect()));
    o
}

// ========================================================================
// EXPORT
// ========================================================================

/// Chrome trace format, which Perfetto and every browser devtools reads.
///
/// A recording only one program can read is a lock-in dressed as a format. The
/// point is not generosity toward other tools: it is that a reader can check a
/// conclusion somewhere else, which makes the record trustworthy.
pub fn chrome_trace(spans: &[Span]) -> J {
    let mut events: Vec<J> = Vec::new();
    for s in spans {
        let end = match s.end { Some(e) => e, None => continue };
        let mut e = J::obj();
        e.set("name", J::s(&s.name));
        e.set("cat", J::s(&format!("{},{}", s.kind, s.domain.clone().unwrap_or_default())));
        e.set("ph", J::s("X"));                       // a complete event
        e.set("ts", J::n((s.start * 1_000_000.0).round()));   // microseconds
        e.set("dur", J::n(((end - s.start) * 1_000_000.0).round()));
        // the host becomes the process and the thread carries the domain, which
        // is how these viewers group work
        e.set("pid", J::s(&s.host_id));
        e.set("tid", J::s(&s.domain.clone().unwrap_or_else(|| "main".into())));
        let mut args = J::obj();
        args.set("span_id", J::s(&s.span_id));
        args.set("status", J::s(&s.status));
        if !matches!(s.error, J::Null) { args.set("error", s.error.clone()); }
        if !matches!(s.inputs, J::Null) { args.set("inputs", s.inputs.clone()); }
        e.set("args", args);
        events.push(e);
    }
    let mut o = J::obj();
    o.set("traceEvents", J::Arr(events));
    o.set("displayTimeUnit", J::s("ms"));
    o
}

/// Folded stacks, which is what flamegraph tooling consumes.
///
/// Each line is a causal path and the count is self time in microseconds, so the
/// width of a frame is the time that frame itself spent rather than the time it
/// contained.
pub fn folded_stacks(spans: &[Span]) -> String {
    let by_id: BTreeMap<&str, &Span> = spans.iter().map(|s| (s.span_id.as_str(), s)).collect();
    let selfs = crate::find::self_ms(spans);
    let mut lines: Vec<String> = Vec::new();
    for s in spans {
        let us = match selfs.get(s.span_id.as_str()) { Some(v) => (*v * 1000.0).round() as i64, None => continue };
        if us <= 0 { continue; }
        let mut chain = vec![s.name.clone()];
        let (mut cur, mut guard) = (s, 0);
        while let Some(p) = &cur.parent_span_id {
            match by_id.get(p.as_str()) {
                Some(pp) => { chain.push(pp.name.clone()); cur = pp; }
                None => break,
            }
            guard += 1; if guard > 64 { break; }
        }
        chain.reverse();
        lines.push(format!("{} {}", chain.join(";"), us));
    }
    lines.sort();
    lines.join("\n") + "\n"
}
