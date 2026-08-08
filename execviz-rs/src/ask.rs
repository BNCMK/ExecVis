// ========================================================================
//  MANIFEST
// ========================================================================
//  script_name: ask.rs
//  script_path: execviz-rs/src/ask.rs
//  module_name: ask
//  version: 0.53.1
//  description: A language for questions nobody anticipated.
//  kind: module
//  spec: internal
//  internal_dependencies: json, store, syscalls
//  external_dependencies: std
//  features: ask
//  api_version: execvis-v1.0.0
//  last_updated: 2026-08-07
// ========================================================================

//! A language for questions nobody anticipated.
//!
//! A capture worth keeping outlasts the reports it was built to answer, so it
//! needs a language rather than a fixed set of questions.
//!
//! What is unusual here is not the language, it is what the language refuses.
//! The honesty rules of this project are enforced BY it rather than left to
//! whoever writes a query:
//!
//!   - a rollup that is not a monoid cannot be expressed: median and percentile
//!     are rejected at parse time with the reason
//!   - a percentile below the sample threshold refuses rather than answers
//!   - no result carries a field named `cause`, because co-occurrence is not it
//!
//! The grammar is small and deliberately so:
//!
//!   from spans | floor
//!   where <field> <op> <value>       (= != > < >= <= ~ contains)
//!   group by <field>
//!   show count | sum(<f>) | min(<f>) | max(<f>) | mean(<f>) | any(<f>)
//!   sort by <field> [desc] | limit <n>

use crate::json::J;
use crate::store::Span;
use crate::syscalls::Record;
use std::collections::BTreeMap;

// ========================================================================
// CONSTANTS
// ========================================================================

/// The smallest sample a spread statistic is allowed to speak about.
const MIN_SAMPLE: usize = 20;

// ========================================================================
// TYPES
// ========================================================================

pub struct Query {
    source: String,
    filters: Vec<(String, String, String)>,
    group: Option<String>,
    show: Vec<(String, String)>,   // (aggregate, field)
    sort: Option<(String, bool)>,
    limit: usize,
}

// ========================================================================
// INTERNALS
// ========================================================================

/// Aggregates that survive being combined out of order.
///
/// A rollup must be a monoid: count, sum, min, max and any all combine
/// associatively, so a tier can be built from tiers. Mean is allowed only
/// because it is carried as a sum and a count and divided at reading.
fn is_monoid(agg: &str) -> bool {
    matches!(agg, "count" | "sum" | "min" | "max" | "any" | "mean")
}

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

pub fn parse(q: &str) -> Result<Query, String> {
    let mut out = Query {
        source: "spans".into(), filters: Vec::new(), group: None,
        show: vec![("count".into(), String::new())], sort: None, limit: 40,
    };
    let lower = q.to_lowercase();
    let toks: Vec<&str> = q.split_whitespace().collect();
    let mut i = 0;
    while i < toks.len() {
        match toks[i].to_lowercase().as_str() {
            "from" => {
                i += 1;
                let s = toks.get(i).ok_or("`from` needs a source: spans or floor")?;
                if *s != "spans" && *s != "floor" {
                    return Err(format!("unknown source `{}`: this reads spans or floor", s));
                }
                out.source = s.to_string();
            }
            "where" => {
                i += 1;
                let f = toks.get(i).ok_or("`where` needs a field")?.to_string();
                i += 1;
                let op = toks.get(i).ok_or("`where` needs an operator")?.to_string();
                i += 1;
                let v = toks.get(i).ok_or("`where` needs a value")?.trim_matches('"').to_string();
                out.filters.push((f, op, v));
            }
            "group" => {
                i += 1;
                if toks.get(i).map(|t| t.to_lowercase()) != Some("by".into()) {
                    return Err("`group` is followed by `by`".into());
                }
                i += 1;
                out.group = Some(toks.get(i).ok_or("`group by` needs a field")?.to_string());
            }
            "show" => {
                i += 1;
                out.show.clear();
                while i < toks.len() && !["sort", "limit", "where", "group", "from"]
                        .contains(&toks[i].to_lowercase().as_str()) {
                    let t = toks[i].trim_end_matches(',');
                    let (agg, field) = match t.split_once('(') {
                        Some((a, rest)) => (a.to_string(), rest.trim_end_matches(')').to_string()),
                        None => (t.to_string(), String::new()),
                    };
                    // The refusal that matters, and it happens before anything runs.
                    if agg == "median" || agg == "percentile" || agg.starts_with("p9") || agg.starts_with("p5") {
                        return Err(format!(
                            "`{}` cannot be expressed here: it is not a monoid, so a tier built \
                             from tiers would be wrong, and this tool refuses to compute a figure \
                             it could not roll up accurately. Use min, max, mean or count.", agg));
                    }
                    if !is_monoid(&agg) {
                        return Err(format!("unknown aggregate `{}`: count, sum, min, max, mean, any", agg));
                    }
                    out.show.push((agg, field));
                    i += 1;
                }
                continue;
            }
            "sort" => {
                i += 1;
                if toks.get(i).map(|t| t.to_lowercase()) == Some("by".into()) { i += 1; }
                let f = toks.get(i).ok_or("`sort by` needs a field")?.to_string();
                let desc = toks.get(i + 1).map(|t| t.to_lowercase() == "desc").unwrap_or(false);
                if desc { i += 1; }
                out.sort = Some((f, desc));
            }
            "limit" => {
                i += 1;
                out.limit = toks.get(i).and_then(|t| t.parse().ok()).ok_or("`limit` needs a number")?;
            }
            other => return Err(format!("unexpected `{}` in: {}", other, lower)),
        }
        i += 1;
    }
    Ok(out)
}

