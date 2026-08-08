// ========================================================================
//  MANIFEST
// ========================================================================
//  script_name: rollup.rs
//  script_path: execviz-rs/src/rollup.rs
//  module_name: rollup
//  version: 0.53.1
//  description: Rolled-up tiers: a stored summary per node plus a digest over it.
//  kind: module
//  spec: internal
//  internal_dependencies: json, sha256, store
//  external_dependencies: std
//  features: rollup, store
//  api_version: execvis-v1.0.0
//  last_updated: 2026-08-07
// ========================================================================

//! Rolled-up tiers: a stored summary per node plus a digest over it.
//!
//! The two answer different questions. The digest answers "did anything below
//! this change", in constant time and without reading below. The rollup answers
//! "what is down there", and is what a tier draws from. Carrying only one of
//! them would leave the scheme either unreadable or uncomparable.
use crate::json::J;
use crate::store::Span;
use std::collections::BTreeMap;

// ========================================================================
// TYPES
// ========================================================================

/// A summary that a parent can compute from its children alone.
///
/// Every field here is associative with an identity, which is the constraint
/// that makes the tier scheme work at all. Note what is *not* here:
/// no median, no exact distinct count, and no ratio stored as a ratio. The io
/// share travels as a numerator and a denominator and is divided at the point of
/// reading, because dividing before combining gives an average of averages,
/// which is a different and wrong number.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rollup {
    pub spans: u64,
    pub errors: u64,
    pub open: u64,
    pub io_spans: u64,
    pub total_ms: f64,
    pub first: f64,
    pub last: f64,
    pub kinds: BTreeMap<String, u64>,
}

// ========================================================================
// IMPLEMENTATIONS
// ========================================================================

impl Rollup {
    /// The identity element. Combining with it changes nothing, which is what
    /// lets an empty subtree be handled without a special case.
    pub fn empty() -> Rollup {
        Rollup { first: f64::INFINITY, last: f64::NEG_INFINITY, ..Default::default() }
    }

    pub fn leaf(s: &Span) -> Rollup {
        let mut kinds = BTreeMap::new();
        kinds.insert(s.kind.clone(), 1u64);
        Rollup {
            spans: 1,
            errors: if s.status == "error" { 1 } else { 0 },
            open: if s.end.is_none() { 1 } else { 0 },
            io_spans: if matches!(s.kind.as_str(), "io" | "external" | "wait") { 1 } else { 0 },
            total_ms: s.duration_ms().unwrap_or(0.0),
            first: s.start,
            last: s.end.unwrap_or(s.start),
            kinds,
        }
    }

    /// Associative combine. `a.combine(b).combine(c)` and `a.combine(b.combine(c))`
    /// must agree, or a tier's answer would depend on the order it happened to
    /// walk its children.
    pub fn combine(&self, other: &Rollup) -> Rollup {
        let mut kinds = self.kinds.clone();
        for (k, v) in &other.kinds { *kinds.entry(k.clone()).or_insert(0) += v; }
        Rollup {
            spans: self.spans + other.spans,
            errors: self.errors + other.errors,
            open: self.open + other.open,
            io_spans: self.io_spans + other.io_spans,
            total_ms: self.total_ms + other.total_ms,
            first: self.first.min(other.first),
            last: self.last.max(other.last),
            kinds,
        }
    }

    /// The ratio, divided only now. Stored as a ratio it could not be combined.
    pub fn io_share(&self) -> f64 {
        if self.spans == 0 { 0.0 } else { self.io_spans as f64 / self.spans as f64 }
    }

    pub fn to_json(&self) -> J {
        let mut o = J::obj();
        o.set("spans", J::n(self.spans as f64));
        o.set("errors", J::n(self.errors as f64));
        o.set("open", J::n(self.open as f64));
        o.set("io_share", J::n((self.io_share() * 1000.0).round() / 1000.0));
        o.set("total_ms", J::n((self.total_ms * 100.0).round() / 100.0));
        if self.spans > 0 {
            o.set("first", J::n(self.first));
            o.set("last", J::n(self.last));
        }
        let mut k = J::obj();
        for (name, c) in &self.kinds { k.set(name, J::n(*c as f64)); }
        o.set("kinds", k);
        o
    }
}

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

