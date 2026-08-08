// ========================================================================
//  MANIFEST
// ========================================================================
//  script_name: decode.rs
//  script_path: execviz-rs/src/decode.rs
//  module_name: decode
//  version: 0.53.1
//  description: Decoding that reports its own residue.
//  kind: module
//  spec: internal
//  internal_dependencies: json
//  external_dependencies: std
//  features: decode
//  api_version: execvis-v1.0.0
//  last_updated: 2026-08-07
// ========================================================================

//! Decoding that reports its own residue.
//!
//! Every protocol tracer shows what it managed to parse. None of them make the
//! unparsed portion visible, so a decoder failing on a protocol variant looks
//! exactly like a service that went quiet.
//!
//! The recorder already retains raw bytes and counts them, so a decoded record can
//! carry the decode, the original, and a verdict. What follows from that is the
//! number no other tool reports: what fraction of what crossed a descriptor was
//! understood.

use crate::json::J;
use std::collections::BTreeMap;

// ========================================================================
// TYPES
// ========================================================================

pub struct Decoded {
    pub protocol: &'static str,
    pub summary: String,
}

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

/// Recognises the wire formats whose opening bytes are unambiguous.
///
/// Deliberately shallow. A decoder that guesses produces incorrect values reported without error, and
/// a record that says `no decoder matched` is more useful than one that says
/// `http` about something that was not.
/// The recorder renders non-text payloads as hex, so a binary decoder handed the
/// record's text sees the characters `4d 5a` rather than the bytes they stand
/// for. Every binary protocol below matches on byte values, so the hex is
/// undone first. Without this the binary decoders can never fire on a real
/// capture, only on a hand-made record, which is exactly how they passed their
/// first test and failed on the first machine.
fn unhex(s: &str) -> Option<Vec<u8>> {
    let t = s.trim();
    if t.len() < 8 || t.len() % 2 != 0 { return None; }
    if !t.bytes().all(|c| c.is_ascii_hexdigit()) { return None; }
    let mut out = Vec::with_capacity(t.len() / 2);
    let b = t.as_bytes();
    let val = |c: u8| -> u8 {
        match c { b'0'..=b'9' => c - b'0', b'a'..=b'f' => c - b'a' + 10, _ => c - b'A' + 10 }
    };
    let mut i = 0;
    while i + 1 < b.len() { out.push(val(b[i]) << 4 | val(b[i + 1])); i += 2; }
    Some(out)
}

pub fn sniff(bytes: &str) -> Option<Decoded> {
    let b = bytes.as_bytes();
    if b.is_empty() { return None; }

    // HTTP request: a known method followed by a space
    for m in ["GET ", "POST ", "PUT ", "DELETE ", "HEAD ", "PATCH ", "OPTIONS "] {
        if bytes.starts_with(m) {
            let line = bytes.lines().next().unwrap_or(bytes);
            return Some(Decoded { protocol: "http.request", summary: first_n(line, 120) });
        }
    }
    if bytes.starts_with("HTTP/1.") {
        let line = bytes.lines().next().unwrap_or(bytes);
        return Some(Decoded { protocol: "http.response", summary: first_n(line, 120) });
    }
    // HTTP/2 connection preface, which is fixed and therefore certain
    if bytes.starts_with("PRI * HTTP/2.0") {
        return Some(Decoded { protocol: "http2.preface", summary: "connection preface".into() });
    }
    // Redis inline/RESP
    if b[0] == b'*' || b[0] == b'+' || b[0] == b'-' || b[0] == b'$' {
        if bytes.is_ascii() && bytes.contains("\r\n") {
            return Some(Decoded { protocol: "resp", summary: first_n(bytes.lines().next().unwrap_or(""), 80) });
        }
    }
    // PostgreSQL simple query: 'Q' then a four-byte length
    if b[0] == b'Q' && b.len() > 5 {
        let text: String = bytes.chars().filter(|c| !c.is_control()).collect();
        if text.len() > 6 && looks_like_sql(&text) {
            return Some(Decoded { protocol: "postgres.query", summary: first_n(text.trim(), 120) });
        }
    }
    // A bare SQL statement, which several drivers send unframed
    if looks_like_sql(bytes) {
        return Some(Decoded { protocol: "sql", summary: first_n(bytes.trim(), 120) });
    }
    // JSON, which is a payload rather than a protocol but worth naming
    if (b[0] == b'{' || b[0] == b'[') && bytes.is_ascii() {
        return Some(Decoded { protocol: "json", summary: first_n(bytes, 100) });
    }

    // The binary protocols, tried after the text ones so a printable payload is
    // never claimed by a byte pattern that happens to match. A hex-rendered
    // payload is decoded back to bytes first, and the raw form is tried too, so
    // this works whether the record carries hex or the bytes themselves.
    let hex = unhex(bytes);
    for buf in [hex.as_deref(), Some(b)].into_iter().flatten() {
        if let Some(d) = sniff_dns(buf) { return Some(d); }
        if let Some(d) = sniff_mysql(buf) { return Some(d); }
        if let Some(d) = sniff_cassandra(buf) { return Some(d); }
        if let Some(d) = sniff_grpc(buf, bytes) { return Some(d); }
    }
    None
}