// ========================================================================
// INTERNALS
// ========================================================================

fn span_field(s: &Span, f: &str) -> String {
    match f {
        "name" => s.name.clone(),
        "kind" => s.kind.clone(),
        "status" => s.status.clone(),
        "host" | "host_id" => s.host_id.clone(),
        "domain" => s.domain.clone().unwrap_or_default(),
        "trace" | "trace_id" => s.trace_id.clone(),
        "duration" => s.end.map(|e| (e - s.start).to_string()).unwrap_or_default(),
        "start" => s.start.to_string(),
        "open" => s.end.is_none().to_string(),
        _ => s.attributes.get(f).map(|v| match v {
            J::Str(x) => x.clone(), other => other.dump(),
        }).unwrap_or_default(),
    }
}

fn rec_field(r: &Record, f: &str) -> String {
    match f {
        "call" | "name" => r.name.clone(),
        "tid" => r.tid.to_string(),
        "comm" | "who" => r.comm.clone().unwrap_or_default(),
        "t" | "start" => r.t.to_string(),
        "duration" => r.dur.to_string(),
        _ => String::new(),
    }
}

fn matches(v: &str, op: &str, want: &str) -> bool {
    match op {
        "=" | "==" => v == want,
        "!=" => v != want,
        "~" | "contains" => v.contains(want),
        ">" | "<" | ">=" | "<=" => {
            let (a, b) = (v.parse::<f64>(), want.parse::<f64>());
            match (a, b) {
                (Ok(a), Ok(b)) => match op { ">" => a > b, "<" => a < b, ">=" => a >= b, _ => a <= b },
                _ => false,
            }
        }
        _ => false,
    }
}

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

