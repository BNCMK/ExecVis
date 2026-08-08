
// ========================================================================
//  MANIFEST
// ========================================================================
//  script_name: main.rs
//  script_path: execviz-rs/src/main.rs
//  module_name: main
//  version: 0.53.1
//  description: execviz: one binary, four jobs.
//  kind: module
//  spec: internal
//  internal_dependencies: sha256, store
//  external_dependencies: json, std, store
//  features: main
//  api_version: execvis-v1.0.0
//  last_updated: 2026-08-07
// ========================================================================

// ========================================================================
// MODULES
// ========================================================================

//! execviz: one binary, four jobs.
//!
//!   execviz serve  <db> [--port N] [--collect] [--ui FILE]
//!   execviz node   --collector URL --db FILE [--host-id ID] [--interval SECS]
//!   execviz view   <db> --lod system|field|cluster|channel|span [...]
//!   execviz query  <db> --q stale|errors|races|slowest|hotpaths|descendants|ancestors
//!   execviz diff   <db> --against capture.json
//!   execviz capture <db>
mod drift; mod profile; mod stress; mod doctor; mod scrutiny; mod bundle; mod witness; mod decode; mod otel; mod ask; mod shapes_rules; mod sha256; mod auth; mod egress; mod watch; mod notes; mod step; mod audit; mod skew; mod compare; mod relate; mod expect; mod retain; mod find; mod stats; mod json; mod store; mod views; mod http; mod conform; mod logs; mod syscalls; mod peer; mod finger;
mod flame; mod rollup; mod tests;

use json::J;
use store::{Span, Store};
use std::collections::BTreeMap;

#[cfg(unix)]
unsafe fn libc_signal_ignore() {
    // SIG_DFL for SIGPIPE terminates quietly, which is the conventional CLI
    // behaviour; the Rust runtime otherwise sets it to ignore and turns the
    // failed write into a panic.
    extern "C" { fn signal(sig: i32, handler: usize) -> usize; }
    const SIGPIPE: i32 = 13;
    const SIG_DFL: usize = 0;
    signal(SIGPIPE, SIG_DFL);
}

// ========================================================================
// INTERNALS
// ========================================================================

fn args() -> (String, Vec<String>, BTreeMap<String, String>) {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let cmd = raw.first().cloned().unwrap_or_else(|| "help".into());
    let mut pos = Vec::new();
    let mut flags = BTreeMap::new();
    let mut i = 1;
    while i < raw.len() {
        if let Some(k) = raw[i].strip_prefix("--") {
            let v = if i + 1 < raw.len() && !raw[i + 1].starts_with("--") {
                i += 1; raw[i].clone()
            } else { "true".into() };
            flags.insert(k.to_string(), v);
        } else { pos.push(raw[i].clone()); }
        i += 1;
    }
    (cmd, pos, flags)
}

fn renderer_feed(spans: &[Span]) -> String { renderer_feed_since(spans, 0.0, usize::MAX) }

/// The feed, from a position.
///
/// Times are sent **raw**, with the window the whole store currently spans. The
/// client normalises. That is not a detail: normalising here would compute the
/// scale from whatever subset is being sent, so a delta would arrive on a
/// different scale than the spans the client already holds, and the two would
/// not line up. The window travels so the client can place everything it holds
/// on one clock and re-place it when the window grows.
fn renderer_feed_since(spans: &[Span], cursor: f64, limit: usize) -> String {
    renderer_feed_floor(spans, cursor, limit, 0.0)
}

/// The feed, told what the store no longer holds.
///
/// A reader whose cursor lies below the recorder has missed spans that were
/// removed. Delivering what remains and letting it believe it has everything is
/// the one dishonest option, so the delivery is marked as a gap and the correct
/// reaction is to reset rather than continue.
fn renderer_feed_floor(spans: &[Span], cursor: f64, limit: usize, floor: f64) -> String {
    let mut lo = f64::MAX; let mut hi = f64::MIN;
    for s in spans {
        if s.start < lo { lo = s.start; }
        if s.start > hi { hi = s.start; }
        if let Some(e) = s.end { if e > hi { hi = e; } }
    }
    if spans.is_empty() { lo = 0.0; hi = 1.0; }

    // A span moves forward in this ordering when its second phase lands, so a
    // completion is delivered again rather than being missed.
    let mut changed: Vec<&Span> = spans.iter()
        .filter(|s| s.end.unwrap_or(s.start) >= cursor)
        .collect();
    changed.sort_by(|a, b| a.end.unwrap_or(a.start).partial_cmp(&b.end.unwrap_or(b.start)).unwrap());
    let truncated = changed.len() > limit;
    changed.truncate(limit);
    let next_cursor = changed.iter()
        .map(|s| s.end.unwrap_or(s.start)).fold(cursor, f64::max);

    let arr: Vec<J> = changed.iter().map(|s| s.to_json()).collect();

    let mut seen: std::collections::BTreeSet<(String, String)> = std::collections::BTreeSet::new();
    for s in spans {
        seen.insert((s.host_id.clone(), s.domain.clone().unwrap_or_else(|| "unknown".into())));
    }
    let role = |d: &str| -> &'static str {
        match d {
            "gateway" | "MainThread" | "edge-agent" | "api" => "entry",
            "sensors" | "queue" => "data",
            _ => "logic",
        }
    };
    let mut slots: BTreeMap<(String, String), i64> = BTreeMap::new();
    let clusters: Vec<J> = seen.into_iter().map(|(h, d)| {
        let region = role(&d).to_string();
        let key = (h.clone(), region.clone());
        let slot = *slots.get(&key).unwrap_or(&0);
        slots.insert(key, slot + 1);
        let mut c = J::obj();
        c.set("id", J::s(&format!("{}/{}", h, d)));
        c.set("label", J::s(&d));
        c.set("region", J::s(&region));
        c.set("slot", J::n(slot as f64));
        c.set("host", J::s(&h));
        c
    }).collect();

    let mut window = J::obj();
    window.set("lo", J::n(lo));
    window.set("hi", J::n(hi));

    let mut o = J::obj();
    o.set("window", window);
    o.set("cursor", J::n(next_cursor));
    o.set("floor", J::n(floor));
    if floor > 0.0 && cursor > 0.0 && cursor < floor {
        o.set("gap", J::Bool(true));
        o.set("gap_note", J::s("spans older than the retention floor were trimmed and are gone; reset and re-read"));
    }
    o.set("total", J::n(spans.len() as f64));
    o.set("delivered", J::n(arr.len() as f64));
    o.set("truncated", J::Bool(truncated));
    o.set("spans", J::Arr(arr));
    o.set("clusters", J::Arr(clusters));
    o.dump()
}

// ========================================================================
// CONSTANTS
// ========================================================================

/// The wire protocol this build speaks. A sender declaring a newer major
/// version is refused with a reason rather than being half-understood: a
/// mismatch that produces a plausible-looking partial capture differs from one
/// that refuses.
pub const WIRE_VERSION: u32 = 1;

/// Ingest limits (spec 5.6, gap 33). A collector with no bound is a
/// disk-filling device operated by whichever adapter misbehaves first.
pub const MAX_SPANS_PER_BATCH: usize = 20_000;

// ========================================================================
// TYPES
// ========================================================================

/// Whether the store is accepting writes, and why not.
///
/// A tool that records other programs must be able to say when it has stopped
/// recording. Under a full disk every span was refused with an accurate reason
/// *to the sender* while `/api/health` answered 200 and said nothing; a monitor
/// watching the service would have seen a healthy process quietly recording
/// nothing.
pub struct WriteHealth {
    failures: std::sync::atomic::AtomicU64,
    last_ok: std::sync::Mutex<f64>,
    last_error: std::sync::Mutex<String>,
}

// ========================================================================
// IMPLEMENTATIONS
// ========================================================================

impl WriteHealth {
    pub fn new() -> WriteHealth {
        WriteHealth {
            failures: std::sync::atomic::AtomicU64::new(0),
            last_ok: std::sync::Mutex::new(0.0),
            last_error: std::sync::Mutex::new(String::new()),
        }
    }
    pub fn ok(&self) {
        self.failures.store(0, std::sync::atomic::Ordering::Relaxed);
        if let Ok(mut g) = self.last_ok.lock() { *g = now_secs(); }
    }
    pub fn failed(&self, why: &str) {
        self.failures.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if let Ok(mut g) = self.last_error.lock() { *g = why.to_string(); }
    }
    pub fn consecutive_failures(&self) -> u64 {
        self.failures.load(std::sync::atomic::Ordering::Relaxed)
    }
    pub fn last_error(&self) -> String {
        self.last_error.lock().map(|g| g.clone()).unwrap_or_default()
    }
    pub fn last_write(&self) -> f64 {
        self.last_ok.lock().map(|g| *g).unwrap_or(0.0)
    }
}

// ========================================================================
// INTERNALS
// ========================================================================

fn now_secs() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

// ========================================================================
// TYPES
// ========================================================================

/// The result of an ingest, including what was refused and why.
///
/// The previous version returned only a count, so a sender whose spans were
/// dropped was told "ok" and a number smaller than it sent; which nobody
/// checks. Silence about a rejection is how a broken adapter stays broken.
pub struct Ingested {
    pub accepted: usize,
    pub rejected: Vec<String>,
    /// Refusals the *store* made rather than the validator: a full disk, a
    /// locked file. These say something about the instance, not the sender.
    pub store_errors: Vec<String>,
}

/// Opens a store for reading, or explains and exits.
///
/// A mistyped path is the commonest mistake anyone makes with this tool, and it
/// used to answer with a Rust panic and a backtrace note. A command-line tool
/// that reports a user's typo as an internal error teaches the user to distrust
/// every other message it prints.

// ========================================================================
// INTERNALS
// ========================================================================

/// A usage error: the caller asked for something that cannot be done as asked.
///
/// Exit 2 throughout, distinct from 1 (the command ran and the answer was
/// "no"). The two were previously mixed: a missing flag *panicked* with a Rust
/// backtrace and exit 101, and a mistyped subcommand printed help and exited 0,
/// so a typo in a CI script passed silently.
fn usage_error(msg: &str) -> ! {
    eprintln!("execviz: {}", msg);
    eprintln!("  run `execviz --help` for the full list");
    std::process::exit(2);
}

/// A required argument, or a clean explanation.
fn require<'a>(v: Option<&'a str>, msg: &str) -> &'a str {
    match v { Some(x) => x, None => usage_error(msg) }
}

fn open_read(path: &str) -> Store {
    match Store::open_ro(path) {
        Ok(s) => s,
        Err(e) => {
            let missing = !std::path::Path::new(path).exists();
            if missing {
                eprintln!("execviz: no capture at '{}'", path);
                eprintln!("  a capture is created by `execviz serve <db> --collect` or by an adapter");
            } else {
                eprintln!("execviz: cannot read the capture at '{}': {}", path, e);
                eprintln!("  if the file exists, check permissions, or run `execviz integrity {}`", path);
            }
            std::process::exit(2);
        }
    }
}

/// Opens a store for writing, or explains and exits.
fn open_write(path: &str) -> Store {
    match Store::open(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("execviz: cannot open '{}' for writing: {}", path, e);
            eprintln!("  check the directory exists and is writable");
            std::process::exit(2);
        }
    }
}

