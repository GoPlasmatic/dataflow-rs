extern crate dataflow_rs_v4 as dataflow_rs;

// A `Run` as JSON (I126): `{status, error?, retryable, partial, matched,
// workflows: [{id, index, selection, error?, sweeps, exit}]}`, with the
// trace and the arena counters left out. Each test serializes a real run
// and compares it with a literal.

use dataflow_rs::run::CancelToken;
use dataflow_rs::{Engine, Error, Message, Run, RunOptions};
use datavalue::OwnedDataValue;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

fn engine(workflows: &[Value]) -> Engine {
    workflows
        .iter()
        .fold(Engine::builder(), |b, w| b.workflow_json(w))
        .build()
        .unwrap_or_else(|issues| panic!("the engine builds: {issues:?}"))
}

fn message(data: Value) -> Message {
    Message::from_parts(
        OwnedDataValue::Null,
        OwnedDataValue::from(&data),
        OwnedDataValue::Null,
    )
    .with_id("m")
}

fn map(id: &str, path: &str, logic: Value) -> Value {
    json!({"id": id, "function": {"name": "map", "input": {
        "mappings": [{"path": path, "logic": logic}]}}})
}

/// An append to `data.s`, which fails over a string.
fn fails(id: &str) -> Value {
    json!({"id": id, "function": {"name": "map", "input": {
        "mappings": [{"path": "data.s", "logic": "x", "mode": "append"}]}}})
}

fn requires_ok(id: &str) -> Value {
    json!({"id": id, "function": {"name": "validation", "input": {"rules": [
        {"logic": {"==": [{"var": "data.ok"}, true]}, "message": "not ok"}]}}})
}

fn rejects(id: &str, on_reject: &str) -> Value {
    json!({"id": id, "function": {"name": "filter", "input": {
        "condition": false, "on_reject": on_reject}}})
}

fn workflow(id: &str, extra: &[(&str, Value)], tasks: Vec<Value>) -> Value {
    let mut w = json!({"id": id, "tasks": tasks});
    for (key, value) in extra {
        w[*key] = value.clone();
    }
    w
}

fn json_of(run: &Run) -> Value {
    serde_json::to_value(run).unwrap()
}

#[tokio::test]
async fn a_completed_run() {
    let e = engine(&[workflow("w", &[], vec![map("t", "data.a", json!(1))])]);
    let run = e.run(&mut message(json!({}))).await;
    assert_eq!(
        json_of(&run),
        json!({"status": "completed", "retryable": false, "partial": false, "matched": 1,
               "workflows": [{"id": "w", "index": 0, "selection": "ran",
                              "sweeps": 1, "exit": "completed"}]})
    );
    // Key order is part of the shape.
    assert_eq!(
        serde_json::to_string(&run).unwrap(),
        r#"{"status":"completed","retryable":false,"partial":false,"matched":1,"workflows":[{"id":"w","index":0,"selection":"ran","sweeps":1,"exit":"completed"}]}"#
    );
}

#[tokio::test]
async fn a_run_with_errors() {
    let e = engine(&[workflow("w", &[], vec![requires_ok("v")])]);
    let run = e.run(&mut message(json!({"ok": false}))).await;
    assert_eq!(
        json_of(&run),
        json!({"status": "completed_with_errors", "retryable": false, "partial": false,
               "matched": 1,
               "workflows": [{"id": "w", "index": 0, "selection": "ran",
                              "sweeps": 1, "exit": "completed"}]})
    );
}

#[tokio::test]
async fn a_stopped_run_carries_its_error() {
    let e = engine(&[
        workflow("w", &[], vec![fails("t")]),
        workflow("next", &[], vec![map("x", "data.x", json!(1))]),
    ]);
    let run = e.run(&mut message(json!({"s": "str"}))).await;
    let v = json_of(&run);
    let error = v["error"].clone();
    let dataflow_rs::run::RunStatus::Stopped(source) = run.status() else {
        panic!("{:?}", run.status());
    };
    assert_eq!(
        error,
        json!({"code": source.code(), "message": source.message(),
               "retryable": source.is_retryable()})
    );
    assert_eq!(
        v,
        json!({"status": "stopped", "error": error, "retryable": false, "partial": true,
               "matched": 1,
               "workflows": [{"id": "w", "index": 0, "selection": "ran",
                              "sweeps": 1, "exit": "stopped"}]})
    );
}

