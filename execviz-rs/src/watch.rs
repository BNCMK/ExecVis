// ========================================================================
//  MANIFEST
// ========================================================================
//  script_name: watch.rs
//  script_path: execviz-rs/src/watch.rs
//  module_name: watch
//  version: 0.53.1
//  description: Watching, sampling metadata and backup.
//  kind: module
//  spec: internal
//  internal_dependencies: egress, json, rollup, stats, store
//  external_dependencies: rusqlite, std
//  features: watch
//  api_version: execvis-v1.0.0
//  last_updated: 2026-08-07
// ========================================================================

//! Watching, sampling metadata and backup.
use crate::json::J;
use crate::stats::{assert_all, parse_rules, Rule};
use crate::store::{Span, Store};
use rusqlite::params;
use std::collections::BTreeMap;

// ========================================================================
// WATCHING
// ========================================================================

// ========================================================================
// TYPES
// ========================================================================

/// What a watch remembers between evaluations.
///
/// Without this a condition that stays true for an hour produces an event on
/// every tick, and an alert that fires three thousand times is one nobody reads.
#[derive(Default)]
pub struct WatchState { firing: BTreeMap<String, f64> }

pub struct Firing {
    pub rule: String,
    pub detail: String,
    pub examples: Vec<String>,
    pub state: &'static str,      // "fired" | "recovered"
    pub at: f64,
}

// ========================================================================
// IMPLEMENTATIONS
// ========================================================================

impl WatchState {
    pub fn new() -> WatchState { WatchState::default() }

    /// Evaluates the rules and reports only what *changed*.
    ///
    /// A watch fires on a transition, not on a condition: it fires when a rule
    /// starts failing, and again only once it has recovered and failed anew.
    pub fn evaluate(&mut self, spans: &[Span], rules: &[Rule], now: f64) -> Vec<Firing> {
        let failures = assert_all(spans, rules);
        let mut out = Vec::new();
        let mut seen: Vec<String> = Vec::new();

        for f in &failures {
            seen.push(f.rule.clone());
            if !self.firing.contains_key(&f.rule) {
                self.firing.insert(f.rule.clone(), now);
                out.push(Firing {
                    rule: f.rule.clone(),
                    // what it saw, not merely that it fired: an alert that sends
                    // a person back to the map has done none of the work
                    detail: f.detail.clone(),
                    examples: f.examples.clone(),
                    state: "fired", at: now,
                });
            }
        }
        let recovered: Vec<String> = self.firing.keys()
            .filter(|k| !seen.contains(k)).cloned().collect();
        for r in recovered {
            self.firing.remove(&r);
            out.push(Firing { rule: r, detail: "no longer failing".into(),
                              examples: vec![], state: "recovered", at: now });
        }
        out
    }

    pub fn currently_firing(&self) -> usize { self.firing.len() }
}

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

pub fn firing_json(f: &[Firing], firing_now: usize) -> J {
    let mut o = J::obj();
    o.set("changes", J::n(f.len() as f64));
    o.set("currently_firing", J::n(firing_now as f64));
    o.set("note", J::s("a watch reports transitions: a condition true for an hour is one event, not three thousand"));
    o.set("events", J::Arr(f.iter().map(|x| {
        let mut e = J::obj();
        e.set("rule", J::s(&x.rule));
        e.set("state", J::s(x.state));
        e.set("saw", J::s(&x.detail));
        e.set("examples", J::Arr(x.examples.iter().map(|s| J::s(s)).collect()));
        e.set("at", J::n(x.at));
        e
    }).collect()));
    o
}

pub fn rules_from(path: &str) -> Vec<Rule> {
    parse_rules(&std::fs::read_to_string(path).unwrap_or_default())
}

// ========================================================================
// SAMPLING
// ========================================================================

// ========================================================================
// CONSTANTS
// ========================================================================

pub const SAMPLING_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS sampling (
  id     INTEGER PRIMARY KEY CHECK (id=1),
  rule   TEXT NOT NULL,
  rate   REAL NOT NULL,
  noted  REAL NOT NULL
);
";

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

/// Records that a capture is a sample, and of what.
///
/// Without this, a store holding one span in a hundred is indistinguishable from
/// one holding everything, and every count taken from it is wrong by a factor
/// no reader can recover.
pub fn declare_sampling(store: &Store, rule: &str, rate: f64) -> rusqlite::Result<()> {
    store.conn.execute_batch(SAMPLING_SCHEMA)?;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64()).unwrap_or(0.0);
    store.conn.execute(
        "INSERT INTO sampling (id,rule,rate,noted) VALUES (1,?1,?2,?3)
         ON CONFLICT(id) DO UPDATE SET rule=excluded.rule, rate=excluded.rate, noted=excluded.noted",
        params![rule, rate, now])?;
    Ok(())
}

pub fn sampling(store: &Store) -> Option<(String, f64)> {
    store.conn.query_row("SELECT rule,rate FROM sampling WHERE id=1", [],
        |r| Ok((r.get(0)?, r.get(1)?))).ok()
}