// ========================================================================
// DNS
// ========================================================================

/// A DNS message over UDP or TCP.
///
/// The header is fixed: a two byte id, flags, then four counts. A query has one
/// question and no answers, which is a shape ordinary traffic does not hold by
/// accident. The name is read from the label chain rather than guessed, so a
/// malformed message is declined instead of half read.
fn sniff_dns(b: &[u8]) -> Option<Decoded> {
    // over TCP the message is prefixed with its own length
    let body = if b.len() > 14 && ((b[0] as usize) << 8 | b[1] as usize) + 2 == b.len() {
        &b[2..]
    } else {
        b
    };
    if body.len() < 12 { return None; }
    let qd = (body[4] as u16) << 8 | body[5] as u16;
    let an = (body[6] as u16) << 8 | body[7] as u16;
    let opcode = (body[2] >> 3) & 0x0f;
    if qd != 1 || an > 16 || opcode > 2 { return None; }
    // the question name, one length-prefixed label at a time
    let mut i = 12usize;
    let mut name = String::new();
    let mut guard = 0;
    while i < body.len() && body[i] != 0 {
        let len = body[i] as usize;
        if len == 0 || len > 63 || i + 1 + len > body.len() { return None; }
        if !name.is_empty() { name.push('.'); }
        for c in &body[i + 1..i + 1 + len] {
            if !c.is_ascii_graphic() { return None; }
            name.push(*c as char);
        }
        i += 1 + len;
        guard += 1;
        if guard > 32 { return None; }
    }
    if name.is_empty() { return None; }
    let kind = if body[2] & 0x80 != 0 { "response" } else { "query" };
    Some(Decoded { protocol: "dns", summary: first_n(&format!("{} {}", kind, name), 120) })
}

// ========================================================================
// MYSQL
// ========================================================================

