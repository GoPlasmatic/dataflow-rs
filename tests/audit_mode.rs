//! `AuditMode` (#68): how many audit entries a message keeps. The mode changes
//! only what is kept; control flow, errors, progress and the context come out
//! the same in every mode.

use dataflow_rs::engine::message::Message;
use dataflow_rs::{AuditMode, Engine, TraceOptions, Workflow};
use serde_json::{Value, json};

mod common;

use common::FivehundredTask;

/// A ten-sweep loop whose body writes, fails validation (400), and returns a
/// 500 under `continue_on_error`: three entries per sweep, with error records.
fn engine() -> Engine {
    let workflow = Workflow::from_json(
        &json!({"id": "w", "name": "w", "loop": {"counter": "i", "max": 10}, "tasks": [
            {"id": "write", "name": "write", "function": {"name": "map", "input": {"mappings": [
                {"path": "data.last", "logic": {"var": "temp_data.i"}}]}}},
            {"id": "check", "name": "check", "function": {"name": "validation", "input": {"rules": [
                {"logic": false, "message": "always"}]}}},
            {"id": "boom", "name": "boom", "continue_on_error": true,
             "function": {"name": "five_hundred", "input": {}}}
        ]})
        .to_string(),
    )
    .unwrap();
    Engine::builder()
        .with_workflow(workflow)
        .register("five_hundred", FivehundredTask)
        .build()
        .unwrap()
}

async fn run(mode: AuditMode) -> Message {
    let mut m = Message::builder().audit_mode(mode).build();
    engine().process_message(&mut m).await.unwrap();
    m
}

fn entries(m: &Message) -> Vec<(String, usize, Option<i64>)> {
    m.audit_trail()
        .iter()
        .map(|e| (e.task_id.to_string(), e.status, e.loop_counter))
        .collect()
}

#[tokio::test]
async fn full_keeps_every_entry_and_is_the_default() {
    assert_eq!(Message::builder().build().audit_mode(), AuditMode::Full);
    assert_eq!(
        Message::from_value(&json!({})).audit_mode(),
        AuditMode::Full
    );
    let m = run(AuditMode::Full).await;
    assert_eq!(m.audit_trail().len(), 30);
}

#[tokio::test]
async fn last_keeps_the_most_recent_entries_in_order() {
    let full = run(AuditMode::Full).await;
    for n in [1, 4, 7, 30, 100] {
        let last = run(AuditMode::Last(n)).await;
        let expected = entries(&full);
        let expected = &expected[expected.len().saturating_sub(n)..];
        assert_eq!(entries(&last), expected, "Last({n})");
    }
}

#[tokio::test]
async fn off_and_last_zero_keep_nothing() {
    assert!(run(AuditMode::Off).await.audit_trail().is_empty());
    assert!(run(AuditMode::Last(0)).await.audit_trail().is_empty());
}

#[tokio::test]
async fn the_mode_changes_nothing_but_the_trail() {
    let full = run(AuditMode::Full).await;
    for mode in [AuditMode::Last(2), AuditMode::Off] {
        let m = run(mode).await;
        for root in ["data", "temp_data"] {
            assert_eq!(m.context[root], full.context[root], "{mode:?}: {root}");
        }
        assert_eq!(
            m.context["metadata"]["progress"], full.context["metadata"]["progress"],
            "{mode:?}: progress"
        );
        let codes =
            |m: &Message| -> Vec<String> { m.errors().iter().map(|e| e.code.clone()).collect() };
        assert_eq!(codes(&m), codes(&full), "{mode:?}: error records");
    }
    assert_eq!(
        Value::from(&full.context["metadata"]["progress"]),
        json!({"workflow_id": "w", "task_id": "boom", "status_code": 500})
    );
}

#[tokio::test]
async fn the_wire_shape_carries_only_the_kept_entries() {
    let m = run(AuditMode::Last(2)).await;
    let wire = serde_json::to_value(&m).unwrap();
    let tasks: Vec<&str> = wire["audit_trail"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["task_id"].as_str().unwrap())
        .collect();
    assert_eq!(tasks, ["check", "boom"]);
}

#[tokio::test]
async fn a_trace_under_off_keeps_its_steps_but_reports_no_diff() {
    let options = TraceOptions {
        changes: true,
        ..TraceOptions::default()
    };
    let mut m = Message::builder().audit_mode(AuditMode::Off).build();
    let trace = engine()
        .process_message_with_trace_options(&mut m, options.clone())
        .await
        .unwrap();
    assert_eq!(trace.executed_count(), 30);
    assert!(
        trace
            .steps
            .iter()
            .all(|s| s.changes.as_ref().is_some_and(Vec::is_empty))
    );

    // The same run under Full does carry the map writes.
    let mut m = Message::builder().build();
    let trace = engine()
        .process_message_with_trace_options(&mut m, options)
        .await
        .unwrap();
    assert!(
        trace
            .steps
            .iter()
            .any(|s| s.changes.as_ref().is_some_and(|c| !c.is_empty()))
    );
}
