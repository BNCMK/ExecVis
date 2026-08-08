// ========================================================================
//  MANIFEST
// ========================================================================
//  script_name: logs.rs
//  script_path: execviz-rs/src/logs.rs
//  module_name: logs
//  version: 0.53.1
//  description: The log console. Logs live on spans, so filtering and sorting are queries over the trace rather than text munging over a file. Every field a conventional log line has to carry in its own text is already structure here.
//  kind: module
//  spec: internal
//  internal_dependencies: json, store
//  external_dependencies: std
//  features: logs, console
//  api_version: execvis-v1.0.0
//  last_updated: 2026-08-07
// ========================================================================

//! The log console. Logs live on spans, so filtering and sorting are
//! queries over the trace rather than text munging over a file. Every field a
//! conventional log line has to carry in its own text is already structure here.

use std::collections::BTreeMap;
use crate::json::J;
use crate::store::Span;

// ========================================================================
// TYPES
// ========================================================================

/// Folding repeated lines. A group states how many it stands for and can be
/// unfolded: hiding repetition is a reading aid, discarding it is data loss.
pub struct Folded { pub line: Line, pub count: usize }

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

/// A tally of the noise before any of it is read.
pub fn counts(lines: &[Line]) -> J {
    let mut by_level: BTreeMap<&str, i64> = BTreeMap::new();
    let mut by_host: BTreeMap<&str, i64> = BTreeMap::new();
    let mut by_domain: BTreeMap<&str, i64> = BTreeMap::new();
    for l in lines {
        *by_level.entry(l.level.as_str()).or_insert(0) += 1;
        *by_host.entry(l.host.as_str()).or_insert(0) += 1;
        *by_domain.entry(l.domain.as_str()).or_insert(0) += 1;
    }
    let obj = |m: BTreeMap<&str, i64>| {
        let mut o = J::obj();
        for (k, v) in m { o.set(k, J::n(v as f64)); }
        o
    };
    let mut o = J::obj();
    o.set("total", J::n(lines.len() as f64));
    o.set("by_level", obj(by_level));
    o.set("by_host", obj(by_host));
    o.set("by_domain", obj(by_domain));
    o
}

/// Folds adjacent-in-time lines that say the same thing on the same span.
///
/// Equal lines keep the order they were recorded in, so two runs of one query
/// look the same. A sort that reshuffles equal rows teaches a reader to distrust
/// the tool for no reason.
pub fn fold(lines: Vec<Line>) -> Vec<Folded> {
    let mut out: Vec<Folded> = Vec::new();
    for l in lines {
        match out.last_mut() {
            Some(prev) if prev.line.msg == l.msg
                       && prev.line.level == l.level
                       && prev.line.span_id == l.span_id => { prev.count += 1; }
            _ => out.push(Folded { line: l, count: 1 }),
        }
    }
    out
}

// ========================================================================
// TYPES
// ========================================================================

pub struct Filter<'a> {
    pub host: Option<&'a str>,
    pub domain: Option<&'a str>,
    pub span: Option<&'a str>,
    pub level: Option<&'a str>,
    pub contains: Option<&'a str>,
    pub since: Option<f64>,
    pub until: Option<f64>,
    pub errors_only: bool,
    pub under: Option<&'a str>,     // causal ancestry: everything beneath a span
    pub sort: &'a str,              // time | level | domain | span
    pub group: Option<&'a str>,     // span | domain | host | level
    pub limit: usize,
}

pub struct Line {
    pub t: f64,
    pub level: String,
    pub msg: String,
    pub span_id: String,
    pub span_name: String,
    pub domain: String,
    pub host: String,
    pub status: String,
}

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

pub fn severity(level: &str) -> u8 {
    match level {
        "critical" | "fatal" => 5,
        "error" | "stderr" => 4,
        "warning" | "warn" => 3,
        "info" | "stdout" => 2,
        "debug" => 1,
        _ => 0,
    }
}

// ========================================================================
// INTERNALS
// ========================================================================

/// Everything causally beneath a span, so a single request's logs can be pulled
/// out of a noisy system without any request id having been logged.
fn descendants(spans: &[Span], root: &str) -> Vec<String> {
    let mut out = vec![root.to_string()];
    let mut added = true;
    while added {
        added = false;
        for s in spans {
            if let Some(p) = &s.parent_span_id {
                if out.iter().any(|x| x == p) && !out.iter().any(|x| x == &s.span_id) {
                    out.push(s.span_id.clone());
                    added = true;
                }
            }
        }
    }
    out
}

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

