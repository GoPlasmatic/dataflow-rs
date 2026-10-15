//! v4's traces hold to v3's, compared by replay (I109).
//!
//! For every run step whose recording holds a trace, v4's traced run is
//! compared with v3's: its outcome (code and `retryable`, as the plain
//! outcome is), `truncated`, `message_matches`, and the steps. v4's steps
//! are replayed from `initial` (`dataflow_corpus::replay::apply` over each
//! step's changes) and walked in order:
//!
//! - an engine step (`task_id: null`, `executed`: a sweep's counter, `as`
//!   and `scratch`) and a `failed` step (the step v3 drops, break 5) are
//!   applied and matched to nothing;
//! - every other step is matched to v3's next step, which must agree on
//!   `(workflow_id, task_id, result, loop_counter, element_index)`. When
//!   that v3 step's snapshot `audit_trail` ends in its own task's entry (the
//!   recordings' `snapshot_audit_trail: "own"`), v4's step `status` must
//!   equal that entry's `status`. When the v3 step has a snapshot, the
//!   context rebuilt after v4's step, normalised with the case's
//!   error-context path, must equal the snapshot's `context`, key order
//!   included.
//!
//! Every v3 step must be matched. v3's `mapping_contexts`, `started_at`,
//! `duration_us`, `changes`, the rest of its snapshot's `audit_trail`, and
//! its snapshot's `errors` are not compared.
//!
//! A step's `error` is not held to the record v3's snapshot gained at that
//! step either, because the two are not one to one: v4's `error` is the
//! recorder's record alone, where the records a v3 step gains include the
//! call's own (`validation`'s, a host's, I96), and break 4's wrappers and a
//! workflow's own failure land on the next step's snapshot. Measured over
//! the snapshots: 35 agree on code and message, 7 differ by those shapes,
//! and one by break 4's own code (`fan_out/over_shapes`); the rest have
//! neither.
//!
//! The comparison reads both sides in v3's coordinates: each trace becomes
//! `{outcome, steps, truncated, message_matches}`, its steps as above, so a
//! deviation's pointer into v3's recorded trace names the same place in v4's
//! aligned one, where v4's JSON must be its row of `expect::TRACE_DEVIATED`
//! (each row used once, a stale one failing). A step list v4 shortens at
//! its tail is deviated per missing index, each `MISSING`, so the steps both
//! ran are compared in full. A deviation at a trace's `steps` itself
//! declares a step list that differs otherwise (v4 runs workflows v3 did
//! not): both sides' `steps` are then the matched steps'
//! `"workflow_id/task_id/result"` keys in order, and no context is compared
//! against v3's.
//!
//! Beside v3, every traced run holds to I19's rule, with no deviation: the
//! replay of all of v4's steps from `initial`, normalised, is the traced
//! message's own final context.

use dataflow_corpus::case::{Case, Step};
use dataflow_corpus::compare::diff;
use dataflow_corpus::{Json, case_name, load_cases, normalise, recording_path, replay};
use dataflow_corpus_v4::expect::{Held, TRACE_DEVIATED, expand, under_ignored};
use dataflow_corpus_v4::run::{run_case, satisfies};
use std::fs;

/// Traced runs compared: every run step whose trace is on, in a case that
/// builds and runs in this build. The counts below are the same in every
/// build: the budget case turns its trace off, and `families_on` and
/// `families_off` trace alike.
const TRACED: usize = 85;
/// Traced runs whose step list a deviation declares: break 6's.
const STEP_LISTS: usize = 1;
/// v3 steps matched by a v4 step, in the traced runs whose step list is not
/// deviated: every v3 step of those, since an unmatched one fails, but the
/// ones a deviation declares missing.
const MATCHED: usize = 239;
/// Of those, the steps whose v3 snapshot was compared with v4's rebuilt
/// context: every one but a skip, which v3 records with no snapshot.
const SNAPSHOTS: usize = 227;
/// v4 steps matched to nothing: the engine's step as each sweep is entered
/// and as a loop with a counter exits.
const ENGINE: usize = 41;
/// v4 steps matched to nothing: a task's failure that went to its
/// workflow's policy, the step v3's trace drops (break 5).
const FAILED_TASKS: usize = 9;
/// v4 steps matched to nothing: a workflow's own failure (I61), here
/// `loop/over_shapes`'s non-array `over`.
const FAILED_WORKFLOWS: usize = 1;
/// Deviated trace pointers held to their row of `expect::TRACE_DEVIATED`,
/// one row each.
const TRACE_DEVIATED_ROWS: usize = 4;
/// Of the matched steps, those whose v4 `status` was held to v3's: every
/// one whose v3 snapshot's audit trail ends in the step's own entry.
const STATUSES: usize = 215;