fn ingest(store: &Store, payload: &J) -> Ingested {
    let host = payload.get("host_id").and_then(|h| h.as_str());
    let mut out = Ingested { accepted: 0, rejected: Vec::new(), store_errors: Vec::new() };
    let spans = match payload.get("spans").and_then(|s| s.as_arr()) {
        Some(a) => a,
        None => {
            out.rejected.push("payload has no 'spans' array".into());
            return out;
        }
    };
    // A sender that had to drop spans reports it, and the store records it. A
    // capture missing rows is a fact about the record: without it every count
    // taken from that capture is quietly wrong, and no reader can tell.
    if let Some(d) = payload.get("dropped").and_then(|x| x.as_f64()) {
        if d > 0.0 {
            // whole traces, per the sampling invariant: a loss that
            // punched holes in a trace's graph would be a different and worse
            // fact than a loss of complete traces
            let traces = payload.get("dropped_traces").and_then(|x| x.as_f64()).unwrap_or(0.0);
            let abnormal = payload.get("dropped_abnormal").and_then(|x| x.as_f64()).unwrap_or(0.0);
            let who = host.unwrap_or("unknown");
            let _ = store.record_loss(who, d as i64, traces as i64, abnormal as i64);
        }
    }
    for v in spans {
        let sp = match Span::from_json(v, host) {
            Some(sp) => sp,
            None => { out.rejected.push("a span had no span_id and was refused".into()); continue }
        };
        // validated before it is written down, never after: a capture that
        // disagrees with itself is more expensive than a refused span
        if let Err(why) = sp.validate() { out.rejected.push(why); continue }
        match store.upsert(&sp) {
            Ok(_) => out.accepted += 1,
            Err(e) => {
                // a refusal the store made (disk full, locked) is a different
                // fact from a span that failed validation, and the operator
                // needs the first one
                let msg = e.to_string();
                if !matches!(e, rusqlite::Error::InvalidParameterName(_)) {
                    out.store_errors.push(msg.clone());
                }
                out.rejected.push(format!("{}: could not be stored: {}", sp.span_id, msg));
            }
        }
    }
    out
}

// ========================================================================
// CONSTANTS
// ========================================================================

