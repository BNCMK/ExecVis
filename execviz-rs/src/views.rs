// ========================================================================
//  MANIFEST
// ========================================================================
//  script_name: views.rs
//  script_path: execviz-rs/src/views.rs
//  module_name: views
//  version: 0.53.1
//  description: Progressive summarisation and queries over the two edge sets. Each tier returns aggregates rather than the tier below it, so a large trace is consumed one level at a time.
//  kind: module
//  spec: internal
//  internal_dependencies: json, store
//  external_dependencies: std
//  features: views
//  api_version: execvis-v1.0.0
//  last_updated: 2026-08-07
// ========================================================================

//! Progressive summarisation and queries over the two edge sets
//!. Each tier returns aggregates rather than the tier below it, so a
//! large trace is consumed one level at a time.
use crate::json::J;
use crate::store::Span;
use std::collections::BTreeMap;

// ========================================================================
// CONSTANTS
// ========================================================================

pub const STALE_SECONDS: f64 = 2.0;

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

pub fn now_ref(spans: &[Span]) -> f64 {
    let mut m = 0.0f64;
    for s in spans {
        if s.start > m { m = s.start; }
        if let Some(e) = s.end { if e > m { m = e; } }
    }
    m
}

pub fn is_stale(s: &Span, r: f64) -> bool {
    s.end.is_none() && (r - s.start) > STALE_SECONDS
}

// ========================================================================
// INTERNALS
// ========================================================================

fn counts<'a, I: Iterator<Item = &'a str>>(it: I) -> J {
    let mut m: BTreeMap<String, i64> = BTreeMap::new();
    for k in it { *m.entry(k.to_string()).or_insert(0) += 1; }
    let mut o = J::obj();
    for (k, v) in m { o.set(&k, J::n(v as f64)); }
    o
}

fn agg(group: &[&Span], r: f64) -> J {
    let mut o = J::obj();
    o.set("spans", J::n(group.len() as f64));
    o.set("errors", J::n(group.iter().filter(|s| s.status == "error").count() as f64));
    o.set("running", J::n(group.iter().filter(|s| s.end.is_none()).count() as f64));
    o.set("stale_running", J::n(group.iter().filter(|s| is_stale(s, r)).count() as f64));
    let total: f64 = group.iter().filter_map(|s| s.duration_ms()).sum();
    o.set("total_ms", J::n((total * 100.0).round() / 100.0));
    o.set("kinds", counts(group.iter().map(|s| s.kind.as_str())));
    o
}

fn routes(spans: &[&Span]) -> J {
    let byid: BTreeMap<&str, &Span> = spans.iter().map(|s| (s.span_id.as_str(), *s)).collect();
    let mut m: BTreeMap<String, (String, String, i64, i64, bool)> = BTreeMap::new();
    for s in spans {
        let p = match &s.parent_span_id { Some(p) => byid.get(p.as_str()), None => None };
        let p = match p { Some(p) => *p, None => continue };
        let a = p.domain.clone().unwrap_or_else(|| "unknown".into());
        let b = s.domain.clone().unwrap_or_else(|| "unknown".into());
        if a == b { continue; }
        let key = format!("{}>{}", a, b);
        let e = m.entry(key).or_insert((a, b, 0, 0, p.host_id != s.host_id));
        e.2 += 1;
        if s.status == "error" { e.3 += 1; }
    }
    let mut v: Vec<_> = m.into_values().collect();
    v.sort_by(|x, y| y.2.cmp(&x.2));
    J::Arr(v.into_iter().map(|(a, b, c, er, xh)| {
        let mut o = J::obj();
        o.set("from", J::s(&a)); o.set("to", J::s(&b));
        o.set("count", J::n(c as f64)); o.set("errors", J::n(er as f64));
        o.set("cross_host", J::Bool(xh));
        o
    }).collect())
}

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