/// A run step's trace, and every pointer under it.
const TRACE: &str = "/steps/*/run/trace";

#[derive(Debug, Default, PartialEq, Eq)]
struct Counts {
    traced: usize,
    step_lists: usize,
    matched: usize,
    snapshots: usize,
    engine: usize,
    failed_tasks: usize,
    failed_workflows: usize,
    trace_deviated: usize,
    statuses: usize,
}

/// The deviation paths of `case` under a run's trace.
fn trace_paths(case: &Case) -> Vec<&str> {
    case.deviations
        .iter()
        .flat_map(|d| d.paths.iter().map(String::as_str))
        .filter(|p| under_ignored(p, &[TRACE]))
        .collect()
}

/// The run steps of `case` whose step list a deviation declares: those
/// whose `/steps/<i>/run/trace/steps` a trace path names in `recording`.
fn step_lists(case: &Case, recording: &Json) -> Vec<usize> {
    trace_paths(case)
        .iter()
        .flat_map(|pattern| expand(recording, pattern))
        .filter_map(|pointer| {
            let rest = pointer.strip_prefix("/steps/")?;
            let (i, tail) = rest.split_once('/')?;
            if tail == "run/trace/steps" {
                i.parse().ok()
            } else {
                None
            }
        })
        .collect()
}

/// A run outcome as the run comparison reads it: `"ok"`, or the error's
/// `code` and `retryable` (its message is v4's wording, I62).
fn outcome(outcome: Option<&Json>) -> Json {
    let Some(error) = outcome.and_then(|o| o.get("error")) else {
        return outcome.cloned().unwrap_or(Json::Null);
    };
    let field = |key: &str| error.get(key).cloned().unwrap_or(Json::Null);
    Json::Object(vec![(
        "error".to_owned(),
        Json::Object(vec![
            ("code".to_owned(), field("code")),
            ("retryable".to_owned(), field("retryable")),
        ]),
    )])
}

/// The five keys a step is matched on, in v3's order, the last two only
/// when present, as v3 writes them.
fn keys(step: &Json) -> Vec<(String, Json)> {
    let mut out = Vec::new();
    for key in ["workflow_id", "task_id", "result"] {
        out.push((key.to_owned(), step.get(key).cloned().unwrap_or(Json::Null)));
    }
    for key in ["loop_counter", "element_index"] {
        if let Some(value) = step.get(key) {
            out.push((key.to_owned(), value.clone()));
        }
    }
    out
}

/// A step's keys as one string, `workflow_id/task_id/result`, for a step
/// list a deviation declares; a workflow's own step has an empty task id.
fn key_text(step: &Json) -> Json {
    let text = |key: &str| step.get(key).and_then(Json::as_str).unwrap_or_default();
    Json::str(format!(
        "{}/{}/{}",
        text("workflow_id"),
        text("task_id"),
        text("result")
    ))
}

/// The status of a v3 step's own audit entry: the last entry of its
/// snapshot's `audit_trail` when that entry is the step's own task's, which
/// the recording's `snapshot_audit_trail: "own"` gives every executed step
/// but a `Skip`, or one whose audit the run kept nothing of.
fn own_status(step: &Json) -> Option<Json> {
    let entry = step.pointer("/message/audit_trail")?.as_array()?.last()?;
    let same = |key: &str| entry.get(key) == step.get(key);
    (same("workflow_id") && same("task_id"))
        .then(|| entry.get("status").cloned().unwrap_or(Json::Null))
}

