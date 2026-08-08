// ========================================================================
//  MANIFEST
// ========================================================================
//  script_name: audit.rs
//  script_path: execviz-rs/src/audit.rs
//  module_name: audit
//  version: 0.53.1
//  description: The audit trail.
//  kind: module
//  spec: internal
//  internal_dependencies: json, store
//  external_dependencies: rusqlite
//  features: audit
//  api_version: execvis-v1.0.0
//  last_updated: 2026-08-07
// ========================================================================

//! The audit trail.
//!
//! A tool that records everything a program did, and records nothing about who
//! read that recording, holds one standard for its subject and another for
//! itself. Reads, exports and peer exchanges are appended here the same way
//! spans are appended to a capture.
use crate::json::J;
use crate::store::Store;
use rusqlite::params;

// ========================================================================
// CONSTANTS
// ========================================================================

pub const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS audit (
  seq     INTEGER PRIMARY KEY AUTOINCREMENT,
  t       REAL NOT NULL,
  account TEXT,
  action  TEXT NOT NULL,
  detail  TEXT,
  bytes   INTEGER,
  peer    TEXT
);
";

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

pub fn ensure(store: &Store) { let _ = store.conn.execute_batch(SCHEMA); }

/// Appends one entry. Failure to record is never allowed to fail the request
/// being recorded: an unavailable trail is a gap in the record, and refusing
/// service would be a worse answer than a gap.
pub fn record(store: &Store, account: Option<&str>, action: &str, detail: &str,
              bytes: usize, peer: Option<&str>) {
    ensure(store);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64()).unwrap_or(0.0);
    let _ = store.conn.execute(
        "INSERT INTO audit (t,account,action,detail,bytes,peer) VALUES (?1,?2,?3,?4,?5,?6)",
        params![now, account, action, detail, bytes as i64, peer]);
}

pub fn read(store: &Store, limit: usize) -> J {
    ensure(store);
    let mut o = J::obj();
    let rows: Vec<J> = store.conn
        .prepare("SELECT seq,t,account,action,detail,bytes,peer FROM audit ORDER BY seq DESC LIMIT ?1")
        .and_then(|mut st| {
            let it = st.query_map(params![limit as i64], |r| {
                let mut e = J::obj();
                e.set("seq", J::n(r.get::<_, i64>(0)? as f64));
                e.set("t", J::n(r.get::<_, f64>(1)?));
                e.set("account", match r.get::<_, Option<String>>(2)? {
                    Some(a) => J::s(&a), None => J::Null });
                e.set("action", J::s(&r.get::<_, String>(3)?));
                e.set("detail", J::s(&r.get::<_, String>(4).unwrap_or_default()));
                e.set("bytes", J::n(r.get::<_, i64>(5).unwrap_or(0) as f64));
                e.set("peer", match r.get::<_, Option<String>>(6)? {
                    Some(a) => J::s(&a), None => J::Null });
                Ok(e)
            })?;
            Ok(it.filter_map(|x| x.ok()).collect::<Vec<J>>())
        }).unwrap_or_default();
    o.set("note", J::s("append-only from the tool's side: anything that can quietly edit this is not a trail"));
    o.set("entries", J::n(rows.len() as f64));
    o.set("log", J::Arr(rows));
    o
}