/// Runs a query and returns rows, or the reason it will not.
pub fn run(q: &Query, spans: &[Span], recs: &[Record]) -> J {
    // rows as (group key, numeric field lookup)
    let mut groups: BTreeMap<String, Vec<BTreeMap<String, f64>>> = BTreeMap::new();
    let mut group_any: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();

    let mut push = |key: String, nums: BTreeMap<String, f64>, strs: BTreeMap<String, String>,
                    groups: &mut BTreeMap<String, Vec<BTreeMap<String, f64>>>,
                    ga: &mut BTreeMap<String, BTreeMap<String, String>>| {
        groups.entry(key.clone()).or_default().push(nums);
        ga.entry(key).or_insert(strs);
    };

    if q.source == "spans" {
        for s in spans {
            if !q.filters.iter().all(|(f, op, v)| matches(&span_field(s, f), op, v)) { continue }
            let key = q.group.as_ref().map(|g| span_field(s, g)).unwrap_or_else(|| "all".into());
            let mut nums = BTreeMap::new();
            if let Some(e) = s.end { nums.insert("duration".to_string(), e - s.start); }
            nums.insert("start".to_string(), s.start);
            let mut strs = BTreeMap::new();
            for f in ["name", "kind", "status", "host", "domain"] {
                strs.insert(f.to_string(), span_field(s, f));
            }
            push(key, nums, strs, &mut groups, &mut group_any);
        }
    } else {
        for r in recs {
            if !q.filters.iter().all(|(f, op, v)| matches(&rec_field(r, f), op, v)) { continue }
            let key = q.group.as_ref().map(|g| rec_field(r, g)).unwrap_or_else(|| "all".into());
            let mut nums = BTreeMap::new();
            nums.insert("duration".to_string(), r.dur);
            nums.insert("start".to_string(), r.t);
            let mut strs = BTreeMap::new();
            for f in ["call", "comm", "tid"] { strs.insert(f.to_string(), rec_field(r, f)); }
            push(key, nums, strs, &mut groups, &mut group_any);
        }
    }

    let mut rows: Vec<J> = Vec::new();
    let mut refused: Vec<J> = Vec::new();
    for (key, members) in &groups {
        let mut row: Vec<(String, J)> = vec![
            (q.group.clone().unwrap_or_else(|| "group".into()), J::Str(key.clone())),
        ];
        for (agg, field) in &q.show {
            match agg.as_str() {
                "count" => row.push(("count".into(), J::Num(members.len() as f64))),
                "any" => {
                    let v = group_any.get(key).and_then(|m| m.get(field)).cloned().unwrap_or_default();
                    row.push((format!("any({})", field), J::Str(v)));
                }
                _ => {
                    let vals: Vec<f64> = members.iter().filter_map(|m| m.get(field).copied()).collect();
                    if vals.is_empty() {
                        row.push((format!("{}({})", agg, field), J::Null));
                        continue;
                    }
                    // Spread statistics state their sample size, and refuse below
                    // the threshold rather than answering from too little.
                    if agg == "mean" && vals.len() < MIN_SAMPLE {
                        refused.push(J::Obj([
                            ("group".to_string(), J::Str(key.clone())),
                            ("refused".to_string(), J::Str(format!("mean({})", field))),
                            ("samples".to_string(), J::Num(vals.len() as f64)),
                            ("reason".to_string(), J::Str(format!(
                                "{} samples is below the threshold of {}; a mean from this little \
                                 is not a measurement", vals.len(), MIN_SAMPLE))),
                        ].into_iter().collect()));
                        row.push((format!("mean({})", field), J::Null));
                        continue;
                    }
                    let v = match agg.as_str() {
                        "sum" => vals.iter().sum::<f64>(),
                        "min" => vals.iter().cloned().fold(f64::INFINITY, f64::min),
                        "max" => vals.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
                        _ => vals.iter().sum::<f64>() / vals.len() as f64,
                    };
                    row.push((format!("{}({})", agg, field), J::Num((v * 1e6).round() / 1e6)));
                    row.push((format!("{}_samples", agg), J::Num(vals.len() as f64)));
                }
            }
        }
        rows.push(J::Obj(row.into_iter().collect()));
    }

    if let Some((f, desc)) = &q.sort {
        rows.sort_by(|a, b| {
            let av = a.get(f).and_then(|x| x.as_f64()).unwrap_or(f64::NEG_INFINITY);
            let bv = b.get(f).and_then(|x| x.as_f64()).unwrap_or(f64::NEG_INFINITY);
            if *desc { bv.partial_cmp(&av).unwrap_or(std::cmp::Ordering::Equal) }
            else { av.partial_cmp(&bv).unwrap_or(std::cmp::Ordering::Equal) }
        });
    }
    rows.truncate(q.limit);

    J::Obj([
        ("rows".to_string(), J::Arr(rows)),
        ("refused".to_string(), J::Arr(refused)),
        ("source".to_string(), J::Str(q.source.clone())),
    ].into_iter().collect())
}
