// ========================================================================
//  MANIFEST
// ========================================================================
//  script_name: auth.rs
//  script_path: execviz-rs/src/auth.rs
//  module_name: auth
//  version: 0.53.1
//  description: Accounts, sessions and credentials.
//  kind: module
//  spec: internal
//  internal_dependencies: json, sha256, store
//  external_dependencies: rusqlite, std
//  features: auth
//  api_version: execvis-v1.0.0
//  last_updated: 2026-08-07
// ========================================================================

//! Accounts, sessions and credentials.
//!
//! Three credential types, because there are three kinds of caller: a person at
//! a browser, a person who already holds a trusted SSH key on this machine, and
//! a program. Each is stored as a verifier, never as the secret itself, and all
//! three converge on one session so the rest of the system has a single notion
//! of "this request is allowed".
use crate::json::J;
use crate::sha256::{constant_time_eq, hex, pbkdf2, sha256};
use crate::store::Store;
use rusqlite::params;
use std::collections::HashMap;
use std::sync::Mutex;

// ========================================================================
// CONSTANTS
// ========================================================================

pub const ITERATIONS: u32 = 210_000;

const SESSION_SECS: f64 = 12.0 * 3600.0;

pub const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS accounts (
  name     TEXT PRIMARY KEY,
  salt     TEXT,
  verifier TEXT,               -- pbkdf2(password); absent for a key-only account
  created  REAL NOT NULL,
  role     TEXT NOT NULL DEFAULT 'admin'   -- admin | viewer
);
CREATE TABLE IF NOT EXISTS ssh_keys (
  account   TEXT NOT NULL,
  key_type  TEXT NOT NULL,
  key_blob  TEXT NOT NULL,     -- the public key, which is not a secret
  label     TEXT,
  added     REAL NOT NULL,
  PRIMARY KEY (account, key_blob)
);
CREATE TABLE IF NOT EXISTS api_keys (
  key_id   TEXT PRIMARY KEY,
  account  TEXT NOT NULL,
  hash     TEXT NOT NULL,      -- the key is shown once and never stored
  label    TEXT,
  created  REAL NOT NULL,
  revoked  INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS sessions (
  token_hash TEXT PRIMARY KEY,
  account    TEXT NOT NULL,
  method     TEXT NOT NULL,
  expires    REAL NOT NULL
);
";

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

pub fn ensure(store: &Store) -> rusqlite::Result<()> {
    store.conn.execute_batch(SCHEMA)?;
    // a store made before roles existed keeps working, and its accounts are
    // admins because that is what they were when they were created
    let _ = store.conn.execute_batch("ALTER TABLE accounts ADD COLUMN role TEXT NOT NULL DEFAULT 'admin';");
    Ok(())
}

/// Two roles, because the common case is a person who needs to look and must not
/// be able to change anything (spec 5.6, gap 34).
///
/// A viewer may read a capture and may not create accounts, issue keys, approve
/// peers, trim, or ingest. All-or-nothing access forces every reader to be an
/// administrator, which is how a debugging tool becomes a way in.
pub fn role_of(store: &Store, account: &str) -> String {
    store.conn.query_row("SELECT role FROM accounts WHERE name=?1", params![account],
        |r| r.get::<_, String>(0)).unwrap_or_else(|_| "admin".into())
}

pub fn may_write(role: &str) -> bool { role == "admin" }

pub fn now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

/// Random bytes from the system, not from a seeded generator: a predictable
/// session token is the same as no session token.
pub fn random_hex(n: usize) -> String {
    use std::io::Read;
    let mut buf = vec![0u8; n];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") { let _ = f.read_exact(&mut buf); }
    hex(&buf)
}

pub fn any_account(store: &Store) -> bool {
    store.conn.query_row("SELECT COUNT(*) FROM accounts", [], |r| r.get::<_, i64>(0))
        .map(|n| n > 0).unwrap_or(false)
}


pub fn create_account_with_role(store: &Store, name: &str, password: Option<&str>, role: &str)
    -> rusqlite::Result<()> {
    let (salt, verifier) = match password {
        Some(p) => { let s = random_hex(16); let v = hex(&pbkdf2(p.as_bytes(), s.as_bytes(), ITERATIONS)); (Some(s), Some(v)) }
        None => (None, None),   // a key-only account is legitimate
    };
    store.conn.execute(
        "INSERT OR REPLACE INTO accounts (name,salt,verifier,created,role) VALUES (?1,?2,?3,?4,?5)",
        params![name, salt, verifier, now(), role])?;
    Ok(())
}

pub fn add_ssh_key(store: &Store, account: &str, line: &str, label: Option<&str>) -> Result<(), String> {
    // "ssh-ed25519 AAAA... comment"; the parts that matter are the type and blob
    let mut it = line.split_whitespace();
    let kt = it.next().ok_or("not a public key line")?;
    let blob = it.next().ok_or("public key line has no key material")?;
    if !kt.starts_with("ssh-") && !kt.starts_with("ecdsa-") {
        return Err(format!("unrecognised key type '{}'", kt));
    }
    store.conn.execute(
        "INSERT OR REPLACE INTO ssh_keys (account,key_type,key_blob,label,added) VALUES (?1,?2,?3,?4,?5)",
        params![account, kt, blob, label, now()]).map_err(|e| e.to_string())?;
    Ok(())
}

/// Issues an API key, returns it once, and stores only its hash.
pub fn create_api_key(store: &Store, account: &str, label: Option<&str>) -> rusqlite::Result<String> {
    let id = random_hex(6);
    let secret = random_hex(24);
    let key = format!("execviz_{}_{}", id, secret);
    store.conn.execute(
        "INSERT INTO api_keys (key_id,account,hash,label,created) VALUES (?1,?2,?3,?4,?5)",
        params![id, account, hex(&sha256(key.as_bytes())), label, now()])?;
    Ok(key)
}

pub fn revoke_api_key(store: &Store, key_id: &str) -> rusqlite::Result<usize> {
    store.conn.execute("UPDATE api_keys SET revoked=1 WHERE key_id=?1", params![key_id])
}

// ========================================================================
// INTERNALS
// ========================================================================

fn issue_session(store: &Store, account: &str, method: &str) -> rusqlite::Result<String> {
    let token = random_hex(32);
    store.conn.execute("INSERT INTO sessions (token_hash,account,method,expires) VALUES (?1,?2,?3,?4)",
        params![hex(&sha256(token.as_bytes())), account, method, now() + SESSION_SECS])?;
    Ok(token)
}

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

pub fn verify_password(store: &Store, name: &str, password: &str) -> Option<String> {
    let row: Option<(Option<String>, Option<String>)> = store.conn.query_row(
        "SELECT salt,verifier FROM accounts WHERE name=?1", params![name],
        |r| Ok((r.get(0)?, r.get(1)?))).ok();
    let (salt, verifier) = match row { Some((Some(s), Some(v))) => (s, v), _ => return None };
    let got = hex(&pbkdf2(password.as_bytes(), salt.as_bytes(), ITERATIONS));
    if !constant_time_eq(&got, &verifier) { return None; }
    issue_session(store, name, "password").ok()
}

pub fn verify_api_key(store: &Store, key: &str) -> Option<String> {
    let h = hex(&sha256(key.as_bytes()));
    let account: Option<String> = store.conn.query_row(
        "SELECT account FROM api_keys WHERE hash=?1 AND revoked=0", params![h],
        |r| r.get(0)).ok();
    account
}

// ========================================================================
// TYPES
// ========================================================================

/// The SSH challenge. A nonce is issued, the caller signs it with the key it
/// already holds, and the signature is checked against the account's registered
/// public keys. The private key never moves, and the person who can already
/// reach the machine by SSH needs no second secret.
pub struct Challenges { inner: Mutex<HashMap<String, (String, f64)>> }

// ========================================================================
// IMPLEMENTATIONS
// ========================================================================
impl Challenges {
    pub fn new() -> Challenges { Challenges { inner: Mutex::new(HashMap::new()) } }
    pub fn issue(&self, account: &str) -> String {
        let nonce = random_hex(24);
        let mut g = self.inner.lock().unwrap();
        g.retain(|_, (_, exp)| *exp > now());
        g.insert(nonce.clone(), (account.to_string(), now() + 120.0));
        nonce
    }
    pub fn take(&self, nonce: &str) -> Option<String> {
        let mut g = self.inner.lock().unwrap();
        match g.remove(nonce) {                       // one nonce, one attempt
            Some((acct, exp)) if exp > now() => Some(acct),
            _ => None,
        }
    }
}

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

pub fn verify_ssh_signature(store: &Store, account: &str, nonce: &str, signature: &str)
    -> Result<String, String> {
    let keys: Vec<(String, String)> = {
        let mut st = store.conn.prepare("SELECT key_type,key_blob FROM ssh_keys WHERE account=?1")
            .map_err(|e| e.to_string())?;
        let rows = st.query_map(params![account], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(|e| e.to_string())?;
        rows.filter_map(|x| x.ok()).collect()
    };
    if keys.is_empty() { return Err("no keys registered for this account".into()); }

    let dir = std::env::temp_dir().join(format!("execviz-ssh-{}", random_hex(6)));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let allowed = dir.join("allowed_signers");
    let body: String = keys.iter()
        .map(|(t, b)| format!("{} {} {}\n", account, t, b)).collect();
    std::fs::write(&allowed, body).map_err(|e| e.to_string())?;
    let sig_path = dir.join("sig");
    std::fs::write(&sig_path, signature).map_err(|e| e.to_string())?;

    // Verification is left to the SSH tooling. A hand-rolled signature checker
    // is exactly the wrong thing to hand-roll, and every machine that has an
    // SSH key already has this.
    let out = std::process::Command::new("ssh-keygen")
        .arg("-Y").arg("verify")
        .arg("-f").arg(&allowed)
        .arg("-I").arg(account)
        .arg("-n").arg("execviz")
        .arg("-s").arg(&sig_path)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            if let Some(mut si) = child.stdin.take() { si.write_all(nonce.as_bytes())?; }
            child.wait_with_output()
        });
    let _ = std::fs::remove_dir_all(&dir);
    match out {
        Ok(o) if o.status.success() => issue_session(store, account, "ssh").map_err(|e| e.to_string()),
        Ok(o) => Err(format!("signature rejected: {}", String::from_utf8_lossy(&o.stderr).trim())),
        Err(e) => Err(format!("ssh-keygen unavailable: {}", e)),
    }
}

