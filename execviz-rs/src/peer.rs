// ========================================================================
//  MANIFEST
// ========================================================================
//  script_name: peer.rs
//  script_path: execviz-rs/src/peer.rs
//  module_name: peer
//  version: 0.53.1
//  description: Peering. Every instance is the same program; what separates two installations is configuration and consent.
//  kind: module
//  spec: internal
//  internal_dependencies: json, store
//  external_dependencies: rusqlite
//  features: peer
//  api_version: execvis-v1.0.0
//  last_updated: 2026-08-07
// ========================================================================

//! Peering. Every instance is the same program; what separates two
//! installations is configuration and consent.
//!
//! A peer row records one direction of a link. `inbound` is a request another
//! instance made of us, which we approve before it may read. `outbound` is a
//! link we asked for, which stays pending until they approve, and which we then
//! pull from on an interval.
use crate::json::J;
use crate::store::{Span, Store};
use rusqlite::params;

// ========================================================================
// CONSTANTS
// ========================================================================

pub const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS peers (
  peer_id   TEXT NOT NULL,
  direction TEXT NOT NULL,            -- inbound | outbound
  url       TEXT,
  status    TEXT NOT NULL,            -- pending | approved | revoked
  cursor    REAL NOT NULL DEFAULT 0,  -- last position pulled from this peer
  last_seen REAL,
  note      TEXT,
  api_key   TEXT,                     -- outbound: the key WE present to them
  PRIMARY KEY (peer_id, direction)
);
ALTER TABLE peers ADD COLUMN api_key TEXT;
";

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

/// The schema is applied twice: once as CREATE IF NOT EXISTS, and once as an
/// ALTER that fails harmlessly on a store that already has the column. A store
/// written before peers carried credentials is still readable, which matters
/// because a peer may be older than the instance talking to it.
pub fn ensure_lenient(store: &Store) {
    let _ = store.conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS peers (peer_id TEXT NOT NULL, direction TEXT NOT NULL, \
         url TEXT, status TEXT NOT NULL, cursor REAL NOT NULL DEFAULT 0, last_seen REAL, \
         note TEXT, api_key TEXT, PRIMARY KEY (peer_id, direction));");
    let _ = store.conn.execute_batch("ALTER TABLE peers ADD COLUMN api_key TEXT;");
}

pub fn ensure(store: &Store) -> rusqlite::Result<()> { ensure_lenient(store); Ok(()) }

// ========================================================================
// TYPES
// ========================================================================

#[derive(Debug)]
pub struct Peer {
    pub peer_id: String,
    pub direction: String,
    pub url: Option<String>,
    pub status: String,
    pub cursor: f64,
    /// Outbound only: the credential this instance presents to that peer, so a
    /// link proves who it is rather than asserting a name about itself.
    pub api_key: Option<String>,
}

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

pub fn list(store: &Store) -> rusqlite::Result<Vec<Peer>> {
    let mut st = store.conn.prepare(
        "SELECT peer_id,direction,url,status,cursor,api_key FROM peers ORDER BY direction,peer_id")?;
    let rows = st.query_map([], |r| Ok(Peer {
        peer_id: r.get(0)?, direction: r.get(1)?, url: r.get(2)?,
        status: r.get(3)?, cursor: r.get(4)?, api_key: r.get(5).ok(),
    }))?;
    let mut out = Vec::new();
    for x in rows { out.push(x?); }
    Ok(out)
}

pub fn upsert(store: &Store, peer_id: &str, direction: &str, url: Option<&str>,
              status: &str) -> rusqlite::Result<()> {
    store.conn.execute(
        "INSERT INTO peers (peer_id,direction,url,status,last_seen) VALUES (?1,?2,?3,?4,?5) \
         ON CONFLICT(peer_id,direction) DO UPDATE SET url=COALESCE(excluded.url,peers.url), \
         status=excluded.status, last_seen=excluded.last_seen",
        params![peer_id, direction, url, status, now()])?;
    Ok(())
}

pub fn set_status(store: &Store, peer_id: &str, direction: &str, status: &str)
    -> rusqlite::Result<usize> {
    store.conn.execute("UPDATE peers SET status=?1 WHERE peer_id=?2 AND direction=?3",
        params![status, peer_id, direction])
}

/// Records the credential to present to an outbound peer.
pub fn set_key(store: &Store, peer_id: &str, key: &str) -> rusqlite::Result<usize> {
    store.conn.execute("UPDATE peers SET api_key=?1 WHERE peer_id=?2 AND direction='outbound'",
        params![key, peer_id])
}

pub fn set_cursor(store: &Store, peer_id: &str, cursor: f64) -> rusqlite::Result<()> {
    store.conn.execute("UPDATE peers SET cursor=?1, last_seen=?2 WHERE peer_id=?3 AND direction='outbound'",
        params![cursor, now(), peer_id])?;
    Ok(())
}

pub fn is_approved(store: &Store, peer_id: &str, direction: &str) -> bool {
    store.conn.query_row(
        "SELECT status FROM peers WHERE peer_id=?1 AND direction=?2",
        params![peer_id, direction], |r| r.get::<_, String>(0))
        .map(|s| s == "approved").unwrap_or(false)
}

pub fn now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

/// Spans this instance is willing to hand to an approved peer: everything that
/// changed at or after the peer's cursor. A span that completes later moves
/// forward in this ordering and is therefore sent again, which is what keeps a
/// two-phase write correct across a link.
pub fn since(spans: &[Span], cursor: f64, limit: usize) -> Vec<&Span> {
    let mut out: Vec<&Span> = spans.iter()
        .filter(|s| s.end.unwrap_or(s.start) >= cursor)
        .collect();
    out.sort_by(|a, b| a.end.unwrap_or(a.start).partial_cmp(&b.end.unwrap_or(b.start)).unwrap());
    out.truncate(limit);
    out
}

pub fn watermark(sent: &[&Span]) -> f64 {
    sent.iter().map(|s| s.end.unwrap_or(s.start)).fold(0.0f64, f64::max)
}

pub fn to_json(peers: &[Peer]) -> J {
    let mut o = J::obj();
    o.set("peers", J::Arr(peers.iter().map(|p| {
        let mut e = J::obj();
        e.set("peer_id", J::s(&p.peer_id));
        e.set("direction", J::s(&p.direction));
        e.set("url", match &p.url { Some(u) => J::s(u), None => J::Null });
        e.set("status", J::s(&p.status));
        e.set("cursor", J::n(p.cursor));
        // the key itself never appears in a listing, only whether one is held
        e.set("credential", J::Bool(p.api_key.is_some()));
        e
    }).collect()));
    o
}