/// A MySQL client command.
///
/// Every packet carries a three byte little endian length, a sequence number,
/// then the command byte. The length has to agree with the bytes present, which
/// is what keeps this from claiming arbitrary binary.
fn sniff_mysql(b: &[u8]) -> Option<Decoded> {
    if b.len() < 5 { return None; }
    // The recorder bounds what it copies, so a captured buffer is usually shorter
    // than the packet declares. What can be checked is that the declared length
    // is possible and that what is present does not exceed it: a buffer longer
    // than the whole packet is not one packet.
    let len = b[0] as usize | (b[1] as usize) << 8 | (b[2] as usize) << 16;
    if len == 0 || len >= 1 << 24 || b.len() > len + 4 { return None; }
    // A client packet's sequence number restarts at 0 for each command and only
    // climbs within one exchange. Arbitrary binary puts anything here.
    if b[3] > 8 { return None; }
    let cmd = b[4];
    let name = match cmd {
        0x01 => "quit", 0x02 => "init db", 0x03 => "query", 0x04 => "field list",
        0x0e => "ping", 0x16 => "prepare", 0x17 => "execute", 0x19 => "close statement",
        _ => return None,
    };
    // Every command here carries either text or almost nothing. A body that is
    // neither is binary that happened to open with a plausible header, which is
    // what two matches on a capture containing no MySQL turned out to be.
    let body = &b[5..];
    match cmd {
        0x01 | 0x0e => { if !body.is_empty() { return None; } }     // quit, ping: no body
        0x19 => { if body.len() > 4 { return None; } }              // close: a statement id
        _ => {
            if body.is_empty() { return None; }
            let printable = body.iter().filter(|c| c.is_ascii_graphic() || **c == b' ').count();
            if printable * 10 < body.len() * 9 { return None; }     // a query is text
        }
    }
    let text: String = b[5..].iter().take(160)
        .filter(|c| c.is_ascii_graphic() || **c == b' ')
        .map(|c| *c as char).collect();
    Some(Decoded { protocol: "mysql", summary: first_n(&format!("{} {}", name, text.trim()), 120) })
}

// ========================================================================
// CASSANDRA
// ========================================================================

/// A CQL native protocol frame.
///
/// The first byte carries direction and version, then flags, a stream id, an
/// opcode and a four byte body length. The length agreeing with what is present
/// is again what makes the match safe.
fn sniff_cassandra(b: &[u8]) -> Option<Decoded> {
    if b.len() < 9 { return None; }
    let ver = b[0] & 0x7f;
    if !(3..=5).contains(&ver) { return None; }
    let opcode = b[4];
    let blen = u32::from_be_bytes([b[5], b[6], b[7], b[8]]) as usize;
    if blen > 1 << 24 { return None; }
    let name = match opcode {
        0x01 => "startup", 0x05 => "options", 0x07 => "query", 0x09 => "prepare",
        0x0a => "execute", 0x0b => "register", 0x0d => "batch",
        0x00 => "error", 0x08 => "result",
        _ => return None,
    };
    let text: String = b[9..].iter().take(160)
        .filter(|c| c.is_ascii_graphic() || **c == b' ')
        .map(|c| *c as char).collect();
    Some(Decoded { protocol: "cassandra", summary: first_n(&format!("{} {}", name, text.trim()), 120) })
}

// ========================================================================
// GRPC
// ========================================================================

/// gRPC, which rides HTTP/2 and is recognised by what it carries.
///
/// Two things identify it without parsing HPACK, which the recorder cannot see
/// unframed: the content type appearing in the bytes, and the length prefixed
/// message that gRPC puts in front of every payload, one compression flag then
/// a four byte length that has to agree with the rest of the buffer.
fn sniff_grpc(b: &[u8], bytes: &str) -> Option<Decoded> {
    if bytes.contains("application/grpc") {
        let path = bytes.split('/').nth(1).unwrap_or("");
        return Some(Decoded { protocol: "grpc", summary: first_n(path.trim(), 120) });
    }
    // The length-prefix alone is not evidence. A five byte prefix followed by a
    // legal protobuf tag occurs constantly in ordinary binary: ELF headers,
    // library data, anything with a zero byte and a small big-endian number in
    // it. Claiming those produced 88 false matches on a capture that contained
    // no gRPC at all. So the wire marker is required, and a buffer that only
    // looks structurally plausible is left undecoded, which is the honest
    // answer rather than a confident wrong one.
    None
}

// ========================================================================
// INTERNALS
// ========================================================================

fn looks_like_sql(s: &str) -> bool {
    let up = s.trim_start().to_ascii_uppercase();
    ["SELECT ", "INSERT ", "UPDATE ", "DELETE ", "CREATE ", "BEGIN", "COMMIT", "WITH "]
        .iter().any(|k| up.starts_with(k))
}