/// A step as compared: its keys, then `status` when `status` is given,
/// then, when `snapshot` is given, its `message.context`, in v3's key order.
fn compared(step: &Json, status: Option<Json>, snapshot: Option<Json>) -> Json {
    let mut pairs = keys(step);
    // After `result`, then `message`, before the two stamps.
    let mut at = 3;
    if let Some(status) = status {
        pairs.insert(at, ("status".to_owned(), status));
        at += 1;
    }
    if let Some(context) = snapshot {
        let message = Json::Object(vec![("context".to_owned(), context)]);
        pairs.insert(at, ("message".to_owned(), message));
    }
    Json::Object(pairs)
}

/// What one traced run is compared on.
fn shape(outcome: Json, steps: Vec<Json>, trace: &Json) -> Json {
    let field = |key: &str| trace.get(key).cloned().unwrap_or(Json::Null);
    Json::Object(vec![
        ("outcome".to_owned(), outcome),
        ("steps".to_owned(), Json::Array(steps)),
        ("truncated".to_owned(), field("truncated")),
        ("message_matches".to_owned(), field("message_matches")),
    ])
}

/// What one v4 step is, for the alignment.
enum Kind {
    /// The engine's writes as a sweep is entered or a loop exits.
    Engine,
    /// A task's failure that went to its workflow's policy.
    FailedTask,
    /// A workflow's own failure.
    FailedWorkflow,
    /// Matched to v3's next step.
    Matched,
}

fn kind(step: &Json) -> Kind {
    let own = step.get("task_id").is_none_or(|t| *t == Json::Null);
    match (step.get("result").and_then(Json::as_str), own) {
        (Some("failed"), true) => Kind::FailedWorkflow,
        (Some("failed"), false) => Kind::FailedTask,
        (Some("executed"), true) => Kind::Engine,
        _ => Kind::Matched,
    }
}

/// One traced run, both sides in v3's coordinates: v3's recorded trace and
/// v4's, aligned (see the module doc). `plain` is v4's plain run, whose
/// message the replay invariant reads; `step_list` says a deviation
/// declares this run's step list.
fn traced_run(
    name: &str,
    i: usize,
    (v3, v4, plain): (&Json, &Json, &Json),
    error_context: Option<&str>,
    step_list: bool,
    counts: &mut Counts,
    failures: &mut Vec<String>,
) -> (Json, Json) {
    let v3_steps = v3.get("steps").and_then(Json::as_array).unwrap_or(&[]);
    let v4_steps = v4.get("steps").and_then(Json::as_array).unwrap_or(&[]);
    let mut context = v4.get("initial").cloned().unwrap_or(Json::Null);
    let mut matched: Vec<(&Json, Json)> = Vec::new();
    for step in v4_steps {
        for change in step.get("changes").and_then(Json::as_array).unwrap_or(&[]) {
            replay::apply(&mut context, change);
        }
        match kind(step) {
            Kind::Engine => counts.engine += 1,
            Kind::FailedTask => counts.failed_tasks += 1,
            Kind::FailedWorkflow => counts.failed_workflows += 1,
            Kind::Matched => {
                let mut rebuilt = context.clone();
                normalise::context(&mut rebuilt, error_context);
                matched.push((step, rebuilt));
            }
        }
    }

    // I19: the replay of every step is the traced message's final context.
    // The traced message is the plain one when `message_matches` holds,
    // which is compared below; a trace past its budget cannot replay.
    let whole = v4.get("truncated") == Some(&Json::Bool(false));
    let same = v4.get("message_matches") == Some(&Json::Bool(true));
    if whole && same {
        normalise::context(&mut context, error_context);
        let message = plain.pointer("/context").cloned().unwrap_or(Json::Null);
        let differs = diff(&message, &context, &[]);
        if !differs.is_empty() {
            let shown: Vec<String> = differs.iter().take(8).map(|d| format!("  {d}")).collect();
            failures.push(format!(
                "{name}: step {i}'s trace does not replay to its message's context \
                 (expected the message, got the replay)\n{}",
                shown.join("\n")
            ));
        }
    }

    let (expected, actual) = if step_list {
        counts.step_lists += 1;
        (
            v3_steps.iter().map(key_text).collect(),
            matched.iter().map(|(step, _)| key_text(step)).collect(),
        )
    } else {
        let mut actual = Vec::new();
        for (j, (step, rebuilt)) in matched.into_iter().enumerate() {
            let v3_step = v3_steps.get(j);
            if v3_step.is_some_and(|v3_step| keys(v3_step) == keys(step)) {
                counts.matched += 1;
            }
            let snapshot = v3_step
                .and_then(|s| s.pointer("/message/context"))
                .map(|_| rebuilt);
            if snapshot.is_some() {
                counts.snapshots += 1;
            }
            // v4's status, where v3's own entry recorded one (C I-1).
            let status = v3_step
                .and_then(own_status)
                .map(|_| step.get("status").cloned().unwrap_or(Json::Null));
            if status.is_some() {
                counts.statuses += 1;
            }
            actual.push(compared(step, status, snapshot));
        }
        let expected = v3_steps
            .iter()
            .map(|s| compared(s, own_status(s), s.pointer("/message/context").cloned()))
            .collect();
        (expected, actual)
    };
    (
        shape(outcome(v3.get("outcome")), expected, v3),
        shape(outcome(v4.get("outcome")), actual, v4),
    )
}

