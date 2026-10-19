// The engine end to end, through `Engine::builder()`, over workflows without
// loops: the run's stamps, the recorder, the policies, the selection rules,
// what a `Run` reports, and the builder's rules.

use crate::support::{engine, map, message};
use dataflow_rs::def::{ParseOptions, parse};
use dataflow_rs::function::{FunctionConfig, FunctionSpec, Reference};
use dataflow_rs::program::{CompileOptions, IssueCode, ResourceOracle, Severity};
use dataflow_rs::run::{AuditMode, EngineConfig, RunStatus, Selection, WorkflowExit};
use dataflow_rs::{
    Ctx, Engine, ErrorKind, Expr, Function, Message, Outcome, Result, Run, RunOptions,
};
use datavalue::OwnedDataValue;
use serde_json::{Value, json};
use std::sync::Arc;

/// A `map` task appending `item` at `data.order`.
fn append(id: &str, item: &str) -> Value {
    json!({"id": id, "function": {"name": "map", "input": {
        "mappings": [{"path": "data.order", "logic": item, "mode": "append"}]}}})
}

/// A `map` task that returns 500 over a string at `data.s` and 200 over an
/// array: an append to a non-array fails the mapping.
fn fails_over_a_string(id: &str) -> Value {
    json!({"id": id, "function": {"name": "map", "input": {
        "mappings": [{"path": "data.s", "logic": "x", "mode": "append"}]}}})
}

/// A `validation` task that returns 400 unless `data.ok` is `true`.
fn requires_ok(id: &str) -> Value {
    json!({"id": id, "function": {"name": "validation", "input": {"rules": [
        {"logic": {"==": [{"var": "data.ok"}, true]}, "message": "not ok"}]}}})
}

/// A `parse_xml` task that fails with `Err` unless `data.xml` is a document.
fn parses_xml(id: &str) -> Value {
    json!({"id": id, "function": {"name": "parse_xml", "input": {
        "source": "data.xml", "target": "parsed"}}})
}

/// A `filter` task whose condition is `false`, rejecting by `on_reject`.
fn rejects(id: &str, on_reject: &str) -> Value {
    json!({"id": id, "function": {"name": "filter", "input": {
        "condition": false, "on_reject": on_reject}}})
}

/// A workflow `id` holding `tasks`, with each `(key, value)` of `extra`.
fn workflow(id: &str, extra: &[(&str, Value)], tasks: Vec<Value>) -> Value {
    crate::support::workflow(id, tasks, extra)
}

/// The message as JSON (keys sorted).
fn json_of(m: &Message) -> Value {
    m.to_json()
}

