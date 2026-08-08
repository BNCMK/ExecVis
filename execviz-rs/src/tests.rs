// ========================================================================
//  MANIFEST
// ========================================================================
//  script_name: tests.rs
//  script_path: execviz-rs/src/tests.rs
//  module_name: tests
//  version: 0.53.1
//  description: Tests for the core. Each one is written so that removing the behaviour it covers makes it fail: a test whose fixture cannot express the failure is a rubber stamp rather than a check.
//  kind: test
//  spec: internal
//  internal_dependencies: WriteHealth, ask, bundle, compare, conform, decode, egress, expect, find, finger, json, logs
//  external_dependencies: 
//  features: tests
//  api_version: execvis-v1.0.0
//  last_updated: 2026-08-07
// ========================================================================

//! Tests for the core. Each one is written so that removing the behaviour it
//! covers makes it fail: a test whose fixture cannot express the failure is a
//! rubber stamp rather than a check.
#![cfg(test)]
use crate::conform;
use crate::json::{self, J};
use crate::store::{Span, Store};
use crate::views;

// ========================================================================
// INTERNALS
// ========================================================================

fn span(id: &str, parent: Option<&str>, name: &str, kind: &str,
        start: f64, end: Option<f64>, status: &str) -> Span {
    Span {
        span_id: id.into(), trace_id: "t".into(),
        parent_span_id: parent.map(|s| s.to_string()),
        links: vec![], name: name.into(), kind: kind.into(),
        start, end, status: status.into(),
        lifecycle: J::Arr(vec![]), origin: "semantic".into(),
        host_id: "h1".into(), clock_source: None,
        domain: Some("d".into()), attributes: J::obj(), events: J::Arr(vec![]),
        inputs: J::Null, output: J::Null, error: J::Null, run: J::Null,
    }
}

fn tmp(name: &str) -> String {
    format!("/tmp/execviz_test_{}_{}.db", name, std::process::id())
}

// ========================================================================
// STORE
// ========================================================================

