// ========================================================================
//  MANIFEST
// ========================================================================
//  script_name: flame.rs
//  script_path: execviz-rs/src/flame.rs
//  module_name: flame
//  version: 0.53.1
//  description: A flamegraph and a critical path, both folded out of the span tree rather than sampled
//  kind: module
//  spec: internal
//  internal_dependencies: store, json
//  external_dependencies: std
//  features: folded stacks, self time, critical path, overlap accounting
//  api_version: execvis-v1.0.0
//  last_updated: 2026-08-07
// ========================================================================

//! Two answers the span tree already contains.
//!
//! A profiler samples stacks on a timer and counts where it landed. That is the
//! only way to see inside a function nobody instrumented, and this does not do
//! it. But a span tree carries a stack of its own: each span names work, and its
//! parent names the work that caused it. Folding that tree gives a flamegraph
//! without sampling anything, weighted by measured time rather than by sample
//! count, which is more accurate for the work that is instrumented and blind to
//! the work that is not. Both of those are worth saying out loud.
//!
//! The critical path is a different question and a more common one. Adding up
//! the durations of everything slow in a request answers nothing when the work
//! overlapped: the total is set by one chain, and the rest cost nothing because
//! it ran alongside. This walks that chain.

use crate::json::J;
use crate::store::Span;
use std::collections::{BTreeMap, HashMap};

// ========================================================================
// SELF TIME
// ========================================================================

/// Time a span spent in itself, with time covered by its children removed.
///
/// Children that overlap each other are merged first. Without that, two children
/// running side by side subtract twice and the parent appears to have spent
/// negative time in itself, which is a number nobody can act on.
fn self_ms(span: &Span, children: &[&Span]) -> f64 {
    let total = match span.end {
        Some(e) => (e - span.start).max(0.0),
        None => return 0.0, // an unfinished span has no duration to divide
    };
    let mut iv: Vec<(f64, f64)> = children
        .iter()
        .filter_map(|c| c.end.map(|e| (c.start.max(span.start), e.min(span.start + total))))
        .filter(|(a, b)| b > a)
        .collect();
    iv.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut covered = 0.0;
    let mut cur: Option<(f64, f64)> = None;
    for (a, b) in iv {
        match cur {
            None => cur = Some((a, b)),
            Some((ca, cb)) if a <= cb => cur = Some((ca, cb.max(b))),
            Some((ca, cb)) => {
                covered += cb - ca;
                cur = Some((a, b));
            }
        }
    }
    if let Some((ca, cb)) = cur {
        covered += cb - ca;
    }
    ((total - covered) * 1000.0).max(0.0)
}

// ========================================================================
// THE FLAMEGRAPH
// ========================================================================

