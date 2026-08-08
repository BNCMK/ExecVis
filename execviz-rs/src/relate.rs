// ========================================================================
//  MANIFEST
// ========================================================================
//  script_name: relate.rs
//  script_path: execviz-rs/src/relate.rs
//  module_name: relate
//  version: 0.53.1
//  description: Correlation and concurrency.
//  kind: module
//  spec: internal
//  internal_dependencies: json, store
//  external_dependencies: std
//  features: relate
//  api_version: execvis-v1.0.0
//  last_updated: 2026-08-07
// ========================================================================

//! Correlation and concurrency.
use crate::json::J;
use crate::store::Span;
use std::collections::BTreeMap;

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

/// What co-occurs with failure.
///
/// This computes co-occurrence and nothing else. The report says errors are N
/// times more common where an attribute holds a value; a fact about the
/// recording; and never that the value causes them, which is a claim about the
/// world that a capture cannot support. A tool that blurs the two produces
/// confident people who are wrong.
pub fn correlations(spans: &[Span], min_support: usize) -> J {
    let total = spans.len().max(1);
    let total_err = spans.iter().filter(|s| s.status == "error").count();
    let base = total_err as f64 / total as f64;

    // every dimension the capture already holds, without asking for more
    let mut buckets: BTreeMap<(String, String), (usize, usize)> = BTreeMap::new();
    let mut note = |k: &str, v: &str, err: bool, b: &mut BTreeMap<(String, String), (usize, usize)>| {
        let e = b.entry((k.to_string(), v.to_string())).or_insert((0, 0));
        e.0 += 1;
        if err { e.1 += 1; }
    };
    for s in spans {
        let err = s.status == "error";
        note("host", &s.host_id, err, &mut buckets);
        note("domain", s.domain.as_deref().unwrap_or("unknown"), err, &mut buckets);
        note("kind", &s.kind, err, &mut buckets);
        if let J::Obj(m) = &s.attributes {
            for (k, v) in m {
                if k == "cost" { continue; }               // a measurement, not a label
                let rendered = v.dump();
                if rendered.len() < 64 { note(k, rendered.trim_matches('"'), err, &mut buckets); }
            }
        }
        if let J::Obj(m) = &s.run {
            for (k, v) in m { note(k, v.dump().trim_matches('"'), err, &mut buckets); }
        }
    }

    let mut rows: Vec<(String, String, usize, usize, f64)> = buckets.into_iter()
        .filter(|(_, (n, _))| *n >= min_support)     // a lift over three spans is noise with a decimal point
        .filter_map(|((k, v), (n, e))| {
            if e == 0 { return None; }
            let rate = e as f64 / n as f64;
            let lift = if base > 0.0 { rate / base } else { 0.0 };
            if lift <= 1.2 { return None; }
            Some((k, v, n, e, lift))
        }).collect();
    rows.sort_by(|a, b| b.4.partial_cmp(&a.4).unwrap());

    let mut o = J::obj();
    o.set("note", J::s("co-occurrence, not cause: this says where errors are more common in this recording, never why"));
    o.set("min_support", J::n(min_support as f64));
    o.set("baseline_error_rate", J::n((base * 10000.0).round() / 10000.0));
    o.set("findings", J::Arr(rows.iter().take(25).map(|(k, v, n, e, lift)| {
        let mut x = J::obj();
        x.set("attribute", J::s(k));
        x.set("value", J::s(v));
        x.set("spans", J::n(*n as f64));
        x.set("errors", J::n(*e as f64));
        x.set("error_rate", J::n(((*e as f64 / *n as f64) * 10000.0).round() / 10000.0));
        x.set("times_the_baseline", J::n((lift * 100.0).round() / 100.0));
        x.set("reads_as", J::s(&format!(
            "errors are {:.1}x more common where {}={} in this capture", lift, k, v)));
        x
    }).collect()));
    o
}

/// How much ran at once, from a sweep over starts and ends.
pub fn concurrency(spans: &[Span]) -> J {
    let mut events: Vec<(f64, i32)> = Vec::new();
    for s in spans {
        if let Some(e) = s.end {
            events.push((s.start, 1));
            events.push((e, -1));
        }
    }
    events.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap().then(a.1.cmp(&b.1)));

    let (mut cur, mut peak, mut peak_at) = (0i32, 0i32, 0.0f64);
    let mut idle_ms = 0.0;
    let mut at_peak_ms = 0.0;
    let mut last_t = events.first().map(|e| e.0).unwrap_or(0.0);
    let mut hist: BTreeMap<i32, f64> = BTreeMap::new();
    for (t, d) in &events {
        let dt = (t - last_t) * 1000.0;
        if dt > 0.0 {
            *hist.entry(cur).or_insert(0.0) += dt;
            if cur == 0 { idle_ms += dt; }
            if cur == peak && peak > 0 { at_peak_ms += dt; }
        }
        cur += d;
        if cur > peak { peak = cur; peak_at = *t; at_peak_ms = 0.0; }
        last_t = *t;
    }

    let mut o = J::obj();
    o.set("peak_parallelism", J::n(peak as f64));
    o.set("peak_at", J::n(peak_at));
    o.set("time_at_peak_ms", J::n((at_peak_ms * 100.0).round() / 100.0));
    o.set("idle_ms", J::n((idle_ms * 100.0).round() / 100.0));
    // A pool pinned at its limit is evidence of a limit, not of a problem. What
    // makes it a finding is time spent waiting while pinned; the queue the
    // limit created; so both figures are reported and neither is called a fault.
    o.set("note", J::s("a level held for a long time suggests a limit; a limit is only a finding if work was waiting behind it"));
    o.set("time_by_level_ms", J::Arr(hist.iter().map(|(lvl, ms)| {
        let mut e = J::obj();
        e.set("running", J::n(*lvl as f64));
        e.set("ms", J::n((*ms * 100.0).round() / 100.0));
        e
    }).collect()));
    o
}