/// A small non-cryptographic digest (FNV-1a, 64-bit).
///
/// The job here is change detection between cooperating parts of one system, not
/// resistance to a forger, and a hash function that needs no dependency keeps
/// the adapter and the core equally able to compute one. If this ever guards a
/// trust boundary it must be replaced by a cryptographic hash, and that is a
/// different requirement rather than a stronger version of this one.
pub fn digest_of(parts: &[&str]) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for p in parts {
        for b in p.as_bytes() {
            h ^= *b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        h ^= 0xff;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{:016x}", h)
}

pub fn leaf_digest(s: &Span) -> String {
    // The state that can change is what the digest covers: a completion moves
    // the digest, which is reported.
    digest_of(&[
        &s.span_id, &s.kind, &s.status,
        &s.end.map(|e| format!("{:.6}", e)).unwrap_or_else(|| "open".into()),
    ])
}

// ========================================================================
// TYPES
// ========================================================================

#[derive(Clone, Debug)]
pub struct Node {
    pub id: String,
    pub tier: &'static str,
    pub digest: String,
    pub rollup: Rollup,
    pub children: Vec<Node>,
}

// ========================================================================
// IMPLEMENTATIONS
// ========================================================================

impl Node {
    pub fn to_json(&self, depth: usize) -> J {
        let mut o = J::obj();
        o.set("id", J::s(&self.id));
        o.set("tier", J::s(self.tier));
        o.set("digest", J::s(&self.digest));
        o.set("rollup", self.rollup.to_json());
        o.set("children", J::n(self.children.len() as f64));
        if depth > 0 && !self.children.is_empty() {
            o.set("nodes", J::Arr(self.children.iter().map(|c| c.to_json(depth - 1)).collect()));
        }
        o
    }
}

// ========================================================================
// INTERNALS
// ========================================================================

fn node(id: String, tier: &'static str, children: Vec<Node>) -> Node {
    let mut r = Rollup::empty();
    for c in &children { r = r.combine(&c.rollup); }
    let mut parts: Vec<&str> = vec![&id];
    for c in &children { parts.push(&c.digest); }
    let digest = digest_of(&parts);
    Node { id, tier, digest, rollup: r, children }
}

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

/// host -> domain -> kind -> spans. The hierarchy the map already uses, so the
/// tiers a reader zooms through and the tiers the digest covers are the same
/// tiers rather than two parallel schemes that can disagree.
pub fn build(spans: &[Span]) -> Node {
    let mut by_host: BTreeMap<&str, BTreeMap<String, BTreeMap<&str, Vec<&Span>>>> = BTreeMap::new();
    for s in spans {
        by_host.entry(s.host_id.as_str()).or_default()
            .entry(s.domain.clone().unwrap_or_else(|| "unknown".into())).or_default()
            .entry(s.kind.as_str()).or_default()
            .push(s);
    }
    let hosts: Vec<Node> = by_host.into_iter().map(|(h, doms)| {
        let domains: Vec<Node> = doms.into_iter().map(|(d, kinds)| {
            let kind_nodes: Vec<Node> = kinds.into_iter().map(|(k, list)| {
                let mut r = Rollup::empty();
                let mut parts: Vec<String> = Vec::with_capacity(list.len() + 1);
                parts.push(format!("{}/{}/{}", h, d, k));
                for s in &list { r = r.combine(&Rollup::leaf(s)); parts.push(leaf_digest(s)); }
                let refs: Vec<&str> = parts.iter().map(|x| x.as_str()).collect();
                Node { id: format!("{}/{}/{}", h, d, k), tier: "kind",
                       digest: digest_of(&refs), rollup: r, children: vec![] }
            }).collect();
            node(format!("{}/{}", h, d), "cluster", kind_nodes)
        }).collect();
        node(h.to_string(), "host", domains)
    }).collect();
    node("field".into(), "field", hosts)
}

/// Walk to a node by id, so a client can ask for one subtree instead of the tree.
pub fn find<'a>(n: &'a Node, id: &str) -> Option<&'a Node> {
    if n.id == id { return Some(n); }
    for c in &n.children { if let Some(f) = find(c, id) { return Some(f); } }
    None
}

/// One side of a comparison: a node's identity and digest, without its rollup.
/// This is what crosses the wire when two instances ask each other what differs,
/// and it is deliberately small: the answer to "are we the same" should not cost
/// the data being compared.
pub fn skeleton(n: &Node, depth: usize) -> J {
    let mut o = J::obj();
    o.set("id", J::s(&n.id));
    o.set("tier", J::s(n.tier));
    o.set("digest", J::s(&n.digest));
    o.set("spans", J::n(n.rollup.spans as f64));
    if depth > 0 && !n.children.is_empty() {
        o.set("nodes", J::Arr(n.children.iter().map(|c| skeleton(c, depth - 1)).collect()));
    }
    o
}

// ========================================================================
// TYPES
// ========================================================================

#[derive(Debug, Clone)]
pub struct Divergence {
    pub id: String,
    pub tier: String,
    pub mine: Option<String>,
    pub theirs: Option<String>,
    pub my_spans: u64,
    pub their_spans: u64,
}

// ========================================================================
// INTERNALS
// ========================================================================

fn child_map(v: &J) -> BTreeMap<String, &J> {
    let mut m = BTreeMap::new();
    if let Some(list) = v.get("nodes").and_then(|x| x.as_arr()) {
        for c in list {
            if let Some(id) = c.get("id").and_then(|x| x.as_str()) { m.insert(id.to_string(), c); }
        }
    }
    m
}

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