/// Folded stacks, in the format every flamegraph renderer already reads.
///
/// Each line is a stack of span names separated by semicolons, then the self
/// time in milliseconds. That is the same shape `perf script | stackcollapse`
/// produces, so the output opens in speedscope, flamegraph.pl or a browser
/// without this having to draw anything.
pub fn folded(spans: &[Span]) -> J {
    let by_id: HashMap<&str, &Span> = spans.iter().map(|s| (s.span_id.as_str(), s)).collect();
    let mut kids: HashMap<&str, Vec<&Span>> = HashMap::new();
    for s in spans {
        if let Some(p) = s.parent_span_id.as_deref() {
            kids.entry(p).or_default().push(s);
        }
    }

    // the stack of a span is the chain of names above it, root first
    let stack_of = |s: &Span| -> String {
        let mut parts = vec![s.name.clone()];
        let mut cur = s;
        let mut guard = 0;
        while let Some(p) = cur.parent_span_id.as_deref() {
            match by_id.get(p) {
                Some(par) => {
                    parts.push(par.name.clone());
                    cur = par;
                }
                None => break,
            }
            guard += 1;
            if guard > 256 {
                break; // a cycle in recorded parentage is a defect, not a stack
            }
        }
        parts.reverse();
        parts.join(";")
    };

    let mut folded: BTreeMap<String, f64> = BTreeMap::new();
    let empty: Vec<&Span> = Vec::new();
    let mut unfinished = 0usize;
    for s in spans {
        if s.end.is_none() {
            unfinished += 1;
            continue;
        }
        let ms = self_ms(s, kids.get(s.span_id.as_str()).unwrap_or(&empty));
        if ms > 0.0 {
            *folded.entry(stack_of(s)).or_insert(0.0) += ms;
        }
    }

    let mut rows: Vec<J> = Vec::new();
    let mut lines: Vec<String> = Vec::new();
    for (stack, ms) in &folded {
        let mut o = J::obj();
        o.set("stack", J::s(stack));
        o.set("self_ms", J::n((ms * 100.0).round() / 100.0));
        rows.push(o);
        lines.push(format!("{} {}", stack, ms.round() as i64));
    }

    let mut out = J::obj();
    out.set("frames", J::Arr(rows));
    out.set("folded", J::s(&lines.join("\n")));
    out.set("unfinished_spans", J::n(unfinished as f64));
    out.set("this_does_not_say", J::s(
        "where time went inside a span. This is folded from spans, so it is exact \
         for work that was instrumented and silent about work that was not. A \
         sampling profiler answers the opposite way round.",
    ));
    out
}

// ========================================================================
// THE CRITICAL PATH
// ========================================================================

