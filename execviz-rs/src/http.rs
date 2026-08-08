// ========================================================================
//  MANIFEST
// ========================================================================
//  script_name: http.rs
//  script_path: execviz-rs/src/http.rs
//  module_name: http
//  version: 0.53.1
//  description: A small threaded HTTP server and client. The surface is fixed and narrow, so a full async stack would add build weight and deployment size for no gain.
//  kind: module
//  spec: internal
//  internal_dependencies: 
//  external_dependencies: std
//  features: http
//  api_version: execvis-v1.0.0
//  last_updated: 2026-08-07
// ========================================================================

//! A small threaded HTTP server and client. The surface is fixed and narrow, so
//! a full async stack would add build weight and deployment size for no gain.
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs};

// ========================================================================
// TYPES
// ========================================================================

pub struct Req {
    pub method: String,
    pub path: String,
    pub query: BTreeMap<String, String>,
    pub body: String,
    pub headers: BTreeMap<String, String>,
}

// ========================================================================
// IMPLEMENTATIONS
// ========================================================================

impl Req {
    pub fn cookie(&self, name: &str) -> Option<String> {
        let raw = self.headers.get("cookie")?;
        for part in raw.split(';') {
            let (k, v) = part.split_once('=')?;
            if k.trim() == name { return Some(v.trim().to_string()); }
        }
        None
    }
    pub fn bearer(&self) -> Option<String> {
        if let Some(k) = self.headers.get("x-execviz-key") { return Some(k.clone()); }
        self.headers.get("authorization")
            .and_then(|a| a.strip_prefix("Bearer ").map(|s| s.to_string()))
    }
}

// ========================================================================
// CONSTANTS
// ========================================================================

/// A response is either a complete body or a stream the handler feeds until the
/// client goes away. Streaming is what server-sent events need; nothing above
/// the transport changes.
/// The largest request body accepted, before allocation.
///
/// Ingest separately bounds a batch by span count (spec 5.6, gap 33); this
/// bounds the bytes, which is the limit that protects the process itself.
pub const MAX_BODY: usize = 64 * 1024 * 1024;

// ========================================================================
// TYPES
// ========================================================================

pub enum Resp {
    Body(u16, String, String),
    /// A 200 that also sets a cookie: the one response that has to.
    Cookie(String, String),
    Stream(Box<dyn FnMut(&mut dyn Write) -> std::io::Result<()> + Send>),
}

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

pub fn serve<F>(addr: &str, handler: F) -> std::io::Result<()>
where F: Fn(&Req) -> Resp + Send + Sync + 'static {
    let listener = TcpListener::bind(addr)?;
    let handler = std::sync::Arc::new(handler);
    for stream in listener.incoming() {
        let stream = match stream { Ok(s) => s, Err(_) => continue };
        let h = handler.clone();
        std::thread::spawn(move || {
            // One bad request must not silently drop a connection. A panic here
            // unwinds only this thread, so the server survives either way; but
            // without this the client waits for a reply that never comes, which
            // looks like a hang rather than a fault. Catching it turns an
            // invisible failure into a 500 the caller can act on.
            let peer = stream.try_clone().ok();
            let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _ = handle(stream, h);
            }));
            if caught.is_err() {
                if let Some(mut s) = peer {
                    use std::io::Write;
                    let body = "{\"error\":\"the server failed while handling this request\",\"hint\":\"this is a defect; the request was not completed\"}";
                    let _ = write!(s, "HTTP/1.1 500 Internal Server Error\r\n\
Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(), body);
                    let _ = s.flush();
                }
            }
        });
    }
    Ok(())
}

// ========================================================================
// INTERNALS
// ========================================================================

