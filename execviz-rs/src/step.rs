// ========================================================================
//  MANIFEST
// ========================================================================
//  script_name: step.rs
//  script_path: execviz-rs/src/step.rs
//  module_name: step
//  version: 0.53.1
//  description: Stepping through the record.
//  kind: module
//  spec: internal
//  internal_dependencies: json, store
//  external_dependencies: std
//  features: step
//  api_version: execvis-v1.0.0
//  last_updated: 2026-08-07
// ========================================================================

//! Stepping through the record.
//!
//! This is not time-travel debugging in the usual sense. That means
//! re-executing a program from a recording of every source of nondeterminism,
//! so a person can evaluate new expressions at an old moment. This tool records
//! observations, not the machine.
//!
//! What it does is walk the recorded execution in causal order, forwards or
//! backwards, showing what went in, what came back, and what was logged; and
//! saying so plainly wherever nothing was recorded.
use crate::json::J;
use crate::store::Span;
use std::collections::BTreeMap;

// ========================================================================
// TYPES
// ========================================================================

pub struct Step<'a> {
    pub index: usize,
    pub depth: usize,
    pub span: &'a Span,
    pub kind: &'static str,      // "enter" | "leave"
    pub at: f64,
}

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

/// The execution as a sequence of entries and exits, ordered causally.
///
/// Both edges are steps because a person stepping through wants to stop when
/// work finishes as well as when it starts; that is where a return value
/// exists and where a failure becomes visible.
pub fn timeline<'a>(spans: &'a [Span], trace: Option<&str>) -> Vec<Step<'a>> {
    let chosen: Vec<&Span> = spans.iter()
        .filter(|s| trace.map(|t| s.trace_id == t).unwrap_or(true))
        .collect();
    if chosen.is_empty() { return Vec::new(); }

    let mut children: BTreeMap<&str, Vec<&Span>> = BTreeMap::new();
    let ids: std::collections::BTreeSet<&str> = chosen.iter().map(|s| s.span_id.as_str()).collect();
    let mut roots: Vec<&Span> = Vec::new();
    for s in &chosen {
        match &s.parent_span_id {
            Some(p) if ids.contains(p.as_str()) => children.entry(p.as_str()).or_default().push(s),
            _ => roots.push(s),
        }
    }
    for v in children.values_mut() { v.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap()); }
    roots.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap());

    let mut out: Vec<Step> = Vec::new();
    fn walk<'a>(s: &'a Span, depth: usize, kids: &BTreeMap<&str, Vec<&'a Span>>,
                out: &mut Vec<Step<'a>>) {
        out.push(Step { index: out.len(), depth, span: s, kind: "enter", at: s.start });
        if let Some(cs) = kids.get(s.span_id.as_str()) {
            for c in cs { walk(c, depth + 1, kids, out); }
        }
        out.push(Step { index: out.len(), depth, span: s, kind: "leave",
                        at: s.end.unwrap_or(s.start) });
    }
    for r in roots { walk(r, 0, &children, &mut out); }
    for (i, st) in out.iter_mut().enumerate() { st.index = i; }
    out
}

/// One step, rendered.
///
/// Absence is stated: a step over work whose values were never recorded reports it
/// rather than showing an empty frame, because a gap that looks like a value is
/// worse than a gap that says it is one.
pub fn render(st: &Step) -> J {
    let s = st.span;
    let mut o = J::obj();
    o.set("index", J::n(st.index as f64));
    o.set("depth", J::n(st.depth as f64));
    o.set("at", J::n(st.at));
    o.set("event", J::s(st.kind));
    o.set("name", J::s(&s.name));
    o.set("kind", J::s(&s.kind));
    o.set("span_id", J::s(&s.span_id));

    if st.kind == "enter" {
        match &s.inputs {
            J::Null => { o.set("inputs", J::Null);
                         o.set("inputs_note", J::s("not recorded: value capture was off for this span")); }
            v => { o.set("inputs", v.clone()); }
        }
    } else {
        o.set("status", J::s(&s.status));
        o.set("duration_ms", match s.duration_ms() { Some(d) => J::n(d), None => J::Null });
        if s.end.is_none() {
            o.set("note", J::s("this work never finished; there is no return to step over"));
        }
        match &s.output {
            J::Null if s.status != "error" => {
                o.set("output", J::Null);
                o.set("output_note", J::s("not recorded: value capture was off for this span"));
            }
            v => { o.set("output", v.clone()); }
        }
        if !matches!(s.error, J::Null) { o.set("error", s.error.clone()); }
    }
    if let J::Arr(events) = &s.events {
        let mine: Vec<J> = events.iter().filter(|e| {
            e.get("t").and_then(|x| x.as_f64())
                .map(|t| if st.kind == "enter" { t <= st.at + 1e-9 } else { t <= st.at + 1e-9 })
                .unwrap_or(false)
        }).cloned().collect();
        if st.kind == "leave" && !mine.is_empty() { o.set("logged", J::Arr(mine)); }
    }
    o
}

pub fn to_json(steps: &[Step], from: usize, count: usize) -> J {
    let end = (from + count).min(steps.len());
    let slice = if from < steps.len() { &steps[from..end] } else { &[][..] };
    let mut o = J::obj();
    o.set("steps", J::n(steps.len() as f64));
    o.set("from", J::n(from as f64));
    o.set("note", J::s("this replays the record, not the program: it cannot evaluate an expression that was never recorded, enter a function nobody instrumented, or show a variable that was not captured"));
    o.set("reversible", J::Bool(true));
    o.set("sequence", J::Arr(slice.iter().map(render).collect()));
    o
}

/// A readable walkthrough, which is what a person reading in a terminal wants.
pub fn text(steps: &[Step], from: usize, count: usize) -> String {
    let mut out = String::new();
    let end = (from + count).min(steps.len());
    for st in &steps[from.min(steps.len())..end] {
        let pad = "  ".repeat(st.depth);
        let s = st.span;
        if st.kind == "enter" {
            let inputs = match &s.inputs {
                J::Null => "(values not recorded)".to_string(),
                v => v.get("values").map(|x| x.dump()).unwrap_or_else(|| v.dump()),
            };
            out.push_str(&format!("{:>4}  {}-> {} {}\n", st.index, pad, s.name, inputs));
        } else {
            let output = match &s.output {
                J::Null => if s.status == "error" { String::new() } else { "(not recorded)".into() },
                v => v.get("values").map(|x| x.dump()).unwrap_or_else(|| v.dump()),
            };
            let err = match &s.error {
                J::Null => String::new(),
                e => format!("  !! {} {}",
                    e.get("type").and_then(|x| x.as_str()).unwrap_or("error"),
                    e.get("message").and_then(|x| x.as_str()).unwrap_or("")),
            };
            let dur = s.duration_ms().map(|d| format!("{:.1}ms", d)).unwrap_or_else(|| "never finished".into());
            out.push_str(&format!("{:>4}  {}<- {} {} [{}]{}\n", st.index, pad, s.name, output, dur, err));
        }
    }
    out
}