#[tokio::test]
async fn v4_traces_every_run_as_v3_recorded_it() {
    let mut counts = Counts::default();
    let mut held = Held::new("TRACE_DEVIATED", TRACE_DEVIATED);
    let mut failures = Vec::new();
    for (path, case) in load_cases() {
        let name = case_name(&path);
        let recording = Json::from_text(
            &fs::read_to_string(recording_path(&path)).unwrap_or_else(|e| panic!("{name}: {e}")),
        )
        .unwrap_or_else(|e| panic!("{name}: {e}"));
        let actual = run_case(&case, &recording).await;
        let built = matches!(actual.get("build"), Some(Json::String(ok)) if ok == "ok");
        // `tests/runs.rs` holds the cases with no build and counts the
        // skipped ones.
        if !built || !satisfies(&case.requires) {
            continue;
        }
        let error_context = case.engine.error_context.as_ref().map(|e| e.path.as_str());
        let lists = step_lists(&case, &recording);
        let mut expected = Vec::new();
        let mut got = Vec::new();
        for (i, step) in case.steps.iter().enumerate() {
            let at = format!("/steps/{i}/run");
            let (v3, v4) = (
                recording.pointer(&format!("{at}/trace")),
                actual.pointer(&format!("{at}/trace")),
            );
            let pair = match (step, v3, v4) {
                (Step::Run(_), Some(v3), Some(v4)) => {
                    counts.traced += 1;
                    let plain = actual
                        .pointer(&format!("{at}/message"))
                        .expect("a run records its message");
                    let (v3, v4) = traced_run(
                        &name,
                        i,
                        (v3, v4, plain),
                        error_context,
                        lists.contains(&i),
                        &mut counts,
                        &mut failures,
                    );
                    let wrap = |trace| {
                        Json::Object(vec![(
                            "run".to_owned(),
                            Json::Object(vec![("trace".to_owned(), trace)]),
                        )])
                    };
                    (wrap(v3), wrap(v4))
                }
                (_, None, None) => (Json::object(), Json::object()),
                (_, v3, _) => {
                    failures.push(format!(
                        "{name}: step {i} has a trace in {} only",
                        if v3.is_some() {
                            "v3's recording"
                        } else {
                            "v4's record"
                        }
                    ));
                    (Json::object(), Json::object())
                }
            };
            expected.push(pair.0);
            got.push(pair.1);
        }
        let expected = Json::Object(vec![("steps".to_owned(), Json::Array(expected))]);
        let got = Json::Object(vec![("steps".to_owned(), Json::Array(got))]);
        let deviated = trace_paths(&case);
        let differs = diff(&expected, &got, &deviated);
        if !differs.is_empty() {
            let shown: Vec<String> = differs.iter().take(12).map(|d| format!("  {d}")).collect();
            failures.push(format!(
                "{name}: {} trace differences\n{}",
                differs.len(),
                shown.join("\n")
            ));
        }
        held.check(&name, &deviated, (&expected, &got), &[], &mut failures);
    }
    counts.trace_deviated = held.finish(&mut failures);
    println!("{counts:?}");
    assert!(failures.is_empty(), "\n{}", failures.join("\n\n"));
    assert_eq!(
        counts,
        Counts {
            traced: TRACED,
            step_lists: STEP_LISTS,
            matched: MATCHED,
            snapshots: SNAPSHOTS,
            engine: ENGINE,
            failed_tasks: FAILED_TASKS,
            failed_workflows: FAILED_WORKFLOWS,
            trace_deviated: TRACE_DEVIATED_ROWS,
            statuses: STATUSES,
        }
    );
}

