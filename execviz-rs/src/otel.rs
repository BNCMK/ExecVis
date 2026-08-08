// ========================================================================
//  MANIFEST
// ========================================================================
//  script_name: otel.rs
//  script_path: execviz-rs/src/otel.rs
//  module_name: otel
//  version: 0.53.1
//  description: Export to the OpenTelemetry span model.
//  kind: module
//  spec: internal
//  internal_dependencies: json, sha256, store
//  external_dependencies: 
//  features: otel
//  api_version: execvis-v1.0.0
//  last_updated: 2026-08-07
// ========================================================================

//! Export to the OpenTelemetry span model.
//!
//! A capture that nothing else can read is one somebody must adopt wholesale or
//! not at all. This is a compatibility surface and not a differentiator, so it
//! is deliberately plain.
//!
//! What matters here is the leaving: this project records things the OTLP span
//! model has no home for, and those are NAMED on the way out rather than dropped
//! quietly. A reader of the exported file can see exactly what did not survive
//! the translation, which is the same rule the rest of the tool follows.

use crate::json::J;
use crate::store::Span;

// ========================================================================
// INTERNALS
// ========================================================================

fn hex_id(s: &str, width: usize) -> String {
    // OTLP ids are fixed-width hex. The project's own ids are identifiers, not
    // necessarily hex, so they are hashed rather than reinterpreted: silently
    // truncating an id would make two different spans collide.
    let h = crate::sha256::hex(&crate::sha256::sha256(s.as_bytes()));
    h.chars().take(width).collect()
}

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

pub fn export(spans: &[Span]) -> J {
    let mut out: Vec<J> = Vec::new();
    let mut dropped_lifecycle = 0usize;
    let mut dropped_links = 0usize;
    let mut dropped_origin = 0usize;
    let mut dropped_clock = 0usize;
    let mut open_spans = 0usize;

    for s in spans {
        let mut attrs: Vec<J> = Vec::new();
        let mut push_attr = |k: &str, v: J, attrs: &mut Vec<J>| {
            attrs.push(J::Obj([
                ("key".to_string(), J::Str(k.to_string())),
                ("value".to_string(), v),
            ].into_iter().collect()));
        };
        if let J::Obj(m) = &s.attributes {
            for (k, v) in m {
                let val = match v {
                    J::Str(x) => J::Obj([("stringValue".to_string(), J::Str(x.clone()))].into_iter().collect()),
                    J::Num(n) => J::Obj([("doubleValue".to_string(), J::Num(*n))].into_iter().collect()),
                    J::Bool(b) => J::Obj([("boolValue".to_string(), J::Bool(*b))].into_iter().collect()),
                    other => J::Obj([("stringValue".to_string(), J::Str(other.dump()))].into_iter().collect()),
                };
                push_attr(k, val, &mut attrs);
            }
        }
        // The project's own kind has no OTLP equivalent, so it is carried as an
        // attribute rather than mapped onto SpanKind, which means something else.
        push_attr("execviz.kind", J::Obj(
            [("stringValue".to_string(), J::Str(s.kind.clone()))].into_iter().collect()), &mut attrs);
        push_attr("execviz.domain", J::Obj(
            [("stringValue".to_string(), J::Str(s.domain.clone().unwrap_or_default()))].into_iter().collect()), &mut attrs);

        if s.lifecycle.as_arr().map(|a| !a.is_empty()).unwrap_or(false) { dropped_lifecycle += 1; }
        if !s.links.is_empty() { dropped_links += 1; }
        if s.origin != "semantic" { dropped_origin += 1; }
        if s.clock_source.is_some() { dropped_clock += 1; }

        let end_ns = match s.end {
            Some(e) => (e * 1e9) as u64,
            None => { open_spans += 1; 0 }
        };

        let links: Vec<J> = s.links.iter().map(|l| J::Obj([
            ("traceId".to_string(), J::Str(hex_id(&s.trace_id, 32))),
            ("spanId".to_string(), J::Str(hex_id(l, 16))),
        ].into_iter().collect())).collect();

        out.push(J::Obj([
            ("traceId".to_string(), J::Str(hex_id(&s.trace_id, 32))),
            ("spanId".to_string(), J::Str(hex_id(&s.span_id, 16))),
            ("parentSpanId".to_string(), match &s.parent_span_id {
                Some(p) => J::Str(hex_id(p, 16)), None => J::Str(String::new()) }),
            ("name".to_string(), J::Str(s.name.clone())),
            ("startTimeUnixNano".to_string(), J::Num((s.start * 1e9) as u64 as f64)),
            ("endTimeUnixNano".to_string(), J::Num(end_ns as f64)),
            ("status".to_string(), J::Obj([
                ("code".to_string(), J::Num(if s.status == "error" { 2.0 } else { 1.0 })),
            ].into_iter().collect())),
            ("attributes".to_string(), J::Arr(attrs)),
            ("links".to_string(), J::Arr(links)),
        ].into_iter().collect()));
    }

    // What did not survive. Stated, because a translation that loses things
    // silently is how two systems come to disagree without anyone noticing.
    let mut lost: Vec<J> = Vec::new();
    let mut note = |what: &str, n: usize, why: &str, lost: &mut Vec<J>| {
        if n > 0 {
            lost.push(J::Obj([
                ("field".to_string(), J::Str(what.to_string())),
                ("spans_affected".to_string(), J::Num(n as f64)),
                ("why".to_string(), J::Str(why.to_string())),
            ].into_iter().collect()));
        }
    };
    note("lifecycle", dropped_lifecycle,
         "claimed and released are states of a span over time; the OTLP model has no place for them", &mut lost);
    note("links", dropped_links,
         "carried as OTLP links, but the fan-in meaning does not survive: a reader sees an association, not a join", &mut lost);
    note("origin", dropped_origin,
         "whether a span was semantic or observed is not expressible, so an observed span is indistinguishable from an instrumented one", &mut lost);
    note("clock_source", dropped_clock,
         "which clock a host stamped with makes skew analysis possible, and has no field here", &mut lost);
    note("open spans", open_spans,
         "a span with no end is exported with endTimeUnixNano 0, because the model has no way to say 'still running'; an unfinished span does not survive", &mut lost);

    J::Obj([
        ("resourceSpans".to_string(), J::Arr(vec![J::Obj([
            ("scopeSpans".to_string(), J::Arr(vec![J::Obj([
                ("scope".to_string(), J::Obj([
                    ("name".to_string(), J::Str("execviz".to_string())),
                ].into_iter().collect())),
                ("spans".to_string(), J::Arr(out)),
            ].into_iter().collect())])),
        ].into_iter().collect())])),
        ("execviz_not_exported".to_string(), J::Arr(lost)),
        ("note".to_string(), J::Str(
            "Fields this capture carries that the OTLP span model has no home \
             for are listed rather than dropped quietly.".to_string())),
    ].into_iter().collect())
}
