//! Caller-owned tracing: `process_message_tracing` records into a trace the
//! caller already holds, so steps survive an early `Err`.

use dataflow_rs::engine::message::Message;
use dataflow_rs::{Engine, ExecutionStep, ExecutionTrace, Workflow};
use serde_json::json;

mod common;

use common::{FailingTask, dv};

// =============================================================================
// Caller-owned tracing — regression coverage for the dropped-trace defect
// =============================================================================
//
// `process_message_with_trace` returns the trace by value, so a `?` at the call
// site discards every step that already ran. `process_message_tracing` records
// into a caller-owned trace instead, so the steps survive the `Err`. These tests
// pin the retained steps on each of the three dispatch paths, plus the append
// contract and the metadata stamp.

/// Build a single-workflow engine from JSON with the `fail` handler registered.
fn tracing_engine(workflow_json: &str) -> Engine {
    Engine::builder()
        .with_workflow(Workflow::from_json(workflow_json).unwrap())
        .register("fail", FailingTask)
        .build()
        .unwrap()
}

/// `(workflow_id, task_id)` pairs for every step, in order.
fn step_ids(trace: &ExecutionTrace) -> Vec<(&str, Option<&str>)> {
    trace
        .steps
        .iter()
        .map(|s| (s.workflow_id.as_str(), s.task_id.as_deref()))
        .collect()
}

#[tokio::test]
async fn tracing_retains_steps_when_async_task_fails() {
    // Async-task failure path: the `map` runs in a sync stretch, then the
    // custom `fail` handler is dispatched at the async boundary and errors.
    let engine = tracing_engine(
        r#"{
        "id": "wf",
        "name": "wf",
        "condition": true,
        "tasks": [
            { "id": "step_ok", "name": "ok", "function": {
                "name": "map",
                "input": { "mappings": [ { "path": "data.a", "logic": 1 } ] } } },
            { "id": "step_boom", "name": "boom", "function": {
                "name": "fail", "input": {} } }
        ]
    }"#,
    );

    let mut message = Message::from_value(&json!({}));
    let mut trace = ExecutionTrace::new();
    let result = engine
        .process_message_tracing(&mut message, &mut trace)
        .await;

    assert!(result.is_err(), "the engine must still stop early");

    // The step that completed before the failure is retained. The failing
    // task's own step is not recorded — `handle_task_result` propagates before
    // `add_step` — so the trace ends at the last known-good step.
    assert_eq!(step_ids(&trace), vec![("wf", Some("step_ok"))]);
    assert_eq!(trace.executed_count(), 1);

    // The audit trail already survived the failure because it lives on
    // `&mut Message`; the trace now matches that guarantee.
    assert_eq!(message.audit_trail().len(), 2);
}

#[tokio::test]
async fn tracing_retains_steps_when_sync_stretch_fails() {
    // Sync-stretch path: both tasks are sync built-ins, so they share one
    // arena scope. `parse_xml` errors because its source is not a string.
    let engine = tracing_engine(
        r#"{
        "id": "wf",
        "name": "wf",
        "condition": true,
        "tasks": [
            { "id": "step_ok", "name": "ok", "function": {
                "name": "map",
                "input": { "mappings": [ { "path": "data.n", "logic": 7 } ] } } },
            { "id": "step_boom", "name": "boom", "function": {
                "name": "parse_xml",
                "input": { "source": "data.n", "target": "parsed" } } }
        ]
    }"#,
    );

    let mut message = Message::from_value(&json!({}));
    let mut trace = ExecutionTrace::new();
    let result = engine
        .process_message_tracing(&mut message, &mut trace)
        .await;

    assert!(result.is_err(), "parse_xml on a non-string must error");
    assert_eq!(step_ids(&trace), vec![("wf", Some("step_ok"))]);
}