/// One workflow the run considered: `(id, selection, sweeps, exit)`, the
/// selection by name.
type Considered = (String, &'static str, u32, Option<WorkflowExit>);

/// Each workflow the run considered.
fn considered(run: &Run) -> Vec<Considered> {
    run.workflows()
        .map(|w| {
            let selection = match w.selection {
                Selection::Ran => "ran",
                Selection::SkippedByCondition => "condition",
                Selection::ConditionFailed(_) => "condition failed",
                Selection::SkippedByRollout => "rollout",
                _ => "other",
            };
            (w.id.to_owned(), selection, w.sweeps, w.exit)
        })
        .collect()
}

/// `(workflow, task, status)` of each audit entry.
fn audit(m: &Message) -> Vec<(String, String, usize)> {
    m.audit()
        .iter()
        .map(|e| (e.workflow_id.to_string(), e.task_id.to_string(), e.status))
        .collect()
}

fn entry(workflow: &str, task: &str, status: usize) -> (String, String, usize) {
    (workflow.to_owned(), task.to_owned(), status)
}

/// `(code, message)` of each error record.
fn errors(m: &Message) -> Vec<(String, String)> {
    m.errors()
        .iter()
        .map(|e| (e.code.clone(), e.message.clone()))
        .collect()
}

fn stopped_code(run: &Run) -> Option<String> {
    match run.status() {
        RunStatus::Stopped(e) => Some(e.code().to_owned()),
        _ => None,
    }
}

#[tokio::test]
async fn every_run_stamps_processed_at_and_engine_version() {
    let e = engine(&[workflow(
        "w",
        &[],
        vec![map("t", "data.a", json!(1)), requires_ok("v")],
    )]);
    let mut m = message(json!({}));
    e.run(&mut m).await;
    let j = json_of(&m);
    let meta = &j["context"]["metadata"];
    let at = meta["processed_at"]
        .as_str()
        .expect("processed_at is a string");
    let parsed = chrono::DateTime::parse_from_rfc3339(at).expect("processed_at is RFC 3339");
    // One clock reading: every audit entry and every error record carries it.
    assert_eq!(m.audit().len(), 2);
    for entry in m.audit() {
        assert_eq!(entry.timestamp, parsed);
    }
    assert_eq!(m.errors()[0].timestamp.as_deref(), Some(at));
    assert_eq!(meta["engine_version"], json!(dataflow_rs::VERSION));
    assert!(meta.get("channel").is_none());
    // The stamps come first, in v3's order, before the progress write.
    let wire = serde_json::to_string(&m).unwrap();
    assert!(
        wire.contains(r#""metadata":{"processed_at":"#)
            && wire.contains(r#"","engine_version":""#)
            && wire.contains(r#"","progress":{"#),
        "{wire}"
    );

    // With a channel, `channel` follows `engine_version`. A second run on the
    // same message restamps in place.
    let e = engine(&[workflow(
        "w",
        &[("channel", json!("orders"))],
        vec![map("t", "data.a", json!(1))],
    )]);
    let opts = RunOptions::default().channel("orders");
    e.run_with(&mut m, &opts).await;
    let meta = &json_of(&m)["context"]["metadata"];
    assert_eq!(meta["channel"], json!("orders"));
    let wire = serde_json::to_string(&m).unwrap();
    assert!(
        wire.contains(r#""metadata":{"processed_at":"#)
            && wire.contains(r#"","progress":{"workflow_id":"w","task_id":"t","status_code":200},"channel":"orders"}"#),
        "{wire}"
    );
}

#[tokio::test]
async fn progress_follows_every_task_that_does_not_skip() {
    let e = engine(&[
        workflow(
            "w",
            &[],
            vec![
                map("t1", "data.a", json!(1)),
                map("t2", "data.seen", json!({"var": "metadata.progress"})),
                requires_ok("v"),
                map("t3", "data.after_v", json!({"var": "metadata.progress"})),
            ],
        ),
        // Chained through the progress the first workflow left.
        workflow(
            "w2",
            &[
                ("priority", json!(1)),
                (
                    "condition",
                    json!({"==": [{"var": "metadata.progress.task_id"}, "t3"]}),
                ),
            ],
            vec![map("chained", "data.chained", json!(true))],
        ),
    ]);
    let mut m = message(json!({}));
    let run = e.run(&mut m).await;
    let data = &json_of(&m)["context"]["data"];
    assert_eq!(
        data["seen"],
        json!({"workflow_id": "w", "task_id": "t1", "status_code": 200})
    );
    assert_eq!(
        data["after_v"],
        json!({"workflow_id": "w", "task_id": "v", "status_code": 400})
    );
    assert_eq!(data["chained"], json!(true));
    assert_eq!(
        json_of(&m)["context"]["metadata"]["progress"],
        json!({"workflow_id": "w2", "task_id": "chained", "status_code": 200})
    );
    assert_eq!(run.matched(), 2);
    assert!(matches!(run.status(), RunStatus::CompletedWithErrors));
}

#[tokio::test]
async fn a_skip_records_nothing() {
    let e = engine(&[workflow(
        "w",
        &[],
        vec![
            map("t1", "data.a", json!(1)),
            rejects("f", "skip"),
            map(
                "t2",
                "data.seen",
                json!({"var": "metadata.progress.task_id"}),
            ),
        ],
    )]);
    let mut m = message(json!({}));
    let run = e.run(&mut m).await;
    // No audit entry, no progress write, no error record for the skip; the
    // next task runs.
    assert_eq!(audit(&m), [entry("w", "t1", 200), entry("w", "t2", 200)]);
    assert_eq!(json_of(&m)["context"]["data"]["seen"], json!("t1"));
    assert!(m.errors().is_empty());
    assert!(matches!(run.status(), RunStatus::Completed));
}

#[tokio::test]
async fn a_4xx_continues_and_a_5xx_stops_with_task_error() {
    let e = engine(&[
        workflow(
            "w",
            &[],
            vec![
                requires_ok("v"),
                map("a", "data.after_v", json!(true)),
                fails_over_a_string("m"),
                map("b", "data.after_m", json!(true)),
            ],
        ),
        workflow(
            "w2",
            &[("priority", json!(1))],
            vec![map("c", "data.w2", json!(true))],
        ),
    ]);
    let mut m = message(json!({"s": "str"}));
    let run = e.run(&mut m).await;
    assert_eq!(
        audit(&m),
        [
            entry("w", "v", 400),
            entry("w", "a", 200),
            entry("w", "m", 500)
        ]
    );
    let data = &json_of(&m)["context"]["data"];
    assert_eq!(data["after_v"], json!(true));
    assert!(data.get("after_m").is_none() && data.get("w2").is_none());
    assert_eq!(
        errors(&m),
        [
            ("VALIDATION_ERROR".to_owned(), "not ok".to_owned()),
            (
                "MAPPING_ERROR".to_owned(),
                "mappings[0] (data.s): the target holds a string, not an array".to_owned()
            ),
            (
                "TASK_STATUS_ERROR".to_owned(),
                "Task m returned status 500".to_owned()
            ),
        ]
    );
    let record = &m.errors()[2];
    assert_eq!(
        (record.workflow_id.as_deref(), record.task_id.as_deref()),
        (Some("w"), Some("m"))
    );
    let RunStatus::Stopped(error) = run.status() else {
        panic!("the 5xx stops the run: {:?}", run.status());
    };
    assert_eq!(error.code(), "TASK_ERROR");
    assert_eq!(error.message(), "Task m failed with status 500");
    assert!(!run.retryable() && run.partial());
    assert_eq!(
        considered(&run),
        [("w".to_owned(), "ran", 1, Some(WorkflowExit::Stopped))]
    );
}

#[tokio::test]
async fn terminal_applies_after_classification() {
    let terminal = |mut task: Value| {
        task["terminal"] = json!(true);
        task
    };
    let pair = |task: Value, extra: &[(&str, Value)]| {
        let mut task = terminal(task);
        for (key, value) in extra {
            task[*key] = value.clone();
        }
        engine(&[
            workflow(
                "w",
                &[],
                vec![task, map("after", "data.after", json!(true))],
            ),
            workflow(
                "w2",
                &[("priority", json!(1))],
                vec![map("next", "data.next", json!(true))],
            ),
        ])
    };

    // A 200 halts the workflow; the next one runs.
    let e = pair(fails_over_a_string("t"), &[]);
    let mut m = message(json!({"s": []}));
    let run = e.run(&mut m).await;
    assert_eq!(audit(&m), [entry("w", "t", 200), entry("w2", "next", 200)]);
    assert_eq!(considered(&run)[0].3, Some(WorkflowExit::Halted));

    // A 500 under `on_failure: fail` still records and still stops the run.
    let mut m = message(json!({"s": "str"}));
    let run = e.run(&mut m).await;
    assert_eq!(audit(&m), [entry("w", "t", 500)]);
    assert_eq!(errors(&m)[1].0, "TASK_STATUS_ERROR");
    assert_eq!(stopped_code(&run).as_deref(), Some("TASK_ERROR"));

    // Under `on_failure: continue` the 500 is recorded and the terminal task
    // halts its workflow.
    let e = pair(
        fails_over_a_string("t"),
        &[("on_failure", json!("continue"))],
    );
    let mut m = message(json!({"s": "str"}));
    let run = e.run(&mut m).await;
    assert_eq!(audit(&m), [entry("w", "t", 500), entry("w2", "next", 200)]);
    assert_eq!(errors(&m)[1].0, "TASK_STATUS_ERROR");
    assert_eq!(considered(&run)[0].3, Some(WorkflowExit::Halted));

    // A 400 is a rejection: recorded, then halted by `terminal`.
    let e = pair(requires_ok("t"), &[]);
    let mut m = message(json!({"ok": false}));
    let run = e.run(&mut m).await;
    assert_eq!(audit(&m), [entry("w", "t", 400), entry("w2", "next", 200)]);
    assert!(matches!(run.status(), RunStatus::CompletedWithErrors));
}

#[tokio::test]
async fn a_halt_stops_only_its_own_workflow() {
    let e = engine(&[
        workflow(
            "w",
            &[],
            vec![
                rejects("f", "halt"),
                map("after", "data.after", json!(true)),
            ],
        ),
        workflow(
            "g",
            &[("priority", json!(1))],
            vec![
                json!({"id": "group", "terminal": true, "tasks": [map("in", "data.in", json!(true))]}),
                map("past", "data.past", json!(true)),
            ],
        ),
        workflow(
            "w2",
            &[("priority", json!(2))],
            vec![map("next", "data.next", json!(true))],
        ),
    ]);
    let mut m = message(json!({}));
    let run = e.run(&mut m).await;
    assert_eq!(
        audit(&m),
        [
            entry("w", "f", 299),
            entry("g", "in", 200),
            entry("w2", "next", 200)
        ]
    );
    let exits: Vec<_> = considered(&run).into_iter().map(|w| w.3).collect();
    assert_eq!(
        exits,
        [
            Some(WorkflowExit::Halted),
            Some(WorkflowExit::Halted),
            Some(WorkflowExit::Completed)
        ]
    );
    assert!(matches!(run.status(), RunStatus::Completed));
}

#[tokio::test]
async fn workflows_run_in_priority_order_and_only_active_ones() {
    let e = engine(&[
        workflow("c", &[("priority", json!(3))], vec![append("t", "c")]),
        workflow("a", &[("priority", json!(1))], vec![append("t", "a")]),
        workflow("b", &[("priority", json!(2))], vec![append("t", "b")]),
        workflow(
            "p",
            &[("priority", json!(0)), ("status", json!("paused"))],
            vec![append("t", "p")],
        ),
        workflow(
            "x",
            &[("priority", json!(0)), ("status", json!("archived"))],
            vec![append("t", "x")],
        ),
        // Ties keep input order.
        workflow("a2", &[("priority", json!(1))], vec![append("t", "a2")]),
    ]);
    let mut m = message(json!({}));
    let run = e.run(&mut m).await;
    assert_eq!(
        json_of(&m)["context"]["data"]["order"],
        json!(["a", "a2", "b", "c"])
    );
    let ids: Vec<_> = run.workflows().map(|w| w.id).collect();
    assert_eq!(ids, ["a", "a2", "b", "c"]);
    let indices: Vec<_> = run.workflows().map(|w| w.index.index()).collect();
    assert!(indices.windows(2).all(|w| w[0] < w[1]), "{indices:?}");
}

#[tokio::test]
async fn a_channel_selects_its_active_workflows() {
    let on = |id: &str, channel: &str, status: &str| {
        workflow(
            id,
            &[("channel", json!(channel)), ("status", json!(status))],
            vec![append("t", id)],
        )
    };
    let e = engine(&[
        on("o1", "orders", "active"),
        on("p1", "payments", "active"),
        on("o2", "orders", "paused"),
        workflow("d", &[], vec![append("t", "d")]),
        on("o3", "orders", "active"),
    ]);
    let order = |m: &Message| json_of(m)["context"]["data"]["order"].clone();

    let mut m = message(json!({}));
    let run = e
        .run_with(&mut m, &RunOptions::default().channel("orders"))
        .await;
    assert_eq!(order(&m), json!(["o1", "o3"]));
    assert_eq!(
        run.workflows().map(|w| w.id).collect::<Vec<_>>(),
        ["o1", "o3"]
    );
    assert_eq!(
        json_of(&m)["context"]["metadata"]["channel"],
        json!("orders")
    );

    let mut m = message(json!({}));
    let run = e
        .run_with(&mut m, &RunOptions::default().channel("default"))
        .await;
    assert_eq!(order(&m), json!(["d"]));
    assert_eq!(run.matched(), 1);

    // An unknown channel runs nothing, and still stamps.
    let mut m = message(json!({}));
    let run = e
        .run_with(&mut m, &RunOptions::default().channel("nowhere"))
        .await;
    assert_eq!(run.workflows().count(), 0);
    assert!(json_of(&m)["context"]["data"].get("order").is_none());
    assert!(matches!(run.status(), RunStatus::Completed));
    assert_eq!(
        json_of(&m)["context"]["metadata"]["channel"],
        json!("nowhere")
    );

    // No channel: every Active workflow.
    let mut m = message(json!({}));
    e.run(&mut m).await;
    assert_eq!(order(&m), json!(["o1", "p1", "d", "o3"]));
}

#[tokio::test]
async fn a_rollout_admits_by_bucket_and_no_bucket_is_admitted() {
    let rollout = |start: u8, end: u8| json!({"bucket_start": start, "bucket_end": end});
    let e = engine(&[
        workflow(
            "v1",
            &[("rollout", rollout(0, 50))],
            vec![append("t", "v1")],
        ),
        workflow(
            "v2",
            &[("rollout", rollout(50, 100))],
            vec![append("t", "v2")],
        ),
        workflow("all", &[], vec![append("t", "all")]),
    ]);
    let order = |m: &Message| json_of(m)["context"]["data"]["order"].clone();
    for (bucket, ran, selections) in [
        (Some(10), json!(["v1", "all"]), ["ran", "rollout", "ran"]),
        (Some(49), json!(["v1", "all"]), ["ran", "rollout", "ran"]),
        (Some(50), json!(["v2", "all"]), ["rollout", "ran", "ran"]),
        (Some(99), json!(["v2", "all"]), ["rollout", "ran", "ran"]),
        // A bucket past 99 is served as 99, as in 3.x, through the builder
        // and the field alike.
        (Some(100), json!(["v2", "all"]), ["rollout", "ran", "ran"]),
        (Some(255), json!(["v2", "all"]), ["rollout", "ran", "ran"]),
        (None, json!(["v1", "v2", "all"]), ["ran", "ran", "ran"]),
    ] {
        let mut opts = RunOptions::default();
        if let Some(b) = bucket {
            opts = if b == 255 {
                let mut opts = opts;
                opts.routing_bucket = Some(b);
                opts
            } else {
                opts.routing_bucket(b)
            };
        }
        let mut m = message(json!({}));
        let run = e.run_with(&mut m, &opts).await;
        assert_eq!(order(&m), ran, "bucket {bucket:?}");
        let got: Vec<_> = considered(&run).into_iter().map(|w| w.1).collect();
        assert_eq!(got, selections, "bucket {bucket:?}");
        for (_, selection, sweeps, exit) in considered(&run) {
            assert_eq!(
                (sweeps, exit.is_some()),
                (u32::from(selection == "ran"), selection == "ran")
            );
        }
    }
}

#[tokio::test]
async fn a_failing_workflow_condition_is_condition_failed() {
    let e = engine(&[
        workflow(
            "broken",
            &[("condition", json!({"/": [1, {"var": "data.zero"}]}))],
            vec![append("t", "broken")],
        ),
        workflow(
            "truthy",
            &[("condition", json!({"var": "data.one"}))],
            vec![append("t", "truthy")],
        ),
        workflow(
            "true",
            &[("condition", json!({"var": "data.yes"}))],
            vec![append("t", "true")],
        ),
    ]);
    let mut m = message(json!({"zero": 0, "one": 1, "yes": true}));
    let run = e.run(&mut m).await;
    let mut records = run.workflows();
    let broken = records.next().unwrap();
    let Selection::ConditionFailed(error) = broken.selection else {
        panic!("{:?}", broken.selection);
    };
    assert_eq!(error.kind(), &ErrorKind::Eval);
    assert_eq!((broken.sweeps, broken.exit), (0, None));
    // Only a literal `true` passes: `1` is false.
    assert!(matches!(
        records.next().unwrap().selection,
        Selection::SkippedByCondition
    ));
    assert!(matches!(records.next().unwrap().selection, Selection::Ran));
    assert_eq!(json_of(&m)["context"]["data"]["order"], json!(["true"]));
    // A condition that fails records nothing on the message, as v3.
    assert!(m.errors().is_empty() && m.audit().len() == 1);
    assert!(matches!(run.status(), RunStatus::Completed));
    assert_eq!(run.matched(), 1);
}

#[tokio::test]
async fn run_reports_selection_sweeps_and_exit() {
    let rollout = json!({"bucket_start": 0, "bucket_end": 10});
    let e = engine(&[
        workflow("a", &[], vec![append("t", "a")]),
        workflow("b", &[("condition", json!(false))], vec![append("t", "b")]),
        workflow("c", &[("rollout", rollout)], vec![append("t", "c")]),
        workflow("d", &[], vec![rejects("f", "halt"), append("t", "d")]),
        workflow(
            "e",
            &[("on_failure", json!("continue"))],
            vec![fails_over_a_string("m"), append("t", "e")],
        ),
        workflow("f", &[], vec![parses_xml("x"), append("t", "f")]),
        workflow("g", &[], vec![append("t", "g")]),
    ]);
    let opts = RunOptions::default().routing_bucket(99);
    let ran = |id: &str, exit| (id.to_owned(), "ran", 1, Some(exit));
    let skipped = |id: &str, why| (id.to_owned(), why, 0, None);

    // `f`'s `Err` stops the run: `g` is not considered.
    let mut m = message(json!({"s": "str", "xml": "<a>"}));
    let run = e.run_with(&mut m, &opts).await;
    let expected: Vec<Considered> = vec![
        ran("a", WorkflowExit::Completed),
        skipped("b", "condition"),
        skipped("c", "rollout"),
        ran("d", WorkflowExit::Halted),
        ran("e", WorkflowExit::Failed),
        ran("f", WorkflowExit::Stopped),
    ];
    assert_eq!(considered(&run), expected);
    assert_eq!(run.matched(), 4);
    assert_eq!(stopped_code(&run).as_deref(), Some("VALIDATION_ERROR"));
    assert!(run.partial() && !run.retryable());
    let error = run.into_result().unwrap_err();
    assert_eq!(error.code(), "VALIDATION_ERROR");

    // Without the stop, every workflow is considered and `e`'s record makes
    // the run `CompletedWithErrors`.
    let mut m = message(json!({"s": "str", "xml": "<a>1</a>"}));
    let run = e.run_with(&mut m, &opts).await;
    assert_eq!(run.workflows().count(), 7);
    assert_eq!(considered(&run)[5].3, Some(WorkflowExit::Completed));
    assert!(matches!(run.status(), RunStatus::CompletedWithErrors));
    assert!(!run.partial());
    assert!(run.into_result().is_ok());

    // And with nothing recorded, `Completed`.
    let mut m = message(json!({"s": [], "xml": "<a>1</a>"}));
    let run = e.run_with(&mut m, &opts).await;
    assert!(matches!(run.status(), RunStatus::Completed));
    assert_eq!(considered(&run)[4].3, Some(WorkflowExit::Completed));
    assert_eq!(run.matched(), 5);
}

#[tokio::test]
async fn audit_last_keeps_exactly_n_after_the_run() {
    let tasks: Vec<Value> = (0..10)
        .map(|i| map(&format!("t{i}"), "data.x", json!(i)))
        .collect();
    let e = engine(&[workflow("w", &[], tasks)]);
    for (mode, kept) in [
        (AuditMode::Full, (0..10).collect::<Vec<_>>()),
        (AuditMode::Off, vec![]),
        (AuditMode::Last(0), vec![]),
        (AuditMode::Last(1), vec![9]),
        (AuditMode::Last(3), vec![7, 8, 9]),
        (AuditMode::Last(10), (0..10).collect()),
        (AuditMode::Last(25), (0..10).collect()),
    ] {
        let mut m = message(json!({}));
        e.run_with(&mut m, &RunOptions::default().audit(mode)).await;
        let got: Vec<String> = m.audit().iter().map(|e| e.task_id.to_string()).collect();
        let want: Vec<String> = kept.iter().map(|i| format!("t{i}")).collect();
        assert_eq!(got, want, "{mode:?}");
        // The progress write does not depend on the audit.
        assert_eq!(
            json_of(&m)["context"]["metadata"]["progress"]["task_id"],
            json!("t9")
        );
    }
}

#[tokio::test]
async fn capture_off_records_empty_changes() {
    let e = engine(&[workflow(
        "w",
        &[],
        vec![
            map("m", "data.a", json!(1)),
            json!({"id": "p", "function": {"name": "parse_json", "input": {
                "source": "data.raw", "target": "parsed"}}}),
            json!({"id": "x", "function": {"name": "parse_xml", "input": {
                "source": "data.xml", "target": "doc"}}}),
            json!({"id": "j", "function": {"name": "publish_json", "input": {
                "source": "a", "target": "out"}}}),
        ],
    )]);
    let data = json!({"raw": "{\"k\": 1}", "xml": "<a>1</a>"});
    for capture in [true, false] {
        let mut m = message(data.clone());
        e.run_with(&mut m, &RunOptions::default().capture_changes(capture))
            .await;
        assert_eq!(m.audit().len(), 4);
        for entry in m.audit() {
            assert_eq!(entry.status, 200);
            assert_eq!(entry.changes.is_empty(), !capture, "{}", entry.task_id);
        }
        // The writes happen either way.
        let d = &json_of(&m)["context"]["data"];
        assert_eq!(
            (&d["a"], &d["parsed"], &d["out"]),
            (&json!(1), &json!({"k": 1}), &json!("1"))
        );
    }
}

#[derive(FunctionConfig)]
struct QueryConfig {
    connector: Option<Expr<String>>,
}

/// A host function whose `connector` names a connector.
struct Query;

impl Function for Query {
    type Config = QueryConfig;

    fn spec(&self) -> FunctionSpec {
        FunctionSpec::default().with_references(vec![Reference::new("connector", "connector")])
    }

    async fn call(&self, _ctx: &mut Ctx<'_>, _cfg: &QueryConfig) -> Result<Outcome> {
        Ok(Outcome::Ok)
    }
}

/// An oracle that knows only `db1`.
struct Known;

impl ResourceOracle for Known {
    fn resolve(&self, _kind: &str, name: &str) -> std::result::Result<(), String> {
        if name == "db1" {
            Ok(())
        } else {
            Err(format!("only `db1` is declared, not `{name}`"))
        }
    }
}

fn query(connector: &str) -> Value {
    workflow(
        "q",
        &[],
        vec![json!({"id": "t", "function": {"name": "query", "input": {"connector": connector}}})],
    )
}

#[test]
fn build_fails_with_every_rejected_issue() {
    let good = workflow("good", &[], vec![map("t", "data.a", json!(1))]);
    let unparsed = json!({"id": "p", "tasks": [{"id": "t"}]});
    let unknown = workflow(
        "u",
        &[],
        vec![json!({"id": "t", "function": {"name": "fetch"}})],
    );
    let unparsed_too = json!({"id": "p2", "tasks": "none"});
    let parse_issues = |raw: &Value| parse(raw, ParseOptions::default()).issues;
    let compiler = dataflow_rs::Registry::default().compiler(CompileOptions::default());
    let compile_issues =
        |raw: &Value| compiler.check(&parse(raw, ParseOptions::default()).def.unwrap());

    let issues = Engine::builder()
        .workflow_json(&good)
        .workflow_json(&unparsed)
        .workflow_json(&unknown)
        .workflow(parse(&unknown, ParseOptions::default()).def.unwrap())
        .workflow_json(&unparsed_too)
        .build()
        .unwrap_err();
    let mut expected = parse_issues(&unparsed);
    expected.extend(compile_issues(&unknown));
    expected.extend(compile_issues(&unknown));
    expected.extend(parse_issues(&unparsed_too));
    assert!(
        expected
            .iter()
            .any(|i| i.code == IssueCode::UnknownFunction)
    );
    assert!(expected.iter().any(|i| i.severity == Severity::Rejected));
    assert_eq!(issues, expected);

    // A definition with only Advisories and Defects builds, and its issues
    // are dropped: a v3 spelling, and a near-miss key.
    let advised = json!({"id": "w", "tasks": [{"id": "t", "continue_on_error": true, "conditon": true,
        "function": {"name": "map", "input": {"mappings": []}}}]});
    assert!(!parse_issues(&advised).is_empty());
    assert!(Engine::builder().workflow_json(&advised).build().is_ok());
}

#[test]
fn build_keeps_a_rejected_definitions_parse_issues_first() {
    // A definition its parse reads, with issues, and its compile
    // rejects: a v3 spelling, a near-miss key, and an unknown function.
    let raw = json!({"id": "w", "tasks": [{"id": "t", "continue_on_error": true, "conditon": true,
        "function": {"name": "fetch"}}]});
    let parsed = parse(&raw, ParseOptions::default());
    let def = parsed.def.expect("the parse reads it");
    assert!(!parsed.issues.is_empty());
    let compile = dataflow_rs::Registry::default()
        .compiler(CompileOptions::default())
        .check(&def);
    assert!(compile.iter().any(|i| i.code == IssueCode::UnknownFunction));
    let issues = Engine::builder().workflow_json(&raw).build().unwrap_err();
    let mut expected = parsed.issues;
    expected.extend(compile);
    assert_eq!(issues, expected);
}

#[tokio::test]
async fn engine_compiler_uses_the_programs_registry_and_options() {
    let e = Engine::builder()
        .register("query", Query)
        .options(CompileOptions::default().resources(Arc::new(Known)))
        .workflow_json(&query("db1"))
        .build()
        .unwrap();
    let compiler = e.compiler();
    assert!(compiler.registry().catalogue().any(|f| f.name == "query"));
    // The options: the oracle refuses `db2`.
    let def = |raw: &Value| parse(raw, ParseOptions::default()).def.unwrap();
    let issues = compiler.check(&def(&query("db2")));
    assert_eq!(
        issues.iter().map(|i| i.code).collect::<Vec<_>>(),
        [IssueCode::UnresolvedReference]
    );
    // The registry: what it compiles links into a program the engine runs.
    let compiled = compiler.compile_one(&def(&query("db1"))).unwrap();
    let program = compiler.link([compiled.workflow]);
    let reloaded = e.with_program(program);
    assert_eq!(reloaded.program().workflows()[0].id, "q");
}

#[tokio::test]
async fn with_program_keeps_the_config() {
    let e = Engine::builder()
        .config(EngineConfig::default())
        .workflow_json(&workflow("old", &[], vec![append("t", "old")]))
        .build()
        .unwrap();
    let compiler = e.compiler();
    let def = parse(
        &workflow("new", &[], vec![append("t", "new")]),
        ParseOptions::default(),
    )
    .def
    .unwrap();
    let program = compiler.link([compiler.compile_one(&def).unwrap().workflow]);
    let reloaded = e.with_program(program);
    assert_eq!(
        reloaded.config().observer.is_some(),
        e.config().observer.is_some()
    );
    assert_eq!(reloaded.config().error_context, e.config().error_context);
    assert_ne!(reloaded.program().id(), e.program().id());
    // The reload runs the new program; the old engine keeps the old one.
    let order = |m: &Message| json_of(m)["context"]["data"]["order"].clone();
    let mut m = message(json!({}));
    reloaded.run(&mut m).await;
    assert_eq!(order(&m), json!(["new"]));
    let mut m = message(json!({}));
    e.run(&mut m).await;
    assert_eq!(order(&m), json!(["old"]));
}

#[tokio::test]
async fn a_host_call_no_longer_stops_the_run() {
    // The engine used to stop the run at a host call; it now runs it, between
    // the tasks around it, and records it as a built-in.
    let e = Engine::builder()
        .register("query", Query)
        .workflow_json(&workflow(
            "w",
            &[],
            vec![
                append("before", "before"),
                json!({"id": "t", "function": {"name": "query", "input": {"connector": "db1"}}}),
                append("after", "after"),
            ],
        ))
        .build()
        .unwrap();
    let mut m = message(json!({}));
    let run = e.run(&mut m).await;
    assert!(
        matches!(run.status(), RunStatus::Completed),
        "{:?}",
        run.status()
    );
    assert_eq!(
        audit(&m),
        [
            entry("w", "before", 200),
            entry("w", "t", 200),
            entry("w", "after", 200)
        ]
    );
    assert_eq!(
        json_of(&m)["context"]["data"]["order"],
        json!(["before", "after"])
    );
    assert!(m.errors().is_empty());
}

#[test]
fn the_run_future_is_send() {
    fn send<T: Send>(_: &T) {}
    let e = engine(&[workflow("w", &[], vec![append("t", "a")])]);
    let mut m = message(json!({}));
    let opts = RunOptions::default();
    send(&e.run(&mut m));
    send(&e.run_with(&mut m, &opts));
}

#[tokio::test]
async fn an_append_past_the_last_index_fails_the_mapping() {
    // Each appended element's change is at `path.<index>`, and an index
    // past the cap parses back as a key, which no replay could put in the
    // array: the mapping fails, and the array is left as it was. Up to the
    // cap an append lands.
    let cap = dataflow_rs::expr::MAX_ARRAY_INDEX + 1;
    let with_list = |len: usize| {
        Message::from_parts(
            OwnedDataValue::Null,
            OwnedDataValue::Object(vec![(
                "order".to_owned(),
                OwnedDataValue::Array(vec![OwnedDataValue::Null; len]),
            )]),
            OwnedDataValue::Null,
        )
    };
    let e = engine(&[workflow(
        "w",
        &[("on_failure", json!("continue"))],
        vec![{
            let mut t = append("a", "x");
            t["on_failure"] = json!("continue");
            t
        }],
    )]);

    let mut m = with_list(cap);
    e.run(&mut m).await;
    let codes: Vec<(&str, &str)> = m
        .errors()
        .iter()
        .map(|r| (r.code.as_str(), r.message.as_str()))
        .collect();
    let refused = format!(
        "mappings[0] (data.order): the array would grow past {cap} elements, the most an \
         index reaches"
    );
    assert_eq!(
        codes,
        [
            ("MAPPING_ERROR", refused.as_str()),
            ("TASK_STATUS_ERROR", "Task a returned status 500"),
        ]
    );
    let list = m
        .context()
        .data()
        .get("order")
        .and_then(|v| v.as_array().map(<[_]>::len));
    assert_eq!(list, Some(cap), "the array is left as it was");

    let mut m = with_list(cap - 1);
    e.run(&mut m).await;
    assert!(m.errors().is_empty(), "{:?}", m.errors());
    let list = m
        .context()
        .data()
        .get("order")
        .and_then(|v| v.as_array().map(<[_]>::len));
    assert_eq!(list, Some(cap));
}
