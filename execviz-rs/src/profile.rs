// ========================================================================
//  MANIFEST
// ========================================================================
//  script_name: profile.rs
//  script_path: execviz-rs/src/profile.rs
//  module_name: profile
//  version: 0.53.1
//  description: The project profile: indicators this project has named.
//  kind: module
//  spec: internal
//  internal_dependencies: json
//  external_dependencies: std
//  features: profile
//  api_version: execvis-v1.0.0
//  last_updated: 2026-08-07
// ========================================================================

//! The project profile: indicators this project has named.
//!
//! Everything else in this tool reports what it can work out on its own. This is
//! the one place a project says what its own output MEANS. A line reading
//! "connection reset by peer" is a fault in one service and the normal end of a
//! polling loop in another, and nothing outside the project can settle that.
//!
//! An indicator is a label, a meaning, and something to match on. The label is
//! the project's word for it, so findings read in the project's own vocabulary
//! rather than in syscall numbers.
//!
//! WHY THE OUTPUT IS A SUMMARY AND NOT AN ANNOTATED CAPTURE.
//!
//! Keeping months of raw captures is not practical: they are large and they are
//! mostly the same thing over and over. A profile summary is small and fixed in
//! shape, so a project can keep one per week for a year and still diff any two.
//! That makes "what changed since last month" answerable at all, and it
//! is why the summary carries counts and first and last sightings rather than
//! the records themselves.
//!
//! The honesty rules that govern the rest of the tool govern this too:
//!
//! - An indicator that matched NOTHING is reported as silent, not omitted. A
//!   fault that stopped appearing is a finding, and one that never worked is a
//!   different finding; both are invisible if silence is dropped.
//! - The records no indicator matched are counted and reported. A profile that
//!   labels a tenth of the output and says nothing about the rest is describing
//!   the tenth it happened to think of.
//! - An unknown match field is a usage failure, not a rule that quietly matches
//!   nothing, for the same reason an unknown detect predicate exits 2: a typo
//!   and a clean run must not look the same.
//! - Nothing here decides that a fault is bad. The project said what it means;
//!   this counts it and reports it.

use crate::json::J;
use std::collections::BTreeMap;

// ========================================================================
// TYPES
// ========================================================================

pub struct Indicator {
    pub label: String,
    pub means: String,
    pub field: String,
    pub value: String,
    pub note: String,
}

pub struct Seen {
    pub count: usize,
    pub first: f64,
    pub last: f64,
    pub example: String,
}

// ========================================================================
// CONSTANTS
// ========================================================================

/// Fields an indicator may match on. Anything else is a usage error rather than
/// an indicator that silently never fires.
const FIELDS: [&str; 6] = ["text", "kind", "level", "where", "comm", "direction"];

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

pub fn load(profile_json: &str) -> Result<(String, Vec<Indicator>), String> {
    let v = crate::json::parse(profile_json).map_err(|e| format!("profile is not valid JSON: {}", e))?;
    let project = v.get("project").and_then(|x| x.as_str()).unwrap_or("unnamed").to_string();
    let arr = match v.get("indicators").and_then(|x| x.as_arr()) {
        Some(a) => a,
        None => return Err("profile has no `indicators` array".to_string()),
    };
    let mut out = Vec::new();
    for (i, it) in arr.iter().enumerate() {
        let label = it.get("label").and_then(|x| x.as_str()).unwrap_or("").to_string();
        if label.is_empty() { return Err(format!("indicator {} has no label", i)); }
        let means = it.get("means").and_then(|x| x.as_str()).unwrap_or("informational").to_string();
        let m = match it.get("match") {
            Some(m) => m,
            None => return Err(format!("indicator `{}` has no `match`", label)),
        };
        let mut field = String::new();
        let mut value = String::new();
        for f in FIELDS.iter() {
            if let Some(s) = m.get(f).and_then(|x| x.as_str()) {
                field = (*f).to_string();
                value = s.to_string();
            }
        }
        if field.is_empty() {
            return Err(format!(
                "indicator `{}` matches on no known field; one of: {}",
                label, FIELDS.join(", ")));
        }
        out.push(Indicator {
            label, means, field, value,
            note: it.get("note").and_then(|x| x.as_str()).unwrap_or("").to_string(),
        });
    }
    Ok((project, out))
}