/// Resolves a request to an account, by session cookie or by API key header.
/// Removes sessions that have expired.
///
/// Expiry was only enforced when the expired token itself was presented, so a
/// row for a session nobody returns to lived forever. This is called on the
/// authentication path, which is the moment the table is already open.
pub fn sweep_sessions(store: &Store) {
    let _ = store.conn.execute("DELETE FROM sessions WHERE expires < ?1", params![now()]);
}

pub fn authenticate(store: &Store, cookie: Option<&str>, bearer: Option<&str>) -> Option<String> {
    if let Some(k) = bearer {
        if let Some(a) = verify_api_key(store, k.trim()) { return Some(a); }
    }
    sweep_sessions(store);
    let token = cookie?;
    let h = hex(&sha256(token.as_bytes()));
    let row: Option<(String, f64)> = store.conn.query_row(
        "SELECT account,expires FROM sessions WHERE token_hash=?1", params![h],
        |r| Ok((r.get(0)?, r.get(1)?))).ok();
    match row {
        Some((acct, exp)) if exp > now() => Some(acct),
        Some(_) => { let _ = store.conn.execute("DELETE FROM sessions WHERE token_hash=?1", params![h]); None }
        None => None,
    }
}

pub fn sign_out(store: &Store, token: &str) {
    let _ = store.conn.execute("DELETE FROM sessions WHERE token_hash=?1",
        params![hex(&sha256(token.as_bytes()))]);
}

