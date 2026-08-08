// ========================================================================
//  MANIFEST
// ========================================================================
//  script_name: finger.rs
//  script_path: execviz-rs/src/finger.rs
//  module_name: finger
//  version: 0.53.1
//  description: The fingerprint invariants.
//  kind: module
//  spec: internal
//  internal_dependencies: json, store, syscalls
//  external_dependencies: std
//  features: finger
//  api_version: execvis-v1.0.0
//  last_updated: 2026-08-07
// ========================================================================

//! The fingerprint invariants.
//!
//! Each is a deterministic function of the captured spans and nothing else, so
//! two runs of the same system produce two readings of the same quantity rather
//! than two opinions about it. Every value is normalised to 0..1 so the set can
//! be read on one scale; the raw figure travels alongside it, because a
//! normalised number is a position and not a measurement.
use crate::json::J;
use crate::store::Span;
use std::collections::BTreeMap;

// ========================================================================
// TYPES
// ========================================================================

#[derive(Clone, Debug)]
pub struct Invariant { pub name: &'static str, pub raw: f64, pub norm: f64 }

// ========================================================================
// INTERNALS
// ========================================================================

/// Squashes an unbounded positive quantity onto 0..1 without a hard ceiling, so
/// an outlier compresses rather than clipping and losing its ordering.
fn soft(x: f64, mid: f64) -> f64 { x / (x + mid) }

fn entropy(counts: &[usize]) -> f64 {
    let total: usize = counts.iter().sum();
    if total == 0 { return 0.0; }
    let mut h = 0.0;
    for &c in counts {
        if c == 0 { continue; }
        let p = c as f64 / total as f64;
        h -= p * p.log2();
    }
    h
}

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

pub fn invariants(spans: &[Span]) -> Vec<Invariant> {
    let n = spans.len().max(1) as f64;

    // children per span, which is the fan-out distribution
    let mut kids: BTreeMap<&str, usize> = BTreeMap::new();
    for s in spans {
        if let Some(p) = &s.parent_span_id { *kids.entry(p.as_str()).or_insert(0) += 1; }
    }
    let fanouts: Vec<usize> = kids.values().copied().collect();
    let mean_fanout = if fanouts.is_empty() { 0.0 }
        else { fanouts.iter().sum::<usize>() as f64 / fanouts.len() as f64 };
    let branching = entropy(&fanouts);

    // how unevenly the fan-out is spread: one hot node against many even ones
    let max_fanout = fanouts.iter().copied().max().unwrap_or(0) as f64;
    let concentration = if mean_fanout > 0.0 { max_fanout / (mean_fanout * fanouts.len() as f64).max(1.0) } else { 0.0 };

    // loop weight, taken from what the capture layer aggregated rather than from
    // matching names: the adapter already decided what was a loop, and inferring
    // it again from strings would be a second, worse answer to a settled question
    let loop_iters: f64 = spans.iter()
        .filter(|s| s.kind == "loop")
        .map(|s| s.attributes.get("iterations").and_then(|x| x.as_f64()).unwrap_or(0.0))
        .sum();
    let loop_density = loop_iters / n;

    // spacing between causal siblings: regular spacing is a sequential system,
    // scattered spacing is a concurrent one
    let mut by_parent: BTreeMap<&str, Vec<f64>> = BTreeMap::new();
    for s in spans {
        if let Some(p) = &s.parent_span_id { by_parent.entry(p.as_str()).or_default().push(s.start); }
    }
    let mut cvs = Vec::new();
    for v in by_parent.values_mut() {
        if v.len() < 3 { continue; }
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let gaps: Vec<f64> = v.windows(2).map(|w| w[1] - w[0]).collect();
        let m = gaps.iter().sum::<f64>() / gaps.len() as f64;
        if m <= 0.0 { continue; }
        let sd = (gaps.iter().map(|g| (g - m) * (g - m)).sum::<f64>() / gaps.len() as f64).sqrt();
        cvs.push(sd / m);
    }
    let jitter = if cvs.is_empty() { 0.0 } else { cvs.iter().sum::<f64>() / cvs.len() as f64 };

    // How much of the work is spent at the boundary rather than inside the
    // program. Counted by span, not by duration: durations move with the
    // machine and the load, so a duration-weighted ratio is unstable run to run
    // and blurs exactly the systems it should tell apart. The count is a
    // property of the program's shape and holds still.
    let io_spans = spans.iter()
        .filter(|s| matches!(s.kind.as_str(), "io" | "external" | "wait"))
        .count() as f64;
    let io_ratio = io_spans / n;

    // how deep the causal graph runs
    let byid: BTreeMap<&str, &Span> = spans.iter().map(|s| (s.span_id.as_str(), s)).collect();
    let mut depth_sum = 0.0;
    for s in spans {
        let (mut d, mut cur) = (0, Some(s));
        while let Some(c) = cur {
            match &c.parent_span_id { Some(p) => { cur = byid.get(p.as_str()).copied(); d += 1; }, None => break }
            if d > 64 { break; }
        }
        depth_sum += d as f64;
    }
    let mean_depth = depth_sum / n;

    vec![
        Invariant { name: "branching",     raw: branching,     norm: soft(branching, 1.2) },
        Invariant { name: "concentration", raw: concentration, norm: concentration.clamp(0.0, 1.0) },
        Invariant { name: "loop_density",  raw: loop_density,  norm: soft(loop_density, 2.0) },
        Invariant { name: "jitter",        raw: jitter,        norm: soft(jitter, 1.0) },
        Invariant { name: "io_ratio",      raw: io_ratio,      norm: io_ratio.clamp(0.0, 1.0) },
        Invariant { name: "depth",         raw: mean_depth,    norm: soft(mean_depth, 4.0) },
    ]
}

pub fn to_json(inv: &[Invariant]) -> J {
    let mut o = J::obj();
    o.set("invariants", J::Arr(inv.iter().map(|i| {
        let mut e = J::obj();
        e.set("name", J::s(i.name));
        e.set("raw", J::n((i.raw * 10000.0).round() / 10000.0));
        e.set("norm", J::n((i.norm * 10000.0).round() / 10000.0));
        e
    }).collect()));
    o.set("vector", J::Arr(inv.iter().map(|i| J::n((i.norm * 10000.0).round() / 10000.0)).collect()));
    o
}

/// Distance between two signatures, on the normalised scale only. Used to ask
/// the question the spec makes the deciding one: are two runs of one system
/// closer to each other than to a different system?
pub fn distance(a: &[Invariant], b: &[Invariant]) -> f64 {
    let n = a.len().min(b.len());
    let mut acc = 0.0;
    for i in 0..n { let d = a[i].norm - b[i].norm; acc += d * d; }
    (acc / n.max(1) as f64).sqrt()
}

/// Comparison across captures: the operation the fingerprint exists for. A
/// baseline of several runs gives a band per axis; a candidate is read against
/// it, and the answer names the axis that moved rather than only saying that
/// something did.
pub fn compare(baseline: &[Vec<Invariant>], candidate: &[Invariant]) -> J {
    let n = candidate.len();
    let mut axes = Vec::new();
    let mut worst = 0.0f64;
    let mut worst_axis = "";
    for i in 0..n {
        let vals: Vec<f64> = baseline.iter().filter_map(|b| b.get(i)).map(|x| x.norm).collect();
        if vals.is_empty() { continue; }
        let mean = vals.iter().sum::<f64>() / vals.len() as f64;
        let sd = (vals.iter().map(|v| (v - mean) * (v - mean)).sum::<f64>() / vals.len() as f64).sqrt();
        let v = candidate[i].norm;
        // a band that is exactly zero wide would call every run an anomaly, so
        // the recorder is the smallest difference worth noticing at all
        let band = sd.max(0.01) * 3.0;
        let dev = (v - mean).abs();
        let outside = dev > band;
        if dev > worst { worst = dev; worst_axis = candidate[i].name; }
        let mut o = J::obj();
        o.set("axis", J::s(candidate[i].name));
        o.set("baseline", J::n((mean * 10000.0).round() / 10000.0));
        o.set("band", J::n((band * 10000.0).round() / 10000.0));
        o.set("value", J::n((v * 10000.0).round() / 10000.0));
        o.set("deviation", J::n((dev * 10000.0).round() / 10000.0));
        o.set("outside_band", J::Bool(outside));
        axes.push(o);
    }
    let any = axes.iter().any(|a| a.get("outside_band") == Some(&J::Bool(true)));
    let mut out = J::obj();
    out.set("runs_in_baseline", J::n(baseline.len() as f64));
    out.set("matches_baseline", J::Bool(!any));
    out.set("largest_departure", J::s(worst_axis));
    out.set("axes", J::Arr(axes));
    out
}

/// Six invariants computed from recorder records instead of from spans.
///
/// The fingerprint identifies a program by execution shape rather than by a name
/// somebody assigned to it. Computed here it needs no instrumentation at all,
/// which makes it usable on a process nobody has ever traced: an
/// unlabelled workload, a binary after a deploy, a container whose metadata is
/// wrong.
///
/// The invariants are deliberately the same *kinds* of measure as the span
/// version (mix, rate, spread, burstiness) so a reader learns one idea, but they
/// are not comparable across the two: a shape derived from syscalls and a shape
/// derived from spans measure different things, and the caller is told which
/// this is.
pub fn invariants_from_floor(recs: &[crate::syscalls::Record]) -> Vec<Invariant> {
    if recs.is_empty() { return Vec::new(); }
    let n = recs.len() as f64;

    let mut by_call: std::collections::BTreeMap<&str, usize> = Default::default();
    let mut threads: std::collections::BTreeSet<i64> = Default::default();
    let mut times: Vec<f64> = Vec::with_capacity(recs.len());
    for r in recs {
        *by_call.entry(r.name.as_str()).or_insert(0) += 1;
        threads.insert(r.tid);
        times.push(r.t);
    }
    times.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    // 1. how many distinct calls the program uses
    let variety = by_call.len() as f64;

    // 2. how concentrated it is on its favourite call: a program that does one
    //    thing looks nothing like one that does twenty
    let top = by_call.values().max().copied().unwrap_or(0) as f64;
    let concentration = top / n;

    // 3. shannon evenness over the call mix
    let mut entropy = 0.0;
    for c in by_call.values() {
        let p = *c as f64 / n;
        if p > 0.0 { entropy -= p * p.ln(); }
    }
    let evenness = if variety > 1.0 { entropy / variety.ln() } else { 0.0 };

    // 4. how much of the work is spread across threads
    let concurrency = threads.len() as f64;

    // 5. burstiness: coefficient of variation of the gaps between calls
    let mut gaps: Vec<f64> = Vec::new();
    for w in times.windows(2) { gaps.push(w[1] - w[0]); }
    let burst = if gaps.len() > 1 {
        let mean = gaps.iter().sum::<f64>() / gaps.len() as f64;
        if mean > 0.0 {
            let var = gaps.iter().map(|g| (g - mean) * (g - mean)).sum::<f64>() / gaps.len() as f64;
            var.sqrt() / mean
        } else { 0.0 }
    } else { 0.0 };

    // 6. rate, which separates a busy program from an idle one of the same shape
    let span_secs = times.last().unwrap_or(&0.0) - times.first().unwrap_or(&0.0);
    let rate = if span_secs > 0.0 { n / span_secs } else { 0.0 };

    vec![
        Invariant { name: "call_variety", raw: variety, norm: (variety / 40.0).min(1.0) },
        Invariant { name: "concentration", raw: concentration, norm: concentration.min(1.0) },
        Invariant { name: "call_evenness", raw: evenness, norm: evenness.min(1.0) },
        Invariant { name: "thread_spread", raw: concurrency, norm: (concurrency / 16.0).min(1.0) },
        Invariant { name: "burstiness", raw: burst, norm: (burst / 8.0).min(1.0) },
        Invariant { name: "call_rate", raw: rate, norm: (rate / 5000.0).min(1.0) },
    ]
}

/// Splits recorder records by the program that wrote them, and fingerprints each.
///
/// Every result carries its sample size, because a shape from forty records is
/// evidence of very little and saying so is the difference between a measurement
/// and a claim.
pub fn recorder_identities(recs: &[crate::syscalls::Record], min_records: usize) -> J {
    let mut by_who: std::collections::BTreeMap<String, Vec<crate::syscalls::Record>> = Default::default();
    for r in recs {
        let who = r.comm.clone().unwrap_or_else(|| format!("tid {}", r.tid));
        by_who.entry(who).or_default().push(crate::syscalls::Record {
            t: r.t, dur: r.dur, tid: r.tid, name: r.name.clone(), comm: r.comm.clone(), fd: None });
    }
    let mut out: Vec<J> = Vec::new();
    let mut thin: Vec<J> = Vec::new();
    for (who, rs) in &by_who {
        if rs.len() < min_records {
            // Not fingerprinted, and said so rather than fingerprinted badly.
            thin.push(J::Obj([
                ("who".to_string(), J::Str(who.clone())),
                ("records".to_string(), J::Num(rs.len() as f64)),
                ("reason".to_string(), J::Str(
                    format!("fewer than {} records; a shape from this little is not evidence", min_records))),
            ].into_iter().collect()));
            continue;
        }
        let inv = invariants_from_floor(rs);
        let mut o = to_json(&inv);
        o.set("who", J::Str(who.clone()));
        o.set("records", J::Num(rs.len() as f64));
        o.set("derived_from", J::Str("recorder".to_string()));
        out.push(o);
    }
    J::Obj([
        ("identities".to_string(), J::Arr(out)),
        ("not_fingerprinted".to_string(), J::Arr(thin)),
        ("note".to_string(), J::Str(
            "A shape is evidence, not proof, and a shape derived from syscalls is \
             not comparable with one derived from spans.".to_string())),
    ].into_iter().collect())
}