#[tokio::test]
async fn tracing_retains_earlier_workflow_steps_across_a_shared_arena_run() {
    // Cross-workflow shared-arena path: workflow A is fully sync and succeeds,
    // workflow B fails. A never failed, so losing its steps is the most
    // surprising case of the old behaviour.
    let wf_a = Workflow::from_json(
        r#"{
        "id": "wf_a",
        "name": "wf_a",
        "priority": 0,
        "condition": true,
        "tasks": [
            { "id": "a_map", "name": "a_map", "function": {
                "name": "map",
                "input": { "mappings": [ { "path": "data.a", "logic": 1 } ] } } }
        ]
    }"#,
    )
    .unwrap();
    let wf_b = Workflow::from_json(
        r#"{
        "id": "wf_b",
        "name": "wf_b",
        "priority": 1,
        "condition": true,
        "tasks": [
            { "id": "b_map", "name": "b_map", "function": {
                "name": "map",
                "input": { "mappings": [ { "path": "data.b", "logic": 2 } ] } } },
            { "id": "b_boom", "name": "b_boom", "function": {
                "name": "parse_xml",
                "input": { "source": "data.b", "target": "parsed" } } }
        ]
    }"#,
    )
    .unwrap();

    let engine = Engine::builder()
        .with_workflows(vec![wf_a, wf_b])
        .build()
        .unwrap();

    let mut message = Message::from_value(&json!({}));
    let mut trace = ExecutionTrace::new();
    let result = engine
        .process_message_tracing(&mut message, &mut trace)
        .await;

    assert!(result.is_err());
    assert_eq!(
        step_ids(&trace),
        vec![("wf_a", Some("a_map")), ("wf_b", Some("b_map"))],
        "the successful workflow's steps must survive the later failure"
    );
}

#[tokio::test]
async fn tracing_retains_skipped_steps_before_a_failure() {
    // Both skip kinds are recorded before the failure: a workflow-level skip
    // (`workflow_skipped`, no task_id) and a task-level skip (`task_skipped`).
    let wf_skipped = Workflow::from_json(
        r#"{
        "id": "wf_skipped",
        "name": "wf_skipped",
        "priority": 0,
        "condition": { "==": [1, 2] },
        "tasks": [
            { "id": "never", "name": "never", "function": {
                "name": "map",
                "input": { "mappings": [ { "path": "data.never", "logic": 1 } ] } } }
        ]
    }"#,
    )
    .unwrap();
    let wf_main = Workflow::from_json(
        r#"{
        "id": "wf_main",
        "name": "wf_main",
        "priority": 1,
        "condition": true,
        "tasks": [
            { "id": "task_skipped", "name": "task_skipped",
              "condition": { "==": [1, 2] },
              "function": {
                "name": "map",
                "input": { "mappings": [ { "path": "data.skip", "logic": 1 } ] } } },
            { "id": "task_ok", "name": "task_ok", "function": {
                "name": "map",
                "input": { "mappings": [ { "path": "data.ok", "logic": 1 } ] } } },
            { "id": "task_boom", "name": "task_boom", "function": {
                "name": "fail", "input": {} } }
        ]
    }"#,
    )
    .unwrap();

    let engine = Engine::builder()
        .with_workflows(vec![wf_skipped, wf_main])
        .register("fail", FailingTask)
        .build()
        .unwrap();

    let mut message = Message::from_value(&json!({}));
    let mut trace = ExecutionTrace::new();
    let result = engine
        .process_message_tracing(&mut message, &mut trace)
        .await;

    assert!(result.is_err());
    assert_eq!(
        step_ids(&trace),
        vec![
            ("wf_skipped", None),
            ("wf_main", Some("task_skipped")),
            ("wf_main", Some("task_ok")),
        ]
    );
    assert_eq!(trace.skipped_count(), 2);
    assert_eq!(trace.executed_count(), 1);
}