// ========================================================================
// TYPES
// ========================================================================

/// An exposed login is what a scanner finds first, so attempts are limited.
pub struct Limiter { inner: Mutex<HashMap<String, (u32, f64)>> }

// ========================================================================
// IMPLEMENTATIONS
// ========================================================================
impl Limiter {
    pub fn new() -> Limiter { Limiter { inner: Mutex::new(HashMap::new()) } }
    pub fn allow(&self, who: &str) -> bool {
        let mut g = match self.inner.lock() { Ok(g) => g, Err(p) => p.into_inner() };
        // Expired entries are swept before a new one is added. Without this the
        // map is keyed by a name the caller chooses, so a caller offering a
        // million distinct names would grow it without bound; a rate limiter
        // that becomes the memory exhaustion it was added to prevent.
        let t = now();
        if g.len() > 1024 { g.retain(|_, (_, exp)| *exp > t); }
        let e = g.entry(who.to_string()).or_insert((0, t + 60.0));
        if t > e.1 { *e = (0, t + 60.0); }
        e.0 += 1;
        e.0 <= 8
    }
}

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

pub fn accounts_json(store: &Store) -> J {
    let mut out = J::obj();
    let names: Vec<String> = store.conn.prepare("SELECT name FROM accounts ORDER BY name").ok()
        .and_then(|mut st| st.query_map([], |r| r.get::<_, String>(0)).ok()
            .map(|rows| rows.filter_map(|x| x.ok()).collect()))
        .unwrap_or_default();
    out.set("accounts", J::Arr(names.iter().map(|n| {
        let mut o = J::obj();
        o.set("name", J::s(n));
        let keys: i64 = store.conn.query_row("SELECT COUNT(*) FROM ssh_keys WHERE account=?1",
            params![n], |r| r.get(0)).unwrap_or(0);
        let apis: i64 = store.conn.query_row("SELECT COUNT(*) FROM api_keys WHERE account=?1 AND revoked=0",
            params![n], |r| r.get(0)).unwrap_or(0);
        o.set("role", J::s(&role_of(store, n)));
        o.set("ssh_keys", J::n(keys as f64));
        o.set("api_keys", J::n(apis as f64));
        o
    }).collect()));
    out
}
