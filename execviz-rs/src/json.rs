// ========================================================================
//  MANIFEST
// ========================================================================
//  script_name: json.rs
//  script_path: execviz-rs/src/json.rs
//  module_name: json
//  version: 0.53.1
//  description: Minimal JSON value + parser + writer. Keeps the binary dependency-light: the wire format is small and fully known, so a general-purpose serde stack is not warranted here.
//  kind: module
//  spec: internal
//  internal_dependencies: 
//  external_dependencies: std
//  features: json
//  api_version: execvis-v1.0.0
//  last_updated: 2026-08-07
// ========================================================================

//! Minimal JSON value + parser + writer. Keeps the binary dependency-light:
//! the wire format is small and fully known, so a general-purpose serde stack
//! is not warranted here.
use std::collections::BTreeMap;
use std::fmt::Write as _;

// ========================================================================
// TYPES
// ========================================================================

#[derive(Clone, Debug, PartialEq)]
pub enum J {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<J>),
    Obj(BTreeMap<String, J>),
}

// ========================================================================
// IMPLEMENTATIONS
// ========================================================================

impl J {
    pub fn obj() -> J { J::Obj(BTreeMap::new()) }
    pub fn set(&mut self, k: &str, v: J) {
        if let J::Obj(m) = self { m.insert(k.to_string(), v); }
    }
    pub fn get(&self, k: &str) -> Option<&J> {
        if let J::Obj(m) = self { m.get(k) } else { None }
    }
    pub fn as_str(&self) -> Option<&str> { if let J::Str(s)=self { Some(s) } else { None } }
    pub fn as_f64(&self) -> Option<f64> { if let J::Num(n)=self { Some(*n) } else { None } }
    pub fn as_arr(&self) -> Option<&Vec<J>> { if let J::Arr(a)=self { Some(a) } else { None } }
    pub fn is_null(&self) -> bool { matches!(self, J::Null) }
    pub fn s(v: &str) -> J { J::Str(v.to_string()) }
    pub fn n(v: f64) -> J { J::Num(v) }

    pub fn dump(&self) -> String { let mut o = String::new(); self.write(&mut o); o }

    fn write(&self, out: &mut String) {
        match self {
            J::Null => out.push_str("null"),
            J::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            J::Num(n) => {
                if n.is_finite() {
                    if (n.fract()).abs() < f64::EPSILON && n.abs() < 1e15 {
                        let _ = write!(out, "{}", *n as i64);
                    } else { let _ = write!(out, "{}", n); }
                } else { out.push_str("null"); }
            }
            J::Str(s) => write_str(s, out),
            J::Arr(a) => {
                out.push('[');
                for (i, v) in a.iter().enumerate() {
                    if i > 0 { out.push(','); }
                    v.write(out);
                }
                out.push(']');
            }
            J::Obj(m) => {
                out.push('{');
                for (i, (k, v)) in m.iter().enumerate() {
                    if i > 0 { out.push(','); }
                    write_str(k, out);
                    out.push(':');
                    v.write(out);
                }
                out.push('}');
            }
        }
    }
}

// ========================================================================
// INTERNALS
// ========================================================================

fn write_str(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => { let _ = write!(out, "\\u{:04x}", c as u32); }
            c => out.push(c),
        }
    }
    out.push('"');
}

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

pub fn parse(src: &str) -> Result<J, String> {
    let b: Vec<char> = src.chars().collect();
    let mut i = 0usize;
    let v = pval(&b, &mut i)?;
    Ok(v)
}

// ========================================================================
// INTERNALS
// ========================================================================

fn ws(b: &[char], i: &mut usize) { while *i < b.len() && b[*i].is_whitespace() { *i += 1; } }

fn pval(b: &[char], i: &mut usize) -> Result<J, String> {
    ws(b, i);
    if *i >= b.len() { return Err("eof".into()); }
    match b[*i] {
        '{' => {
            *i += 1;
            let mut m = BTreeMap::new();
            ws(b, i);
            if *i < b.len() && b[*i] == '}' { *i += 1; return Ok(J::Obj(m)); }
            loop {
                ws(b, i);
                let k = match pval(b, i)? { J::Str(s) => s, _ => return Err("key".into()) };
                ws(b, i);
                if *i >= b.len() || b[*i] != ':' { return Err("colon".into()); }
                *i += 1;
                let v = pval(b, i)?;
                m.insert(k, v);
                ws(b, i);
                if *i < b.len() && b[*i] == ',' { *i += 1; continue; }
                if *i < b.len() && b[*i] == '}' { *i += 1; break; }
                return Err("obj".into());
            }
            Ok(J::Obj(m))
        }
        '[' => {
            *i += 1;
            let mut a = Vec::new();
            ws(b, i);
            if *i < b.len() && b[*i] == ']' { *i += 1; return Ok(J::Arr(a)); }
            loop {
                a.push(pval(b, i)?);
                ws(b, i);
                if *i < b.len() && b[*i] == ',' { *i += 1; continue; }
                if *i < b.len() && b[*i] == ']' { *i += 1; break; }
                return Err("arr".into());
            }
            Ok(J::Arr(a))
        }
        '"' => {
            *i += 1;
            let mut s = String::new();
            while *i < b.len() {
                let c = b[*i];
                *i += 1;
                if c == '"' { return Ok(J::Str(s)); }
                if c == '\\' {
                    if *i >= b.len() { break; }
                    let e = b[*i]; *i += 1;
                    match e {
                        'n' => s.push('\n'), 't' => s.push('\t'), 'r' => s.push('\r'),
                        'b' => s.push('\u{8}'), 'f' => s.push('\u{c}'),
                        'u' => {
                            let mut code = 0u32;
                            for _ in 0..4 {
                                if *i < b.len() {
                                    code = code * 16 + b[*i].to_digit(16).unwrap_or(0);
                                    *i += 1;
                                }
                            }
                            if let Some(ch) = char::from_u32(code) { s.push(ch); }
                        }
                        other => s.push(other),
                    }
                } else { s.push(c); }
            }
            Err("string".into())
        }
        't' => { *i += 4; Ok(J::Bool(true)) }
        'f' => { *i += 5; Ok(J::Bool(false)) }
        'n' => { *i += 4; Ok(J::Null) }
        _ => {
            let st = *i;
            while *i < b.len() && (b[*i].is_ascii_digit() || "+-.eE".contains(b[*i])) { *i += 1; }
            let s: String = b[st..*i].iter().collect();
            // A number that parses but is not finite (1e400 overflows to
            // infinity) is refused here rather than downstream: SQLite stores a
            // non-finite REAL as NULL, so accepting one would silently turn a
            // timestamp into an absent value; exactly the "absent values are reported as absent, not as zero"
            // failure this design exists to avoid.
            match s.parse::<f64>() {
                Ok(n) if n.is_finite() => Ok(J::Num(n)),
                Ok(_) => Err(format!("number '{}' is not finite", s)),
                Err(_) => Err(format!("'{}' is not a number", s)),
            }
        }
    }
}