#[tokio::test]
async fn tracing_retains_mapping_contexts_before_a_failure() {
    // Per-mapping snapshots are only populated in trace mode; they must survive
    // the failure along with the step that carries them.
    let engine = tracing_engine(
        r#"{
        "id": "wf",
        "name": "wf",
        "condition": true,
        "tasks": [
            { "id": "two_mappings", "name": "two_mappings", "function": {
                "name": "map",
                "input": { "mappings": [
                    { "path": "data.first", "logic": 1 },
                    { "path": "data.second", "logic": 2 }
                ] } } },
            { "id": "step_boom", "name": "boom", "function": {
                "name": "fail", "input": {} } }
        ]
    }"#,
    );

    let mut message = Message::from_value(&json!({}));
    let mut trace = ExecutionTrace::new();
    let result = engine
        .process_message_tracing(&mut message, &mut trace)
        .await;

    assert!(result.is_err());
    assert_eq!(trace.steps.len(), 1);
    let contexts = trace.steps[0]
        .mapping_contexts
        .as_ref()
        .expect("map task in trace mode must carry per-mapping snapshots");
    assert_eq!(contexts.len(), 2, "one snapshot per mapping");
}

#[tokio::test]
async fn tracing_appends_to_an_existing_trace() {
    // The documented contract is append, not clear: a caller can accumulate
    // steps across a chain of calls.
    let engine = tracing_engine(
        r#"{
        "id": "wf",
        "name": "wf",
        "condition": true,
        "tasks": [
            { "id": "step_ok", "name": "ok", "function": {
                "name": "map",
                "input": { "mappings": [ { "path": "data.a", "logic": 1 } ] } } },
            { "id": "step_boom", "name": "boom", "function": {
                "name": "fail", "input": {} } }
        ]
    }"#,
    );

    let mut trace = ExecutionTrace::new();
    trace.add_step(ExecutionStep::workflow_skipped("preexisting"));

    let mut message = Message::from_value(&json!({}));
    let result = engine
        .process_message_tracing(&mut message, &mut trace)
        .await;

    assert!(result.is_err());
    assert_eq!(
        step_ids(&trace),
        vec![("preexisting", None), ("wf", Some("step_ok"))],
        "the pre-existing step is kept and new steps are appended after it"
    );
}

#[tokio::test]
async fn tracing_stamps_processing_metadata_even_when_the_run_fails() {
    // A hand-rolled consumer workaround cannot stamp this, so it must not
    // regress on the failing path.
    let engine = tracing_engine(
        r#"{
        "id": "wf",
        "name": "wf",
        "condition": true,
        "tasks": [
            { "id": "step_boom", "name": "boom", "function": {
                "name": "fail", "input": {} } }
        ]
    }"#,
    );

    let mut message = Message::from_value(&json!({}));
    let mut trace = ExecutionTrace::new();
    let result = engine
        .process_message_tracing(&mut message, &mut trace)
        .await;

    assert!(result.is_err());
    let metadata = &message.context["metadata"];
    assert!(
        metadata.get("processed_at").is_some(),
        "processed_at must be stamped on a failing tracing run"
    );
    assert!(
        metadata.get("engine_version").is_some(),
        "engine_version must be stamped on a failing tracing run"
    );
}

#[tokio::test]
async fn channel_tracing_stamps_channel_metadata_and_retains_steps() {
    let wf = Workflow::from_json(
        r#"{
        "id": "wf_ch",
        "name": "wf_ch",
        "channel": "payments",
        "condition": true,
        "tasks": [
            { "id": "step_ok", "name": "ok", "function": {
                "name": "map",
                "input": { "mappings": [ { "path": "data.a", "logic": 1 } ] } } },
            { "id": "step_boom", "name": "boom", "function": {
                "name": "fail", "input": {} } }
        ]
    }"#,
    )
    .unwrap();

    let engine = Engine::builder()
        .with_workflow(wf)
        .register("fail", FailingTask)
        .build()
        .unwrap();

    let mut message = Message::from_value(&json!({}));
    let mut trace = ExecutionTrace::new();
    let result = engine
        .process_message_for_channel_tracing("payments", &mut message, &mut trace)
        .await;

    assert!(result.is_err());
    // The channel path must be fixed too, not only the non-channel path.
    assert_eq!(step_ids(&trace), vec![("wf_ch", Some("step_ok"))]);
    assert_eq!(
        message.context["metadata"]["channel"],
        dv(json!("payments"))
    );
}