// ========================================================================
// INTERNALS
// ========================================================================

fn field_of<'a>(v: &'a J, field: &str) -> Option<&'a str> {
    match field {
        "text" => v.get("log").and_then(|x| x.as_str()),
        "kind" => v.get("kind").and_then(|x| x.as_str()),
        "level" => v.get("level").and_then(|x| x.as_str()),
        "where" => v.get("where").and_then(|x| x.as_str()),
        "comm" => v.get("comm").and_then(|x| x.as_str()),
        "direction" => v.get("direction").and_then(|x| x.as_str()),
        _ => None,
    }
}

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

/// Read a capture through the project's own vocabulary.
pub fn summarise(ndjson: &str, project: &str, inds: &[Indicator]) -> (J, i32) {
    let mut seen: BTreeMap<String, Seen> = BTreeMap::new();
    let (mut total, mut labelled, mut payloads) = (0usize, 0usize, 0usize);
    let (mut tmin, mut tmax) = (f64::MAX, 0.0f64);

    for line in ndjson.lines() {
        let line = line.trim();
        if line.is_empty() { continue; }
        let v = match crate::json::parse(line) { Ok(v) => v, Err(_) => continue };
        total += 1;
        let t = v.get("t").and_then(|x| x.as_f64()).unwrap_or(0.0);
        if t > 0.0 { if t < tmin { tmin = t; } if t > tmax { tmax = t; } }
        // only records carrying output can be labelled by text; a bare call
        // record has nothing for an indicator to read
        let has_payload = v.get("log").and_then(|x| x.as_str()).is_some();
        if has_payload { payloads += 1; }

        let mut hit = false;
        for ind in inds {
            let got = match field_of(&v, &ind.field) { Some(g) => g, None => continue };
            let matched = if ind.field == "text" {
                got.to_lowercase().contains(&ind.value.to_lowercase())
            } else {
                got == ind.value
            };
            if !matched { continue; }
            hit = true;
            let e = seen.entry(ind.label.clone()).or_insert(Seen {
                count: 0, first: t, last: t,
                example: got.chars().take(120).collect(),
            });
            e.count += 1;
            if t > 0.0 { if t < e.first || e.first == 0.0 { e.first = t; } if t > e.last { e.last = t; } }
        }
        if hit { labelled += 1; }
    }

    let mut out = J::obj();
    out.set("project", J::s(project));
    out.set("records", J::n(total as f64));
    out.set("records_with_output", J::n(payloads as f64));
    out.set("records_labelled", J::n(labelled as f64));
    out.set("captured_from", J::n(if tmin == f64::MAX { 0.0 } else { tmin }));
    out.set("captured_to", J::n(tmax));

    let mut found: Vec<J> = Vec::new();
    let mut silent: Vec<J> = Vec::new();
    let mut faults = 0usize;

    for ind in inds {
        match seen.get(&ind.label) {
            Some(s) => {
                if ind.means == "fault" { faults += s.count; }
                let mut o = J::obj();
                o.set("label", J::s(&ind.label));
                o.set("means", J::s(&ind.means));
                o.set("count", J::n(s.count as f64));
                o.set("first_seen", J::n(s.first));
                o.set("last_seen", J::n(s.last));
                o.set("example", J::s(&s.example));
                if !ind.note.is_empty() { o.set("note", J::s(&ind.note)); }
                found.push(o);
            }
            None => {
                // An indicator that did not fire is reported. A fault that has
                // stopped appearing and one that never matched anything are
                // different facts, and both vanish if silence is dropped.
                let mut o = J::obj();
                o.set("label", J::s(&ind.label));
                o.set("means", J::s(&ind.means));
                o.set("count", J::n(0.0));
                o.set("silent", J::s(
                    "this indicator matched nothing in this capture; that is either the \
                     condition not occurring or the indicator not matching what it was \
                     meant to"));
                silent.push(o);
            }
        }
    }

    out.set("indicators", J::Arr(found));
    out.set("silent_indicators", J::Arr(silent));
    // The negative space, in the project's own terms.
    let unlabelled = payloads.saturating_sub(labelled);
    out.set("output_not_labelled", J::n(unlabelled as f64));
    out.set("unlabelled_note", J::s(
        "output this profile has no indicator for. A profile that names a small part of \
         what a program says is describing that part, not the program."));
    out.set("faults", J::n(faults as f64));
    // 1 is "the answer is no": something the project itself calls a fault occurred.
    (out, if faults > 0 { 1 } else { 0 })
}