fn j(text: &str) -> Json {
    Json::from_text(text).unwrap()
}

/// v3's trace of a run of `w`'s `a` then `b`, each with its snapshot, and
/// `b`'s with its own audit entry.
fn v3_trace() -> Json {
    j(r#"{"outcome": "ok", "steps": [
        {"workflow_id": "w", "task_id": "a", "result": "executed",
         "message": {"context": {"data": {"a": 1}, "metadata": {}, "temp_data": {"i": 0}},
                     "audit_trail": []},
         "started_at": "<timestamp>", "duration_us": "<duration>", "changes": [],
         "loop_counter": 0},
        {"workflow_id": "w", "task_id": "b", "result": "executed",
         "message": {"context": {"data": {"a": 1, "b": 2}, "metadata": {}, "temp_data": {"i": 0}},
                     "audit_trail": [{"workflow_id": "w", "task_id": "b",
                                      "timestamp": "<timestamp>", "changes": [], "status": 200}]},
         "started_at": "<timestamp>", "duration_us": "<duration>", "changes": [],
         "loop_counter": 0}
    ], "truncated": false, "message_matches": true}"#)
}

/// v4's trace of the same run: the sweep's engine step, `a`, a failing
/// call v3 dropped, then `b`, with the given third call.
fn v4_trace(third: &str) -> Json {
    v4_trace_with(third, 200)
}

/// [`v4_trace`] with the third call's status.
fn v4_trace_with(third: &str, status: u16) -> Json {
    j(&format!(
        r#"{{"outcome": "ok", "initial": {{"data": {{}}, "metadata": {{}}, "temp_data": {{}}}},
        "steps": [
            {{"workflow": 0, "workflow_id": "w", "task": null, "task_id": null,
              "result": "executed", "status": null, "loop_counter": 0,
              "changes": [{{"path": "temp_data.i", "old_value": null, "new_value": 0}}]}},
            {{"workflow": 0, "workflow_id": "w", "task": 0, "task_id": "a",
              "result": "executed", "status": 200, "loop_counter": 0,
              "changes": [{{"path": "data.a", "old_value": null, "new_value": 1}}]}},
            {{"workflow": 0, "workflow_id": "w", "task": 1, "task_id": "f",
              "result": "failed", "status": 500, "loop_counter": 0, "changes": []}},
            {{"workflow": 0, "workflow_id": "w", "task": 2, "task_id": "{third}",
              "result": "executed", "status": {status}, "loop_counter": 0,
              "changes": [{{"path": "data.b", "old_value": null, "new_value": 2}}]}}
        ], "truncated": false, "message_matches": true}}"#
    ))
}