pub fn view(spans: &[Span], lod: &str, host: Option<&str>, cluster: Option<&str>,
            family: Option<&str>, span_id: Option<&str>) -> J {
    let r = now_ref(spans);
    let mut out = J::obj();
    out.set("lod", J::s(lod));
    match lod {
        "system" => {
            let mut m: BTreeMap<&str, Vec<&Span>> = BTreeMap::new();
            for s in spans { m.entry(s.host_id.as_str()).or_default().push(s); }
            out.set("hosts", J::Arr(m.into_iter().map(|(h, g)| {
                let mut o = agg(&g, r); o.set("host", J::s(h)); o
            }).collect()));
        }
        "field" => {
            let pool: Vec<&Span> = spans.iter()
                .filter(|s| host.map_or(true, |h| s.host_id == h)).collect();
            let mut m: BTreeMap<String, Vec<&Span>> = BTreeMap::new();
            for s in &pool {
                m.entry(s.domain.clone().unwrap_or_else(|| "unknown".into()))
                    .or_default().push(s);
            }
            out.set("host", match host { Some(h) => J::s(h), None => J::Null });
            out.set("clusters", J::Arr(m.into_iter().map(|(c, g)| {
                let mut o = agg(&g, r); o.set("cluster", J::s(&c)); o
            }).collect()));
            out.set("routes", routes(&pool));
        }
        "cluster" => {
            let pool: Vec<&Span> = spans.iter()
                .filter(|s| s.domain.as_deref().unwrap_or("unknown") == cluster.unwrap_or(""))
                .collect();
            let mut m: BTreeMap<&str, Vec<&Span>> = BTreeMap::new();
            for s in &pool { m.entry(s.family()).or_default().push(s); }
            out.set("cluster", J::s(cluster.unwrap_or("")));
            out.set("families", J::Arr(m.into_iter().map(|(f, g)| {
                let mut o = agg(&g, r); o.set("family", J::s(f)); o
            }).collect()));
        }
        "channel" => {
            let mut pool: Vec<&Span> = spans.iter()
                .filter(|s| s.domain.as_deref().unwrap_or("unknown") == cluster.unwrap_or(""))
                .filter(|s| family.map_or(true, |f| s.family() == f))
                .collect();
            pool.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap());
            out.set("cluster", J::s(cluster.unwrap_or("")));
            out.set("family", match family { Some(f) => J::s(f), None => J::Null });
            out.set("rows", J::Arr(pool.iter().map(|s| {
                let mut o = J::obj();
                o.set("span_id", J::s(&s.span_id));
                o.set("name", J::s(&s.name));
                o.set("kind", J::s(&s.kind));
                o.set("status", J::s(&s.status));
                o.set("duration_ms", match s.duration_ms() { Some(d)=>J::n(d), None=>J::Null });
                o.set("stale", J::Bool(is_stale(s, r)));
                o.set("links", J::Arr(s.links.iter().map(|l| J::s(l)).collect()));
                o
            }).collect()));
        }
        "span" => {
            let found = spans.iter().find(|s| Some(s.span_id.as_str()) == span_id);
            match found {
                Some(s) => {
                    let mut o = s.to_json();
                    o.set("stale", J::Bool(is_stale(s, r)));
                    o.set("children", J::Arr(spans.iter()
                        .filter(|x| x.parent_span_id.as_deref() == span_id)
                        .map(|x| J::s(&x.span_id)).collect()));
                    out.set("span", o);
                }
                None => { out.set("span", J::Null); out.set("error", J::s("not found")); }
            }
        }
        _ => { out.set("error", J::s("unknown lod")); }
    }
    out
}