/// Compare two profile summaries taken at different times.
///
/// This is the reason the summary is small: a project can keep one per week and
/// still ask what changed between any two of them, long after the captures
/// themselves are gone.
pub fn diff(before: &str, after: &str) -> Result<J, String> {
    let b = crate::json::parse(before).map_err(|e| format!("baseline summary is not valid JSON: {}", e))?;
    let a = crate::json::parse(after).map_err(|e| format!("summary is not valid JSON: {}", e))?;

    let counts = |v: &J| -> BTreeMap<String, (f64, String)> {
        let mut m = BTreeMap::new();
        for key in ["indicators", "silent_indicators"] {
            if let Some(arr) = v.get(key).and_then(|x| x.as_arr()) {
                for it in arr {
                    let l = it.get("label").and_then(|x| x.as_str()).unwrap_or("").to_string();
                    let c = it.get("count").and_then(|x| x.as_f64()).unwrap_or(0.0);
                    let m2 = it.get("means").and_then(|x| x.as_str()).unwrap_or("").to_string();
                    if !l.is_empty() { m.insert(l, (c, m2)); }
                }
            }
        }
        m
    };
    let (cb, ca) = (counts(&b), counts(&a));

    let mut appeared: Vec<J> = Vec::new();
    let mut stopped: Vec<J> = Vec::new();
    let mut moved: Vec<J> = Vec::new();
    let mut unknown: Vec<J> = Vec::new();

    for (label, (ac, means)) in &ca {
        match cb.get(label) {
            None => {
                let mut o = J::obj();
                o.set("label", J::s(label)); o.set("means", J::s(means));
                o.set("count", J::n(*ac));
                o.set("change", J::s("this indicator did not exist in the baseline profile"));
                unknown.push(o);
            }
            Some((bc, _)) => {
                if *bc == 0.0 && *ac > 0.0 {
                    let mut o = J::obj();
                    o.set("label", J::s(label)); o.set("means", J::s(means));
                    o.set("now", J::n(*ac));
                    o.set("change", J::s("silent in the baseline, occurring now"));
                    appeared.push(o);
                } else if *bc > 0.0 && *ac == 0.0 {
                    let mut o = J::obj();
                    o.set("label", J::s(label)); o.set("means", J::s(means));
                    o.set("was", J::n(*bc));
                    o.set("change", J::s(
                        "occurring in the baseline, silent now; either it stopped or the \
                         indicator no longer matches what it was meant to"));
                    stopped.push(o);
                } else if *bc > 0.0 && (*ac / *bc > 2.0 || *ac / *bc < 0.5) {
                    let mut o = J::obj();
                    o.set("label", J::s(label)); o.set("means", J::s(means));
                    o.set("was", J::n(*bc)); o.set("now", J::n(*ac));
                    o.set("change", J::s("count moved by more than a factor of two"));
                    moved.push(o);
                }
            }
        }
    }
    for label in cb.keys() {
        if !ca.contains_key(label) {
            let mut o = J::obj();
            o.set("label", J::s(label));
            o.set("change", J::s("present in the baseline profile and absent from this one"));
            unknown.push(o);
        }
    }

    let mut out = J::obj();
    out.set("baseline_captured_to", b.get("captured_to").cloned().unwrap_or(J::n(0.0)));
    out.set("captured_to", a.get("captured_to").cloned().unwrap_or(J::n(0.0)));
    out.set("appeared", J::Arr(appeared));
    out.set("stopped", J::Arr(stopped));
    out.set("count_moved", J::Arr(moved));
    out.set("profile_changed", J::Arr(unknown));
    out.set("this_does_not_say", J::s(
        "whether any change is an improvement. Two captures taken weeks apart differ in \
         load and in what the machine was asked to do as well as in the code, and nothing \
         here separates those."));
    Ok(out)
}