/// The chain that set the duration, rather than everything that was slow.
///
/// From the root, the next step is the child that finishes last, because that is
/// the one the parent waited on. Work that overlapped it cost nothing: removing
/// it would not make the request faster, and a list of slow spans cannot tell
/// the difference.
pub fn critical_path(spans: &[Span], root_id: Option<&str>) -> J {
    let by_id: HashMap<&str, &Span> = spans.iter().map(|s| (s.span_id.as_str(), s)).collect();
    let mut kids: HashMap<&str, Vec<&Span>> = HashMap::new();
    for s in spans {
        if let Some(p) = s.parent_span_id.as_deref() {
            kids.entry(p).or_default().push(s);
        }
    }

    // the root asked for, or the longest finished span that has no parent
    let root: Option<&Span> = match root_id {
        Some(id) => by_id.get(id).copied(),
        None => spans
            .iter()
            .filter(|s| s.parent_span_id.is_none() && s.end.is_some())
            .max_by(|a, b| {
                let da = a.end.unwrap_or(a.start) - a.start;
                let db = b.end.unwrap_or(b.start) - b.start;
                da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
            }),
    };

    let mut out = J::obj();
    let root = match root {
        Some(r) => r,
        None => {
            out.set("error", J::s("no finished root span in this capture"));
            return out;
        }
    };

    let empty: Vec<&Span> = Vec::new();
    let mut chain: Vec<J> = Vec::new();
    let mut cur = root;
    let mut guard = 0;
    let mut total = 0.0;
    loop {
        let ch = kids.get(cur.span_id.as_str()).unwrap_or(&empty);
        let own = self_ms(cur, ch);
        total += own;
        let mut o = J::obj();
        o.set("span_id", J::s(&cur.span_id));
        o.set("name", J::s(&cur.name));
        o.set("kind", J::s(&cur.kind));
        o.set("status", J::s(&cur.status));
        o.set("self_ms", J::n((own * 100.0).round() / 100.0));
        if let Some(e) = cur.end {
            o.set("duration_ms", J::n(((e - cur.start) * 1000.0 * 100.0).round() / 100.0));
        }
        chain.push(o);

        // the child that finished last is the one this span waited on
        let next = ch
            .iter()
            .filter(|c| c.end.is_some())
            .max_by(|a, b| {
                a.end.unwrap_or(0.0)
                    .partial_cmp(&b.end.unwrap_or(0.0))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .copied();
        match next {
            Some(n) => cur = n,
            None => break,
        }
        guard += 1;
        if guard > 512 {
            break;
        }
    }

    let span_total = root.end.map(|e| (e - root.start) * 1000.0).unwrap_or(0.0);
    out.set("root", J::s(&root.name));
    out.set("path", J::Arr(chain));
    out.set("path_self_ms", J::n((total * 100.0).round() / 100.0));
    out.set("root_duration_ms", J::n((span_total * 100.0).round() / 100.0));
    out.set("this_does_not_say", J::s(
        "that every span on this path is worth optimising, only that shortening \
         anything off it changes nothing. Work that overlapped the path cost no \
         wall time however slow it was.",
    ));
    out
}


// ========================================================================
// SAMPLED STACKS
// ========================================================================

/// Fold what the sampler recorded.
///
/// The span-folded flamegraph above is exact for instrumented work and blind to
/// the rest. This is the other half: `execviz-cpu` interrupts the machine on a
/// timer and records where it was standing, so a slow function nobody wrapped
/// appears here and nowhere else. The two answer opposite halves of the same
/// question and are reported separately rather than merged, because merging an
/// exact measurement with a statistical one produces a number that is neither.
pub fn sampled(records: &str) -> J {
    let mut folded: BTreeMap<String, u64> = BTreeMap::new();
    let mut by_pid: BTreeMap<u64, u64> = BTreeMap::new();
    let mut total = 0u64;
    let mut unreadable = 0u64;

    for line in records.lines() {
        let line = line.trim();
        if line.is_empty() { continue; }
        let v = match crate::json::parse(line) { Ok(v) => v, Err(_) => { unreadable += 1; continue } };
        if v.get("kind").and_then(|x| x.as_str()) != Some("cpu") { continue; }
        let pid = v.get("pid").and_then(|x| x.as_f64()).unwrap_or(0.0) as u64;
        let frames: Vec<String> = match v.get("stack").and_then(|x| x.as_arr()) {
            Some(a) => a.iter().filter_map(|f| f.as_str().map(|s| s.to_string())).collect(),
            None => { unreadable += 1; continue }
        };
        if frames.is_empty() { continue; }
        *folded.entry(frames.join(";")).or_insert(0) += 1;
        *by_pid.entry(pid).or_insert(0) += 1;
        total += 1;
    }

    let mut rows: Vec<J> = Vec::new();
    let mut lines: Vec<String> = Vec::new();
    let mut ordered: Vec<(&String, &u64)> = folded.iter().collect();
    ordered.sort_by(|a, b| b.1.cmp(a.1));
    for (stack, n) in ordered.iter().take(500) {
        let mut o = J::obj();
        o.set("stack", J::s(stack));
        o.set("samples", J::n(**n as f64));
        o.set("share", J::n(if total > 0 { (**n as f64 / total as f64 * 1000.0).round() / 1000.0 } else { 0.0 }));
        rows.push(o);
        lines.push(format!("{} {}", stack, n));
    }

    let mut procs: Vec<J> = Vec::new();
    for (pid, n) in &by_pid {
        let mut o = J::obj();
        o.set("pid", J::n(*pid as f64));
        o.set("samples", J::n(*n as f64));
        procs.push(o);
    }

    let mut out = J::obj();
    out.set("samples", J::n(total as f64));
    out.set("distinct_stacks", J::n(folded.len() as f64));
    out.set("by_process", J::Arr(procs));
    out.set("stacks", J::Arr(rows));
    out.set("folded", J::s(&lines.join("\n")));
    out.set("unreadable_records", J::n(unreadable as f64));
    out.set("this_does_not_say", J::s(
        "which function an address belongs to. Frames are addresses, because \
         resolving one needs the symbol table of whatever mapped it, and a name \
         this could not verify would be invented. Resolve them against the \
         process maps to read them.",
    ));
    out
}