pub fn collect(spans: &[Span], f: &Filter) -> Vec<Line> {
    let subtree = f.under.map(|r| descendants(spans, r));
    let mut lines = Vec::new();
    for s in spans {
        if let Some(h) = f.host { if s.host_id != h { continue; } }
        if let Some(d) = f.domain { if s.domain.as_deref() != Some(d) { continue; } }
        if let Some(sp) = f.span {
            if s.span_id != sp && !s.name.contains(sp) { continue; }
        }
        if let Some(t) = &subtree { if !t.contains(&s.span_id) { continue; } }
        if f.errors_only && s.status != "error" { continue; }
        if let J::Arr(evs) = &s.events {
            for e in evs {
                let level = e.get("level").and_then(|x| x.as_str()).unwrap_or("info");
                let msg = e.get("msg").and_then(|x| x.as_str()).unwrap_or("");
                let t = e.get("t").and_then(|x| x.as_f64()).unwrap_or(s.start);
                if let Some(l) = f.level { if severity(level) < severity(l) { continue; } }
                if let Some(c) = f.contains {
                    if !msg.to_lowercase().contains(&c.to_lowercase()) { continue; }
                }
                if let Some(a) = f.since { if t < a { continue; } }
                if let Some(b) = f.until { if t > b { continue; } }
                lines.push(Line {
                    t, level: level.to_string(), msg: msg.to_string(),
                    span_id: s.span_id.clone(), span_name: s.name.clone(),
                    domain: s.domain.clone().unwrap_or_else(|| "-".into()),
                    host: s.host_id.clone(), status: s.status.clone(),
                });
            }
        }
    }
    // sort_by is stable, so lines equal on the key keep the order they were
    // recorded in and the same query always reads the same way
    match f.sort {
        "level" => lines.sort_by(|a, b| severity(&b.level).cmp(&severity(&a.level))
            .then(a.t.partial_cmp(&b.t).unwrap())),
        "domain" => lines.sort_by(|a, b| a.domain.cmp(&b.domain)
            .then(a.t.partial_cmp(&b.t).unwrap())),
        "span" => lines.sort_by(|a, b| a.span_name.cmp(&b.span_name)
            .then(a.t.partial_cmp(&b.t).unwrap())),
        "host" => lines.sort_by(|a, b| a.host.cmp(&b.host)
            .then(a.t.partial_cmp(&b.t).unwrap())),
        _ => lines.sort_by(|a, b| a.t.partial_cmp(&b.t).unwrap()),
    }
    lines.truncate(f.limit);
    lines
}

pub fn folded_json(groups: &[Folded]) -> J {
    let mut o = J::obj();
    o.set("groups", J::n(groups.len() as f64));
    o.set("lines", J::n(groups.iter().map(|g| g.count).sum::<usize>() as f64));
    o.set("rows", J::Arr(groups.iter().map(|g| {
        let mut e = J::obj();
        e.set("t", J::n(g.line.t));
        e.set("level", J::s(&g.line.level));
        e.set("msg", J::s(&g.line.msg));
        e.set("span", J::s(&g.line.span_name));
        e.set("domain", J::s(&g.line.domain));
        e.set("host", J::s(&g.line.host));
        e.set("repeats", J::n(g.count as f64));
        e
    }).collect()));
    o
}

pub fn to_json(lines: &[Line]) -> J {
    let mut o = J::obj();
    o.set("count", J::n(lines.len() as f64));
    o.set("lines", J::Arr(lines.iter().map(|l| {
        let mut e = J::obj();
        e.set("t", J::n(l.t));
        e.set("level", J::s(&l.level));
        e.set("msg", J::s(&l.msg));
        e.set("span", J::s(&l.span_name));
        e.set("span_id", J::s(&l.span_id));
        e.set("domain", J::s(&l.domain));
        e.set("host", J::s(&l.host));
        e.set("span_status", J::s(&l.status));
        e
    }).collect()));
    o
}

/// Human output. Columns are aligned so the eye can scan one field at a time,
/// and the span is shown on every line because that is the correlation a
/// conventional log makes you reconstruct.
pub fn render(lines: &[Line], group: Option<&str>, t0: f64) -> String {
    let mut out = String::new();
    let head = format!("{:>9}  {:<8} {:<20} {:<14} {:<10} {}\n",
        "t(ms)", "level", "span", "domain", "host", "message");
    let rule = "-".repeat(108) + "\n";
    match group {
        Some(g) => {
            let key = |l: &Line| match g {
                "domain" => l.domain.clone(),
                "host" => l.host.clone(),
                "level" => l.level.clone(),
                _ => format!("{}  [{}]", l.span_name, l.status),
            };
            let mut seen: Vec<String> = Vec::new();
            for l in lines { let k = key(l); if !seen.contains(&k) { seen.push(k); } }
            for k in seen {
                let group_lines: Vec<&Line> = lines.iter().filter(|l| key(l) == k).collect();
                out.push_str(&format!("\n=== {}  ({} lines) ===\n", k, group_lines.len()));
                for l in group_lines {
                    out.push_str(&format!("{:>9.1}  {:<8} {}\n",
                        (l.t - t0) * 1000.0, l.level, l.msg));
                }
            }
        }
        None => {
            out.push_str(&head);
            out.push_str(&rule);
            for l in lines {
                out.push_str(&format!("{:>9.1}  {:<8} {:<20} {:<14} {:<10} {}\n",
                    (l.t - t0) * 1000.0,
                    l.level,
                    trunc(&l.span_name, 20),
                    trunc(&l.domain, 14),
                    trunc(&l.host, 10),
                    l.msg));
            }
        }
    }
    out
}

// ========================================================================
// INTERNALS
// ========================================================================

fn trunc(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() }
    else { s.chars().take(n.saturating_sub(1)).collect::<String>() + "\u{2026}" }
}