pub fn query(spans: &[Span], q: &str, span_id: Option<&str>, limit: usize,
             min_overlap_ms: f64) -> J {
    let r = now_ref(spans);
    let mut out = J::obj();
    out.set("query", J::s(q));
    match q {
        "stale" => {
            let hits: Vec<&Span> = spans.iter().filter(|s| is_stale(s, r)).collect();
            out.set("note", J::s("running past threshold, an unfinished span (2.3)"));
            out.set("count", J::n(hits.len() as f64));
            out.set("results", J::Arr(hits.iter().take(limit).map(|s| {
                let mut o = J::obj();
                o.set("span_id", J::s(&s.span_id)); o.set("name", J::s(&s.name));
                o.set("domain", match &s.domain { Some(d)=>J::s(d), None=>J::Null });
                o.set("host", J::s(&s.host_id)); o.set("kind", J::s(&s.kind));
                o.set("running_for_ms", J::n(((r - s.start) * 1000.0).round()));
                o
            }).collect()));
        }
        "errors" => {
            let hits: Vec<&Span> = spans.iter().filter(|s| s.status == "error").collect();
            out.set("count", J::n(hits.len() as f64));
            out.set("results", J::Arr(hits.iter().take(limit).map(|s| {
                let mut o = J::obj();
                o.set("span_id", J::s(&s.span_id)); o.set("name", J::s(&s.name));
                o.set("domain", match &s.domain { Some(d)=>J::s(d), None=>J::Null });
                o.set("attributes", s.attributes.clone());
                o
            }).collect()));
        }
        "races" => {
            // Causal siblings that overlapped in time: the causal graph permits
            // it, the temporal graph shows it happened.
            let mut sib: BTreeMap<&str, Vec<&Span>> = BTreeMap::new();
            for s in spans {
                if let Some(p) = &s.parent_span_id { sib.entry(p.as_str()).or_default().push(s); }
            }
            let mut res: Vec<(f64, J)> = Vec::new();
            for (parent, g) in sib {
                if g.len() < 2 { continue; }
                for i in 0..g.len() {
                    for j in (i + 1)..g.len() {
                        let (a, b) = (g[i], g[j]);
                        let ae = a.end.unwrap_or(r);
                        let be = b.end.unwrap_or(r);
                        let ov = ae.min(be) - a.start.max(b.start);
                        if ov * 1000.0 >= min_overlap_ms {
                            let mut o = J::obj();
                            o.set("parent", J::s(parent));
                            o.set("a", J::s(&a.name)); o.set("b", J::s(&b.name));
                            o.set("a_span", J::s(&a.span_id)); o.set("b_span", J::s(&b.span_id));
                            o.set("overlap_ms", J::n((ov * 100000.0).round() / 100.0));
                            o.set("same_domain", J::Bool(a.domain == b.domain));
                            res.push((ov, o));
                        }
                    }
                }
            }
            res.sort_by(|x, y| y.0.partial_cmp(&x.0).unwrap());
            out.set("note", J::s("causal siblings overlapping in time"));
            out.set("count", J::n(res.len() as f64));
            out.set("results", J::Arr(res.into_iter().take(limit).map(|(_, o)| o).collect()));
        }
        "slowest" => {
            let mut hits: Vec<&Span> = spans.iter().filter(|s| s.end.is_some()).collect();
            hits.sort_by(|a, b| b.duration_ms().unwrap().partial_cmp(&a.duration_ms().unwrap()).unwrap());
            out.set("results", J::Arr(hits.iter().take(limit).map(|s| {
                let mut o = J::obj();
                o.set("span_id", J::s(&s.span_id)); o.set("name", J::s(&s.name));
                o.set("domain", match &s.domain { Some(d)=>J::s(d), None=>J::Null });
                o.set("kind", J::s(&s.kind));
                o.set("duration_ms", J::n(s.duration_ms().unwrap()));
                o
            }).collect()));
        }
        "hotpaths" => {
            let refs: Vec<&Span> = spans.iter().collect();
            out.set("results", routes(&refs));
        }
        "descendants" | "ancestors" => {
            let byid: BTreeMap<&str, &Span> =
                spans.iter().map(|s| (s.span_id.as_str(), s)).collect();
            let mut res = Vec::new();
            if q == "ancestors" {
                let mut cur = span_id.and_then(|i| byid.get(i)).copied();
                while let Some(c) = cur {
                    match &c.parent_span_id {
                        Some(p) => {
                            match byid.get(p.as_str()) {
                                Some(pp) => {
                                    let mut o = J::obj();
                                    o.set("span_id", J::s(&pp.span_id));
                                    o.set("name", J::s(&pp.name));
                                    o.set("domain", match &pp.domain { Some(d)=>J::s(d), None=>J::Null });
                                    res.push(o);
                                    cur = Some(*pp);
                                }
                                None => break,
                            }
                        }
                        None => break,
                    }
                    if res.len() >= limit { break; }
                }
            } else {
                let mut kids: BTreeMap<&str, Vec<&Span>> = BTreeMap::new();
                for s in spans {
                    if let Some(p) = &s.parent_span_id { kids.entry(p.as_str()).or_default().push(s); }
                }
                let mut stack: Vec<&Span> =
                    span_id.and_then(|i| kids.get(i)).cloned().unwrap_or_default();
                while let Some(s) = stack.first().copied() {
                    stack.remove(0);
                    let mut o = J::obj();
                    o.set("span_id", J::s(&s.span_id)); o.set("name", J::s(&s.name));
                    o.set("domain", match &s.domain { Some(d)=>J::s(d), None=>J::Null });
                    o.set("kind", J::s(&s.kind)); o.set("status", J::s(&s.status));
                    res.push(o);
                    if res.len() >= limit { break; }
                    if let Some(k) = kids.get(s.span_id.as_str()) { stack.extend(k.iter().copied()); }
                }
            }
            out.set("of", match span_id { Some(i)=>J::s(i), None=>J::Null });
            out.set("results", J::Arr(res));
        }
        _ => { out.set("error", J::s("unknown query")); }
    }
    out
}