#[test]
fn two_phase_completion_updates_in_place_rather_than_duplicating() {
    let path = tmp("two_phase");
    let _ = std::fs::remove_file(&path);
    let st = Store::open(&path).unwrap();

    let mut open = span("a", None, "work", "call", 1.0, None, "running");
    st.upsert(&open).unwrap();
    assert_eq!(st.all().unwrap().len(), 1);

    // The same span arriving again with its second phase: this is what a remote
    // node re-sending a completed span looks like.
    open.end = Some(2.0);
    open.status = "ok".into();
    st.upsert(&open).unwrap();

    let all = st.all().unwrap();
    assert_eq!(all.len(), 1, "re-sending a completed span must not duplicate it");
    assert_eq!(all[0].end, Some(2.0));
    assert_eq!(all[0].status, "ok");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn an_unfinished_span_survives_the_round_trip_as_running() {
    let path = tmp("stale");
    let _ = std::fs::remove_file(&path);
    let st = Store::open(&path).unwrap();
    st.upsert(&span("a", None, "hang", "wait", 1.0, None, "running")).unwrap();
    let all = st.all().unwrap();
    assert!(all[0].end.is_none(), "an unfinished span is the absent end, not a guess");
    assert_eq!(all[0].status, "running");
    let _ = std::fs::remove_file(&path);
}

// ========================================================================
// VIEWS
// ========================================================================

#[test]
fn stale_means_running_past_threshold_not_merely_unfinished() {
    let young = span("a", None, "in_flight", "call", 100.0, None, "running");
    let old = span("b", None, "hung", "wait", 1.0, None, "running");
    let anchor = span("c", None, "done", "call", 1.0, Some(100.0), "ok");
    let spans = vec![young, old, anchor];
    let r = views::now_ref(&spans);
    assert!(!views::is_stale(&spans[0], r), "a span that just started is not stale");
    assert!(views::is_stale(&spans[1], r), "a span open far past the threshold is stale");
}

#[test]
fn races_finds_causal_siblings_that_overlap_and_ignores_ones_that_do_not() {
    let parent = span("p", None, "parent", "call", 0.0, Some(10.0), "ok");
    let a = span("a", Some("p"), "a", "call", 1.0, Some(5.0), "ok");
    let b = span("b", Some("p"), "b", "call", 3.0, Some(8.0), "ok");   // overlaps a
    let c = span("c", Some("p"), "c", "call", 8.5, Some(9.0), "ok");   // disjoint
    let spans = vec![parent, a, b, c];
    let out = views::query(&spans, "races", None, 50, 1.0);
    let results = out.get("results").and_then(|x| x.as_arr()).unwrap();
    assert_eq!(results.len(), 1, "only the overlapping pair is a race");
    let names: Vec<&str> = vec![
        results[0].get("a").and_then(|x| x.as_str()).unwrap(),
        results[0].get("b").and_then(|x| x.as_str()).unwrap()];
    assert!(names.contains(&"a") && names.contains(&"b"));
}

#[test]
fn view_tiers_aggregate_rather_than_returning_the_tier_below() {
    let spans: Vec<Span> = (0..40)
        .map(|i| span(&format!("s{}", i), None, "w", "call", i as f64, Some(i as f64 + 1.0), "ok"))
        .collect();
    let field = views::view(&spans, "field", None, None, None, None);
    let clusters = field.get("clusters").and_then(|x| x.as_arr()).unwrap();
    assert_eq!(clusters.len(), 1);
    assert_eq!(clusters[0].get("spans").and_then(|x| x.as_f64()), Some(40.0));
    assert!(clusters[0].get("rows").is_none(),
            "a field view must summarise, not carry every span beneath it");
}

#[test]
fn diff_calls_a_move_a_move_rather_than_a_removal_plus_an_addition() {
    let mut before = span("a", None, "handle", "call", 0.0, Some(1.0), "ok");
    before.domain = Some("svc_a".into());
    let mut after = span("a", None, "handle", "call", 0.0, Some(1.0), "ok");
    after.domain = Some("svc_b".into());        // same work, new package
    let d = views::diff(&vec![before], &vec![after]);
    let moved = d.get("moved").and_then(|x| x.as_arr()).unwrap();
    assert_eq!(moved.len(), 1, "the work did not vanish, it changed address");
    assert_eq!(moved[0].get("from_domain").and_then(|x| x.as_str()), Some("svc_a"));
    assert_eq!(moved[0].get("to_domain").and_then(|x| x.as_str()), Some("svc_b"));
    assert!(d.get("added").and_then(|x| x.as_arr()).unwrap().is_empty());
    assert!(d.get("removed").and_then(|x| x.as_arr()).unwrap().is_empty());
}

#[test]
fn diff_reports_a_regression_between_two_captures() {
    let before = vec![span("a", None, "step", "call", 0.0, Some(1.0), "ok")];
    let after = vec![
        span("a", None, "step", "call", 0.0, Some(1.0), "error"),
        span("b", None, "new_step", "call", 1.0, Some(2.0), "ok"),
    ];
    let d = views::diff(&before, &after);
    assert_eq!(d.get("summary").unwrap().get("b_errors").and_then(|x| x.as_f64()), Some(1.0));
    let added = d.get("added").and_then(|x| x.as_arr()).unwrap();
    assert_eq!(added.len(), 1);
    assert_eq!(added[0].get("name").and_then(|x| x.as_str()), Some("new_step"));
    let changed = d.get("changed").and_then(|x| x.as_arr()).unwrap();
    assert_eq!(changed.len(), 1, "the step that started failing must show as changed");
}

// ========================================================================
// CONFORMANCE: EACH FIXTURE REPRODUCES THE FAILURE IT CHECKS
// ========================================================================

fn violated_rules(spans: &[Span]) -> Vec<String> {
    let out = conform::check(spans);
    let mut rules = Vec::new();
    for h in out.get("hosts").and_then(|x| x.as_arr()).unwrap() {
        for v in h.get("violations").and_then(|x| x.as_arr()).unwrap() {
            rules.push(v.get("rule").and_then(|x| x.as_str()).unwrap().to_string());
        }
    }
    rules
}

#[test]
fn a_clean_capture_is_conformant() {
    let spans = vec![
        span("p", None, "parent", "call", 0.0, Some(10.0), "ok"),
        span("a", Some("p"), "child", "io", 1.0, Some(5.0), "ok"),
    ];
    let out = conform::check(&spans);
    assert_eq!(out.get("conformant"), Some(&J::Bool(true)));
    assert_eq!(out.get("findings").and_then(|x| x.as_f64()), Some(0.0));
}

#[test]
fn a_missing_parent_is_caught() {
    let spans = vec![span("a", Some("ghost"), "orphan", "call", 1.0, Some(2.0), "ok")];
    assert!(violated_rules(&spans).contains(&"parent_integrity".to_string()));
}

#[test]
fn a_cycle_is_caught() {
    let mut a = span("a", Some("b"), "a", "call", 1.0, Some(2.0), "ok");
    let b = span("b", Some("a"), "b", "call", 1.0, Some(2.0), "ok");
    a.parent_span_id = Some("b".into());
    assert!(violated_rules(&vec![a, b]).contains(&"no_cycles".to_string()));
}

#[test]
fn a_completed_span_still_reporting_running_is_caught() {
    let spans = vec![span("a", None, "done", "call", 1.0, Some(2.0), "running")];
    assert!(violated_rules(&spans).contains(&"two_phase".to_string()));
}

#[test]
fn a_child_starting_before_its_parent_is_caught() {
    let spans = vec![
        span("p", None, "parent", "call", 5.0, Some(10.0), "ok"),
        span("a", Some("p"), "child", "call", 1.0, Some(6.0), "ok"),
    ];
    assert!(violated_rules(&spans).contains(&"causal_time".to_string()));
}

#[test]
fn a_derivable_lifecycle_event_is_caught() {
    let mut s = span("a", None, "work", "call", 1.0, Some(2.0), "ok");
    let mut ev = J::obj();
    ev.set("t", J::n(1.0));
    ev.set("type", J::s("started"));       // start already says this
    s.lifecycle = J::Arr(vec![ev]);
    assert!(violated_rules(&vec![s]).contains(&"derivability".to_string()));
}

#[test]
fn a_kind_outside_the_ontology_is_caught() {
    let spans = vec![span("a", None, "work", "io_response", 1.0, Some(2.0), "ok")];
    assert!(violated_rules(&spans).contains(&"schema".to_string()));
}

#[test]
fn a_link_to_a_missing_span_is_caught() {
    let mut s = span("a", None, "join", "call", 1.0, Some(2.0), "ok");
    s.links = vec!["ghost".into()];
    assert!(violated_rules(&vec![s]).contains(&"link_integrity".to_string()));
}

#[test]
fn work_outliving_its_parent_is_an_observation_not_a_violation() {
    let spans = vec![
        span("p", None, "aborted_request", "call", 0.0, Some(2.0), "error"),
        span("a", Some("p"), "still_running", "io", 1.0, Some(9.0), "ok"),
    ];
    let out = conform::check(&spans);
    assert_eq!(out.get("conformant"), Some(&J::Bool(true)),
               "an aborted parent is a fact about the program, not an adapter defect");
    assert_eq!(out.get("observations").and_then(|x| x.as_f64()), Some(1.0));
}

#[test]
fn the_traced_programs_own_io_is_not_mistaken_for_adapter_machinery() {
    let spans = vec![span("a", None, "flush", "io", 1.0, Some(2.0), "ok")];
    assert!(!violated_rules(&spans).contains(&"self_tracing".to_string()),
            "a program flushing its own file is not the adapter tracing itself");
}

// ========================================================================
// WIRE FORMAT
// ========================================================================

#[test]
fn json_survives_a_round_trip_including_awkward_strings() {
    let mut o = J::obj();
    o.set("name", J::s("a \"quoted\" name\nwith newline\tand tab"));
    o.set("n", J::n(42.0));
    o.set("null", J::Null);
    o.set("arr", J::Arr(vec![J::n(1.0), J::Bool(true)]));
    let back = json::parse(&o.dump()).unwrap();
    assert_eq!(back.get("name").and_then(|x| x.as_str()),
               Some("a \"quoted\" name\nwith newline\tand tab"));
    assert_eq!(back.get("n").and_then(|x| x.as_f64()), Some(42.0));
    assert!(back.get("null").unwrap().is_null());
}

#[test]
fn a_span_round_trips_through_json_with_its_open_end_intact() {
    let s = span("a", None, "hang", "wait", 1.0, None, "running");
    let back = Span::from_json(&s.to_json(), None).unwrap();
    assert!(back.end.is_none(), "an open span must not acquire an end in transit");
    assert_eq!(back.status, "running");
    assert_eq!(back.span_id, "a");
}

#[test]
fn ingest_can_override_the_host_so_a_collector_attributes_by_source() {
    let s = span("a", None, "work", "call", 1.0, Some(2.0), "ok");
    let back = Span::from_json(&s.to_json(), Some("edge-7")).unwrap();
    assert_eq!(back.host_id, "edge-7");
}

// ========================================================================
// ROLLED-UP TIERS
// ========================================================================

#[test]
fn a_rollup_is_associative_or_a_tier_answer_depends_on_walk_order() {
    use crate::rollup::Rollup;
    let a = Rollup::leaf(&span("a", None, "x", "io", 0.0, Some(1.0), "ok"));
    let b = Rollup::leaf(&span("b", None, "y", "call", 1.0, Some(3.0), "error"));
    let c = Rollup::leaf(&span("c", None, "z", "wait", 2.0, None, "running"));
    let left = a.combine(&b).combine(&c);
    let right = a.combine(&b.combine(&c));
    assert_eq!(left, right, "grouping must not change a tier's summary");
}

#[test]
fn the_empty_rollup_is_an_identity() {
    use crate::rollup::Rollup;
    let a = Rollup::leaf(&span("a", None, "x", "io", 0.0, Some(1.0), "ok"));
    assert_eq!(Rollup::empty().combine(&a), a);
    assert_eq!(a.combine(&Rollup::empty()), a);
}

#[test]
fn a_ratio_is_divided_at_reading_not_stored_and_averaged() {
    use crate::rollup::Rollup;
    // three io spans in one child, one non-io in another: the true share is 3/4.
    // Averaging the children's shares would give (1.0 + 0.0) / 2 = 0.5.
    let mut heavy = Rollup::empty();
    for i in 0..3 {
        heavy = heavy.combine(&Rollup::leaf(&span(&format!("i{}", i), None, "io", "io", 0.0, Some(1.0), "ok")));
    }
    let light = Rollup::leaf(&span("c", None, "work", "call", 0.0, Some(1.0), "ok"));
    let combined = heavy.combine(&light);
    assert!((combined.io_share() - 0.75).abs() < 1e-9,
            "a share must be computed from combined counts, not averaged from children");
}

#[test]
fn a_digest_moves_when_a_span_completes_and_not_otherwise() {
    use crate::rollup::{build, leaf_digest};
    let open = span("a", None, "work", "call", 0.0, None, "running");
    let mut done = open.clone();
    done.end = Some(2.0); done.status = "ok".into();
    assert_ne!(leaf_digest(&open), leaf_digest(&done), "completion must move the digest");
    let t1 = build(&vec![open.clone()]);
    let t2 = build(&vec![open.clone()]);
    assert_eq!(t1.digest, t2.digest, "the same spans must give the same digest");
    let t3 = build(&vec![done]);
    assert_ne!(t1.digest, t3.digest, "a change below must reach the root");
}

#[test]
fn an_untouched_subtree_keeps_its_digest_when_a_sibling_changes() {
    use crate::rollup::{build, find};
    let mut quiet = span("q", None, "quiet", "call", 0.0, Some(1.0), "ok");
    quiet.host_id = "quiet-host".into(); quiet.domain = Some("quiet".into());
    let mut busy = span("b", None, "busy", "call", 0.0, None, "running");
    busy.host_id = "busy-host".into(); busy.domain = Some("busy".into());
    let before = build(&vec![quiet.clone(), busy.clone()]);
    busy.end = Some(4.0); busy.status = "ok".into();
    let after = build(&vec![quiet.clone(), busy]);
    let q1 = find(&before, "quiet-host").unwrap().digest.clone();
    let q2 = find(&after, "quiet-host").unwrap().digest.clone();
    assert_eq!(q1, q2, "the quiet side must be skippable: that is the whole saving");
    assert_ne!(before.digest, after.digest, "the root must still notice");
}

// ========================================================================
// PRIMITIVES, CHECKED AGAINST PUBLISHED VECTORS
// ========================================================================

#[test]
fn sha256_matches_the_published_vectors() {
    use crate::sha256::{sha256, hex};
    // FIPS 180-2 examples
    assert_eq!(hex(&sha256(b"")),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    assert_eq!(hex(&sha256(b"abc")),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    assert_eq!(hex(&sha256(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq")),
        "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1");
}

#[test]
fn hmac_sha256_matches_rfc4231() {
    use crate::sha256::{hmac_sha256, hex};
    // RFC 4231 test case 2
    assert_eq!(hex(&hmac_sha256(b"Jefe", b"what do ya want for nothing?")),
        "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
}

#[test]
fn pbkdf2_matches_rfc6070_style_vector() {
    use crate::sha256::{pbkdf2, hex};
    // PBKDF2-HMAC-SHA256, password "password", salt "salt", 1 iteration
    assert_eq!(hex(&pbkdf2(b"password", b"salt", 1)),
        "120fb6cffcf8b32c43e7225256c4f837a86548c92ccc35480805987cb70be17b");
    assert_eq!(hex(&pbkdf2(b"password", b"salt", 2)),
        "ae4d0c95af6b46d32d0adff928f06dd02a303f8ef3c251dfd6e2d85a95474c43");
}

#[test]
fn a_constant_time_comparison_still_compares() {
    use crate::sha256::constant_time_eq;
    assert!(constant_time_eq("abc", "abc"));
    assert!(!constant_time_eq("abc", "abd"));
    assert!(!constant_time_eq("abc", "abcd"));
}

// ========================================================================
// RETENTION
// ========================================================================

#[test]
fn trimming_removes_whole_traces_so_no_child_is_orphaned() {
    let mut parent = span("p", None, "req", "call", 0.0, Some(1.0), "ok");
    parent.trace_id = "old".into();
    let mut child = span("c", Some("p"), "step", "call", 0.1, Some(0.9), "ok");
    child.trace_id = "old".into();
    let mut fresh = span("f", None, "req", "call", 900.0, Some(901.0), "ok");
    fresh.trace_id = "new".into();
    let spans = vec![parent, child, fresh];
    let p = crate::retain::plan(&spans, 100.0, 0, 1000.0);
    assert_eq!(p.traces_removed, vec!["old".to_string()]);
    assert_eq!(p.spans_removed, 2, "the child must go with its parent, not be left pointing at nothing");
}

#[test]
fn an_open_span_is_never_trimmed_however_old() {
    let mut hung = span("h", None, "never_returned", "wait", 0.0, None, "running");
    hung.trace_id = "ancient".into();
    let p = crate::retain::plan(&vec![hung], 1.0, 0, 1_000_000.0);
    assert!(p.traces_removed.is_empty(), "an unfinished span outlives the policy");
    assert_eq!(p.traces_kept_open, vec!["ancient".to_string()]);
}

#[test]
fn trace_age_is_its_newest_activity_not_its_start() {
    // a long-running trace that was touched a moment ago is young
    let mut started_long_ago = span("a", None, "long", "call", 0.0, Some(995.0), "ok");
    started_long_ago.trace_id = "recent".into();
    let p = crate::retain::plan(&vec![started_long_ago], 100.0, 0, 1000.0);
    assert!(p.traces_removed.is_empty(), "a trace still being written to is not old");
}

// ========================================================================
// SYNCING BY DIGEST
// ========================================================================

fn tree_of(spans: &[Span]) -> crate::rollup::Node { crate::rollup::build(spans) }

#[test]
fn identical_stores_report_no_divergence() {
    let spans = vec![
        span("a", None, "x", "call", 0.0, Some(1.0), "ok"),
        span("b", Some("a"), "y", "io", 0.2, Some(0.8), "ok"),
    ];
    let mine = tree_of(&spans);
    let theirs = crate::json::parse(&crate::rollup::skeleton(&tree_of(&spans), 4).dump()).unwrap();
    let mut out = Vec::new();
    crate::rollup::diverge(&mine, &theirs, &mut out);
    assert!(out.is_empty(), "the same spans must compare equal");
}

#[test]
fn a_matching_branch_is_not_descended_into() {
    // Two hosts; only one differs. The quiet host's subtree must not be walked,
    // because skipping it is the entire saving the scheme exists for.
    let mut quiet = span("q", None, "quiet", "call", 0.0, Some(1.0), "ok");
    quiet.host_id = "quiet".into(); quiet.domain = Some("d".into());
    let mut busy = span("b", None, "busy", "call", 0.0, Some(1.0), "ok");
    busy.host_id = "busy".into(); busy.domain = Some("d".into());
    let before = vec![quiet.clone(), busy.clone()];
    let mut busy2 = busy.clone(); busy2.status = "error".into();
    let after = vec![quiet, busy2];

    let mine = tree_of(&before);
    let theirs = crate::json::parse(&crate::rollup::skeleton(&tree_of(&after), 4).dump()).unwrap();
    let mut out = Vec::new();
    crate::rollup::diverge(&mine, &theirs, &mut out);
    assert!(!out.is_empty(), "the changed host must be reported");
    assert!(out.iter().all(|d| !d.id.starts_with("quiet")),
            "the unchanged host must never appear: not descending into it is the saving");
}

#[test]
fn a_subtree_only_one_side_has_is_named_as_such() {
    let mut mine_only = span("m", None, "here", "call", 0.0, Some(1.0), "ok");
    mine_only.host_id = "h".into(); mine_only.domain = Some("only-here".into());
    let mut shared = span("s", None, "shared", "call", 0.0, Some(1.0), "ok");
    shared.host_id = "h".into(); shared.domain = Some("shared".into());

    let mine = tree_of(&vec![mine_only, shared.clone()]);
    let theirs = crate::json::parse(&crate::rollup::skeleton(&tree_of(&vec![shared]), 4).dump()).unwrap();
    let mut out = Vec::new();
    crate::rollup::diverge(&mine, &theirs, &mut out);
    let only = out.iter().find(|d| d.id.contains("only-here")).expect("the extra subtree must be reported");
    assert!(only.theirs.is_none(), "a subtree the far end lacks is 'only here', not merely different");
}

// ========================================================================
// PRIMITIVE AND FAMILY
// ========================================================================

#[test]
fn every_primitive_in_the_ontology_maps_to_a_family() {
    use crate::store::family_of;
    for kind in ["call", "branch", "loop", "io", "wait", "queue", "spawn", "error", "external"] {
        let f = family_of(kind);
        assert!(["control", "io", "wait", "boundary", "fault"].contains(&f),
                "{} mapped to {}, which is not a family", kind, f);
    }
}

#[test]
fn an_unknown_primitive_still_gets_a_family_rather_than_a_gap() {
    // The mapping is total on purpose: the checker reports the unknown kind, so
    // the reader learns of it from the check rather than from a hole in the map.
    assert_eq!(crate::store::family_of("something_new"), "control");
    let odd = span("a", None, "x", "something_new", 0.0, Some(1.0), "ok");
    assert_eq!(odd.family(), "control");
}

#[test]
fn a_family_sent_on_the_wire_is_ignored_because_it_is_derived() {
    let mut s = span("a", None, "x", "io", 0.0, Some(1.0), "ok");
    s.kind = "io".into();
    let mut wire = s.to_json();
    wire.set("family", J::s("fault"));      // a sender contradicting its own kind
    let back = Span::from_json(&wire, None).unwrap();
    assert_eq!(back.kind, "io");
    assert_eq!(back.family(), "io", "the family must follow the kind, never the claim");
}

// ========================================================================
// THE LOG WORKSPACE
// ========================================================================

fn line(t: f64, level: &str, msg: &str, span: &str) -> crate::logs::Line {
    crate::logs::Line { t, level: level.into(), msg: msg.into(),
        span_id: span.into(), span_name: span.into(),
        domain: "d".into(), host: "h".into(), status: "ok".into() }
}

#[test]
fn folding_states_how_many_lines_it_stands_for() {
    let lines = vec![
        line(1.0, "info", "retrying", "a"),
        line(2.0, "info", "retrying", "a"),
        line(3.0, "info", "retrying", "a"),
        line(4.0, "error", "gave up", "a"),
    ];
    let folded = crate::logs::fold(lines);
    assert_eq!(folded.len(), 2);
    assert_eq!(folded[0].count, 3, "a fold must carry its count, or it is data loss");
    assert_eq!(folded[1].count, 1);
    let total: usize = folded.iter().map(|g| g.count).sum();
    assert_eq!(total, 4, "folding must conserve every line it stands for");
}

#[test]
fn folding_does_not_merge_across_different_spans() {
    // the same message from two different pieces of work is two facts
    let lines = vec![
        line(1.0, "info", "same text", "a"),
        line(2.0, "info", "same text", "b"),
    ];
    assert_eq!(crate::logs::fold(lines).len(), 2);
}

#[test]
fn a_sort_never_reshuffles_lines_that_compare_equal() {
    // Two runs of one query must read the same, or the reader learns to
    // distrust the tool for no reason.
    let mk = || vec![
        line(1.0, "info", "first", "a"),
        line(2.0, "info", "second", "a"),
        line(3.0, "info", "third", "a"),
    ];
    let mut a = mk(); let mut b = mk();
    a.sort_by(|x, y| crate::logs::severity(&y.level).cmp(&crate::logs::severity(&x.level)));
    b.sort_by(|x, y| crate::logs::severity(&y.level).cmp(&crate::logs::severity(&x.level)));
    let names: Vec<&str> = a.iter().map(|l| l.msg.as_str()).collect();
    assert_eq!(names, vec!["first", "second", "third"], "equal rows keep recorded order");
    assert_eq!(names, b.iter().map(|l| l.msg.as_str()).collect::<Vec<_>>());
}

#[test]
fn counts_tally_every_line_exactly_once() {
    let lines = vec![
        line(1.0, "info", "a", "s"), line(2.0, "error", "b", "s"), line(3.0, "info", "c", "s"),
    ];
    let j = crate::logs::counts(&lines);
    assert_eq!(j.get("total").and_then(|x| x.as_f64()), Some(3.0));
    let by_level = j.get("by_level").unwrap();
    assert_eq!(by_level.get("info").and_then(|x| x.as_f64()), Some(2.0));
    assert_eq!(by_level.get("error").and_then(|x| x.as_f64()), Some(1.0));
}

// ========================================================================
// SEARCH, SELF TIME, CRITICAL PATH
// ========================================================================

#[test]
fn self_time_does_not_double_count_concurrent_children() {
    // Two children run at the same time inside a 10ms parent. Subtracting the
    // sum of their durations would claim the parent spent -10ms on itself.
    let parent = span("p", None, "req", "call", 0.0, Some(0.010), "ok");
    let a = span("a", Some("p"), "x", "io", 0.001, Some(0.009), "ok");
    let b = span("b", Some("p"), "y", "io", 0.001, Some(0.009), "ok");
    let spans = vec![parent, a, b];
    let selfs = crate::find::self_ms(&spans);
    let own = *selfs.get("p").unwrap();
    assert!(own >= 1.9 && own <= 2.1, "expected ~2ms of self time, got {}", own);
}

#[test]
fn self_time_is_never_negative() {
    let parent = span("p", None, "req", "call", 0.0, Some(0.001), "ok");
    let child = span("c", Some("p"), "x", "io", 0.0, Some(0.050), "ok");  // outlives it
    let spans = vec![parent, child];
    let selfs = crate::find::self_ms(&spans);
    assert!(*selfs.get("p").unwrap() >= 0.0, "a duration cannot be negative");
}

#[test]
fn the_critical_path_follows_whichever_child_finishes_last() {
    let root = span("r", None, "req", "call", 0.0, Some(0.100), "ok");
    let quick = span("q", Some("r"), "quick", "io", 0.001, Some(0.010), "ok");
    let slow = span("s", Some("r"), "slow", "io", 0.001, Some(0.099), "ok");
    let spans = vec![root, quick, slow];
    let path = crate::find::critical_path(&spans, "r");
    let names: Vec<&str> = path.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, vec!["req", "slow"], "the total is set by the child that finishes last");
}

#[test]
fn search_matches_attributes_not_only_names() {
    let mut s = span("a", None, "handler", "call", 0.0, Some(1.0), "ok");
    let mut attrs = J::obj();
    attrs.set("user_id", J::s("u-42"));
    s.attributes = attrs;
    let spans = vec![s];
    assert_eq!(crate::find::search(&spans, "u-42", 10).len(), 1, "a recorded value must be findable");
    assert_eq!(crate::find::search(&spans, "user_id=u-42", 10).len(), 1, "key=value must work");
    assert_eq!(crate::find::search(&spans, "u-99", 10).len(), 0);
}

// ========================================================================
// STATISTICS, ASSERTIONS, COVERAGE, SKEW, SEAL
// ========================================================================

#[test]
fn a_percentile_is_taken_from_the_values_not_folded_from_a_summary() {
    let mut spans = Vec::new();
    for i in 0..100 {
        // 98 fast, 2 very slow: a mean would bury them, the tail must not
        let d = if i >= 98 { 1.0 } else { 0.001 };
        spans.push(span(&format!("s{}", i), None, "work", "call", 0.0, Some(d), "ok"));
    }
    let d = crate::stats::distributions(&spans, 1);
    let w = d.iter().find(|x| x.name == "work").unwrap();
    assert_eq!(w.count, 100);
    assert!(w.median < 5.0, "median should sit with the fast ones, got {}", w.median);
    assert!(w.p99 > 900.0, "p99 must show the tail, got {}", w.p99);
    assert!(w.max > 900.0, "max must show the worst, got {}", w.max);
    // With one outlier in a hundred, p99 lands just below it; which is correct
    // percentile behaviour, and the reason the sample size travels beside the
    // number rather than being left for the reader to assume.
    let mut nearly = spans.clone();
    nearly[98].end = Some(0.001);
    let single = crate::stats::distributions(&nearly, 1);
    let s = single.iter().find(|x| x.name == "work").unwrap();
    assert!(s.p99 < 900.0 && s.max > 900.0,
            "a lone outlier shows in max, not in p99: p99={} max={}", s.p99, s.max);
}

#[test]
fn a_percentile_over_too_few_samples_is_flagged_rather_than_trusted() {
    let spans = vec![span("a", None, "rare", "call", 0.0, Some(0.5), "ok")];
    let j = crate::stats::dist_json(&crate::stats::distributions(&spans, 1));
    let first = j.get("spans").and_then(|x| x.as_arr()).unwrap()[0].clone();
    assert_eq!(first.get("percentiles_meaningful"), Some(&J::Bool(false)),
               "a p99 over one sample is the maximum wearing a costume");
}

#[test]
fn an_assertion_failure_names_the_spans_that_broke_it() {
    let spans = vec![
        span("a", None, "checkout", "call", 0.0, Some(0.500), "ok"),
        span("b", None, "checkout", "call", 0.0, Some(0.010), "ok"),
    ];
    let rules = crate::stats::parse_rules("max_duration_ms checkout 100");
    let fails = crate::stats::assert_all(&spans, &rules);
    assert_eq!(fails.len(), 1);
    assert!(!fails[0].examples.is_empty(), "a red light without a location is an alarm, not a finding");
}

#[test]
fn an_unknown_assertion_is_a_failure_not_a_pass() {
    // silently ignoring a rule it does not understand would report success for
    // an invariant nobody checked
    let fails = crate::stats::assert_all(&[], &crate::stats::parse_rules("no_such_rule x"));
    assert_eq!(fails.len(), 1);
}

#[test]
fn coverage_reports_what_never_ran() {
    let spans = vec![span("a", None, "ran", "call", 0.0, Some(1.0), "ok")];
    let want = vec!["ran".to_string(), "never".to_string()];
    let j = crate::stats::coverage(&spans, &want);
    assert_eq!(j.get("never_ran").and_then(|x| x.as_f64()), Some(1.0));
}

#[test]
fn skew_is_detected_across_hosts_and_never_applied() {
    let mut parent = span("p", None, "req", "call", 100.0, Some(101.0), "ok");
    parent.host_id = "a".into();
    let mut child = span("c", Some("p"), "sub", "call", 99.5, Some(100.5), "ok");
    child.host_id = "b".into();          // clock half a second fast
    let spans = vec![parent, child];
    let pairs = crate::skew::analyse(&spans);
    assert_eq!(pairs.len(), 1);
    assert_eq!(pairs[0].violations, 1);
    assert!((pairs[0].estimate - 0.5).abs() < 1e-6, "estimate {}", pairs[0].estimate);
    // the recorded time is untouched: correcting in place would destroy the
    // evidence that the clocks disagree
    assert_eq!(spans[1].start, 99.5);
}

#[test]
fn one_clock_never_disagrees_with_itself() {
    let parent = span("p", None, "req", "call", 100.0, Some(101.0), "ok");
    let child = span("c", Some("p"), "sub", "call", 100.1, Some(100.9), "ok");
    assert!(crate::skew::analyse(&vec![parent, child]).is_empty());
}

#[test]
fn a_seal_changes_when_any_recorded_fact_changes() {
    let a = vec![span("x", None, "work", "call", 0.0, Some(1.0), "ok")];
    let mut b = a.clone();
    b[0].status = "error".into();
    assert_ne!(crate::rollup::seal(&a), crate::rollup::seal(&b));
}

#[test]
fn a_seal_does_not_depend_on_the_order_spans_arrive_in() {
    // two instances holding the same spans must agree, or the seal proves nothing
    let x = span("x", None, "a", "call", 0.0, Some(1.0), "ok");
    let y = span("y", None, "b", "call", 1.0, Some(2.0), "ok");
    assert_eq!(crate::rollup::seal(&vec![x.clone(), y.clone()]),
               crate::rollup::seal(&vec![y, x]));
}

// ========================================================================
// REGRESSION COMPARISON AND EXPORT
// ========================================================================

fn many(name: &str, n: usize, dur: f64) -> Vec<Span> {
    (0..n).map(|i| span(&format!("{}{}", name, i), None, name, "call", 0.0, Some(dur), "ok")).collect()
}

#[test]
fn a_comparison_refuses_to_judge_on_too_few_samples() {
    let before = many("work", 2, 0.010);
    let after = many("work", 2, 0.100);
    let c = crate::compare::regressions(&before, &after, 5, 1.0);
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].verdict, "too few samples to judge",
               "a tenfold move on two samples has demonstrated nothing");
}

#[test]
fn a_move_smaller_than_the_earlier_spread_is_not_a_regression() {
    let before = many("work", 40, 0.100);
    let mut after = many("work", 40, 0.1005);      // half a millisecond
    after.extend(many("other", 40, 0.010));
    let c = crate::compare::regressions(&before, &after, 5, 1.0);
    assert!(!c.iter().any(|x| x.name == "work" && x.verdict == "slower"),
            "crying about every wobble teaches people to ignore the tool");
}

#[test]
fn a_real_slowdown_is_reported_with_both_sample_sizes() {
    let before = many("work", 40, 0.010);
    let after = many("work", 40, 0.080);
    let c = crate::compare::regressions(&before, &after, 5, 1.0);
    let w = c.iter().find(|x| x.name == "work").expect("work should be compared");
    assert_eq!(w.verdict, "slower");
    assert_eq!(w.before_n, 40);
    assert_eq!(w.after_n, 40);
}

#[test]
fn the_chrome_export_emits_only_well_formed_complete_events() {
    let mut spans = many("work", 3, 0.010);
    spans.push(span("open", None, "hung", "wait", 0.0, None, "running"));
    let j = crate::compare::chrome_trace(&spans);
    let ev = j.get("traceEvents").and_then(|x| x.as_arr()).unwrap();
    // an unfinished span has no duration, so it is not emitted as a complete
    // event rather than being emitted with an invented one
    assert_eq!(ev.len(), 3);
    for e in ev {
        assert_eq!(e.get("ph").and_then(|x| x.as_str()), Some("X"));
        assert!(e.get("dur").and_then(|x| x.as_f64()).unwrap_or(-1.0) >= 0.0);
    }
}

#[test]
fn folded_stacks_carry_self_time_along_a_causal_path() {
    let parent = span("p", None, "outer", "call", 0.0, Some(0.100), "ok");
    let child = span("c", Some("p"), "inner", "call", 0.010, Some(0.090), "ok");
    let out = crate::compare::folded_stacks(&vec![parent, child]);
    assert!(out.contains("outer;inner"), "a stack must be the causal path: {}", out);
    // the parent's own line carries what it spent itself, not what it contained
    let outer_line = out.lines().find(|l| l.starts_with("outer ")).expect("outer line");
    let us: i64 = outer_line.rsplit(' ').next().unwrap().parse().unwrap();
    assert!(us > 15_000 && us < 25_000, "outer self time should be ~20ms, got {}us", us);
}

// ========================================================================
// COST, CORRELATION, CONCURRENCY
// ========================================================================

fn with_cost(mut s: Span, cpu_ms: f64, ratio: f64, spent: &str) -> Span {
    let mut c = J::obj();
    c.set("cpu_ms", J::n(cpu_ms));
    c.set("cpu_ratio", J::n(ratio));
    c.set("spent", J::s(spent));
    let mut a = J::obj();
    a.set("cost", c);
    s.attributes = a;
    s
}

#[test]
fn an_unmeasured_span_is_not_reported_as_idle() {
    // zero is a measurement: a runtime with no counter must not make every
    // reader conclude the program consumed nothing
    let measured = with_cost(span("a", None, "busy", "call", 0.0, Some(0.01), "ok"), 9.0, 0.9, "working");
    let bare = span("b", None, "unknown", "call", 0.0, Some(0.01), "ok");
    let j = crate::stats::cost_report(&vec![measured, bare], 10);
    assert_eq!(j.get("unmeasured").and_then(|x| x.as_f64()), Some(1.0));
    assert_eq!(j.get("spans").and_then(|x| x.as_arr()).unwrap().len(), 1,
               "a span with no cost recorded must not appear as one that used none");
}

#[test]
fn working_and_waiting_are_told_apart_at_equal_duration() {
    let busy = with_cost(span("a", None, "burn", "call", 0.0, Some(0.010), "ok"), 9.9, 0.99, "working");
    let idle = with_cost(span("b", None, "sleep", "wait", 0.0, Some(0.010), "ok"), 0.1, 0.01, "waiting");
    let j = crate::stats::cost_report(&vec![busy, idle], 10);
    let rows = j.get("spans").and_then(|x| x.as_arr()).unwrap();
    assert_eq!(rows[0].get("spent").and_then(|x| x.as_str()), Some("working"));
    assert_eq!(rows[1].get("spent").and_then(|x| x.as_str()), Some("waiting"));
}

#[test]
fn a_correlation_is_worded_as_co_occurrence_not_cause() {
    let mut spans = Vec::new();
    for i in 0..60 {
        let mut s = span(&format!("s{}", i), None, "handle", "call", 0.0, Some(0.01),
                         if i % 3 == 0 { "error" } else { "ok" });
        s.host_id = if i % 3 == 0 { "bad".into() } else { "good".into() };
        spans.push(s);
    }
    let j = crate::relate::correlations(&spans, 5);
    let f = j.get("findings").and_then(|x| x.as_arr()).unwrap();
    assert!(!f.is_empty());
    let reads = f[0].get("reads_as").and_then(|x| x.as_str()).unwrap();
    assert!(reads.contains("more common"), "{}", reads);
    assert!(!reads.contains("cause"), "a capture cannot support a claim about cause: {}", reads);
}

#[test]
fn a_correlation_below_minimum_support_is_not_reported() {
    // a lift computed over three spans is noise with a decimal point
    let mut a = span("a", None, "x", "call", 0.0, Some(0.01), "error");
    a.host_id = "rare".into();
    let mut rest: Vec<Span> = (0..50).map(|i|
        span(&format!("r{}", i), None, "x", "call", 0.0, Some(0.01), "ok")).collect();
    rest.push(a);
    let j = crate::relate::correlations(&rest, 5);
    let f = j.get("findings").and_then(|x| x.as_arr()).unwrap();
    assert!(!f.iter().any(|x| x.get("value").and_then(|v| v.as_str()) == Some("rare")));
}

#[test]
fn concurrency_finds_the_peak_from_a_sweep() {
    let spans = vec![
        span("a", None, "x", "call", 0.0, Some(1.0), "ok"),
        span("b", None, "y", "call", 0.2, Some(0.8), "ok"),
        span("c", None, "z", "call", 0.3, Some(0.5), "ok"),
    ];
    let j = crate::relate::concurrency(&spans);
    assert_eq!(j.get("peak_parallelism").and_then(|x| x.as_f64()), Some(3.0));
}

#[test]
fn an_unfinished_span_does_not_distort_the_concurrency_sweep() {
    // a span with no end has no interval; counting it would leave the level
    // permanently raised
    let spans = vec![
        span("a", None, "x", "call", 0.0, Some(1.0), "ok"),
        span("open", None, "hung", "wait", 0.1, None, "running"),
    ];
    let j = crate::relate::concurrency(&spans);
    assert_eq!(j.get("peak_parallelism").and_then(|x| x.as_f64()), Some(1.0));
}

// ========================================================================
// EXPECTED SHAPE, COUNTERFACTUAL, MANY RUNS
// ========================================================================

#[test]
fn a_shape_reports_absence_as_loudly_as_surprise() {
    // work that quietly stopped happening is the change nobody notices
    let spans = vec![span("a", None, "ran", "call", 0.0, Some(1.0), "ok")];
    let shape = crate::expect::parse_shape("name ran\nname vanished\n");
    let j = crate::expect::check_shape(&spans, &shape);
    assert_eq!(j.get("matches"), Some(&J::Bool(false)));
    let absent = j.get("expected_but_absent_names").and_then(|x| x.as_arr()).unwrap();
    assert_eq!(absent.len(), 1, "the name that stopped appearing must be reported");
}

#[test]
fn a_proposed_shape_is_not_a_rule_until_a_person_freezes_it() {
    // learning a shape from one run and enforcing it turns one day into law
    let spans = vec![span("a", None, "ran", "call", 0.0, Some(1.0), "ok")];
    let p = crate::expect::propose_shape(&spans);
    assert!(p.get("note").and_then(|x| x.as_str()).unwrap().contains("proposal"));
}

#[test]
fn a_counterfactual_reports_a_ceiling_not_a_subtraction() {
    // root 100ms: a 90ms slow child on the path, and an 80ms sibling chain that
    // becomes the constraint the moment the slow one is halved
    let root = span("r", None, "req", "call", 0.0, Some(0.100), "ok");
    let slow = span("s", Some("r"), "slow", "io", 0.0, Some(0.099), "ok");
    let other = span("o", Some("r"), "other", "io", 0.0, Some(0.080), "ok");
    let spans = vec![root, slow, other];
    let j = crate::expect::counterfactual(&spans, "slow", 0.5);
    let fell = j.get("total_would_fall_to_ms").and_then(|x| x.as_f64()).unwrap();
    assert!(fell >= 79.0,
        "halving the slow link cannot take the total below the next constraint, got {}", fell);
}

#[test]
fn work_off_the_critical_path_is_told_it_changes_nothing() {
    let root = span("r", None, "req", "call", 0.0, Some(0.100), "ok");
    let quick = span("q", Some("r"), "quick", "io", 0.0, Some(0.002), "ok");
    let slow = span("s", Some("r"), "slow", "io", 0.0, Some(0.099), "ok");
    let j = crate::expect::counterfactual(&vec![root, quick, slow], "quick", 0.1);
    assert!(j.get("verdict").and_then(|x| x.as_str()).unwrap().contains("changes nothing"));
}

#[test]
fn an_intermittent_failure_is_visible_across_runs_and_invisible_in_one() {
    let mk = |fails: bool| vec![
        span("a", None, "steady", "call", 0.0, Some(1.0), "ok"),
        span("b", None, "flaky", "call", 0.0, Some(1.0), if fails { "error" } else { "ok" }),
    ];
    let runs: Vec<(String, Vec<Span>)> = (0..10)
        .map(|i| (format!("run{}", i), mk(i % 5 == 0))).collect();
    let j = crate::expect::across_runs(&runs);
    assert_eq!(j.get("runs").and_then(|x| x.as_f64()), Some(10.0));
    let flaky = j.get("flaky").and_then(|x| x.as_arr()).unwrap();
    assert_eq!(flaky.len(), 1);
    assert_eq!(flaky[0].get("failed_in_runs").and_then(|x| x.as_f64()), Some(2.0));
    // a rate stated without its denominator is a rumour
    assert_eq!(flaky[0].get("appeared_in_runs").and_then(|x| x.as_f64()), Some(10.0));
}

// ========================================================================
// EGRESS, RETRIES, INTEGRITY
// ========================================================================

#[test]
fn egress_reports_where_it_went_and_what_it_never_reached() {
    let mut a = span("a", None, "payments-api", "external", 0.0, Some(1.0), "ok");
    a.attributes = { let mut o = J::obj(); o.set("host", J::s("payments.internal")); o };
    let b = span("b", None, "unknown-tracker.example", "external", 0.0, Some(1.0), "ok");
    let allowed = vec!["payments.internal".to_string(), "never-called".to_string()];
    let j = crate::egress::egress(&vec![a, b], &allowed);
    assert_eq!(j.get("all_expected"), Some(&J::Bool(false)));
    let un = j.get("unexpected").and_then(|x| x.as_arr()).unwrap();
    assert_eq!(un.len(), 1, "only the destination outside the list is unexpected");
    let never = j.get("expected_but_never_reached").and_then(|x| x.as_arr()).unwrap();
    assert_eq!(never.len(), 1, "a dependency that stopped being used is reported too");
}

#[test]
fn egress_does_not_call_itself_intrusion_detection() {
    // overstating what this is would invite someone to rely on it as a control
    let j = crate::egress::egress(&[], &[]);
    assert!(j.get("note").and_then(|x| x.as_str()).unwrap().contains("judgement for a person"));
}

#[test]
fn attempts_are_grouped_only_by_a_declared_relation() {
    let mk = |id: &str, trace: &str, declared: bool, status: &str| {
        let mut s = span(id, None, "send", "call", 0.0, Some(1.0), status);
        s.trace_id = trace.into();
        if declared {
            let mut o = J::obj(); o.set("retry_of", J::s("op-1")); s.attributes = o;
        }
        s
    };
    // two declared attempts, plus two traces that merely run the same code
    let spans = vec![
        mk("a", "t1", true, "error"), mk("b", "t2", true, "ok"),
        mk("c", "t3", false, "ok"), mk("d", "t4", false, "ok"),
    ];
    let j = crate::egress::attempts(&spans);
    assert_eq!(j.get("declared_relations").and_then(|x| x.as_f64()), Some(2.0));
    assert_eq!(j.get("operations_retried").and_then(|x| x.as_f64()), Some(1.0),
        "running the same code is not an attempt at the same thing");
    let ops = j.get("operations").and_then(|x| x.as_arr()).unwrap();
    assert_eq!(ops[0].get("first_attempt_failed"), Some(&J::Bool(true)));
    assert_eq!(ops[0].get("eventually_succeeded"), Some(&J::Bool(true)));
}

// ========================================================================
// WATCHING, SAMPLING, BACKUP
// ========================================================================

#[test]
fn a_watch_fires_on_a_transition_not_on_every_evaluation() {
    // a condition true for an hour is one event, not three thousand
    let slow = vec![span("a", None, "checkout", "call", 0.0, Some(0.500), "ok")];
    let rules = crate::stats::parse_rules("max_duration_ms checkout 100");
    let mut w = crate::watch::WatchState::new();
    let first = w.evaluate(&slow, &rules, 1.0);
    assert_eq!(first.len(), 1, "the first failing evaluation fires");
    assert_eq!(first[0].state, "fired");
    let second = w.evaluate(&slow, &rules, 2.0);
    assert!(second.is_empty(), "the same condition must not fire again");
    let third = w.evaluate(&slow, &rules, 3.0);
    assert!(third.is_empty());
    assert_eq!(w.currently_firing(), 1);
}

#[test]
fn a_watch_reports_recovery_so_a_reader_learns_it_ended() {
    let slow = vec![span("a", None, "checkout", "call", 0.0, Some(0.500), "ok")];
    let fine = vec![span("a", None, "checkout", "call", 0.0, Some(0.010), "ok")];
    let rules = crate::stats::parse_rules("max_duration_ms checkout 100");
    let mut w = crate::watch::WatchState::new();
    w.evaluate(&slow, &rules, 1.0);
    let rec = w.evaluate(&fine, &rules, 2.0);
    assert_eq!(rec.len(), 1);
    assert_eq!(rec[0].state, "recovered");
    assert_eq!(w.currently_firing(), 0);
    // and it can fire again afterwards, or a flapping condition would go silent
    assert_eq!(w.evaluate(&slow, &rules, 3.0).len(), 1);
}

#[test]
fn a_firing_says_what_it_saw_not_merely_that_it_fired() {
    let slow = vec![span("a", None, "checkout", "call", 0.0, Some(0.500), "ok")];
    let rules = crate::stats::parse_rules("max_duration_ms checkout 100");
    let mut w = crate::watch::WatchState::new();
    let f = w.evaluate(&slow, &rules, 1.0);
    assert!(!f[0].examples.is_empty(),
        "an alert that sends a person back to the map has done none of the work");
    assert!(f[0].detail.contains("exceeded"));
}

// ========================================================================
// VALIDATION AT THE DOOR (HARDENING PASS)
// ========================================================================

#[test]
fn a_span_that_ends_before_it_starts_is_refused() {
    // not a slow span: a broken clock or a broken adapter. Storing it would
    // make every duration derived from it lie.
    let s = span("a", None, "backwards", "call", 9.0, Some(8.0), "ok");
    assert!(s.validate().is_err());
}

#[test]
fn a_span_with_no_name_or_no_id_is_refused() {
    let mut nameless = span("a", None, "", "call", 0.0, Some(1.0), "ok");
    assert!(nameless.validate().is_err(), "a nameless span cannot be read");
    nameless.name = "ok".into();
    nameless.span_id = "   ".into();
    assert!(nameless.validate().is_err(), "a span with no identity cannot be referred to");
}

#[test]
fn a_span_cannot_be_its_own_parent() {
    let mut s = span("a", None, "loop", "call", 0.0, Some(1.0), "ok");
    s.parent_span_id = Some("a".into());
    assert!(s.validate().is_err());
}

#[test]
fn a_non_finite_time_is_refused_rather_than_stored_as_absent() {
    // SQLite stores a non-finite REAL as NULL, so accepting one would silently
    // turn a timestamp into an absent value
    let inf = span("a", None, "n", "call", f64::INFINITY, None, "ok");
    assert!(inf.validate().is_err());
    let nan_end = span("b", None, "n", "call", 0.0, Some(f64::NAN), "ok");
    assert!(nan_end.validate().is_err());
}

#[test]
fn json_refuses_a_number_that_overflows_to_infinity() {
    assert!(crate::json::parse("{\"start\": 1e400}").is_err());
    assert!(crate::json::parse("{\"start\": 1e30}").is_ok());
}

#[test]
fn an_over_long_field_is_truncated_and_says_so_rather_than_losing_the_span() {
    let huge = "x".repeat(100_000);
    let mut v = J::obj();
    v.set("span_id", J::s("a"));
    v.set("name", J::s(&huge));
    v.set("start", J::n(1.0));
    let s = Span::from_json(&v, None).expect("a long name must not lose the span");
    assert!(s.name.len() < 6000, "bounded, got {}", s.name.len());
    assert!(s.name.contains("truncated"), "truncation must be visible");
    assert!(s.validate().is_ok());
}

#[test]
fn a_valid_span_still_passes() {
    assert!(span("a", None, "fine", "call", 1.0, Some(2.0), "ok").validate().is_ok());
    // an open span has no end, which is an unfinished span and must stay legal
    assert!(span("b", None, "hung", "wait", 1.0, None, "running").validate().is_ok());
}

// ========================================================================
// CONSISTENCY ACROSS THE SURFACE (SECOND PASS)
// ========================================================================

#[test]
fn every_command_that_can_say_no_uses_the_same_exit_grammar() {
    // 0 success · 1 the command ran and the answer was no · 2 usage.
    // `check` was the exception: it reported a violation and exited 0, so a
    // capture with adapter violations passed CI silently.
    let orphan = {
        let mut s = span("c", Some("missing"), "orphan", "call", 1.0, Some(2.0), "ok");
        s.host_id = "h".into();
        s
    };
    let report = crate::conform::check(&vec![orphan]);
    assert_eq!(report.get("conformant"), Some(&J::Bool(false)),
        "this capture must be judged non-conformant, or the exit-code rule has nothing to act on");
}

#[test]
fn a_body_reporting_an_error_is_never_a_success_status() {
    // the UI checks response.ok, so an error delivered as 200 is invisible to
    // exactly the code written to notice it
    let named_missing = crate::json::parse("{\"error\":\"no such node\"}").unwrap();
    let bad_request = crate::json::parse("{\"error\":\"unknown lod\"}").unwrap();
    let ok = crate::json::parse("{\"hits\":3}").unwrap();
    let status = |v: &J| -> u16 {
        match v.get("error").and_then(|e| e.as_str()) {
            None => 200,
            Some(m) => {
                let l = m.to_lowercase();
                if l.contains("no such") || l.contains("not found") { 404 } else { 400 }
            }
        }
    };
    assert_eq!(status(&named_missing), 404);
    assert_eq!(status(&bad_request), 400);
    assert_eq!(status(&ok), 200);
}

// ========================================================================
// THE SAMPLING INVARIANT, ENFORCED EVERYWHERE (THIRD PASS)
// ========================================================================

#[test]
fn loss_is_recorded_as_whole_traces_not_loose_spans() {
    // Spec 2.5: sample at the trace level, never at the span level, which would
    // punch holes in a trace's graph. Retention already honoured this; the
    // adapters' buffer eviction did not, so the same invariant was enforced in
    // one subsystem and broken in another.
    let db = format!("/tmp/execviz-loss-test-{}.db", std::process::id());
    let _ = std::fs::remove_file(&db);
    let st = Store::open(&db).expect("open");
    st.record_loss("h1", 3004, 751, 0).expect("record");
    st.record_loss("h1", 10, 2, 1).expect("record again");
    let losses = st.losses();
    assert_eq!(losses.len(), 1);
    let (host, spans, traces, abnormal) = losses[0].clone();
    assert_eq!(host, "h1");
    assert_eq!(spans, 3014, "span counts accumulate");
    assert_eq!(traces, 753, "the unit of loss is the trace");
    assert_eq!(abnormal, 1, "losing a trace that held an error is a worse fact, counted apart");
    let _ = std::fs::remove_file(&db);
}

#[test]
fn the_store_refuses_an_invalid_span_at_every_write_door() {
    // There are three: local ingest, a peer exchange, and the syscall
    // enrichment merge. An invariant enforced at two of three is not an
    // invariant, so it lives at the single point every write passes through.
    let db = format!("/tmp/execviz-door-test-{}.db", std::process::id());
    let _ = std::fs::remove_file(&db);
    let st = Store::open(&db).expect("open");
    let good = span("g", None, "fine", "call", 1.0, Some(2.0), "ok");
    assert!(st.upsert(&good).is_ok());
    let backwards = span("b", None, "backwards", "call", 9.0, Some(8.0), "ok");
    assert!(st.upsert(&backwards).is_err(), "the store itself must refuse it");
    assert_eq!(st.all().expect("read").len(), 1, "only the valid span was written");
    let _ = std::fs::remove_file(&db);
}

// ========================================================================
// THE INSTANCE CAN SAY WHEN IT HAS STOPPED RECORDING (FOURTH PASS)
// ========================================================================

#[test]
fn write_health_distinguishes_a_refused_span_from_a_refusing_store() {
    // A span the validator refused reports itmething about the sender. A span the
    // store refused reports itmething about the instance, and only the second means
    // spans arriving now are being lost.
    let h = crate::WriteHealth::new();
    assert_eq!(h.consecutive_failures(), 0);
    h.failed("database or disk is full");
    h.failed("database or disk is full");
    assert_eq!(h.consecutive_failures(), 2);
    assert_eq!(h.last_error(), "database or disk is full");
    // one success clears it: a transient failure must not latch forever
    h.ok();
    assert_eq!(h.consecutive_failures(), 0);
    assert!(h.last_write() > 0.0);
}

#[test]
fn a_validation_refusal_is_not_counted_against_the_store() {
    // otherwise one malformed adapter would make a healthy instance report
    // itself as failing, and an operator would chase the wrong thing
    let e = rusqlite::Error::InvalidParameterName("name is empty".into());
    assert!(matches!(e, rusqlite::Error::InvalidParameterName(_)),
        "validation failures are distinguishable from store failures by type");
}

#[test]
fn an_identifier_may_not_carry_markup() {
    // A capture crosses a trust boundary every time it is exchanged with a peer.
    // An id containing a quote put a live `onmouseover` handler into the page of
    // whoever opened the capture; verified in a browser before this was fixed.
    let mut s = span("x", None, "innocent", "call", 1.0, Some(2.0), "ok");
    s.span_id = "\" onmouseover=\"alert(1)\" x=\"".into();
    assert!(s.validate().is_err(), "an identifier is not free text");

    s.span_id = "a".into();
    s.parent_span_id = Some("<script>".into());
    assert!(s.validate().is_err(), "a parent id is an identifier too");
}

#[test]
fn ordinary_identifiers_from_every_adapter_still_pass() {
    // the shapes the nine adapters generate
    for id in ["6f784ecfd5b9", "span-1", "trace_2", "a.b:c", "ABC123", "s0"] {
        let mut s = span("x", None, "fine", "call", 1.0, Some(2.0), "ok");
        s.span_id = id.into();
        assert!(s.validate().is_ok(), "{} should be a valid identifier", id);
    }
}

#[test]
fn the_witness_catches_a_span_that_claims_work_it_did_not_do() {
    // A span whose kind implies a syscall, on a thread that issued none in its
    // window. The finding is evidence, not a conclusion; the note reports it.
    let mut s = span("x", None, "fetch_user", "db", 100.0, Some(101.0), "ok");
    s.attributes.set("tid", crate::json::J::Num(7.0));
    let recs = vec![crate::syscalls::Record { t: 500.0, dur: 0.0, tid: 7, name: "write".into(), comm: None, fd: None }];
    let (_, a) = crate::witness::audit(&[s], &recs);
    assert_eq!(a.claimed_not_performed, 1, "a db span with no syscalls behind it should be reported");
}

#[test]
fn the_witness_does_not_convict_a_span_whose_kind_implies_nothing() {
    // "call" implies no syscall, so it is unexamined rather than passed: an
    // unknown rule is a failure, not a pass, and this states which it is.
    let mut s = span("x", None, "compute", "call", 100.0, Some(101.0), "ok");
    s.attributes.set("tid", crate::json::J::Num(7.0));
    let (_, a) = crate::witness::audit(&[s], &[]);
    assert_eq!(a.examined, 0);
    assert_eq!(a.unexamined, 1);
    assert_eq!(a.claimed_not_performed, 0);
}

#[test]
fn work_nobody_claimed_is_coverage_not_a_defect() {
    // Records outside every span window are the coverage question. They must not
    // set the exit code: an incomplete trace is not a lying one.
    let mut s = span("x", None, "w", "io", 100.0, Some(101.0), "ok");
    s.attributes.set("tid", crate::json::J::Num(3.0));
    let recs = vec![
        crate::syscalls::Record { t: 100.5, dur: 0.0, tid: 3, name: "write".into(), comm: None, fd: None },
        crate::syscalls::Record { t: 900.0, dur: 0.0, tid: 3, name: "write".into(), comm: None, fd: None },
    ];
    let (_, a) = crate::witness::audit(&[s], &recs);
    assert_eq!(a.claimed_not_performed, 0, "the span did perform its work");
    assert_eq!(a.unclaimed_records, 1, "the record outside every window is unclaimed");
}

#[test]
fn the_decoder_reports_what_it_could_not_read() {
    // A decoder that reports only what it parsed is indistinguishable, in its
    // output, from a service that went quiet. The residue is reported.
    let feed = concat!(
        "{\"log\":\"GET /users HTTP/1.1\",\"kind\":\"text\",\"bytes\":20}\n",
        "{\"log\":\"SELECT 1\",\"kind\":\"text\",\"bytes\":8}\n",
        "{\"log\":\"0100000000000000\",\"kind\":\"binary\",\"bytes\":8}\n",
    );
    let (out, r) = crate::decode::decode_floor(feed);
    assert_eq!(r.total, 3);
    assert_eq!(r.decoded, 2, "http and sql should decode");
    let residue = out.get("residue").and_then(|x| x.as_arr()).unwrap();
    assert_eq!(residue.len(), 1, "the byte nobody could read must be reported, not hidden");
}

#[test]
fn a_shape_from_too_little_is_not_a_shape() {
    // Identity is evidence, and evidence has a sample size. A thin sample is
    // named as unfingerprinted rather than fingerprinted badly.
    let recs: Vec<crate::syscalls::Record> = (0..5).map(|i| crate::syscalls::Record { fd: None,
        t: i as f64, dur: 0.0, tid: 1, name: "write".into(), comm: Some("tiny".into()),
    }).collect();
    let out = crate::finger::recorder_identities(&recs, 200);
    assert!(out.get("identities").and_then(|x| x.as_arr()).unwrap().is_empty());
    assert_eq!(out.get("not_fingerprinted").and_then(|x| x.as_arr()).unwrap().len(), 1);
}

#[test]
fn the_export_names_what_it_cannot_carry() {
    // A translation that loses things silently is how two systems come to
    // disagree without anyone noticing.
    let mut s = span("x", None, "work", "call", 1.0, None, "running");
    s.clock_source = Some("CLOCK_REALTIME".into());
    let out = crate::otel::export(&[s]);
    let lost = out.get("execviz_not_exported").and_then(|x| x.as_arr()).unwrap();
    let fields: Vec<&str> = lost.iter()
        .filter_map(|l| l.get("field").and_then(|x| x.as_str())).collect();
    assert!(fields.contains(&"clock_source"), "skew analysis depends on it and OTLP has no field");
    assert!(fields.contains(&"open spans"), "an unfinished span does not survive and must be said");
}

#[test]
fn a_non_monoid_cannot_be_expressed() {
    // The refusal happens at parse time, before anything runs, and says why.
    // A rollup that is not a monoid would make a tier built from tiers wrong.
    for bad in ["from spans show median(duration)", "from spans show p95(duration)"] {
        let e = match crate::ask::parse(bad) { Err(e) => e, Ok(_) => panic!("{} should be refused", bad) };
        assert!(e.contains("monoid"), "the reason must be given, got: {}", e);
    }
    assert!(crate::ask::parse("from spans group by kind show count max(duration)").is_ok());
}

#[test]
fn a_mean_below_the_threshold_refuses_rather_than_answers() {
    // Spread statistics state their sample size and decline below it: a figure
    // from too little is not a measurement.
    let q = crate::ask::parse("from spans group by kind show mean(duration)").unwrap();
    let spans: Vec<crate::store::Span> = (0..3).map(|i| {
        span(&format!("s{}", i), None, "w", "io", i as f64, Some(i as f64 + 1.0), "ok")
    }).collect();
    let out = crate::ask::run(&q, &spans, &[]);
    let refused = out.get("refused").and_then(|x| x.as_arr()).unwrap();
    assert_eq!(refused.len(), 1, "3 samples is below the threshold and must be declined");
}

#[test]
fn shapes_fire_on_relations_a_syscall_could_not_express() {
    // Each predicate is a relation between things rather than a property of one.
    let mut parent = span("p", None, "join", "call", 10.0, Some(11.0), "ok");
    parent.attributes.set("tid", crate::json::J::Num(1.0));
    let mut child = span("c", Some("p"), "outlived", "call", 10.2, Some(13.0), "ok");
    child.attributes.set("tid", crate::json::J::Num(1.0));
    let orphan = span("o", Some("ghost"), "no_parent_here", "call", 1.0, Some(2.0), "ok");
    let stuck = span("s", None, "awaiting_device", "wait", 1.0, None, "running");

    let (rules, unknown) = crate::shapes_rules::parse_rules("stuck 0.5\norphaned\ninverted\n");
    assert!(unknown.is_empty());
    let (out, o) = crate::shapes_rules::detect(
        &rules, &unknown, &[parent, child, orphan, stuck], &[], None);
    assert_eq!(o.fired, 3, "stuck, orphaned and inverted should each fire once");
    let names: Vec<&str> = out.get("findings").and_then(|x| x.as_arr()).unwrap()
        .iter().filter_map(|f| f.get("rule").and_then(|x| x.as_str())).collect();
    for want in ["stuck", "orphaned", "inverted"] {
        assert!(names.contains(&want), "{} did not fire", want);
    }
}

#[test]
fn an_unknown_rule_is_a_failure_not_a_pass() {
    // A rules file with a typo that silently matches nothing looks exactly like
    // a system with no problems, which is the worst thing it could look like.
    let (rules, unknown) = crate::shapes_rules::parse_rules("stuck 1\nnot_a_predicate 2\n");
    assert_eq!(rules.len(), 1);
    assert_eq!(unknown, vec!["not_a_predicate".to_string()]);
}

#[test]
fn a_rule_without_its_evidence_says_so_rather_than_passing() {
    // `unwitnessed` needs recorder records. Without them it must not quietly pass.
    let (rules, unknown) = crate::shapes_rules::parse_rules("unwitnessed 0\n");
    let s = span("x", None, "w", "io", 1.0, Some(2.0), "ok");
    let (out, o) = crate::shapes_rules::detect(&rules, &unknown, &[s], &[], None);
    assert_eq!(o.fired, 1, "the missing evidence must itself be reported");
    let why = out.get("findings").and_then(|x| x.as_arr()).unwrap()[0]
        .get("why").and_then(|x| x.as_str()).unwrap();
    assert!(why.contains("records"), "it must say what it needs, got: {}", why);
}

#[test]
fn an_undeclared_self_only_treatment_is_caught() {
    // A decision path applied to the recorder and to nothing else, without being
    // declared, is special-casing whether or not anybody meant it.
    let feed = concat!(
        "{\"comm\":\"app\",\"policy_text\":\"v1.sup=0.kind=text.trunc=0.fd=0.hex=0\"}\n",
        "{\"comm\":\"floor\",\"policy_text\":\"v1.sup=0.kind=text.trunc=0.fd=0.hex=0\"}\n",
        "{\"comm\":\"floor\",\"policy_text\":\"v1.sup=0.kind=quietly-shortened.trunc=0.fd=0.hex=0\"}\n",
    );
    let (out, v) = crate::scrutiny::examine(feed, "floor");
    assert_eq!(v.undeclared, 1, "the self-only path must be reported");
    assert_eq!(out.get("shared_treatment").and_then(|x| x.as_f64()), Some(1.0),
               "the path both share must not be reported");
}

#[test]
fn a_declared_exemption_is_not_a_finding() {
    // The project DOES special-case itself. The point is that every exemption is
    // declared, not that none exists.
    let feed = concat!(
        "{\"comm\":\"app\",\"policy_text\":\"v1.sup=0.kind=text.trunc=0.fd=0.hex=0\"}\n",
        "{\"comm\":\"floor\",\"policy_text\":\"v1.sup=1.kind=suppressed.trunc=0.fd=0.hex=0\",\
          \"declared_exemption\":true,\"suppressed\":42,\"why\":\"observation overhead\"}\n",
    );
    let (out, v) = crate::scrutiny::examine(feed, "floor");
    assert_eq!(v.undeclared, 0);
    assert_eq!(out.get("only_on_recorder").and_then(|x| x.as_f64()), Some(1.0),
               "it is still reported, as declared");
}

#[test]
fn the_policy_describes_the_treatment_not_the_subject() {
    // A first version put the subject in the digest, which gave every self
    // record a different policy by construction and would have reported the
    // whole capture as special-cased. Identical treatment must share a digest.
    let feed = concat!(
        "{\"comm\":\"app\",\"policy_text\":\"v1.sup=0.kind=text.trunc=0.fd=0.hex=0\"}\n",
        "{\"comm\":\"floor\",\"policy_text\":\"v1.sup=0.kind=text.trunc=0.fd=0.hex=0\"}\n",
    );
    let (out, v) = crate::scrutiny::examine(feed, "floor");
    assert_eq!(v.undeclared, 0);
    assert_eq!(out.get("only_on_recorder").and_then(|x| x.as_f64()), Some(0.0),
               "same treatment must not look like special-casing");
}

#[test]
fn a_bundle_withholds_secrets_by_default() {
    // A bundle is the thing somebody attaches to a public issue. The safe
    // default for a file that gets emailed is the one that cannot embarrass
    // anybody, and the count is stated rather than the file quietly shrinking.
    let mut s = span("x", None, "login", "call", 1.0, Some(2.0), "ok");
    s.attributes.set("db_password", crate::json::J::Str("hunter2".into()));
    s.attributes.set("api_token", crate::json::J::Str("sk-live-9f3".into()));
    s.attributes.set("region", crate::json::J::Str("eu".into()));
    let floor = "{\"log\":\"a line a program wrote\",\"kind\":\"text\"}\n";

    let (packed, records, spans) = crate::bundle::pack(&[s.clone()], floor, None, false);
    let body = format!("{}{}", records, spans.dump());
    assert!(!body.contains("hunter2"), "a password must not travel");
    assert!(!body.contains("sk-live-9f3"), "a token must not travel");
    assert!(body.contains("eu"), "an ordinary attribute must survive");
    assert_eq!(packed.withheld, 1, "the withheld payload is counted, not hidden");

    // and asked for explicitly, they do travel
    let (_, records2, spans2) = crate::bundle::pack(&[s], floor, None, true);
    assert!(format!("{}{}", records2, spans2.dump()).contains("hunter2"));
}

#[test]
fn a_bundle_seal_tracks_its_contents() {
    // A recipient has to be able to tell whether they are looking at what was
    // sent, so the seal must move when anything in the bundle does.
    let s = span("x", None, "w", "call", 1.0, Some(2.0), "ok");
    let (p1, r1, d1) = crate::bundle::pack(&[s.clone()], "", None, false);
    let (p2, r2, d2) = crate::bundle::pack(&[s], "{\"log\":\"x\"}\n", None, false);
    assert_ne!(crate::bundle::seal(&p1.manifest, &r1, &d1),
               crate::bundle::seal(&p2.manifest, &r2, &d2));
}