/// Walks two skeletons together and reports only where they disagree.
///
/// A branch whose digest matches is identical beneath it, whatever its size, and
/// is not descended into. That skip is the entire saving: the cost of the
/// comparison is the size of the disagreement, not the size of the capture.
///
/// A matching digest proves the two agree *as recorded*. It does not prove
/// either is complete; both could be missing the same span. This detects
/// divergence, not truth.
pub fn diverge(mine: &Node, theirs: &J, out: &mut Vec<Divergence>) {
    let their_digest = theirs.get("digest").and_then(|x| x.as_str()).unwrap_or("");
    if their_digest == mine.digest { return; }          // identical beneath: skip
    let their_spans = theirs.get("spans").and_then(|x| x.as_f64()).unwrap_or(0.0) as u64;

    let kids = child_map(theirs);
    let descendable = !mine.children.is_empty() && !kids.is_empty();
    if !descendable {
        out.push(Divergence {
            id: mine.id.clone(), tier: mine.tier.to_string(),
            mine: Some(mine.digest.clone()), theirs: Some(their_digest.to_string()),
            my_spans: mine.rollup.spans, their_spans,
        });
        return;
    }
    for c in &mine.children {
        match kids.get(&c.id) {
            Some(t) => diverge(c, t, out),
            None => out.push(Divergence {
                id: c.id.clone(), tier: c.tier.to_string(),
                mine: Some(c.digest.clone()), theirs: None,
                my_spans: c.rollup.spans, their_spans: 0,
            }),
        }
    }
    // theirs and not mine: a subtree this side has never seen at all
    let my_ids: Vec<&str> = mine.children.iter().map(|c| c.id.as_str()).collect();
    for (id, t) in kids {
        if my_ids.contains(&id.as_str()) { continue; }
        out.push(Divergence {
            id, tier: t.get("tier").and_then(|x| x.as_str()).unwrap_or("?").to_string(),
            mine: None, theirs: t.get("digest").and_then(|x| x.as_str()).map(|s| s.to_string()),
            my_spans: 0,
            their_spans: t.get("spans").and_then(|x| x.as_f64()).unwrap_or(0.0) as u64,
        });
    }
}

pub fn divergence_json(list: &[Divergence], compared: usize) -> J {
    let mut o = J::obj();
    o.set("in_sync", J::Bool(list.is_empty()));
    o.set("nodes_compared", J::n(compared as f64));
    o.set("diverging", J::n(list.len() as f64));
    o.set("nodes", J::Arr(list.iter().take(64).map(|d| {
        let mut e = J::obj();
        e.set("id", J::s(&d.id));
        e.set("tier", J::s(&d.tier));
        e.set("mine", match &d.mine { Some(x) => J::s(x), None => J::Null });
        e.set("theirs", match &d.theirs { Some(x) => J::s(x), None => J::Null });
        e.set("my_spans", J::n(d.my_spans as f64));
        e.set("their_spans", J::n(d.their_spans as f64));
        e.set("state", J::s(match (&d.mine, &d.theirs) {
            (Some(_), None) => "only here",
            (None, Some(_)) => "only there",
            _ => "differs",
        }));
        e
    }).collect()));
    o
}

pub fn count_nodes(n: &Node) -> usize {
    1 + n.children.iter().map(count_nodes).sum::<usize>()
}

/// A cryptographic seal over a capture (spec 5.6, gap 38).
///
/// The tier digests are FNV: fast, dependency-free, and fine for asking a
/// cooperating peer whether anything changed. They are useless against someone
/// who edits a capture on purpose. A seal is different work for a different
/// question; is this the capture that was taken; and so it uses SHA-256 over
/// a canonical rendering of every span's identity-bearing fields.
///
/// Canonical means sorted and fully specified: two instances holding the same
/// spans must produce the same seal, or the seal proves nothing.
pub fn seal(spans: &[Span]) -> String {
    let mut rows: Vec<String> = spans.iter().map(|s| format!(
        "{}|{}|{}|{}|{:.6}|{}|{}|{}",
        s.span_id, s.trace_id, s.parent_span_id.clone().unwrap_or_default(),
        s.name, s.start,
        s.end.map(|e| format!("{:.6}", e)).unwrap_or_else(|| "open".into()),
        s.status, s.host_id)).collect();
    rows.sort();
    let joined = rows.join("\n");
    crate::sha256::hex(&crate::sha256::sha256(joined.as_bytes()))
}

pub fn seal_json(spans: &[Span]) -> J {
    let mut o = J::obj();
    o.set("seal", J::s(&seal(spans)));
    o.set("algorithm", J::s("sha256 over a canonical rendering of every span"));
    o.set("spans", J::n(spans.len() as f64));
    o.set("note", J::s("the tier digests answer 'did anything change' between cooperating parts; this answers 'is this the capture that was taken' against someone who edits one on purpose"));
    o
}