/// Compare two captures by (domain, name, kind) signature: run, patch,
/// re-capture, compare.
pub fn diff(a: &[Span], b: &[Span]) -> J {
    type Prof = BTreeMap<(String, String, String), (i64, i64, f64, i64)>;
    fn prof(spans: &[Span]) -> Prof {
        let mut p: Prof = BTreeMap::new();
        for s in spans {
            let k = (s.domain.clone().unwrap_or_default(), s.name.clone(), s.kind.clone());
            let e = p.entry(k).or_insert((0, 0, 0.0, 0));
            e.0 += 1;
            if s.status == "error" { e.1 += 1; }
            e.2 += s.duration_ms().unwrap_or(0.0);
            if s.end.is_none() { e.3 += 1; }
        }
        p
    }
    let (pa, pb) = (prof(a), prof(b));
    let mut keys: Vec<_> = pa.keys().chain(pb.keys()).cloned().collect();
    keys.sort(); keys.dedup();
    let (mut added, mut removed, mut changed, mut moved) = (vec![], vec![], vec![], vec![]);
    for k in keys {
        let base = |o: &mut J| {
            o.set("domain", J::s(&k.0)); o.set("name", J::s(&k.1)); o.set("kind", J::s(&k.2));
        };
        match (pa.get(&k), pb.get(&k)) {
            (None, Some(y)) => { let mut o = J::obj(); base(&mut o);
                o.set("count", J::n(y.0 as f64)); o.set("errors", J::n(y.1 as f64)); added.push(o); }
            (Some(x), None) => { let mut o = J::obj(); base(&mut o);
                o.set("count", J::n(x.0 as f64)); o.set("errors", J::n(x.1 as f64)); removed.push(o); }
            (Some(x), Some(y)) => {
                let (dc, de, dms, ds) = (y.0 - x.0, y.1 - x.1, y.2 - x.2, y.3 - x.3);
                if dc != 0 || de != 0 || ds != 0 || dms.abs() > 1e-6 {
                    let mut o = J::obj(); base(&mut o);
                    o.set("count_delta", J::n(dc as f64));
                    o.set("error_delta", J::n(de as f64));
                    o.set("total_ms_delta", J::n((dms * 100.0).round() / 100.0));
                    o.set("stale_delta", J::n(ds as f64));
                    changed.push(o);
                }
            }
            (None, None) => {}
        }
    }
    // Moving code between packages renames its domain, so a naive signature diff
    // reports every span in it as removed and added at once. Matching the
    // leftovers on (name, kind) recovers the truth: the work did not appear or
    // vanish, it changed address.
    {
        let mut rem_idx: Vec<usize> = (0..removed.len()).collect();
        let mut add_used = vec![false; added.len()];
        let mut drop_rem = vec![false; removed.len()];
        for &ri in rem_idx.iter() {
            let rn = removed[ri].get("name").and_then(|x| x.as_str()).unwrap_or("").to_string();
            let rk = removed[ri].get("kind").and_then(|x| x.as_str()).unwrap_or("").to_string();
            let rd = removed[ri].get("domain").and_then(|x| x.as_str()).unwrap_or("").to_string();
            for (ai, a) in added.iter().enumerate() {
                if add_used[ai] { continue; }
                let an = a.get("name").and_then(|x| x.as_str()).unwrap_or("");
                let ak = a.get("kind").and_then(|x| x.as_str()).unwrap_or("");
                let ad = a.get("domain").and_then(|x| x.as_str()).unwrap_or("");
                if an == rn && ak == rk && ad != rd {
                    let mut o = J::obj();
                    o.set("name", J::s(&rn));
                    o.set("kind", J::s(&rk));
                    o.set("from_domain", J::s(&rd));
                    o.set("to_domain", J::s(ad));
                    moved.push(o);
                    add_used[ai] = true; drop_rem[ri] = true;
                    break;
                }
            }
        }
        let mut i = 0; added.retain(|_| { let k = !add_used[i]; i += 1; k });
        let mut j = 0; removed.retain(|_| { let k = !drop_rem[j]; j += 1; k });
        rem_idx.clear();
    }

    let mut sum = J::obj();
    sum.set("a_spans", J::n(a.len() as f64));
    sum.set("b_spans", J::n(b.len() as f64));
    sum.set("a_errors", J::n(a.iter().filter(|s| s.status=="error").count() as f64));
    sum.set("b_errors", J::n(b.iter().filter(|s| s.status=="error").count() as f64));
    sum.set("a_stale", J::n(a.iter().filter(|s| s.end.is_none()).count() as f64));
    sum.set("b_stale", J::n(b.iter().filter(|s| s.end.is_none()).count() as f64));
    let mut out = J::obj();
    out.set("summary", sum);
    out.set("added", J::Arr(added));
    out.set("removed", J::Arr(removed));
    out.set("changed", J::Arr(changed));
    out.set("moved", J::Arr(moved));
    out
}