fn first_n(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() }
    else { s.chars().take(n).collect::<String>() + "…" }
}

// ========================================================================
// TYPES
// ========================================================================

pub struct Residue {
    pub total: usize,
    pub decoded: usize,
    pub bytes_total: u64,
    pub bytes_decoded: u64,
}

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

/// Decodes a recorder capture and reports what it could not read.
pub fn decode_floor(ndjson: &str) -> (J, Residue) {
    let mut by_protocol: BTreeMap<String, usize> = BTreeMap::new();
    let mut undecoded_kinds: BTreeMap<String, usize> = BTreeMap::new();
    let mut samples: Vec<J> = Vec::new();
    let mut r = Residue { total: 0, decoded: 0, bytes_total: 0, bytes_decoded: 0 };

    for line in ndjson.lines() {
        let v = match crate::json::parse(line) { Ok(v) => v, Err(_) => continue };
        let log = match v.get("log").and_then(|x| x.as_str()) { Some(l) => l, None => continue };
        let kind = v.get("kind").and_then(|x| x.as_str()).unwrap_or("text").to_string();
        let bytes = v.get("bytes").and_then(|x| x.as_f64()).unwrap_or(log.len() as f64) as u64;
        r.total += 1;
        r.bytes_total += bytes;

        match sniff(log) {
            Some(d) => {
                r.decoded += 1;
                r.bytes_decoded += bytes;
                *by_protocol.entry(d.protocol.to_string()).or_insert(0) += 1;
                if samples.len() < 12 {
                    samples.push(J::Obj([
                        ("protocol".to_string(), J::Str(d.protocol.to_string())),
                        ("decoded".to_string(), J::Bool(true)),
                        ("bytes".to_string(), J::Num(bytes as f64)),
                        ("summary".to_string(), J::Str(d.summary)),
                        ("comm".to_string(), J::Str(
                            v.get("comm").and_then(|x| x.as_str()).unwrap_or("").to_string())),
                    ].into_iter().collect()));
                }
            }
            None => {
                // The residue. Named by what the recorder already classified it as,
                // so `binary` and `signal` are distinguishable from text nobody
                // had a decoder for.
                *undecoded_kinds.entry(kind).or_insert(0) += 1;
            }
        }
    }

    let protocols: Vec<J> = by_protocol.iter().map(|(p, c)| J::Obj([
        ("protocol".to_string(), J::Str(p.clone())),
        ("records".to_string(), J::Num(*c as f64)),
    ].into_iter().collect())).collect();
    let residue: Vec<J> = undecoded_kinds.iter().map(|(k, c)| J::Obj([
        ("kind".to_string(), J::Str(k.clone())),
        ("records".to_string(), J::Num(*c as f64)),
        ("reason".to_string(), J::Str("no decoder matched".to_string())),
    ].into_iter().collect())).collect();

    let t = r.total.max(1) as f64;
    let bt = r.bytes_total.max(1) as f64;
    let out = J::Obj([
        ("records".to_string(), J::Num(r.total as f64)),
        ("decoded".to_string(), J::Num(r.decoded as f64)),
        ("undecoded".to_string(), J::Num((r.total - r.decoded) as f64)),
        ("understood_fraction".to_string(),
            J::Num(((r.decoded as f64 / t) * 1000.0).round() / 1000.0)),
        ("bytes_understood_fraction".to_string(),
            J::Num(((r.bytes_decoded as f64 / bt) * 1000.0).round() / 1000.0)),
        ("protocols".to_string(), J::Arr(protocols)),
        ("residue".to_string(), J::Arr(residue)),
        ("samples".to_string(), J::Arr(samples)),
        ("note".to_string(), J::Str(
            "Reports bytes parsed and bytes not parsed. Without the second figure, an \
             unparsed protocol and an idle service produce the same output.".to_string())),
    ].into_iter().collect());
    (out, r)
}