/// The sign-in page. Deliberately plain: it exists to take one credential and
/// to say accurately what the connection does and does not protect.
const LOGIN_PAGE: &str = r#"<!DOCTYPE html><html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1"><title>execviz - sign in</title>
<style>
:root{--bg:#0b0f14;--panel:#121821;--ink:#e8edf3;--muted:#8b97a6;--faint:#566;--line:#1e2730;
--mono:ui-monospace,Menlo,Consolas,monospace}
*{box-sizing:border-box}body{margin:0;height:100vh;display:grid;place-items:center;background:var(--bg);
color:var(--ink);font-family:system-ui,sans-serif}
.card{width:360px;border:1px solid var(--line);border-radius:10px;padding:22px;background:var(--panel)}
h1{font-size:15px;margin:0 0 4px}
p.sub{font-family:var(--mono);font-size:10.5px;color:var(--faint);margin:0 0 18px;line-height:1.5}
label{display:block;font-family:var(--mono);font-size:10px;color:var(--muted);margin:12px 0 5px}
input{width:100%;padding:8px 10px;background:#0d131a;color:var(--ink);border:1px solid var(--line);
border-radius:6px;font-family:var(--mono);font-size:12px}
button{width:100%;margin-top:16px;padding:9px;background:#1f6feb;color:#fff;border:none;border-radius:6px;
font-size:12.5px;cursor:pointer}
.alt{margin-top:16px;padding-top:14px;border-top:1px solid var(--line);font-family:var(--mono);
font-size:10px;color:var(--faint);line-height:1.6}
code{color:var(--muted)}
.err{margin-top:12px;font-family:var(--mono);font-size:10.5px;color:#ff7b72;min-height:14px}
.warn{margin-top:14px;font-family:var(--mono);font-size:9.5px;color:#e3b341;line-height:1.5}
</style></head><body>
<div class="card">
  <h1>execviz</h1>
  <p class="sub">This instance serves its own capture. Sign in to watch it.</p>
  <label>account</label><input id="acct" autocomplete="username">
  <label>password</label><input id="pw" type="password" autocomplete="current-password">
  <button id="go">sign in</button>
  <div class="err" id="err"></div>
  <div class="alt">
    an SSH key you already trust on this machine:<br>
    <code>curl -s HOST/api/auth/challenge?account=NAME</code><br>
    <code>echo -n NONCE | ssh-keygen -Y sign -f ~/.ssh/id_ed25519 -n execviz -</code><br>
    then POST it to <code>/api/auth/ssh</code>.<br><br>
    a program: send its key as <code>X-Execviz-Key</code>.
  </div>
  <div class="warn">Over plain HTTP this password and the capture itself cross the
  network in the clear. On anything but a trusted network, put this behind TLS or
  an SSH tunnel.</div>
</div>
<script>

const go=document.getElementById('go'),err=document.getElementById('err');
async function submit(){
  err.textContent='';
  const r=await fetch('/api/auth/login',{method:'POST',headers:{'Content-Type':'application/json'},
    body:JSON.stringify({account:document.getElementById('acct').value,password:document.getElementById('pw').value})});
  if(r.ok){ location.reload(); return; }
  const d=await r.json().catch(()=>({error:'sign in failed'}));
  err.textContent=d.error||'sign in failed';
}
go.onclick=submit;
document.getElementById('pw').addEventListener('keydown',e=>{if(e.key==='Enter')submit();});
</script></body></html>"#;

// ========================================================================
// INTERNALS
// ========================================================================

fn main() {
    // A downstream `head` closing the pipe is a normal way to use a CLI, not a
    // crash. Without this, the default SIGPIPE handling panics mid-write.
    #[cfg(unix)]
    unsafe { libc_signal_ignore(); }
    let (cmd, pos, flags) = args();
    let db = pos.first().cloned().unwrap_or_else(|| "run.db".into());
    let f = |k: &str| flags.get(k).map(|s| s.as_str());
    let limit: usize = f("limit").and_then(|v| v.parse().ok()).unwrap_or(50);

    match cmd.as_str() {
        "serve" => {
            let port: u16 = f("port").and_then(|v| v.parse().ok()).unwrap_or(8900);
            let collect = flags.contains_key("collect");
            // Serving without an account is a decision somebody makes out loud.
            // It exists for a demo and for the tutorial, and it says so on start.
            let open_access = flags.contains_key("open");
            let ui = f("ui").map(|s| std::fs::read_to_string(s).unwrap_or_default());
            let dbp = db.clone();
            let writer = if collect { Some(std::sync::Mutex::new(
                Store::open(&dbp).expect("open store rw"))) } else { None };
            let identity = f("identity").map(|s| s.to_string()).unwrap_or_else(|| format!("execviz-{}", port));
            let pair_token = f("pair-token").unwrap_or("").to_string();
            println!("execviz serve on :{} db={} identity={} collect={}", port, dbp, identity, collect);
            {   // Pull from approved outbound peers: restartable, no reconnect
                // protocol, each side sets its own rate.
                let sync_db = dbp.clone(); let me = identity.clone();
                std::thread::spawn(move || loop {
                    std::thread::sleep(std::time::Duration::from_secs_f64(2.0));
                    let w = match Store::open(&sync_db) { Ok(s)=>s, Err(_)=>continue };
                    if peer::ensure(&w).is_err() { continue; }
                    let peers = match peer::list(&w) { Ok(p)=>p, Err(_)=>continue };
                    // A pending outbound link is retried: the other end approves on
                    // its own schedule and has no way to tell us, so the next
                    // successful exchange is how we find out. Revocation lands the
                    // same way, in reverse.
                    for p in peers.iter().filter(|p| p.direction=="outbound"
                                                  && (p.status=="approved" || p.status=="pending")) {
                        let url = match &p.url { Some(u)=>u.clone(), None=>continue };
                        let target = format!("{}/api/peer/spans?peer={}&since={}", url.trim_end_matches('/'), me, p.cursor);
                        // present the credential rather than asserting a name
                        let body = match http::get_with_key(&target, p.api_key.as_deref()) {
                            Ok(b)=>b, Err(_)=>continue };
                        let v = match json::parse(&body) { Ok(v)=>v, Err(_)=>continue };
                        if v.get("error").is_some() {
                            if p.status=="approved" { let _=peer::set_status(&w,&p.peer_id,"outbound","pending"); }
                            continue;
                        }
                        if p.status=="pending" {
                            let _=peer::set_status(&w,&p.peer_id,"outbound","approved");
                            println!("[peer] {} approved us", p.peer_id);
                        }
                        let arr = match v.get("spans").and_then(|s| s.as_arr()) { Some(a)=>a, None=>continue };
                        let mut n=0;
                        // A peer is not more trusted than a local adapter: the
                        // same validation applies, or the rule would be enforced
                        // at the door a sender controls and skipped at the one
                        // it does not.
                        let mut refused = 0usize;
                        for sv in arr {
                            match Span::from_json(sv, None) {
                                Some(sp) if sp.validate().is_ok() => {
                                    if w.upsert(&sp).is_ok() { n += 1; }
                                }
                                _ => refused += 1,
                            }
                        }
                        if refused > 0 {
                            eprintln!("peer exchange: refused {} span(s) that failed validation", refused);
                        }
                        if let Some(c)=v.get("cursor").and_then(|x| x.as_f64()) {
                            if c>p.cursor { let _=peer::set_cursor(&w,&p.peer_id,c); } }
                        if n>0 { println!("[peer] pulled {} spans from {}", n, p.peer_id); }
                    }
                });
            }
            let feed_db = dbp.clone();
            let write_health = std::sync::Arc::new(WriteHealth::new());
            let health_for_handler = write_health.clone();
            let challenges = std::sync::Arc::new(auth::Challenges::new());
            let limiter = std::sync::Arc::new(auth::Limiter::new());
            // Accounts are made on the machine, never over the wire: there is no
            // route that creates one, so the only way to get an account is a
            // shell on the host, whether that shell arrived over SSH or is sitting
            // at the keyboard.
            {
                let st = Store::open(&dbp).expect("open store");
                auth::ensure(&st).expect("accounts table");
                if auth::any_account(&st) {
                    println!("access requires an account");
                } else if open_access {
                    println!("--open was given: this instance serves without an account. \
                              Anyone who can reach the port can read the capture.");
                } else {
                    println!("no account exists yet, so nothing can sign in. Create one here:");
                    println!("  execviz account {} create <name> --password <password>", db);
                    println!("or authorise an SSH key:");
                    println!("  execviz account {} authorize <name> --key ~/.ssh/id_ed25519.pub", db);
                }
            }
            let handler = move |req: &http::Req| -> http::Resp {
                // One notion of "allowed", whichever credential produced it.
                // Asked per request rather than once at startup. Deciding it at
                // boot means an account created while the server runs changes
                // nothing until somebody restarts it, and an instance left open
                // because it had no accounts when it started stays open after it
                // has some.
                // Reaching this over a network requires an account, always. The
                // absence of accounts is not permission: it means nobody can sign
                // in yet, and an account has to be made on the machine first.
                // Treating "no accounts" as "open" is how an instance ends up
                // published to a network with its capture readable by anyone who
                // finds the port.
                let require_auth = !open_access;
                let who = if !require_auth { Some("open".to_string()) } else {
                    Store::open(&dbp).ok().and_then(|st| {
                        auth::authenticate(&st, req.cookie("execviz_session").as_deref(),
                                           req.bearer().as_deref())
                    })
                };
                if req.path == "/api/auth/challenge" {
                    let acct = req.query.get("account").cloned().unwrap_or_default();
                    let nonce = challenges.issue(&acct);
                    let mut o = J::obj();
                    o.set("nonce", J::s(&nonce));
                    o.set("namespace", J::s("execviz"));
                    o.set("sign_with", J::s("ssh-keygen -Y sign -f <key> -n execviz -"));
                    return http::Resp::Body(200, "application/json".into(), o.dump());
                }
                if req.method == "POST" && (req.path == "/api/auth/login" || req.path == "/api/auth/ssh") {
                    let st = match Store::open(&dbp) { Ok(s)=>s, Err(e)=>
                        return http::Resp::Body(500,"application/json".into(), format!("{{\"error\":\"{}\"}}", e)) };
                    let _ = auth::ensure(&st);
                    let p = json::parse(&req.body).unwrap_or(J::Null);
                    let name = p.get("account").and_then(|x| x.as_str()).unwrap_or("");
                    if !limiter.allow(name) {
                        return http::Resp::Body(429,"application/json".into(),
                            "{\"error\":\"too many attempts; wait a minute\"}".into());
                    }
                    let issued = if req.path == "/api/auth/login" {
                        auth::verify_password(&st, name, p.get("password").and_then(|x| x.as_str()).unwrap_or(""))
                            .ok_or_else(|| "invalid account or password".to_string())
                    } else {
                        let nonce = p.get("nonce").and_then(|x| x.as_str()).unwrap_or("");
                        match challenges.take(nonce) {
                            None => Err("challenge expired or already used".to_string()),
                            Some(acct) => auth::verify_ssh_signature(&st, &acct, nonce,
                                p.get("signature").and_then(|x| x.as_str()).unwrap_or("")),
                        }
                    };
                    return match issued {
                        Ok(tok) => http::Resp::Cookie(
                            format!("execviz_session={}; Path=/; HttpOnly; SameSite=Strict; Max-Age=43200", tok),
                            "{\"ok\":true}".into()),
                        Err(e) => http::Resp::Body(401,"application/json".into(),
                            format!("{{\"error\":\"{}\"}}", e)),
                    };
                }
                // Signing out ends the session on the server, not only in the
                // browser. A sign-in with no way out leaves a token valid for
                // its full twelve hours on any machine it was left on, and the
                // function to end it existed with nothing reaching it.
                if req.method == "POST" && req.path == "/api/auth/logout" {
                    if let Some(tok) = req.cookie("execviz_session") {
                        if let Ok(w) = Store::open(&dbp) {
                            auth::sign_out(&w, &tok);
                            audit::record(&w, who.as_deref(), "logout", "/api/auth/logout", 0, None);
                        }
                    }
                    return http::Resp::Cookie(
                        "execviz_session=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0".into(),
                        "{\"ok\":true}".into());
                }

                // the trail is written for reads that return data, against the
                // account that asked
                if who.is_some() && (req.path.starts_with("/api/") || req.path == "/spans") {
                    if let Ok(w) = Store::open(&dbp) {
                        audit::record(&w, who.as_deref(), "read", &req.path, 0, None);
                    }
                }
                // a viewer may look and may not act (spec 5.6, gap 34)
                if let Some(acct) = &who {
                    let writes = req.method == "POST"
                        || req.path.starts_with("/api/peer/request")
                        || req.path == "/api/ingest";
                    if writes && acct != "open" && !req.path.starts_with("/api/auth/") {
                        let role = Store::open_ro(&dbp).map(|s| auth::role_of(&s, acct))
                            .unwrap_or_else(|_| "viewer".into());
                        if !auth::may_write(&role) {
                            return http::Resp::Body(403, "application/json".into(),
                                format!("{{\"error\":\"account '{}' is a viewer\",\"hint\":\"a viewer may read a capture and may not change one\"}}", acct));
                        }
                    }
                }
                if who.is_none() {
                    if req.path == "/" || req.path == "/index.html" {
                        return http::Resp::Body(200, "text/html; charset=utf-8".into(), LOGIN_PAGE.to_string());
                    }
                    return http::Resp::Body(401, "application/json".into(),
                        "{\"error\":\"sign in required\",\"hint\":\"POST /api/auth/login, or /api/auth/challenge for an SSH key, or send an X-Execviz-Key header\"}".into());
                }
                let q = |k: &str| req.query.get(k).map(|s| s.as_str());
                if req.method == "POST" {
                    let payload = match json::parse(&req.body) { Ok(p) => p, Err(e) =>
                        return http::Resp::Body(400, "application/json".into(),
                                format!("{{\"error\":\"{}\"}}", e)) };
                    if req.path == "/api/peer/request" {
                        let w = match Store::open(&dbp) { Ok(w)=>w, Err(e)=>
                            return http::Resp::Body(500,"application/json".into(), format!("{{\"error\":\"{}\"}}",e)) };
                        let _ = peer::ensure(&w);
                        let who = payload.get("peer_id").and_then(|x| x.as_str()).unwrap_or("unknown");
                        let url = payload.get("url").and_then(|x| x.as_str());
                        let tok = payload.get("token").and_then(|x| x.as_str()).unwrap_or("");
                        let auto = !pair_token.is_empty() && tok == pair_token;
                        // An approval already granted is reported back, and never
                        // overwritten. Answering `pending` to a peer that has been
                        // approved leaves the requester waiting on something that
                        // already happened, and re-asking sets the approval back to
                        // pending, so the link can never complete from either end.
                        let already = peer::is_approved(&w, who, "inbound");
                        let status = if already || auto { "approved" } else { "pending" };
                        if !already {
                            let _ = peer::upsert(&w, who, "inbound", url, status);
                        }
                        let mut o = J::obj();
                        o.set("identity", J::s(&identity));
                        o.set("status", J::s(status));
                        return http::Resp::Body(200,"application/json".into(), o.dump());
                    }
                    // findings are written beside the capture, not into a browser
                    if req.path == "/api/note" || req.path == "/api/view" {
                        let who = who.clone().filter(|w| w != "open");
                        return match Store::open(&dbp) {
                            Err(e) => http::Resp::Body(500, "application/json".into(),
                                format!("{{\"error\":\"{}\"}}", e)),
                            Ok(w) => {
                                let p = json::parse(&req.body).unwrap_or(J::Null);
                                let ok = if req.path == "/api/note" {
                                    notes::add_note(&w,
                                        p.get("span_id").and_then(|x| x.as_str()),
                                        p.get("body").and_then(|x| x.as_str()).unwrap_or(""),
                                        who.as_deref()).is_ok()
                                } else {
                                    notes::save_view(&w,
                                        p.get("name").and_then(|x| x.as_str()).unwrap_or("untitled"),
                                        p.get("state").and_then(|x| x.as_str()).unwrap_or(""),
                                        who.as_deref(),
                                        p.get("note").and_then(|x| x.as_str())).is_ok()
                                };
                                http::Resp::Body(if ok { 200 } else { 500 },
                                    "application/json".into(), format!("{{\"saved\":{}}}", ok))
                            }
                        };
                    }
                    if req.path == "/api/ingest" {
                        // A sender speaking a newer major version is refused with a
                        // reason: a mismatch that produces a plausible-looking
                        // partial capture differs from one that refuses.
                        if let Some(v) = payload.get("wire").and_then(|x| x.as_f64()) {
                            if (v as u32) > WIRE_VERSION {
                                return http::Resp::Body(409, "application/json".into(),
                                    format!("{{\"error\":\"this build speaks wire version {}, the sender declared {}\",\"hint\":\"upgrade the collector\"}}",
                                            WIRE_VERSION, v as u32));
                            }
                        }
                        // and a batch beyond the bound is refused rather than
                        // silently filling a disk
                        let count = payload.get("spans").and_then(|x| x.as_arr())
                            .map(|a| a.len()).unwrap_or(0);
                        if count > MAX_SPANS_PER_BATCH {
                            return http::Resp::Body(413, "application/json".into(),
                                format!("{{\"error\":\"batch of {} exceeds the limit of {}\",\"hint\":\"send smaller batches; the limit exists so one adapter cannot fill the disk\"}}",
                                        count, MAX_SPANS_PER_BATCH));
                        }
                        return match &writer {
                            Some(w) => {
                                // a poisoned lock must not take the server down with it
                                let g = match w.lock() {
                                    Ok(g) => g,
                                    Err(p) => p.into_inner(),
                                };
                                let r = ingest(&g, &payload);
                                if r.store_errors.is_empty() {
                                    if r.accepted > 0 { health_for_handler.ok(); }
                                } else {
                                    health_for_handler.failed(&r.store_errors[0]);
                                }
                                let mut o = J::obj();
                                o.set("ok", J::Bool(r.rejected.is_empty()));
                                o.set("ingested", J::n(r.accepted as f64));
                                o.set("rejected", J::n(r.rejected.len() as f64));
                                if !r.rejected.is_empty() {
                                    o.set("reasons", J::Arr(r.rejected.iter().take(20)
                                        .map(|x| J::s(x)).collect()));
                                    o.set("note", J::s("refused spans are named so an adapter can be fixed; the accepted ones were stored"));
                                }
                                // a batch that was partly refused is still a
                                // successful exchange: reporting 4xx would make
                                // a sender retry the spans that were accepted
                                http::Resp::Body(200, "application/json".into(), o.dump())
                            }
                            None => http::Resp::Body(400, "application/json".into(),
                                     "{\"error\":\"ingest disabled, pass --collect\"}".into()),
                        };
                    }
                    if req.path == "/api/diff" {
                        let pick = |k: &str| -> Vec<Span> {
                            payload.get(k).and_then(|c| c.get("spans").or(Some(c)))
                                .and_then(|s| s.as_arr())
                                .map(|a| a.iter().filter_map(|v| Span::from_json(v, None)).collect())
                                .unwrap_or_default()
                        };
                        return http::Resp::Body(200, "application/json".into(),
                                views::diff(&pick("a"), &pick("b")).dump());
                    }
                    return http::Resp::Body(404, "application/json".into(), "{\"error\":\"not found\"}".into());
                }
                if req.path == "/" || req.path == "/index.html" {
                    return match &ui {
                        Some(html) => http::Resp::Body(200, "text/html; charset=utf-8".into(), html.clone()),
                        None => http::Resp::Body(200, "text/plain".into(),
                                 "execviz api. pass --ui <file> to serve the map.".into()),
                    };
                }
                if req.path == "/events" {
                    // Push upgrade at the seam: the payload is the
                    // same snapshot the poll endpoint returns.
                    let feed_db = feed_db.clone();
                    let mut last = String::new();
                    let mut sse_cursor: f64 = req.query.get("since").and_then(|v| v.parse().ok()).unwrap_or(0.0);
                    // A stream is one long-lived request, so a permission checked
                    // only when it opened would hold for as long as the socket
                    // does. A session that expires or an account that is revoked
                    // must stop the delivery, not merely stop new requests.
                    let stream_cookie = req.cookie("execviz_session");
                    let stream_key = req.bearer();
                    let auth_db = dbp.clone();
                    let guarded = require_auth;
                    let mut checked_at = 0f64;
                    return http::Resp::Stream(Box::new(move |w: &mut dyn std::io::Write| {
                        loop {
                            if guarded {
                                let now = std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .map(|d| d.as_secs_f64()).unwrap_or(0.0);
                                // re-checked periodically rather than every tick:
                                // often enough that a revocation takes effect in
                                // seconds, rarely enough not to hash a token
                                // several times a second per open stream
                                if now - checked_at > 5.0 {
                                    checked_at = now;
                                    let still = Store::open(&auth_db).ok().and_then(|s| {
                                        auth::authenticate(&s, stream_cookie.as_deref(),
                                                           stream_key.as_deref())
                                    });
                                    if still.is_none() {
                                        // tell the client why before closing, so the
                                        // page can say so instead of appearing frozen
                                        let _ = w.write_all(b"event: unauthorized\ndata: {\"error\":\"this session is no longer valid\"}\n\n");
                                        let _ = w.flush();
                                        return Ok(());
                                    }
                                }
                            }
                            // the stream carries the same cursor as the poll, so a
                            // client that already holds most of a capture is sent
                            // what changed rather than the whole store again
                            let snap = match Store::open_ro(&feed_db).and_then(|s| s.all()) {
                                Ok(spans) => { let f = renderer_feed_since(&spans, sse_cursor, 20000);
                                    if let Ok(v) = json::parse(&f) {
                                        if let Some(c) = v.get("cursor").and_then(|x| x.as_f64()) { sse_cursor = c; }
                                    }
                                    f }
                                Err(_) => String::from("{}"),
                            };
                            if snap != last {
                                w.write_all(format!("data: {}\n\n", snap).as_bytes())?;
                                w.flush()?;
                                last = snap;
                            } else {
                                w.write_all(b": keepalive\n\n")?;
                                w.flush()?;
                            }
                            std::thread::sleep(std::time::Duration::from_millis(700));
                        }
                    }));
                }
                let st = match Store::open_ro(&dbp) { Ok(s) => s, Err(e) =>
                    return http::Resp::Body(500, "application/json".into(), format!("{{\"error\":\"{}\"}}", e)) };
                let spans = st.all().unwrap_or_default();
                // A console command is answered by the endpoint that already
                // answers it. Resolving the name to a path here means there is one
                // routing table rather than two that can drift apart.
                let console_path = if req.path == "/api/command" {
                    let cmd = q("cmd").unwrap_or_default();
                    let name = cmd.split_whitespace().next().unwrap_or("").to_string();
                    const ALLOWED: &[&str] = &["stats", "health", "check", "capture",
                        "concurrency", "selftime", "cost", "skew", "correlate",
                        "fingerprint", "peers", "views", "notes", "stress", "rollup",
                        "flame", "critical"];
                    if ALLOWED.contains(&name.as_str()) { Some(format!("/api/{}", name)) }
                    else { None }
                } else { None };
                let routed: &str = console_path.as_deref().unwrap_or(req.path.as_str());
                let body = match routed {
                    "/api/peers" | "/api/peer/list" => {
                        let ps = Store::open(&dbp).ok()
                            .and_then(|w| { let _=peer::ensure(&w); peer::list(&w).ok() })
                            .unwrap_or_default();
                        peer::to_json(&ps).dump()
                    }
                    // a peer exchange leaves the machine, so it is recorded with
                    // the peer named
                    "/api/peer/spans" => {
                        // an approved inbound peer may read; nobody else may
                        let w = match Store::open(&dbp) { Ok(w)=>w, Err(e)=>
                            return http::Resp::Body(500,"application/json".into(), format!("{{\"error\":\"{}\"}}",e)) };
                        let _ = peer::ensure(&w);
                        let who = q("peer").unwrap_or("");
                        if !peer::is_approved(&w, who, "inbound") {
                            return http::Resp::Body(403,"application/json".into(),
                              "{\"error\":\"not approved\",\"hint\":\"the other end must approve this peer\"}".into());
                        }
                        let cursor: f64 = q("since").and_then(|v| v.parse().ok()).unwrap_or(0.0);
                        let batch = peer::since(&spans, cursor, 2000);
                        let mut o = J::obj();
                        o.set("identity", J::s(&identity));
                        o.set("cursor", J::n(peer::watermark(&batch)));
                        o.set("spans", J::Arr(batch.iter().map(|s| s.to_json()).collect()));
                        o.dump()
                    }
                    // Self-monitoring (spec 5.6, gap 37). A tool that watches other
                    // programs and reports nothing about itself is asking for a
                    // trust it does not extend.
                    "/api/health" => {
                        let mut o = J::obj();
                        o.set("ok", J::Bool(true));
                        o.set("db", J::s(&dbp));
                        o.set("spans", J::n(spans.len() as f64));
                        o.set("wire_version", J::n(WIRE_VERSION as f64));
                        o.set("max_spans_per_batch", J::n(MAX_SPANS_PER_BATCH as f64));
                        o.set("requires_account", J::Bool(require_auth));
                        o.set("store_bytes", J::n(std::fs::metadata(&dbp)
                            .map(|m| m.len() as f64).unwrap_or(0.0)));
                        o.set("open_spans", J::n(spans.iter().filter(|s| s.end.is_none()).count() as f64));
                        o.set("hosts", J::n(spans.iter().map(|s| s.host_id.clone())
                            .collect::<std::collections::BTreeSet<_>>().len() as f64));
                        o.set("retention_floor", J::n(Store::open_ro(&dbp)
                            .map(|w| retain::floor(&w)).unwrap_or(0.0)));
                        // A tool that records other programs must be able to say
                        // when it has stopped recording. Answering 200 while every
                        // write fails would show a monitor a healthy process that
                        // is quietly keeping nothing.
                        let failing = health_for_handler.consecutive_failures();
                        o.set("accepting_writes", J::Bool(failing == 0));
                        if failing > 0 {
                            o.set("ok", J::Bool(false));
                            o.set("consecutive_write_failures", J::n(failing as f64));
                            o.set("last_write_error", J::s(&health_for_handler.last_error()));
                            o.set("note", J::s("the process is answering but the store is refusing writes; spans arriving now are being lost"));
                        }
                        let last = health_for_handler.last_write();
                        if last > 0.0 { o.set("last_successful_write", J::n(last)); }
                        o.dump()
                    }
                    "/api/spans" => {
                        let mut o = J::obj();
                        o.set("spans", J::Arr(spans.iter().map(|s| s.to_json()).collect()));
                        o.dump()
                    }
                    "/spans" => renderer_feed_floor(&spans,
                        q("since").and_then(|v| v.parse().ok()).unwrap_or(0.0),
                        q("limit").and_then(|v| v.parse().ok()).unwrap_or(20000),
                        Store::open_ro(&dbp).map(|w| retain::floor(&w)).unwrap_or(0.0)),
                    "/api/view" => views::view(&spans, q("lod").unwrap_or("field"),
                        q("host"), q("cluster"), q("family"), q("span")).dump(),
                    "/api/query" => views::query(&spans, q("q").unwrap_or("stale"), q("span"),
                        q("limit").and_then(|v| v.parse().ok()).unwrap_or(50),
                        q("min_overlap_ms").and_then(|v| v.parse().ok()).unwrap_or(1.0)).dump(),
                    "/api/check" => conform::check(&spans).dump(),
                    "/api/fingerprint" => {
                        // The signature of what is held right now, plus a band
                        // built from earlier captures if any were named. The
                        // band is what turns a reading into a comparison.
                        let me = finger::invariants(&spans);
                        match q("against") {
                            None => finger::to_json(&me).dump(),
                            Some(list) => {
                                let base: Vec<Vec<finger::Invariant>> = list.split(',')
                                    .filter(|p| !p.trim().is_empty())
                                    .filter_map(|p| Store::open_ro(p.trim()).ok().and_then(|s| s.all().ok()))
                                    .map(|sp| finger::invariants(&sp))
                                    .collect();
                                if base.is_empty() { finger::to_json(&me).dump() }
                                else { finger::compare(&base, &me).dump() }
                            }
                        }
                    }
                    // the small answer to "are we the same": identity and digest, no data
                    "/api/rollup/skeleton" => {
                        let tree = rollup::build(&spans);
                        let depth: usize = q("depth").and_then(|v| v.parse().ok()).unwrap_or(3);
                        rollup::skeleton(&tree, depth).dump()
                    }
                    // =========================================================================
                    // THE COMMAND CONSOLE
                    // =========================================================================
                    // Typing a command in the browser runs the same analysis the
                    // terminal runs, and nothing else. There is no shell here and
                    // no process is started: a name is matched against a list and
                    // the matching function is called in this process, so there is
                    // no argument that can become a command.
                    //
                    // `account` is absent on purpose. Accounts are made on the
                    // machine, and a console that could create one would hand out
                    // over the network exactly what that rule exists to keep off it.
                    // So are `serve` and anything that writes: this reads a capture,
                    // it does not administer an instance.
                    "/api/command" => {
                        let cmd = q("cmd").unwrap_or_default();
                        let name = cmd.split_whitespace().next().unwrap_or("").to_string();
                        let allowed: &[(&str, &str)] = &[
                            ("stats", "counts by host, service and status"),
                            ("health", "is this instance accepting writes, and what does it hold"),
                            ("check", "does this capture conform to the span contract"),
                            ("capture", "is the capture complete, and is it sound"),
                            ("concurrency", "how much ran at once, and where it queued"),
                            ("selftime", "time in a span itself rather than in its children"),
                            ("cost", "what the capture costs to keep"),
                            ("skew", "do the hosts agree on the clock"),
                            ("correlate", "what moves together"),
                            ("fingerprint", "each program named by its behaviour"),
                            ("peers", "other instances this one knows about"),
                            ("views", "saved views of this capture"),
                            ("notes", "findings written beside the capture"),
                            ("stress", "fault injections this capture implies"),
                            ("flame", "folded stacks, weighted by measured self time"),
                            ("critical", "the chain that set the duration, not everything slow"),
                            ("rollup", "the summary tree"),
                        ];
                        if name.is_empty() || name == "help" {
                            let mut o = J::obj();
                            let mut arr: Vec<J> = Vec::new();
                            for (n, what) in allowed {
                                let mut e = J::obj();
                                e.set("command", J::s(n));
                                e.set("answers", J::s(what));
                                arr.push(e);
                            }
                            o.set("commands", J::Arr(arr));
                            o.set("note", J::s(
                                "These read the capture. Anything that administers this \
                                 instance, `account` above all, runs on the machine itself \
                                 and has no route here."));
                            o.dump()
                        } else if !allowed.iter().any(|(n, _)| *n == name) {
                            let mut o = J::obj();
                            o.set("error", J::s(&format!("`{}` is not available here", name)));
                            o.set("hint", J::s(
                                "`help` lists what is. Administration, including creating \
                                 accounts, runs on the machine and deliberately has no route \
                                 over the network."));
                            o.dump()
                        } else {
                            // unreachable: an allowed name was routed above
                            "{}".to_string()
                        }
                    }
                    // a store that cannot be opened is an error to report, not a
                    // panic inside a request thread
                    "/api/notes" => match Store::open_ro(&dbp) {
                        Ok(s) => notes::notes(&s, q("span")).dump(),
                        Err(e) => format!("{{\"error\":\"cannot open the store: {}\"}}", e),
                    },
                    "/api/views" => match Store::open_ro(&dbp) {
                        Ok(s) => notes::views(&s).dump(),
                        Err(e) => format!("{{\"error\":\"cannot open the store: {}\"}}", e),
                    },
                    "/api/flame" => flame::folded(&spans).dump(),
                    "/api/critical" => flame::critical_path(&spans, q("span").as_deref()).dump(),
                    "/api/skew" => skew::to_json(&skew::analyse(&spans)).dump(),
                    "/api/correlate" => relate::correlations(&spans, 5).dump(),
                    "/api/concurrency" => relate::concurrency(&spans).dump(),
                    "/api/cost" => stats::cost_report(&spans, 50).dump(),
                    "/api/stats" => stats::dist_json(&stats::distributions(&spans,
                        q("min-count").and_then(|v| v.parse().ok()).unwrap_or(1))).dump(),
                    "/api/find" => {
                        let needle = q("q").map(|s| s.to_string()).unwrap_or_default();
                        let lim: usize = q("limit").and_then(|v| v.parse().ok()).unwrap_or(40);
                        let hits = find::search(&spans, &needle, lim);
                        find::search_json(&hits, &needle, spans.len()).dump()
                    }
                    "/api/selftime" => find::self_json(&spans,
                        q("limit").and_then(|v| v.parse().ok()).unwrap_or(20)).dump(),
                    "/api/stress" => stress::plan_from_spans(&spans).dump(),
                    "/api/rollup" => {
                        let tree = rollup::build(&spans);
                        let depth: usize = q("depth").and_then(|v| v.parse().ok()).unwrap_or(1);
                        match q("node") {
                            Some(id) => match rollup::find(&tree, id) {
                                Some(n) => n.to_json(depth).dump(),
                                None => "{\"error\":\"no such node\"}".to_string(),
                            },
                            None => {
                                // The summary carries the edges as well as the
                                // nodes. Without them the fleet view shows what
                                // exists and not what moves between it, which is
                                // most of what the view is for. Counts only: no
                                // span identities, no payloads.
                                let mut o = tree.to_json(depth);
                                let mut agg: std::collections::BTreeMap<(String, String), (u64, u64)>
                                    = std::collections::BTreeMap::new();
                                let by_id: std::collections::HashMap<&str, &crate::store::Span> =
                                    spans.iter().map(|s| (s.span_id.as_str(), s)).collect();
                                for s in &spans {
                                    let par = match s.parent_span_id.as_deref() {
                                        Some(p) => match by_id.get(p) { Some(x) => *x, None => continue },
                                        None => continue,
                                    };
                                    let a = format!("{}/{}", par.host_id, par.domain.clone().unwrap_or_default());
                                    let b = format!("{}/{}", s.host_id, s.domain.clone().unwrap_or_default());
                                    if a == b { continue; }
                                    let e = agg.entry((a, b)).or_insert((0, 0));
                                    e.0 += 1;
                                    if s.status == "error" { e.1 += 1; }
                                }
                                let mut edges: Vec<J> = Vec::new();
                                for ((a, b), (n, errs)) in agg {
                                    let mut e = J::obj();
                                    e.set("from", J::s(&a));
                                    e.set("to", J::s(&b));
                                    e.set("count", J::n(n as f64));
                                    e.set("errors", J::n(errs as f64));
                                    edges.push(e);
                                }
                                o.set("edges", J::Arr(edges));
                                o.dump()
                            }
                        }
                    }
                    "/api/logs" => {
                        let filt = logs::Filter {
                            host: q("host"), domain: q("domain"), span: q("span"),
                            level: q("level"), contains: q("contains"),
                            since: q("since").and_then(|v| v.parse().ok()),
                            until: q("until").and_then(|v| v.parse().ok()),
                            errors_only: q("errors").is_some(),
                            under: q("under"),
                            sort: q("sort").unwrap_or("time"),
                            group: q("group"),
                            limit: q("limit").and_then(|v| v.parse().ok()).unwrap_or(200),
                        };
                        logs::to_json(&logs::collect(&spans, &filt)).dump()
                    }
                    "/api/capture" => {
                        let mut o = J::obj();
                        o.set("format", J::s("execviz-replay/1"));
                        o.set("spans", J::Arr(spans.iter().map(|s| s.to_json()).collect()));
                        o.dump()
                    }
                    _ => return http::Resp::Body(404, "application/json".into(), "{\"error\":\"not found\"}".into()),
                };
                // One rule for the whole read surface: a body that reports an
                // error never rides on a 200. Several endpoints answered
                // {"error": ...} with a success status, which a client checking
                // `response.ok` treats as success and then fails to find the
                // fields it expected; the error was invisible to exactly the
                // code written to notice it.
                let status = match json::parse(&body).ok().and_then(|v| {
                    v.get("error").and_then(|e| e.as_str()).map(|s| s.to_string())
                }) {
                    None => 200,
                    Some(msg) => {
                        let lower = msg.to_lowercase();
                        // a named thing that does not exist is 404; anything
                        // else the caller asked for wrongly is 400
                        if lower.contains("no such") || lower.contains("not found") { 404 } else { 400 }
                    }
                };
                http::Resp::Body(status, "application/json".into(), body)
            };
            http::serve(&format!("0.0.0.0:{}", port), handler).expect("serve");
        }

        "node" => {
            let collector = f("collector").expect("--collector URL required").to_string();
            let ndb = f("db").unwrap_or("node.db").to_string();
            let host_id = f("host-id").unwrap_or("remote").to_string();
            let interval: f64 = f("interval").and_then(|v| v.parse().ok()).unwrap_or(1.0);
            let once = flags.contains_key("once");
            println!("execviz node {} -> {} (db={})", host_id, collector, ndb);
            let mut sent: BTreeMap<String, (bool, String)> = BTreeMap::new();
            let mut total = 0usize;
            loop {
                std::thread::sleep(std::time::Duration::from_secs_f64(interval));
                let spans = match Store::open_ro(&ndb).and_then(|s| s.all()) {
                    Ok(s) => s, Err(e) => { println!("[node] waiting: {}", e); if once { break; } continue; }
                };
                // Re-send a span when its second phase lands so completion
                // updates the collector's row rather than duplicating it.
                let batch: Vec<&Span> = spans.iter().filter(|s| {
                    let key = (s.end.is_some(), s.status.clone());
                    match sent.get(&s.span_id) { Some(prev) => *prev != key, None => true }
                }).collect();
                if !batch.is_empty() {
                    let mut o = J::obj();
                    o.set("host_id", J::s(&host_id));
                    o.set("spans", J::Arr(batch.iter().map(|s| s.to_json()).collect()));
                    match http::post(&format!("{}/api/ingest", collector.trim_end_matches('/')), &o.dump()) {
                        Ok(_) => {
                            for s in &batch { sent.insert(s.span_id.clone(), (s.end.is_some(), s.status.clone())); }
                            total += batch.len();
                            println!("[node {}] +{} ({} total)", host_id, batch.len(), total);
                        }
                        Err(e) => println!("[node {}] retry: {}", host_id, e),
                    }
                }
                if once { break; }
            }
        }

        "account" => {
            let sub = pos.get(1).cloned().unwrap_or_else(|| "list".into());
            let st = open_write(&db);
            auth::ensure(&st).expect("accounts table");
            match sub.as_str() {
                "create" => {
                    let name = pos.get(2).cloned().unwrap_or_else(|| usage_error("usage: execviz account <db> create <name> [--password P]"));
                    let role = f("role").unwrap_or("admin");
                    auth::create_account_with_role(&st, &name, f("password"), role)
                        .expect("create account");
                    println!("{{\"account\":\"{}\",\"role\":\"{}\",\"password\":{}}}",
                             name, role, f("password").is_some());
                }
                "add-key" => {
                    let name = pos.get(2).cloned().unwrap_or_else(|| usage_error("usage: execviz account <db> add-key <name> --file id_ed25519.pub"));
                    let line = match f("file") {
                        Some(p) => std::fs::read_to_string(p).expect("read public key"),
                        None => f("key").expect("--file or --key required").to_string(),
                    };
                    match auth::add_ssh_key(&st, &name, line.trim(), f("label")) {
                        Ok(()) => println!("{{\"account\":\"{}\",\"ssh_key\":\"added\"}}", name),
                        Err(e) => println!("{{\"error\":\"{}\"}}", e),
                    }
                }
                "api-key" => {
                    let name = pos.get(2).cloned().unwrap_or_else(|| usage_error("usage: execviz account <db> api-key <name> [--label L]"));
                    let k = auth::create_api_key(&st, &name, f("label")).expect("create api key");
                    // shown once: the store keeps only its hash
                    println!("{{\"account\":\"{}\",\"api_key\":\"{}\",\"note\":\"shown once; store it now\"}}", name, k);
                }
                "revoke" => {
                    let id = pos.get(2).cloned().unwrap_or_else(|| usage_error("usage: execviz account <db> revoke <key_id>"));
                    let n = auth::revoke_api_key(&st, &id).unwrap_or(0);
                    println!("{{\"key_id\":\"{}\",\"revoked\":{}}}", id, n);
                }
                _ => println!("{}", auth::accounts_json(&st).dump()),
            }
        }

        "peer" => {
            let sub = pos.get(1).cloned().unwrap_or_else(|| "list".into());
            // A subcommand this does not know is a usage failure. Falling through
            // to `list` prints an empty peer set, which reads as "no peers" rather
            // than "that is not a command", and the help itself said `request`.
            if !matches!(sub.as_str(), "add" | "request" | "approve" | "revoke" | "list" | "pull") {
                usage_error("usage: execviz peer <db> add <url> | approve <id> | revoke <id> | list | pull");
            }
            let st = open_write(&db);
            peer::ensure(&st).expect("peers table");
            let me = f("identity").unwrap_or("execviz-local").to_string();
            match sub.as_str() {
                "add" | "request" => {
                    let url = pos.get(2).cloned().or_else(|| f("url").map(|u| u.to_string()))
                        .unwrap_or_else(|| usage_error("usage: execviz peer <db> add <url>   (or: request --url URL)"));
                    let mut o = J::obj();
                    o.set("peer_id", J::s(&me));
                    o.set("url", J::s(f("self-url").unwrap_or("")));
                    o.set("token", J::s(f("token").unwrap_or("")));
                    let resp = http::post_with_key(&format!("{}/api/peer/request", url.trim_end_matches('/')), &o.dump(), f("api-key"))
                        .unwrap_or_else(|e| format!("{{\"error\":\"{}\"}}", e));
                    let v = json::parse(&resp).unwrap_or(J::Null);
                    let their_id = v.get("identity").and_then(|x| x.as_str()).unwrap_or("unknown").to_string();
                    let status = if v.get("status").and_then(|x| x.as_str())==Some("approved") { "approved" } else { "pending" };
                    peer::upsert(&st,&their_id,"outbound",Some(&url),status).expect("record peer");
                    // an api key issued by the far end is what we will present
                    if let Some(k) = f("api-key") { let _ = peer::set_key(&st, &their_id, k); }
                    println!("{}", resp);
                }
                "approve" | "revoke" => {
                    let who = pos.get(2).cloned().unwrap_or_else(|| usage_error("usage: execviz peer <db> approve <peer_id>"));
                    let dir = f("direction").unwrap_or("inbound");
                    let status = if sub=="approve" { "approved" } else { "revoked" };
                    let n = peer::set_status(&st,&who,dir,status).unwrap_or(0);
                    println!("{{\"peer\":\"{}\",\"direction\":\"{}\",\"status\":\"{}\",\"updated\":{}}}", who,dir,status,n);
                }
                _ => println!("{}", peer::to_json(&peer::list(&st).unwrap_or_default()).dump()),
            }
        }

        "ask" => {
            // A language for questions nobody anticipated. What is
            // unusual is what it refuses.
            let q = f("q")
                .expect("ask <db> --q \"from spans group by kind show count\"");
            let parsed = match ask::parse(&q) {
                Ok(p) => p,
                Err(e) => { eprintln!("execviz: {}", e); std::process::exit(2); }
            };
            let st = open_read(&db);
            let spans = st.all().expect("read spans");
            let recs = match f("records") {
                Some(p) => syscalls::parse(&std::fs::read_to_string(p).unwrap_or_default()),
                None => Vec::new(),
            };
            println!("{}", ask::run(&parsed, &spans, &recs).dump());
        }

        "otlp" => {
            // Export to the OpenTelemetry span model.
            let st = open_read(&db);
            let spans = st.all().expect("read spans");
            println!("{}", otel::export(&spans).dump());
        }

        "identity" => {
            // Identity by behaviour: what a process IS, from what
            // it does, with no instrumentation and no metadata.
            let path = f("records").expect("--records <file.ndjson> required");
            let txt = match std::fs::read_to_string(&path) {
                Ok(t) => t,
                Err(e) => { eprintln!("execviz: cannot read {}: {}", path, e); std::process::exit(2); }
            };
            let recs = syscalls::parse(&txt);
            let min = f("min-records").and_then(|s| s.parse::<usize>().ok()).unwrap_or(200);
            println!("{}", finger::recorder_identities(&recs, min).dump());
        }

        "drift" => {
            // Behavioural drift with no instrumentation.
            let now = f("records").expect("--records <identity.json> required");
            let base = f("baseline").expect("--baseline <identity.json> required");
            let (a, b) = match (std::fs::read_to_string(&now), std::fs::read_to_string(&base)) {
                (Ok(a), Ok(b)) => (a, b),
                _ => { eprintln!("execviz: cannot read both fingerprints"); std::process::exit(2); }
            };
            match drift::drift(&b, &a) {
                Ok((out, code)) => { println!("{}", out.dump()); if code != 0 { std::process::exit(code); } }
                Err(e) => { eprintln!("execviz: {}", e); std::process::exit(2); }
            }
        }

        "iouring" => {
            // What the syscall boundary cannot show.
            let path = f("records").expect("--records <file.ndjson> required");
            let txt = match std::fs::read_to_string(&path) {
                Ok(t) => t,
                Err(e) => { eprintln!("execviz: cannot read {}: {}", path, e); std::process::exit(2); }
            };
            println!("{}", drift::io_uring(&txt).dump());
        }

        "flame" => {
            // Folded from the span tree: exact for instrumented work, blind to
            // the rest, which is the opposite trade to the sampler.
            let st = open_read(&db);
            let spans = st.all().unwrap_or_default();
            println!("{}", flame::folded(&spans).dump());
        }

        "critical" => {
            // The chain that set the duration, rather than everything slow.
            let st = open_read(&db);
            let spans = st.all().unwrap_or_default();
            println!("{}", flame::critical_path(&spans, f("span")).dump());
        }

        "cpu" => {
            // Where the cpu actually was, from the sampler rather than from spans.
            let path = f("records").expect("--records <samples.ndjson> required");
            let txt = match std::fs::read_to_string(&path) {
                Ok(t) => t,
                Err(e) => { eprintln!("execviz: cannot read {}: {}", path, e); std::process::exit(2); }
            };
            let out = flame::sampled(&txt);
            let n = out.get("samples").and_then(|x| x.as_f64()).unwrap_or(0.0);
            println!("{}", out.dump());
            // A run that sampled nothing and a run on an idle machine look the
            // same in the output, so the exit code separates them.
            if n == 0.0 { std::process::exit(1); }
        }

        "profile" => {
            // The project's own vocabulary over a capture, and a
            // summary small enough to keep so captures can be compared over
            // weeks or months.
            if let (Some(basep), Some(nowp)) = (f("baseline"), f("summary")) {
                let (b, a) = (std::fs::read_to_string(&basep), std::fs::read_to_string(&nowp));
                let (b, a) = match (b, a) {
                    (Ok(b), Ok(a)) => (b, a),
                    _ => { eprintln!("execviz: cannot read both summaries"); std::process::exit(2); }
                };
                match profile::diff(&b, &a) {
                    Ok(out) => { println!("{}", out.dump()); }
                    Err(e) => { eprintln!("execviz: {}", e); std::process::exit(2); }
                }
                return;
            }
            let path = f("records").expect("--records <file.ndjson> required");
            let pp = f("profile").expect("--profile <profile.json> required");
            let txt = match std::fs::read_to_string(&path) {
                Ok(t) => t,
                Err(e) => { eprintln!("execviz: cannot read {}: {}", path, e); std::process::exit(2); }
            };
            let ptxt = match std::fs::read_to_string(&pp) {
                Ok(t) => t,
                Err(e) => { eprintln!("execviz: cannot read {}: {}", pp, e); std::process::exit(2); }
            };
            let (project, inds) = match profile::load(&ptxt) {
                Ok(x) => x,
                // A profile that does not say what it means is a usage failure,
                // not a run that quietly labels nothing.
                Err(e) => { eprintln!("execviz: {}", e); std::process::exit(2); }
            };
            let (out, code) = profile::summarise(&txt, &project, &inds);
            println!("{}", out.dump());
            if code != 0 { std::process::exit(code); }
        }

        "stress" => {
            // A stress plan derived from observed shape. Reports
            // what would be exercised and what would not, and injects nothing.
            let path = f("records").expect("--records <file.ndjson> required");
            let txt = match std::fs::read_to_string(&path) {
                Ok(t) => t,
                Err(e) => { eprintln!("execviz: cannot read {}: {}", path, e); std::process::exit(2); }
            };
            let min = f("min-records").and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(stress::MIN_RECORDS);
            // With a baseline this compares two captures instead of deriving a
            // plan: what changed once the fault was injected.
            if let Some(basep) = f("baseline") {
                let base = match std::fs::read_to_string(&basep) {
                    Ok(t) => t,
                    Err(e) => { eprintln!("execviz: cannot read {}: {}", basep, e); std::process::exit(2); }
                };
                println!("{}", stress::compare(&base, &txt).dump());
                return;
            }
            let (out, code) = stress::plan(&txt, min);
            println!("{}", out.dump());
            // 1 is "the answer is no": there was not enough here to derive a plan.
            if code != 0 { std::process::exit(code); }
        }

        "decode" => {
            // Decoding that reports its own residue.
            let path = f("records").expect("--records <file.ndjson> required");
            let txt = match std::fs::read_to_string(&path) {
                Ok(t) => t,
                Err(e) => { eprintln!("execviz: cannot read {}: {}", path, e); std::process::exit(2); }
            };
            let (out, _r) = decode::decode_floor(&txt);
            println!("{}", out.dump());
        }

        "unclaimed" => {
            // The negative space: what this machine did that no
            // span accounts for. Not a defect list; a coverage picture.
            let path = f("records").expect("--records <file.ndjson> required");
            let txt = match std::fs::read_to_string(&path) {
                Ok(t) => t,
                Err(e) => { eprintln!("execviz: cannot read {}: {}", path, e); std::process::exit(2); }
            };
            let recs = syscalls::parse(&txt);
            let mut comms: std::collections::BTreeMap<i64, String> = Default::default();
            for r in &recs {
                if let Some(c) = &r.comm { comms.entry(r.tid).or_insert_with(|| c.clone()); }
            }
            let st = open_read(&db);
            let spans = st.all().expect("read spans");
            println!("{}", witness::negative_space(&spans, &recs, &comms).dump());
        }

        "detect" => {
            // Detection on shape, not on values.
            let rules_path = f("rules").expect("--rules <file> required");
            let text = match std::fs::read_to_string(&rules_path) {
                Ok(t) => t,
                Err(e) => { eprintln!("execviz: cannot read {}: {}", rules_path, e); std::process::exit(2); }
            };
            let (rules, unknown) = shapes_rules::parse_rules(&text);
            let st = open_read(&db);
            let spans = st.all().expect("read spans");
            let recs = match f("records") {
                Some(p) => syscalls::parse(&std::fs::read_to_string(p).unwrap_or_default()),
                None => Vec::new(),
            };
            // A baseline is another capture's shape, so drift has something to
            // have drifted from.
            let base = f("baseline").map(|p| {
                let bs = open_read(&p);
                finger::invariants(&bs.all().expect("read baseline spans"))
            });
            let (out, o) = shapes_rules::detect(&rules, &unknown, &spans, &recs, base.as_deref());
            println!("{}", out.dump());
            // An unknown rule is a failure, not a pass.
            if o.unknown > 0 { std::process::exit(2); }
            if o.fired > 0 { std::process::exit(1); }
        }

        "bundle" => {
            // A finding, packaged so somebody else can replay it.
            let out_dir = f("to").unwrap_or("execviz-bundle");
            let with_payloads = flags.contains_key("with-payloads");
            let st = open_read(&db);
            let spans = st.all().expect("read spans");
            let floor = match f("records") {
                Some(p) => std::fs::read_to_string(p).unwrap_or_default(),
                None => String::new(),
            };
            let vp = f("viewpoint");
            let (packed, records, spans_doc) =
                bundle::pack(&spans, &floor, vp.as_deref(), with_payloads);
            let sealed = bundle::seal(&packed.manifest, &records, &spans_doc);

            if std::fs::create_dir_all(out_dir).is_err() {
                eprintln!("execviz: cannot create {}", out_dir);
                std::process::exit(2);
            }
            let mut manifest = packed.manifest.clone();
            manifest.set("seal", json::J::Str(sealed.clone()));
            let write = |name: &str, body: &str| {
                let p = format!("{}/{}", out_dir, name);
                if let Err(e) = std::fs::write(&p, body) {
                    eprintln!("execviz: cannot write {}: {}", p, e);
                    std::process::exit(2);
                }
            };
            write("manifest.json", &manifest.dump());
            write("spans.json", &spans_doc.dump());
            write("syscalls.ndjson", &records);
            // The doctor report travels too: half the questions a recipient asks
            // are about the machine rather than the capture.
            let (diag, _) = doctor::diagnose();
            write("machine.json", &diag.dump());

            println!("{}", manifest.dump());
            eprintln!("execviz: bundle written to {}/ (seal {})", out_dir, &sealed[..16]);
            if !with_payloads && packed.withheld > 0 {
                eprintln!("execviz: {} payloads withheld; re-run with --with-payloads \
                           once you have decided that is safe", packed.withheld);
            }
        }

        "scrutiny" => {
            // Did it watch itself the same way?
            let path = f("records").expect("--records <file.ndjson> required");
            let txt = match std::fs::read_to_string(&path) {
                Ok(t) => t,
                Err(e) => { eprintln!("execviz: cannot read {}: {}", path, e); std::process::exit(2); }
            };
            let who: String = match f("recorder") { Some(s) => s.to_string(), None => "execviz-record".to_string() };
            let (out, v) = scrutiny::examine(&txt, &who);
            println!("{}", out.dump());
            // An UNDECLARED exemption is the answer being no.
            if v.undeclared > 0 { std::process::exit(1); }
        }

        "doctor" => {
            // Can this machine run it? Asked before anything is installed,
            // because an install that succeeds and then does not work leaves an
            // operator with a mystery instead of a message.
            // --report adds the distribution and linkage, and is shaped to be
            // pasted into an issue by somebody with a machine nobody here has.
            let (out, ok) = if flags.contains_key("report") {
                let r = doctor::report();
                let ok = r.get("floor_supported").map(|v| matches!(v, json::J::Bool(true))).unwrap_or(false);
                (r, ok)
            } else {
                doctor::diagnose()
            };
            println!("{}", out.dump());
            if !ok { std::process::exit(1); }
        }

        "witness" => {
            // The recorder as witness: put the instrumentation
            // against what the machine did.
            let path = f("records").expect("--records <file.ndjson> required");
            let txt = match std::fs::read_to_string(&path) {
                Ok(t) => t,
                Err(e) => { eprintln!("execviz: cannot read {}: {}", path, e); std::process::exit(2); }
            };
            let recs = syscalls::parse(&txt);
            let st = open_read(&db);
            let spans = st.all().expect("read spans");
            let (report, a) = witness::audit(&spans, &recs);
            println!("{}", report.dump());
            // 1 means the answer is no: the instrumentation and the machine
            // disagree somewhere. Incomplete coverage on its own is not that.
            if a.claimed_not_performed > 0 || a.windows_disagreed > 0 { std::process::exit(1); }
        }

        "syscalls" => {
            let path = f("records").expect("--records <file.ndjson> required");
            let txt = std::fs::read_to_string(path).expect("read records");
            let recs = syscalls::parse(&txt);
            let apply = flags.contains_key("apply");
            let st = if apply { open_write(&db) }
                     else { open_read(&db) };
            let spans = st.all().expect("read spans");
            let (report, _m) = syscalls::merge(&st, &spans, &recs, apply);
            println!("{}", report.dump());
        }

        "logs" => {
            let st = open_read(&db);
            let spans = st.all().expect("read spans");
            let t0 = spans.iter().map(|s| s.start).fold(f64::MAX, f64::min);
            let filt = logs::Filter {
                host: f("host"), domain: f("domain"), span: f("span"),
                level: f("level"), contains: f("contains"),
                since: f("since").and_then(|v| v.parse().ok()),
                until: f("until").and_then(|v| v.parse().ok()),
                errors_only: flags.contains_key("errors"),
                under: f("under"),
                sort: f("sort").unwrap_or("time"),
                group: f("group"),
                limit: f("limit").and_then(|v| v.parse().ok()).unwrap_or(200),
            };
            let lines = logs::collect(&spans, &filt);
            if flags.contains_key("counts") {
                // the shape of the noise, before any of it is read
                println!("{}", logs::counts(&lines).dump());
            } else if flags.contains_key("fold") {
                let groups = logs::fold(lines);
                if flags.contains_key("json") { println!("{}", logs::folded_json(&groups).dump()); }
                else {
                    let base = if t0.is_finite() { t0 } else { 0.0 };
                    for g in &groups {
                        let rep = if g.count > 1 { format!(" ×{}", g.count) } else { String::new() };
                        println!("{:>9.1}  {:<8} {:<20} {}{}",
                            (g.line.t - base) * 1000.0, g.line.level,
                            g.line.span_name.chars().take(20).collect::<String>(), g.line.msg, rep);
                    }
                }
            } else if flags.contains_key("json") { println!("{}", logs::to_json(&lines).dump()); }
            else { print!("{}", logs::render(&lines, filt.group, if t0.is_finite() { t0 } else { 0.0 })); }
        }

        "regress" => {
            let st = open_read(&db);
            let after = st.all().expect("read spans");
            let base = f("against").unwrap_or_else(|| usage_error("usage: execviz regress <db> --against earlier.db"));
            let bst = Store::open_ro(base).expect("open baseline");
            let before = bst.all().expect("read baseline");
            let min_n: usize = f("min-samples").and_then(|v| v.parse().ok()).unwrap_or(5);
            let sens: f64 = f("sensitivity").and_then(|v| v.parse().ok()).unwrap_or(1.0);
            let c = compare::regressions(&before, &after, min_n, sens);
            println!("{}", compare::regressions_json(&c).dump());
            if flags.contains_key("fail-on-regression")
               && c.iter().any(|x| x.verdict == "slower") { std::process::exit(1); }
        }

        "export" => {
            let st = open_read(&db);
            let spans = st.all().expect("read spans");
            match f("format").unwrap_or("chrome") {
                "folded" => print!("{}", compare::folded_stacks(&spans)),
                "chrome" => println!("{}", compare::chrome_trace(&spans).dump()),
                other => println!("{}", {
                    let mut o = J::obj();
                    o.set("error", J::s(&format!("unknown format '{}'", other)));
                    o.set("hint", J::s("try chrome or folded"));
                    o.dump()
                }),
            }
        }

        "seal" => {
            let st = open_read(&db);
            let spans = st.all().expect("read spans");
            match f("verify") {
                None => println!("{}", rollup::seal_json(&spans).dump()),
                Some(expected) => {
                    let got = rollup::seal(&spans);
                    let ok = crate::sha256::constant_time_eq(&got, expected);
                    println!("{{\"intact\":{},\"expected\":\"{}\",\"actual\":\"{}\"}}", ok, expected, got);
                    if !ok { std::process::exit(1); }
                }
            }
        }

        "skew" => {
            let st = open_read(&db);
            let spans = st.all().expect("read spans");
            println!("{}", skew::to_json(&skew::analyse(&spans)).dump());
        }

        // Liveness for a container or a monitor: is the server answering?
        // Deliberately says nothing about the capture, so it needs no credential
        // and leaks nothing.
        "probe" => {
            // the first positional of this subcommand is a URL, not a capture:
            // `db` is the shared positional every other subcommand takes
            let url = f("url").map(|s| s.to_string())
                .or_else(|| pos.get(1).cloned())
                .or_else(|| if db.starts_with("http") { Some(db.clone()) } else { None })
                .unwrap_or_else(|| "http://127.0.0.1:8900/".to_string());
            match http::get(&url) {
                Ok(_) => println!("{{\"answering\":true,\"url\":\"{}\"}}", url),
                Err(e) => {
                    println!("{{\"answering\":false,\"url\":\"{}\",\"error\":\"{}\"}}", url, e);
                    std::process::exit(1);
                }
            }
        }

        "audit" => {
            let st = open_write(&db);
            println!("{}", audit::read(&st, f("limit").and_then(|v| v.parse().ok()).unwrap_or(50)).dump());
        }

        "note" => {
            let st = open_write(&db);
            match f("add") {
                Some(body) => {
                    notes::add_note(&st, f("span"), body, f("author")).expect("add note");
                    println!("{{\"noted\":true}}");
                }
                None => println!("{}", notes::notes(&st, f("span")).dump()),
            }
        }

        "view" if flags.contains_key("save") || flags.contains_key("list") => {
            let st = open_write(&db);
            if let Some(name) = f("save") {
                let state = f("state").unwrap_or("");
                notes::save_view(&st, name, state, f("author"), f("note")).expect("save view");
                println!("{{\"saved\":\"{}\"}}", name);
            } else {
                println!("{}", notes::views(&st).dump());
            }
        }

        "report" => {
            let st = open_read(&db);
            let spans = st.all().expect("read spans");
            let from = f("from").and_then(|v| v.parse().ok());
            let to = f("to").and_then(|v| v.parse().ok());
            print!("{}", notes::report(&st, &spans, from, to));
        }

        "watch" => {
            let path = f("rules").unwrap_or_else(|| usage_error("usage: execviz watch <db> --rules FILE [--interval S]"));
            let rules = watch::rules_from(path);
            let every: f64 = f("interval").and_then(|v| v.parse().ok()).unwrap_or(2.0);
            let once = flags.contains_key("once");
            let mut st8 = watch::WatchState::new();
            loop {
                let spans = Store::open_ro(&db).and_then(|s| s.all()).unwrap_or_default();
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs_f64()).unwrap_or(0.0);
                let ev = st8.evaluate(&spans, &rules, now);
                if !ev.is_empty() {
                    println!("{}", watch::firing_json(&ev, st8.currently_firing()).dump());
                }
                if once { break; }
                std::thread::sleep(std::time::Duration::from_secs_f64(every));
            }
        }

        "step" => {
            let st = open_read(&db);
            let spans = st.all().expect("read spans");
            let steps = step::timeline(&spans, f("trace"));
            let from: usize = f("from").and_then(|v| v.parse().ok()).unwrap_or(0);
            let count: usize = f("count").and_then(|v| v.parse().ok()).unwrap_or(40);
            if flags.contains_key("json") {
                println!("{}", step::to_json(&steps, from, count).dump());
            } else {
                print!("{}", step::text(&steps, from, count));
                println!("--- {} steps; this replays the record, not the program", steps.len());
            }
        }

        "functions" => {
            let st = open_read(&db);
            let spans = st.all().expect("read spans");
            println!("{}", watch::functions(&spans).dump());
        }

        "sampling" => {
            match f("declare") {
                Some(rule) => {
                    let st = open_write(&db);
                    let rate: f64 = f("rate").and_then(|v| v.parse().ok()).unwrap_or(1.0);
                    watch::declare_sampling(&st, rule, rate).expect("declare sampling");
                    println!("{{\"rule\":\"{}\",\"rate\":{}}}", rule, rate);
                }
                None => {
                    let st = open_read(&db);
                    let n = st.all().map(|s| s.len()).unwrap_or(0);
                    println!("{}", watch::describe_counts(&st, n).dump());
                }
            }
        }

        "backup" => {
            let st = open_read(&db);
            let dest = f("to").unwrap_or_else(|| usage_error("usage: execviz backup <db> --to FILE"));
            match watch::backup(&st, dest) {
                Ok(j) => { let ok = j.get("verified") == Some(&J::Bool(true));
                           println!("{}", j.dump()); if !ok { std::process::exit(1); } }
                Err(e) => { println!("{{\"error\":\"{}\"}}", e); std::process::exit(1); }
            }
        }

        "egress" => {
            let st = open_read(&db);
            let spans = st.all().expect("read spans");
            let allowed: Vec<String> = match f("allowed") {
                Some(p) => std::fs::read_to_string(p).expect("read allow list").lines()
                    .map(|l| l.split('#').next().unwrap_or("").trim().to_string())
                    .filter(|l| !l.is_empty()).collect(),
                None => vec![],
            };
            let out = egress::egress(&spans, &allowed);
            let ok = out.get("all_expected") == Some(&J::Bool(true));
            println!("{}", out.dump());
            if !ok && flags.contains_key("fail-on-unexpected") { std::process::exit(1); }
        }

        "attempts" => {
            let st = open_read(&db);
            let spans = st.all().expect("read spans");
            println!("{}", egress::attempts(&spans).dump());
        }

        "integrity" => {
            let st = open_read(&db);
            let spans = st.all().expect("read spans");
            let out = egress::integrity(&st, &spans);
            let ok = out.get("sound") == Some(&J::Bool(true));
            println!("{}", out.dump());
            if !ok { std::process::exit(1); }
        }

        "shape" => {
            let st = open_read(&db);
            let spans = st.all().expect("read spans");
            match f("against") {
                None => println!("{}", expect::propose_shape(&spans).dump()),
                Some(path) => {
                    let text = std::fs::read_to_string(path).expect("read shape");
                    let sh = expect::parse_shape(&text);
                    let out = expect::check_shape(&spans, &sh);
                    let ok = out.get("matches") == Some(&J::Bool(true));
                    println!("{}", out.dump());
                    if !ok && flags.contains_key("fail-on-departure") { std::process::exit(1); }
                }
            }
        }

        "whatif" => {
            let st = open_read(&db);
            let spans = st.all().expect("read spans");
            let target = f("span").unwrap_or_else(|| usage_error("usage: execviz whatif <db> --span NAME [--faster 0.5]"));
            let factor: f64 = f("faster").and_then(|v| v.parse().ok()).unwrap_or(0.5);
            println!("{}", expect::counterfactual(&spans, target, factor).dump());
        }

        "across" => {
            let list = f("runs").unwrap_or_else(|| usage_error("usage: execviz across --runs a.db,b.db,c.db"));
            let runs: Vec<(String, Vec<Span>)> = list.split(',')
                .filter(|p| !p.trim().is_empty())
                .filter_map(|p| Store::open_ro(p.trim()).ok()
                    .and_then(|s| s.all().ok()).map(|sp| (p.trim().to_string(), sp)))
                .collect();
            println!("{}", expect::across_runs(&runs).dump());
        }

        "correlate" => {
            let st = open_read(&db);
            let spans = st.all().expect("read spans");
            let sup: usize = f("min-support").and_then(|v| v.parse().ok()).unwrap_or(5);
            println!("{}", relate::correlations(&spans, sup).dump());
        }

        "concurrency" => {
            let st = open_read(&db);
            let spans = st.all().expect("read spans");
            println!("{}", relate::concurrency(&spans).dump());
        }

        "cost" => {
            let st = open_read(&db);
            let spans = st.all().expect("read spans");
            println!("{}", stats::cost_report(&spans,
                f("limit").and_then(|v| v.parse().ok()).unwrap_or(20)).dump());
        }

        "stats" => {
            let st = open_read(&db);
            let spans = st.all().expect("read spans");
            let minc: usize = f("min-count").and_then(|v| v.parse().ok()).unwrap_or(1);
            println!("{}", stats::dist_json(&stats::distributions(&spans, minc)).dump());
        }

        "assert" => {
            let st = open_read(&db);
            let spans = st.all().expect("read spans");
            let path = f("rules").unwrap_or_else(|| usage_error("usage: execviz assert <db> --rules FILE"));
            let text = std::fs::read_to_string(path).expect("read rules");
            let rules = stats::parse_rules(&text);
            let fails = stats::assert_all(&spans, &rules);
            println!("{}", stats::assert_json(&fails, rules.len()).dump());
            // a regression guard has to be able to fail a build
            if !fails.is_empty() { std::process::exit(1); }
        }

        "coverage" => {
            let st = open_read(&db);
            let spans = st.all().expect("read spans");
            let path = f("expected").unwrap_or_else(|| usage_error("usage: execviz coverage <db> --expected FILE"));
            let text = std::fs::read_to_string(path).expect("read expected");
            let want: Vec<String> = text.lines().map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty() && !l.starts_with('#')).collect();
            println!("{}", stats::coverage(&spans, &want).dump());
        }

        "find" => {
            let st = open_read(&db);
            let spans = st.all().expect("read spans");
            let q = f("q").map(|s| s.to_string())
                .or_else(|| pos.get(1).cloned())
                .unwrap_or_else(|| usage_error("usage: execviz find <db> <text|key=value> [--limit N]"));
            let lim: usize = f("limit").and_then(|v| v.parse().ok()).unwrap_or(40);
            let hits = find::search(&spans, &q, lim);
            println!("{}", find::search_json(&hits, &q, spans.len()).dump());
        }

        "selftime" => {
            let st = open_read(&db);
            let spans = st.all().expect("read spans");
            let lim: usize = f("limit").and_then(|v| v.parse().ok()).unwrap_or(20);
            println!("{}", find::self_json(&spans, lim).dump());
        }

        "critpath" => {
            let st = open_read(&db);
            let spans = st.all().expect("read spans");
            // default to the longest root, which is the request a person means
            let root = match f("span") {
                Some(s) => s.to_string(),
                None => spans.iter().filter(|s| s.parent_span_id.is_none())
                    .max_by(|a, b| a.duration_ms().unwrap_or(0.0)
                        .partial_cmp(&b.duration_ms().unwrap_or(0.0)).unwrap())
                    .map(|s| s.span_id.clone()).unwrap_or_default(),
            };
            println!("{}", find::path_json(&find::critical_path(&spans, &root)).dump());
        }

        "trim" => {
            let apply = flags.contains_key("apply");
            let st = if apply { open_write(&db) }
                     else { open_read(&db) };
            let spans = st.all().expect("read spans");
            let older: f64 = f("older-than-secs").and_then(|v| v.parse().ok()).unwrap_or(0.0);
            let keep: usize = f("keep-last-traces").and_then(|v| v.parse().ok()).unwrap_or(0);
            let now = retain::now_secs();
            let p = retain::plan(&spans, older, keep, now);
            if apply { retain::apply(&st, &p).expect("trim"); }
            println!("{}", retain::to_json(&p, apply).dump());
        }

        "sync" => {
            // Read-only and symmetric: asking what differs changes nothing on
            // either side. What comes back is a list of subtrees to reconcile.
            let url = f("with").unwrap_or_else(|| usage_error("usage: execviz sync <db> --with URL [--api-key K]"));
            let st = open_read(&db);
            let spans = st.all().expect("read spans");
            let mine = rollup::build(&spans);
            let depth = f("depth").unwrap_or("3");
            let target = format!("{}/api/rollup/skeleton?depth={}", url.trim_end_matches('/'), depth);
            match http::get_with_key(&target, f("api-key")) {
                Err(e) => println!("{{\"error\":\"{}\"}}", e),
                Ok(body) => match json::parse(&body) {
                    Err(e) => println!("{{\"error\":\"unreadable skeleton: {}\"}}", e),
                    Ok(theirs) => {
                        let mut out = Vec::new();
                        rollup::diverge(&mine, &theirs, &mut out);
                        println!("{}", rollup::divergence_json(&out, rollup::count_nodes(&mine)).dump());
                    }
                }
            }
        }

        "rollup" => {
            let st = open_read(&db);
            let spans = st.all().expect("read spans");
            let tree = rollup::build(&spans);
            let depth: usize = f("depth").and_then(|v| v.parse().ok()).unwrap_or(1);
            let out = match f("node") {
                Some(id) => match rollup::find(&tree, id) {
                    Some(n) => n.to_json(depth),
                    None => { let mut o = J::obj(); o.set("error", J::s("no such node")); o }
                },
                None => tree.to_json(depth),
            };
            println!("{}", out.dump());
        }

        "fingerprint" => {
            let st = open_read(&db);
            let spans = st.all().expect("read spans");
            let me = finger::invariants(&spans);
            match f("against") {
                None => println!("{}", finger::to_json(&me).dump()),
                Some(list) => {
                    // several baseline captures give the band; one gives a line
                    let base: Vec<Vec<finger::Invariant>> = list.split(',')
                        .filter(|p| !p.trim().is_empty())
                        .filter_map(|p| Store::open_ro(p.trim()).ok().and_then(|s| s.all().ok()))
                        .map(|sp| finger::invariants(&sp))
                        .collect();
                    if base.is_empty() {
                        println!("{{\"error\":\"no readable baseline captures\"}}");
                    } else {
                        println!("{}", finger::compare(&base, &me).dump());
                    }
                }
            }
        }

        "view" | "query" | "diff" | "capture" | "check" => {
            let st = open_read(&db);
            let spans = st.all().expect("read spans");
            let out = match cmd.as_str() {
                "view" => views::view(&spans, f("lod").unwrap_or("system"),
                    f("host"), f("cluster"), f("family"), f("span")),
                "query" => views::query(&spans, f("q").unwrap_or("stale"), f("span"), limit,
                    f("min-overlap-ms").and_then(|v| v.parse().ok()).unwrap_or(1.0)),
                "check" => conform::check(&spans),
                "diff" => {
                    let path = require(f("against"), "diff needs --against <capture.json>");
                    let txt = match std::fs::read_to_string(path) {
                        Ok(t) => t,
                        Err(e) => usage_error(&format!("cannot read '{}': {}", path, e)),
                    };
                    let cap = match json::parse(&txt) {
                        Ok(c) => c,
                        Err(e) => usage_error(&format!("'{}' is not a readable capture: {}", path, e)),
                    };
                    let a: Vec<Span> = cap.get("spans").and_then(|s| s.as_arr())
                        .map(|arr| arr.iter().filter_map(|v| Span::from_json(v, None)).collect())
                        .unwrap_or_default();
                    views::diff(&a, &spans)
                }
                _ => {
                    let mut o = J::obj();
                    o.set("format", J::s("execviz-replay/1"));
                    o.set("spans", J::Arr(spans.iter().map(|s| s.to_json()).collect()));
                    o
                }
            };
            println!("{}", out.dump());
            // `check` is the command most likely to sit in a pipeline, and it
            // was the only one of its family that reported a failure and exited
            // 0; so a capture with adapter violations passed CI silently.
            // Observations are not failures: they describe the program, not the
            // adapter.
            if cmd == "check" && out.get("conformant") == Some(&J::Bool(false)) {
                std::process::exit(1);
            }
        }

        // Help when it was asked for, a usage error when it was not.
        //
        // A mistyped subcommand used to print help and exit 0, so a typo in a CI
        // script passed silently; the same class of failure as a rule the
        // assertion checker does not recognise being treated as a pass.
        cmd => {
            let asked_for_help = matches!(cmd, "help" | "--help" | "-h" | "");
            if !asked_for_help {
                eprintln!("execviz: '{}' is not a command", cmd);
            }
            let usage = format!("execviz 0.9.0
  execviz serve   <db> [--port N] [--collect] [--ui FILE]
  execviz node    --collector URL --db FILE [--host-id ID] [--interval S] [--once]
  execviz view    <db> --lod system|field|cluster|channel|span [--host H] [--cluster C] [--family F] [--span ID]
  execviz query   <db> --q stale|errors|races|slowest|hotpaths|descendants|ancestors [--span ID] [--limit N]
  execviz diff    <db> --against capture.json
  execviz logs    <db> [--host H] [--domain D] [--span S] [--under SPAN_ID]
                       [--level info|warning|error] [--contains TEXT]
                       [--sort time|level|domain|span|host] [--group span|domain|host|level]
                       [--fold] [--counts]
                       [--errors] [--limit N] [--json]
  execviz syscalls <db> --records FILE.ndjson [--apply]
                       merge a syscall stream into the semantic one
  execviz regress <db> --against earlier.db [--min-samples N] [--fail-on-regression]
  execviz export  <db> [--format chrome|folded]   open it in another tool
  execviz seal    <db> [--verify HASH]       is this the capture that was taken?
  execviz skew    <db>                       do the hosts agree about time?
  execviz account <db> create <name> [--password P] [--role admin|viewer]
  execviz account <db> api-key <name> [--label L] | ssh-add <name> --key FILE | list
  execviz peer    <db> request --url URL | approve <id> | list | pull
  execviz bundle    <db> [--records FILE] [--to DIR] [--viewpoint Q] [--with-payloads]
                       package a finding so somebody else can replay it
  execviz scrutiny  --records FILE [--recorder NAME]  did it watch itself the same way?
  execviz doctor [--report]                 can this machine run the recorder? --report is pasteable into an issue
  execviz detect    <db> --rules FILE [--records FILE] [--baseline DB]
                       fire on SHAPES: stuck, orphaned, inverted, drifted, unwitnessed, dark
  execviz witness   <db> --records FILE     compare each span against the syscalls its thread made
  execviz unclaimed <db> --records FILE     syscalls with no span covering them, by program
  execviz decode    --records FILE          parse captured payloads; report the fraction not parsed
  execviz identity  --records FILE          fingerprint each program from its syscall shape
  execviz stress    --records FILE          list fault injections implied by a capture, and those excluded
  execviz drift     --records ID --baseline ID   compare fingerprints against a baseline; report moved invariants
  execviz iouring   --records FILE          count io_uring submissions, which carry work this capture omits
  execviz cpu       --records FILE          fold sampled stacks: where the cpu was, including uninstrumented code
  execviz flame     <db>                    fold the span tree by measured self time
  execviz critical  <db>                    the chain that set the duration, not everything slow
  execviz profile   --records FILE --profile P   count a capture's records against a project's indicators
  execviz profile   --baseline S1 --summary S2   compare two profile summaries: appeared, stopped, count moved
  execviz ask       <db> --q QUERY          query the capture directly
  execviz otlp      <db>                    export to the OpenTelemetry span model, naming what is lost
  execviz probe   [URL]                     is the server answering? (liveness)
  execviz audit   <db> [--limit N]           who read this capture
  execviz note    <db> [--add TEXT --span ID --author WHO]   keep a finding
  execviz view    <db> --save NAME --state FRAGMENT | --list  saved views
  execviz report  <db> [--from T --to T]     assemble the investigation as text
  execviz watch   <db> --rules FILE [--interval S] [--once]   raise a hand
  execviz step    <db> [--trace ID] [--from N] [--count N] [--json]   walk the record
  execviz functions <db>                    cold starts and freezes, derived
  execviz sampling <db> [--declare RULE --rate R]   is this capture a sample?
  execviz backup  <db> --to FILE            a consistent, verified copy
  execviz egress  <db> [--allowed FILE] [--fail-on-unexpected]  where did it go?
  execviz attempts <db>                     declared retries of one operation
  execviz integrity <db>                    is this file sound?; exits 1 if not
  execviz shape   <db> [--against shape.txt] [--fail-on-departure]
  execviz whatif  <db> --span NAME [--faster 0.5]
  execviz across  --runs a.db,b.db,...        flakiness across many runs
  execviz correlate <db> [--min-support N]   what co-occurs with failure
  execviz concurrency <db>                  how much ran at once
  execviz cost    <db> [--limit N]           working or waiting?
  execviz stats   <db> [--min-count N]       distributions per span name
  execviz assert  <db> --rules FILE         what the project says about itself; exits 1 on failure
  execviz coverage <db> --expected FILE     what never ran
  execviz find    <db> <text|key=value> [--limit N]   search names, kinds, hosts, domains, attributes
  execviz selftime <db> [--limit N]          time a span spent itself, not in its children
  execviz critpath <db> [--span ID]          the chain that set the total
  execviz trim    <db> [--older-than-secs S] [--keep-last-traces N] [--apply]
  execviz sync    <db> --with URL [--api-key K] [--depth N]
                       compare two stores by digest and report what differs
  execviz rollup  <db> [--node ID] [--depth N]   content-addressed tiers
  execviz fingerprint <db> [--against a.db,b.db,...]
                       the signature invariants, or a candidate read against a baseline
  execviz check   <db>                       conformance of a capture; exits 1 on a violation
  execviz capture <db>

exit codes: 0 success  ·  1 the command ran and the answer was no  ·  2 usage");
            if asked_for_help {
                println!("{}", usage);
            } else {
                eprintln!("{}", usage);
                std::process::exit(2);
            }
        }
    }
}