fn plain() -> Json {
    j(r#"{"context": {"data": {"a": 1, "b": 2}, "metadata": {}, "temp_data": {"i": 0}}}"#)
}

#[test]
fn the_engines_step_and_a_failed_step_are_matched_to_nothing() {
    let (mut counts, mut failures) = (Counts::default(), Vec::new());
    let (v3, v4) = traced_run(
        "c",
        0,
        (&v3_trace(), &v4_trace("b"), &plain()),
        None,
        false,
        &mut counts,
        &mut failures,
    );
    assert!(failures.is_empty(), "{failures:?}");
    assert!(diff(&v3, &v4, &[]).is_empty(), "{:?}", diff(&v3, &v4, &[]));
    assert_eq!(
        (
            counts.engine,
            counts.failed_tasks,
            counts.matched,
            counts.snapshots,
            counts.statuses
        ),
        (1, 1, 2, 2, 1)
    );
}

#[test]
fn a_status_that_disagrees_with_v3s_own_entry_is_a_difference() {
    let (mut counts, mut failures) = (Counts::default(), Vec::new());
    let (v3, v4) = traced_run(
        "c",
        0,
        (&v3_trace(), &v4_trace_with("b", 299), &plain()),
        None,
        false,
        &mut counts,
        &mut failures,
    );
    assert!(failures.is_empty(), "{failures:?}");
    let paths: Vec<String> = diff(&v3, &v4, &[]).into_iter().map(|d| d.path).collect();
    assert_eq!(paths, ["/steps/1/status"]);
    assert_eq!(counts.statuses, 1);
}

#[test]
fn a_step_that_disagrees_and_a_replay_that_misses_the_message_fail() {
    let (mut counts, mut failures) = (Counts::default(), Vec::new());
    let other = j(r#"{"context": {"data": {}, "metadata": {}, "temp_data": {}}}"#);
    let (v3, v4) = traced_run(
        "c",
        0,
        (&v3_trace(), &v4_trace("c"), &other),
        None,
        false,
        &mut counts,
        &mut failures,
    );
    assert_eq!(counts.matched, 1);
    let paths: Vec<String> = diff(&v3, &v4, &[]).into_iter().map(|d| d.path).collect();
    assert_eq!(paths, ["/steps/1/task_id"]);
    assert_eq!(failures.len(), 1, "{failures:?}");
    assert!(failures[0].contains("does not replay"), "{failures:?}");
}

#[test]
fn a_declared_step_list_is_compared_as_its_keys() {
    let (mut counts, mut failures) = (Counts::default(), Vec::new());
    let (v3, v4) = traced_run(
        "c",
        0,
        (&v3_trace(), &v4_trace("c"), &plain()),
        None,
        true,
        &mut counts,
        &mut failures,
    );
    assert!(failures.is_empty(), "{failures:?}");
    assert_eq!(
        v3.get("steps").unwrap().to_text(),
        r#"["w/a/executed","w/b/executed"]"#
    );
    assert_eq!(
        v4.get("steps").unwrap().to_text(),
        r#"["w/a/executed","w/c/executed"]"#
    );
    assert_eq!((counts.step_lists, counts.matched), (1, 0));
}

#[test]
fn a_step_list_is_named_by_a_deviation_at_a_traces_steps() {
    let case: Case = serde_json::from_value(serde_json::json!({
        "description": "d", "covers": [], "workflows": [],
        "deviations": [{"break": "6", "note": "n", "paths": [
            "/steps/*/run/trace/steps",
            "/steps/0/run/trace/steps/0/message/context/data/partial",
            "/steps/0/run/message",
        ]}],
    }))
    .unwrap();
    let recording = j(r#"{"steps": [
        {"run": {"trace": {"steps": [{"message": {"context": {"data": {"partial": 1}}}}]}}},
        {"reload": "ok"},
        {"run": {"trace": {"steps": []}}}
    ]}"#);
    assert_eq!(step_lists(&case, &recording), [0, 2]);
    assert_eq!(trace_paths(&case).len(), 2);
}
