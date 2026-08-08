// ========================================================================
//  MANIFEST
// ========================================================================
//  script_name: notes.rs
//  script_path: execviz-rs/src/notes.rs
//  module_name: notes
//  version: 0.53.1
//  description: Saved views, notes and reports.
//  kind: module
//  spec: internal
//  internal_dependencies: egress, find, json, rollup, skew, stats, store, watch
//  external_dependencies: rusqlite
//  features: notes
//  api_version: execvis-v1.0.0
//  last_updated: 2026-08-07
// ========================================================================

//! Saved views, notes and reports.
//!
//! All of it lives beside the capture rather than in a browser: a finding that
//! lives in one person's tab is not a finding anyone else has.
use crate::json::J;
use crate::store::{Span, Store};
use rusqlite::params;

// ========================================================================
// CONSTANTS
// ========================================================================

pub const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS views (
  name    TEXT PRIMARY KEY,
  state   TEXT NOT NULL,        -- the permalink fragment, opaque to the store
  author  TEXT,
  created REAL NOT NULL,
  note    TEXT
);
CREATE TABLE IF NOT EXISTS notes (
  id      INTEGER PRIMARY KEY AUTOINCREMENT,
  span_id TEXT,                 -- null means the note is about the capture
  body    TEXT NOT NULL,
  author  TEXT,
  created REAL NOT NULL
);
";

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

pub fn ensure(store: &Store) { let _ = store.conn.execute_batch(SCHEMA); }

// ========================================================================
// INTERNALS
// ========================================================================

fn now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

pub fn save_view(store: &Store, name: &str, state: &str, author: Option<&str>, note: Option<&str>)
    -> rusqlite::Result<()> {
    ensure(store);
    store.conn.execute(
        "INSERT OR REPLACE INTO views (name,state,author,created,note) VALUES (?1,?2,?3,?4,?5)",
        params![name, state, author, now(), note])?;
    Ok(())
}

pub fn views(store: &Store) -> J {
    ensure(store);
    let rows: Vec<J> = store.conn.prepare("SELECT name,state,author,created,note FROM views ORDER BY created DESC")
        .and_then(|mut st| {
            let it = st.query_map([], |r| {
                let mut e = J::obj();
                e.set("name", J::s(&r.get::<_, String>(0)?));
                e.set("state", J::s(&r.get::<_, String>(1)?));
                e.set("author", match r.get::<_, Option<String>>(2)? {
                    Some(a) => J::s(&a), None => J::Null });
                e.set("created", J::n(r.get::<_, f64>(3)?));
                e.set("note", match r.get::<_, Option<String>>(4)? {
                    Some(a) => J::s(&a), None => J::Null });
                Ok(e)
            })?;
            Ok(it.filter_map(|x| x.ok()).collect::<Vec<J>>())
        }).unwrap_or_default();
    let mut o = J::obj();
    o.set("views", J::Arr(rows));
    o
}

/// A note carries who wrote it and when.
///
/// An unattributed annotation on shared evidence differs from none: it invites
/// a reader to treat an opinion as part of the record.
pub fn add_note(store: &Store, span_id: Option<&str>, body: &str, author: Option<&str>)
    -> rusqlite::Result<()> {
    ensure(store);
    store.conn.execute(
        "INSERT INTO notes (span_id,body,author,created) VALUES (?1,?2,?3,?4)",
        params![span_id, body, author, now()])?;
    Ok(())
}

pub fn notes(store: &Store, span_id: Option<&str>) -> J {
    ensure(store);
    let sql = match span_id {
        Some(_) => "SELECT id,span_id,body,author,created FROM notes WHERE span_id=?1 ORDER BY created",
        None => "SELECT id,span_id,body,author,created FROM notes ORDER BY created",
    };
    let map = |r: &rusqlite::Row| -> rusqlite::Result<J> {
        let mut e = J::obj();
        e.set("id", J::n(r.get::<_, i64>(0)? as f64));
        e.set("span_id", match r.get::<_, Option<String>>(1)? { Some(a) => J::s(&a), None => J::Null });
        e.set("body", J::s(&r.get::<_, String>(2)?));
        e.set("author", match r.get::<_, Option<String>>(3)? { Some(a) => J::s(&a), None => J::Null });
        e.set("created", J::n(r.get::<_, f64>(4)?));
        Ok(e)
    };
    let rows: Vec<J> = match span_id {
        Some(id) => store.conn.prepare(sql).and_then(|mut st| {
            let it = st.query_map(params![id], map)?;
            Ok(it.filter_map(|x| x.ok()).collect())
        }).unwrap_or_default(),
        None => store.conn.prepare(sql).and_then(|mut st| {
            let it = st.query_map([], map)?;
            Ok(it.filter_map(|x| x.ok()).collect())
        }).unwrap_or_default(),
    };
    let mut o = J::obj();
    o.set("notes", J::Arr(rows));
    o
}