fn handle<F>(mut stream: TcpStream, handler: std::sync::Arc<F>) -> std::io::Result<()>
where F: Fn(&Req) -> Resp {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("GET").to_string();
    let target = parts.next().unwrap_or("/").to_string();
    let mut clen = 0usize;
    let mut headers: BTreeMap<String, String> = BTreeMap::new();
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h)? == 0 { break; }
        if h.trim().is_empty() { break; }
        if let Some((k, v)) = h.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
        let lower = h.to_ascii_lowercase();
        if let Some(v) = lower.strip_prefix("content-length:") {
            clen = v.trim().parse().unwrap_or(0);
        }
    }
    // A body is bounded before a byte of it is allocated. Trusting
    // Content-Length would let one unauthenticated request claim gigabytes and
    // take the process down by allocation alone; the header is a claim by the
    // sender, not a fact about the request.
    if clen > MAX_BODY {
        let msg = format!("{{\"error\":\"body of {} bytes exceeds the limit of {}\",\"hint\":\"send smaller batches\"}}",
                          clen, MAX_BODY);
        let head = format!("HTTP/1.1 413 Payload Too Large\r\nContent-Type: application/json\r\n\
Content-Length: {}\r\nConnection: close\r\n\r\n{}", msg.len(), msg);
        stream.write_all(head.as_bytes())?;
        return stream.flush();
    }
    let mut body = vec![0u8; clen];
    // read_exact on a sender that promised more than it sends would block until
    // the socket times out, so a short read is an error rather than a wait
    if clen > 0 { reader.read_exact(&mut body)?; }

    let (path, qs) = match target.split_once('?') { Some((p, q)) => (p, q), None => (target.as_str(), "") };
    let mut query = BTreeMap::new();
    for pair in qs.split('&').filter(|s| !s.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        query.insert(urldecode(k), urldecode(v));
    }
    let req = Req { method, path: path.to_string(), query,
                    body: String::from_utf8_lossy(&body).to_string(), headers };
    match handler(&req) {
        Resp::Stream(mut feed) => {
            let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\
Cache-Control: no-cache\r\nAccess-Control-Allow-Origin: *\r\nConnection: keep-alive\r\n\r\n";
            stream.write_all(head.as_bytes())?;
            stream.flush()?;
            return feed(&mut stream);
        }
        Resp::Cookie(cookie, out) => {
            let head = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nSet-Cookie: {}\r\n\
Content-Length: {}\r\nConnection: close\r\n\r\n", cookie, out.len());
            stream.write_all(head.as_bytes())?;
            stream.write_all(out.as_bytes())?;
            return stream.flush();
        }
        Resp::Body(code, ctype, out) => {
    let reason = match code { 200 => "OK", 404 => "Not Found", 400 => "Bad Request", _ => "Error" };
    let head = format!("HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\n\
Access-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n", code, reason, ctype, out.len());
    stream.write_all(head.as_bytes())?;
    stream.write_all(out.as_bytes())?;
    stream.flush()
        }
    }
}

fn urldecode(s: &str) -> String {
    let b: Vec<u8> = s.bytes().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => {
                let hi = (b[i+1] as char).to_digit(16).unwrap_or(0) as u8;
                let lo = (b[i+2] as char).to_digit(16).unwrap_or(0) as u8;
                out.push(hi * 16 + lo); i += 3;
            }
            b'+' => { out.push(b' '); i += 1; }
            c => { out.push(c); i += 1; }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

/// GET, used by the peer pull loop.
pub fn get(url: &str) -> std::io::Result<String> { request("GET", url, None, None) }

/// GET presenting a credential, which is how a peer proves who it is.
pub fn get_with_key(url: &str, key: Option<&str>) -> std::io::Result<String> {
    request("GET", url, None, key)
}

/// POST used to reach another instance.
pub fn post(url: &str, body: &str) -> std::io::Result<String> { request("POST", url, Some(body), None) }

/// POST presenting a credential: the peering handshake needs one too, because
/// an instance that requires an account requires it of everyone, including the
/// peer trying to introduce itself.
pub fn post_with_key(url: &str, body: &str, key: Option<&str>) -> std::io::Result<String> {
    request("POST", url, Some(body), key)
}

// ========================================================================
// INTERNALS
// ========================================================================

fn request(method: &str, url: &str, body: Option<&str>, key: Option<&str>) -> std::io::Result<String> {
    let rest = url.strip_prefix("http://").unwrap_or(url);
    let (hostport, path) = match rest.split_once('/') {
        Some((h, p)) => (h.to_string(), format!("/{}", p)),
        None => (rest.to_string(), "/".to_string()),
    };
    let addr = if hostport.contains(':') { hostport.clone() } else { format!("{}:80", hostport) };
    let sa = addr.to_socket_addrs()?.next()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::Other, "bad addr"))?;
    let mut s = TcpStream::connect_timeout(&sa, std::time::Duration::from_secs(8))?;
    let payload = body.unwrap_or("");
    // a credential, when the caller has one to present
    let auth = key.map(|k| format!("X-Execviz-Key: {}\r\n", k)).unwrap_or_default();
    let req = format!("{} {} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\n{}Content-Length: {}\r\nConnection: close\r\n\r\n{}", method, path, hostport, auth, payload.len(), payload);
    s.write_all(req.as_bytes())?;
    let mut resp = String::new();
    s.read_to_string(&mut resp)?;
    Ok(resp.split("\r\n\r\n").nth(1).unwrap_or("").to_string())
}