/// How to read a count from this capture.
///
/// A rate is attached and the number is labelled an estimate; it is never
/// silently scaled. An estimate labelled as one is useful; a scaled-up number
/// wearing the clothes of a measurement is not.
pub fn describe_counts(store: &Store, observed: usize) -> J {
    let mut o = J::obj();
    o.set("observed", J::n(observed as f64));
    match sampling(store) {
        None => {
            o.set("sampled", J::Bool(false));
            o.set("counts_are", J::s("measurements: this capture records everything it saw"));
        }
        Some((rule, rate)) => {
            o.set("sampled", J::Bool(true));
            o.set("rule", J::s(&rule));
            o.set("rate", J::n(rate));
            o.set("counts_are", J::s("estimates: multiply by 1/rate to project, and say that you did"));
            if rate > 0.0 {
                o.set("projected_estimate", J::n((observed as f64 / rate).round()));
            }
        }
    }
    o
}

// ========================================================================
// BACKUP
// ========================================================================

/// A consistent copy taken while the tool is running.
///
/// Copying the file is not safe under a live writer, and a copy that is not
/// consistent differs from no copy because it looks like one. SQLite's online
/// backup API exists for exactly this.
pub fn backup(store: &Store, dest: &str) -> Result<J, String> {
    if std::path::Path::new(dest).exists() {
        return Err(format!("{} already exists; a backup never overwrites", dest));
    }
    store.conn.execute("VACUUM INTO ?1", params![dest]).map_err(|e| e.to_string())?;

    // verified after writing, because an unverified backup is a belief
    let copy = Store::open_ro(dest).map_err(|e| e.to_string())?;
    let their_spans = copy.all().map_err(|e| e.to_string())?;
    let our_spans = store.all().map_err(|e| e.to_string())?;
    let same_seal = crate::rollup::seal(&our_spans) == crate::rollup::seal(&their_spans);
    let sound = crate::egress::integrity(&copy, &their_spans)
        .get("sound") == Some(&J::Bool(true));

    let mut o = J::obj();
    o.set("destination", J::s(dest));
    o.set("spans", J::n(their_spans.len() as f64));
    o.set("bytes", J::n(std::fs::metadata(dest).map(|m| m.len() as f64).unwrap_or(0.0)));
    o.set("sound", J::Bool(sound));
    o.set("seal_matches", J::Bool(same_seal));
    o.set("verified", J::Bool(sound && same_seal));
    if !(sound && same_seal) {
        o.set("warning", J::s("this copy did not verify; treat it as absent rather than as a backup"));
    }
    Ok(o)
}

// ========================================================================
// FUNCTIONS
// ========================================================================

/// Cold starts and freezes, derived rather than recorded.
///
/// The adapter records which sandbox an invocation ran in, because concurrent
/// sandboxes interleave and nothing in the timestamps separates them. Everything
/// else follows: the first invocation in a sandbox is a cold start, and the gap
/// between one invocation returning and the next beginning is time the sandbox
/// spent frozen.
///
/// This is the derivability rule applied to itself; an earlier
/// version of the adapter recorded both as lifecycle events and the conformance
/// checker rejected it.
pub fn functions(spans: &[Span]) -> J {
    use std::collections::BTreeMap;
    let mut by_sandbox: BTreeMap<String, Vec<&Span>> = BTreeMap::new();
    for s in spans {
        if let J::Obj(m) = &s.attributes {
            if let Some(sb) = m.get("sandbox").and_then(|v| v.as_str()) {
                by_sandbox.entry(sb.to_string()).or_default().push(s);
            }
        }
    }
    let mut rows = Vec::new();
    let mut cold = 0usize;
    let mut frozen_total = 0.0f64;
    for (sb, list) in &mut by_sandbox {
        list.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap());
        let mut prev_end: Option<f64> = None;
        for (i, s) in list.iter().enumerate() {
            let is_cold = i == 0;
            if is_cold { cold += 1; }
            let frozen = prev_end.map(|p| s.start - p).filter(|g| *g > 0.05);
            if let Some(g) = frozen { frozen_total += g; }
            let mut e = J::obj();
            e.set("sandbox", J::s(sb));
            e.set("invocation", J::n((i + 1) as f64));
            e.set("name", J::s(&s.name));
            e.set("status", J::s(&s.status));
            e.set("cold_start", J::Bool(is_cold));
            e.set("wall_ms", J::n(s.duration_ms().unwrap_or(0.0)));
            if let J::Obj(m) = &s.attributes {
                if let Some(c) = m.get("cpu_ms").and_then(|v| v.as_f64()) {
                    e.set("cpu_ms", J::n(c));
                    // wall and execution time are different quantities here, and
                    // a capture that conflates them is lying with arithmetic
                    e.set("waiting_ms", J::n(((s.duration_ms().unwrap_or(0.0) - c) * 100.0).round() / 100.0));
                }
            }
            if let Some(g) = frozen {
                e.set("frozen_before_ms", J::n((g * 1000.0 * 10.0).round() / 10.0));
            }
            rows.push(e);
            prev_end = s.end;
        }
    }
    let mut o = J::obj();
    o.set("sandboxes", J::n(by_sandbox.len() as f64));
    o.set("invocations", J::n(rows.len() as f64));
    o.set("cold_starts", J::n(cold as f64));
    o.set("frozen_total_ms", J::n((frozen_total * 1000.0 * 10.0).round() / 10.0));
    o.set("note", J::s("cold starts and freezes are derived from timestamps and a sandbox identity; a frozen sandbox accrues wall time and no execution"));
    o.set("invocation_list", J::Arr(rows));
    o
}