/// Assembles the investigation as text.
///
/// It states what was measured and stops there. The writing up is a person's
/// job, and a tool that supplies the conclusion invites it to be believed:
/// everything here is a figure computed elsewhere in this program, gathered so
/// nobody has to copy it out by hand.
pub fn report(store: &Store, spans: &[Span], from: Option<f64>, to: Option<f64>) -> String {
    let window: Vec<&Span> = spans.iter().filter(|s| {
        from.map(|f| s.end.unwrap_or(s.start) >= f).unwrap_or(true)
            && to.map(|t| s.start <= t).unwrap_or(true)
    }).collect();
    let owned: Vec<Span> = window.iter().map(|s| (*s).clone()).collect();

    let mut out = String::new();
    out.push_str("# Capture report\n\n");
    if let (Some(f), Some(t)) = (from, to) {
        out.push_str(&format!("Window: {:.3} to {:.3} ({:.1} s)\n\n", f, t, t - f));
    }
    out.push_str(&format!("Spans in scope: {} of {}\n", owned.len(), spans.len()));

    let hosts: std::collections::BTreeSet<&str> = owned.iter().map(|s| s.host_id.as_str()).collect();
    out.push_str(&format!("Hosts: {}\n", hosts.into_iter().collect::<Vec<_>>().join(", ")));
    let errors = owned.iter().filter(|s| s.status == "error").count();
    let open = owned.iter().filter(|s| s.end.is_none()).count();
    out.push_str(&format!("Failed: {}   Still open: {}\n\n", errors, open));

    // whether the capture can be trusted comes before anything derived from it
    out.push_str("## The record\n\n");
    let integ = crate::egress::integrity(store, &owned);
    out.push_str(&format!("- sound: {}\n", integ.get("sound") == Some(&J::Bool(true))));
    // whether the record is complete is a different question from whether it is
    // damaged, and a reader needs both before trusting any figure below
    if integ.get("complete") == Some(&J::Bool(false)) {
        let lost = integ.get("spans_never_delivered").and_then(|x| x.as_f64()).unwrap_or(0.0);
        out.push_str(&format!("- INCOMPLETE: {} span(s) were dropped before delivery, so every count below is a lower bound\n", lost));
    } else {
        out.push_str("- complete: no sender reported dropping spans\n");
    }
    out.push_str(&format!("- seal: {}\n", crate::rollup::seal(&owned)));
    if let Some((rule, rate)) = crate::watch::sampling(store) {
        out.push_str(&format!("- SAMPLED: {} at rate {}; counts below are estimates\n", rule, rate));
    }
    let sk = crate::skew::analyse(&owned);
    let impossible: usize = sk.iter().map(|p| p.violations).sum();
    if impossible > 0 {
        out.push_str(&format!("- clocks disagree: {} impossible crossings between hosts\n", impossible));
    }
    out.push('\n');

    out.push_str("## What took the time\n\n");
    let dist = crate::stats::distributions(&owned, 1);
    for d in dist.iter().take(8) {
        out.push_str(&format!("- {}: n={} median {:.1}ms p95 {:.1}ms{}\n",
            d.name, d.count, d.median, d.p95,
            if d.errors > 0 { format!(" ({} failed)", d.errors) } else { String::new() }));
    }
    if let Some(root) = owned.iter().filter(|s| s.parent_span_id.is_none())
        .max_by(|a, b| a.duration_ms().unwrap_or(0.0).partial_cmp(&b.duration_ms().unwrap_or(0.0)).unwrap()) {
        let path = crate::find::critical_path(&owned, &root.span_id);
        out.push_str(&format!("\nCritical path: {}\n",
            path.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(" -> ")));
    }

    let n = notes(store, None);
    if let Some(arr) = n.get("notes").and_then(|x| x.as_arr()) {
        if !arr.is_empty() {
            out.push_str("\n## Notes\n\n");
            for e in arr {
                let who = e.get("author").and_then(|x| x.as_str()).unwrap_or("unattributed");
                let body = e.get("body").and_then(|x| x.as_str()).unwrap_or("");
                out.push_str(&format!("- ({}) {}\n", who, body));
            }
        }
    }
    out.push_str("\n---\nEvery figure above was measured. What it means is not stated here.\n");
    out
}
