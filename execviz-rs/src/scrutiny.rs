// ========================================================================
//  MANIFEST
// ========================================================================
//  script_name: scrutiny.rs
//  script_path: execviz-rs/src/scrutiny.rs
//  module_name: scrutiny
//  version: 0.53.1
//  description: Did it watch itself the same way?
//  kind: module
//  spec: internal
//  internal_dependencies: json, sha256
//  external_dependencies: std
//  features: scrutiny
//  api_version: execvis-v1.0.0
//  last_updated: 2026-08-07
// ========================================================================

//! Did it watch itself the same way?
//!
//! A recorder that watches everything including itself is only as good as the
//! claim that it watched itself the SAME WAY. An exemption applied quietly to
//! its own records would look exactly like honesty in the output.
//!
//! So each record carries a policy digest: a hash over the DECISIONS that made
//! it, not over what it says. Whether it was suppressed, how it was classified,
//! whether it was truncated, whether the descriptor was resolved, whether the
//! bytes were hexed. Who the record is about is deliberately NOT in there: if it
//! were, every self record would differ by construction and the whole question
//! would answer itself falsely.
//!
//! That reduces an unverifiable promise to arithmetic:
//!
//!     does any policy appear on the recorder's own records and nowhere else,
//!     and was it declared?
//!
//! An undeclared one is special-casing, whether or not anybody meant it.

use crate::json::J;
use std::collections::{BTreeMap, BTreeSet};

// ========================================================================
// INTERNALS
// ========================================================================

/// Combines the distinct policies into one comparable number.
///
/// A Merkle root rather than a flat hash of everything, so a single record's
/// treatment can later be proven against the root without handing over the rest
/// of the capture.
fn merkle_root(leaves: &BTreeSet<String>) -> String {
    if leaves.is_empty() { return "0".repeat(64) }
    let mut level: Vec<String> = leaves.iter()
        .map(|l| crate::sha256::hex(&crate::sha256::sha256(l.as_bytes())))
        .collect();
    while level.len() > 1 {
        let mut next = Vec::with_capacity((level.len() + 1) / 2);
        for pair in level.chunks(2) {
            let joined = match pair {
                [a, b] => format!("{}{}", a, b),
                // an odd node is promoted rather than paired with itself, which
                // would let two different trees share a root
                [a] => a.clone(),
                _ => unreachable!(),
            };
            next.push(crate::sha256::hex(&crate::sha256::sha256(joined.as_bytes())));
        }
        level = next;
    }
    level.remove(0)
}

// ========================================================================
// TYPES
// ========================================================================

pub struct Verdict { pub undeclared: usize, pub records: usize }

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

pub fn examine(ndjson: &str, recorder: &str) -> (J, Verdict) {
    let mut by_self: BTreeSet<String> = BTreeSet::new();
    let mut by_other: BTreeSet<String> = BTreeSet::new();
    let mut declared: BTreeMap<String, String> = BTreeMap::new();
    let mut counts: BTreeMap<String, (usize, usize)> = BTreeMap::new();  // (self, other)
    let mut records = 0usize;
    let mut suppressed_total = 0f64;

    for line in ndjson.lines() {
        let v = match crate::json::parse(line) { Ok(v) => v, Err(_) => continue };
        let pol = match v.get("policy_text").and_then(|x| x.as_str()) {
            Some(p) => p.to_string(), None => continue,
        };
        records += 1;
        let comm = v.get("comm").and_then(|x| x.as_str()).unwrap_or("");
        let is_self = comm == recorder;

        // A declared exemption names itself and says why. Undeclared ones are
        // the entire point of this check.
        if v.get("declared_exemption").is_some() {
            let why = v.get("why").and_then(|x| x.as_str()).unwrap_or("(no reason given)");
            declared.insert(pol.clone(), why.to_string());
            suppressed_total += v.get("suppressed").and_then(|x| x.as_f64()).unwrap_or(0.0);
        }

        let e = counts.entry(pol.clone()).or_insert((0, 0));
        if is_self { e.0 += 1; by_self.insert(pol); } else { e.1 += 1; by_other.insert(pol); }
    }

    let only_self: Vec<&String> = by_self.difference(&by_other).collect();
    let shared: usize = by_self.intersection(&by_other).count();

    let mut findings: Vec<J> = Vec::new();
    let mut undeclared = 0usize;
    for p in &only_self {
        let (s, o) = counts.get(*p).copied().unwrap_or((0, 0));
        match declared.get(*p) {
            Some(why) => findings.push(J::Obj([
                ("policy".to_string(), J::Str((*p).clone())),
                ("applies_only_to".to_string(), J::Str(recorder.to_string())),
                ("declared".to_string(), J::Bool(true)),
                ("why".to_string(), J::Str(why.clone())),
                ("records".to_string(), J::Num(s as f64)),
            ].into_iter().collect())),
            None => {
                undeclared += 1;
                findings.push(J::Obj([
                    ("policy".to_string(), J::Str((*p).clone())),
                    ("applies_only_to".to_string(), J::Str(recorder.to_string())),
                    ("declared".to_string(), J::Bool(false)),
                    ("self_records".to_string(), J::Num(s as f64)),
                    ("other_records".to_string(), J::Num(o as f64)),
                    ("problem".to_string(), J::Str(
                        "a decision path applied to the recorder's own records and to \
                         nothing else, and not declared as an exemption. Whether or not \
                         anybody meant it, this is special-casing.".to_string())),
                ].into_iter().collect()));
            }
        }
    }

    let all: BTreeSet<String> = by_self.union(&by_other).cloned().collect();
    let out = J::Obj([
        ("records".to_string(), J::Num(records as f64)),
        ("policies_total".to_string(), J::Num(all.len() as f64)),
        ("policies_on_others".to_string(), J::Num(by_other.len() as f64)),
        ("policies_on_recorder".to_string(), J::Num(by_self.len() as f64)),
        ("shared_treatment".to_string(), J::Num(shared as f64)),
        ("only_on_recorder".to_string(), J::Num(only_self.len() as f64)),
        ("undeclared".to_string(), J::Num(undeclared as f64)),
        ("suppressed_by_declared_exemption".to_string(), J::Num(suppressed_total)),
        ("merkle_root".to_string(), J::Str(merkle_root(&all))),
        ("findings".to_string(), J::Arr(findings)),
        ("note".to_string(), J::Str(
            "The policy describes the treatment, not the subject: two records that \
             went through the same code path share a digest whatever they say. A \
             dishonest build could compute these disaccurately, so this is evidence \
             against accidental divergence rather than proof against a determined \
             author; the independent checks in SECURITY.md do not depend on this \
             software.".to_string())),
    ].into_iter().collect());
    (out, Verdict { undeclared, records })
}