#[tokio::test]
async fn a_timed_out_and_a_cancelled_run() {
    // A loop checks the deadline and the token as each sweep starts.
    let e = engine(&[
        json!({"id": "w", "loop": {"counter": "i", "max": 100}, "tasks": [
        map("t", "data.a", json!(1))]}),
    ]);
    let past = Instant::now().checked_sub(Duration::from_secs(1)).unwrap();
    let run = e
        .run_with(
            &mut message(json!({})),
            &RunOptions::default().deadline(past),
        )
        .await;
    let v = json_of(&run);
    assert_eq!(v["status"], "timed_out");
    assert_eq!(
        (&v["retryable"], &v["partial"]),
        (&json!(true), &json!(true))
    );
    assert!(v.get("error").is_none());

    let token = CancelToken::new();
    token.cancel();
    let run = e
        .run_with(
            &mut message(json!({})),
            &RunOptions::default().cancel(&token),
        )
        .await;
    let v = json_of(&run);
    assert_eq!(v["status"], "cancelled");
    assert_eq!(
        (&v["retryable"], &v["partial"]),
        (&json!(false), &json!(true))
    );
    assert!(v.get("error").is_none());
}

#[tokio::test]
async fn a_condition_that_failed_carries_its_error() {
    let e = engine(&[workflow(
        "broken",
        &[("condition", json!({"/": [1, {"var": "data.zero"}]}))],
        vec![map("t", "data.a", json!(1))],
    )]);
    let run = e.run(&mut message(json!({"zero": 0}))).await;
    let v = json_of(&run);
    let w = &v["workflows"][0];
    assert_eq!(w["selection"], "condition_failed");
    let error = w["error"].clone();
    assert_eq!(error["code"], "LOGIC_ERROR");
    assert_eq!(error["retryable"], false);
    assert!(error["message"].is_string());
    assert_eq!(
        w,
        &json!({"id": "broken", "index": 0, "selection": "condition_failed",
                "error": error, "sweeps": 0, "exit": null})
    );
    assert_eq!(v["status"], "completed");
    assert_eq!(v["matched"], 0);
}

#[tokio::test]
async fn a_rollout_skip_and_a_condition_skip() {
    let rollout = json!({"bucket_start": 0, "bucket_end": 10});
    let e = engine(&[
        workflow(
            "b",
            &[("condition", json!(false))],
            vec![map("t", "data.b", json!(1))],
        ),
        workflow(
            "c",
            &[("rollout", rollout)],
            vec![map("t", "data.c", json!(1))],
        ),
    ]);
    let run = e
        .run_with(
            &mut message(json!({})),
            &RunOptions::default().routing_bucket(50),
        )
        .await;
    assert_eq!(
        json_of(&run),
        json!({"status": "completed", "retryable": false, "partial": false, "matched": 0,
               "workflows": [
                   {"id": "b", "index": 0, "selection": "skipped_by_condition",
                    "sweeps": 0, "exit": null},
                   {"id": "c", "index": 1, "selection": "skipped_by_rollout",
                    "sweeps": 0, "exit": null}]})
    );
}

#[tokio::test]
async fn a_loop_reports_its_sweeps() {
    let e = engine(&[
        json!({"id": "w", "loop": {"counter": "i", "max": 3}, "tasks": [
        map("m", "data.n", json!({"var": "temp_data.i"}))]}),
    ]);
    let run = e.run(&mut message(json!({}))).await;
    assert_eq!(
        json_of(&run)["workflows"],
        json!([{"id": "w", "index": 0, "selection": "ran", "sweeps": 3, "exit": "completed"}])
    );
}

#[tokio::test]
async fn a_halted_workflow_exits_halted() {
    let e = engine(&[workflow(
        "w",
        &[],
        vec![rejects("f", "halt"), map("t", "data.a", json!(1))],
    )]);
    let run = e.run(&mut message(json!({}))).await;
    assert_eq!(json_of(&run)["workflows"][0]["exit"], "halted");
}

#[test]
fn errors_serialize_as_code_message_and_retryable() {
    let e = Error::service("PARTNER_DOWN", "the partner is down").with_retryable(true);
    assert_eq!(
        serde_json::to_value(&e).unwrap(),
        json!({"code": "PARTNER_DOWN", "message": "the partner is down", "retryable": true})
    );
    assert_eq!(
        serde_json::to_string(&Error::service("X", "m")).unwrap(),
        r#"{"code":"X","message":"m","retryable":false}"#
    );
}
