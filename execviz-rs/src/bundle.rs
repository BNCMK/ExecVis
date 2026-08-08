// ========================================================================
//  MANIFEST
// ========================================================================
//  script_name: bundle.rs
//  script_path: execviz-rs/src/bundle.rs
//  module_name: bundle
//  version: 0.53.1
//  description: A finding, packaged so somebody else can replay it.
//  kind: module
//  spec: internal
//  internal_dependencies: json, sha256, store
//  external_dependencies: 
//  features: bundle
//  api_version: execvis-v1.0.0
//  last_updated: 2026-08-07
// ========================================================================

//! A finding, packaged so somebody else can replay it.
//!
//! Everything needed to open the same investigation elsewhere: the capture, the
//! recorder records it was read against, the findings, the notes, the viewpoint,
//! and a statement of what the machine could and could not do.
//!
//! The first rule decides whether this feature is safe to ship at all.
//!
//! **Redacted by default.** A bundle is the thing somebody attaches to a public
//! issue. It carries what programs wrote, which on a real machine includes
//! credentials, tokens and customer data. Payloads are withheld unless asked
//! for, and the manifest states HOW MANY were withheld rather than quietly
//! producing a smaller file.

use crate::json::J;
use crate::store::Span;

// ========================================================================
// TYPES
// ========================================================================

pub struct Packed {
    pub manifest: J,
    pub withheld: usize,
    pub included: usize,
}

// ========================================================================
// INTERNALS
// ========================================================================

fn looks_secret(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    ["password", "passwd", "secret", "token", "key", "authorization", "auth",
     "cookie", "session", "credential", "bearer", "signature", "private"]
        .iter().any(|m| k.contains(m))
}

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

/// Builds the manifest and the redacted record set.
///
/// `with_payloads` is off unless the operator says otherwise, because the safe
/// default for a file that gets emailed is the one that cannot embarrass anybody.
pub fn pack(spans: &[Span], floor_ndjson: &str, viewpoint: Option<&str>,
            with_payloads: bool) -> (Packed, String, J) {

// ========================================================================
// THE FLOOR RECORDS, WITH PAYLOADS WITHHELD UNLESS ASKED
// ========================================================================
    let mut kept = String::new();
    let mut withheld = 0usize;
    let mut included = 0usize;
    for line in floor_ndjson.lines() {
        let v = match crate::json::parse(line) { Ok(v) => v, Err(_) => continue };
        if let (Some(_), false) = (v.get("log"), with_payloads) {
            // The record survives, its text does not: the shape of the capture
            // is what a reader needs, and the bytes are what gets somebody
            // fired. Withholding is marked so nobody mistakes it for silence.
            let mut o = v.clone();
            o.set("log", J::Str("<withheld: payloads are not included by default>".into()));
            o.set("withheld", J::Bool(true));
            kept.push_str(&o.dump());
            kept.push('\n');
            withheld += 1;
        } else {
            kept.push_str(line);
            kept.push('\n');
            included += 1;
        }
    }

// ========================================================================
// THE SPANS, WITH ATTRIBUTES THAT LOOK LIKE SECRETS REMOVED
// ========================================================================
    let mut attrs_removed = 0usize;
    let mut span_rows: Vec<J> = Vec::new();
    for s in spans {
        let mut a = s.attributes.clone();
        if !with_payloads {
            if let J::Obj(m) = &s.attributes {
                let mut clean = crate::json::J::obj();
                for (k, v) in m {
                    if looks_secret(k) {
                        clean.set(k, J::Str("<withheld>".into()));
                        attrs_removed += 1;
                    } else {
                        clean.set(k, v.clone());
                    }
                }
                a = clean;
            }
        }
        span_rows.push(J::Obj([
            ("span_id".to_string(), J::Str(s.span_id.clone())),
            ("trace_id".to_string(), J::Str(s.trace_id.clone())),
            ("parent_span_id".to_string(), match &s.parent_span_id {
                Some(p) => J::Str(p.clone()), None => J::Null }),
            ("name".to_string(), J::Str(s.name.clone())),
            ("kind".to_string(), J::Str(s.kind.clone())),
            ("start".to_string(), J::Num(s.start)),
            ("end".to_string(), match s.end { Some(e) => J::Num(e), None => J::Null }),
            ("status".to_string(), J::Str(s.status.clone())),
            ("host_id".to_string(), J::Str(s.host_id.clone())),
            ("attributes".to_string(), a),
        ].into_iter().collect()));
    }

// ========================================================================
// WHAT DID NOT TRAVEL, STATED
// ========================================================================
    let mut absent: Vec<J> = Vec::new();
    let mut note = |what: &str, n: usize, why: &str, absent: &mut Vec<J>| {
        if n > 0 {
            absent.push(J::Obj([
                ("what".to_string(), J::Str(what.to_string())),
                ("count".to_string(), J::Num(n as f64)),
                ("why".to_string(), J::Str(why.to_string())),
            ].into_iter().collect()));
        }
    };
    note("floor payloads", withheld,
         "a bundle is often attached to a public issue, and what programs write \
          includes credentials and customer data. Re-run with --with-payloads to \
          include them, having decided that is safe", &mut absent);
    note("span attributes", attrs_removed,
         "attribute names matching password, token, key, cookie, session, \
          credential, bearer, signature or private", &mut absent);
    absent.push(J::Obj([
        ("what".to_string(), J::Str("the machine itself".to_string())),
        ("why".to_string(), J::Str(
            "a bundle replays a record, not a program: it cannot evaluate an \
             unrecorded expression or enter an uninstrumented function".to_string())),
    ].into_iter().collect()));

    let manifest = J::Obj([
        ("format".to_string(), J::Str("execviz-bundle/1".to_string())),
        ("spans".to_string(), J::Num(span_rows.len() as f64)),
        ("recorder_records_included".to_string(), J::Num(included as f64)),
        ("floor_payloads_withheld".to_string(), J::Num(withheld as f64)),
        ("span_attributes_withheld".to_string(), J::Num(attrs_removed as f64)),
        ("payloads".to_string(), J::Str(
            if with_payloads { "INCLUDED because --with-payloads was given".to_string() }
            else { "withheld by default".to_string() })),
        ("viewpoint".to_string(), match viewpoint {
            Some(v) => J::Str(v.to_string()),
            None => J::Null }),
        ("not_included".to_string(), J::Arr(absent)),
        ("replay".to_string(), J::Str(
            "open the map, load spans.json, and use the viewpoint above so the \
             window and camera match what the sender was looking at".to_string())),
    ].into_iter().collect());

    let spans_doc = J::Obj([("spans".to_string(), J::Arr(span_rows))].into_iter().collect());
    (Packed { manifest: manifest.clone(), withheld, included }, kept, spans_doc)
}

/// A hash over the bundle's own contents, so a recipient can tell whether they
/// are looking at what was sent.
pub fn seal(manifest: &J, records: &str, spans: &J) -> String {
    let joined = format!("{}\n{}\n{}", manifest.dump(), records, spans.dump());
    crate::sha256::hex(&crate::sha256::sha256(joined.as_bytes()))
}