#[tokio::test]
async fn channel_tracing_on_an_unknown_channel_is_a_noop() {
    let engine = tracing_engine(
        r#"{
        "id": "wf",
        "name": "wf",
        "condition": true,
        "tasks": [
            { "id": "step_ok", "name": "ok", "function": {
                "name": "map",
                "input": { "mappings": [ { "path": "data.a", "logic": 1 } ] } } }
        ]
    }"#,
    );

    let mut message = Message::from_value(&json!({}));
    let mut trace = ExecutionTrace::new();
    let result = engine
        .process_message_for_channel_tracing("nope", &mut message, &mut trace)
        .await;

    assert!(
        result.is_ok(),
        "an unknown channel is a no-op, not an error"
    );
    assert!(trace.steps.is_empty(), "the trace must be left untouched");
}

#[tokio::test]
async fn tracing_records_the_full_trace_on_a_filter_halt() {
    // A halt is `Ok`, so the trace already survived it. Pin it so the refactor
    // cannot change halt behaviour.
    let engine = tracing_engine(
        r#"{
        "id": "wf",
        "name": "wf",
        "condition": true,
        "tasks": [
            { "id": "step_ok", "name": "ok", "function": {
                "name": "map",
                "input": { "mappings": [ { "path": "data.a", "logic": 1 } ] } } },
            { "id": "gate", "name": "gate", "function": {
                "name": "filter", "input": { "condition": false } } },
            { "id": "never", "name": "never", "function": {
                "name": "map",
                "input": { "mappings": [ { "path": "data.never", "logic": 1 } ] } } }
        ]
    }"#,
    );

    let mut message = Message::from_value(&json!({}));
    let mut trace = ExecutionTrace::new();
    let result = engine
        .process_message_tracing(&mut message, &mut trace)
        .await;

    assert!(result.is_ok(), "a filter halt is not an error");
    assert_eq!(
        step_ids(&trace),
        vec![("wf", Some("step_ok")), ("wf", Some("gate"))],
        "the halting task is recorded; the task after it never runs"
    );
}

#[tokio::test]
async fn with_trace_wrappers_are_unchanged_by_the_tracing_refactor() {
    // Non-regression: on `Ok` the returned trace matches what the caller-owned
    // method records; on `Err` the wrapper still yields no trace at all.
    let ok_json = r#"{
        "id": "wf",
        "name": "wf",
        "condition": true,
        "tasks": [
            { "id": "step_ok", "name": "ok", "function": {
                "name": "map",
                "input": { "mappings": [ { "path": "data.a", "logic": 1 } ] } } }
        ]
    }"#;

    let engine = tracing_engine(ok_json);
    let mut message = Message::from_value(&json!({}));
    let returned = engine
        .process_message_with_trace(&mut message)
        .await
        .expect("a clean run still returns its trace");

    let mut borrowed = ExecutionTrace::new();
    let mut message2 = Message::from_value(&json!({}));
    engine
        .process_message_tracing(&mut message2, &mut borrowed)
        .await
        .unwrap();

    assert_eq!(step_ids(&returned), step_ids(&borrowed));
    assert_eq!(returned.executed_count(), borrowed.executed_count());

    // On `Err` the by-value wrapper behaves exactly as before: no trace.
    let failing = tracing_engine(
        r#"{
        "id": "wf",
        "name": "wf",
        "condition": true,
        "tasks": [
            { "id": "step_ok", "name": "ok", "function": {
                "name": "map",
                "input": { "mappings": [ { "path": "data.a", "logic": 1 } ] } } },
            { "id": "step_boom", "name": "boom", "function": {
                "name": "fail", "input": {} } }
        ]
    }"#,
    );
    let mut message3 = Message::from_value(&json!({}));
    assert!(
        failing
            .process_message_with_trace(&mut message3)
            .await
            .is_err(),
        "callers of the by-value method see no behaviour change"
    );
}
