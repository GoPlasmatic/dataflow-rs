//! The single error channel — every failure reaches `message.errors()`, whether
//! or not `process_message` also returns `Err` — plus `DataflowError::Service`
//! classification.

use async_trait::async_trait;
use dataflow_rs::engine::functions::{AsyncFunctionHandler, FunctionConfig};
use dataflow_rs::engine::message::Message;
use dataflow_rs::{Engine, Result, Task, TaskContext, TaskOutcome, Workflow};
use serde_json::{Value, json};

mod common;

use common::{FailingTask, FivehundredTask, dv};

// =============================================================================
// Single error channel — regression coverage
// =============================================================================
//
// `process_message` now always pushes errors to `message.errors()`, even when
// it returns `Result::Err`. The `Err` only signals "the engine stopped early";
// the `errors` list is the unified channel.

#[tokio::test]
async fn task_err_with_continue_on_error_false_pushes_wrapper_to_errors() {
    let workflow = Workflow {
        id: "fail_workflow".to_string(),
        id_arc: std::sync::Arc::from("fail_workflow"),
        name: "Fail Workflow".to_string(),
        priority: 0,
        description: None,
        tasks: vec![Task {
            id: "boom".to_string(),
            id_arc: std::sync::Arc::from("boom"),
            name: "Boom".to_string(),
            description: None,
            condition: json!(true),
            compiled_condition: None,
            continue_on_error: false,
            function: FunctionConfig::Custom {
                name: "fail".to_string(),
                input: json!({}),
                compiled_input: None,
            },
        }],
        condition: json!(true),
        compiled_condition: None,
        continue_on_error: false,
        ..Default::default()
    };

    let engine = Engine::builder()
        .with_workflow(workflow)
        .register("fail", FailingTask)
        .build()
        .unwrap();

    let mut message = Message::from_value(&json!({}));
    let result = engine.process_message(&mut message).await;

    // `Err` channel — engine stopped early.
    assert!(result.is_err(), "process_message should bubble the error");

    // `message.errors` channel — both the task error and the workflow
    // wrapper are recorded, so callers reading `errors()` see the failure
    // even without inspecting `Result`.
    let codes: Vec<&str> = message.errors().iter().map(|e| e.code.as_str()).collect();
    assert!(
        codes.contains(&"TASK_ERROR"),
        "expected TASK_ERROR in {codes:?}"
    );
    assert!(
        codes.contains(&"WORKFLOW_ERROR"),
        "expected WORKFLOW_ERROR in {codes:?}"
    );
}

#[tokio::test]
async fn task_status_500_pushes_status_error_to_message() {
    let workflow = Workflow {
        id: "five_hundred".to_string(),
        id_arc: std::sync::Arc::from("five_hundred"),
        name: "Five Hundred".to_string(),
        priority: 0,
        description: None,
        tasks: vec![Task {
            id: "task_500".to_string(),
            id_arc: std::sync::Arc::from("task_500"),
            name: "Task 500".to_string(),
            description: None,
            condition: json!(true),
            compiled_condition: None,
            // Continue past the 500 so we can assert on the *push*
            // independently of the `Result::Err` path.
            continue_on_error: true,
            function: FunctionConfig::Custom {
                name: "five_hundred".to_string(),
                input: json!({}),
                compiled_input: None,
            },
        }],
        condition: json!(true),
        compiled_condition: None,
        continue_on_error: true,
        ..Default::default()
    };

    let engine = Engine::builder()
        .with_workflow(workflow)
        .register("five_hundred", FivehundredTask)
        .build()
        .unwrap();

    let mut message = Message::from_value(&json!({}));
    let result = engine.process_message(&mut message).await;
    assert!(result.is_ok(), "continue_on_error keeps the Result Ok");

    let codes: Vec<&str> = message.errors().iter().map(|e| e.code.as_str()).collect();
    assert!(
        codes.contains(&"TASK_STATUS_ERROR"),
        "expected TASK_STATUS_ERROR in {codes:?}"
    );
    assert_eq!(message.audit_trail().len(), 1);
    assert_eq!(message.audit_trail()[0].status, 500);
}

#[tokio::test]
async fn metadata_progress_is_written_even_when_a_task_errors() {
    // `metadata.progress` is documented as written "after every task", never
    // conditionally — a downstream workflow gates on it to chain forward. A
    // task that returns `Err` (not just a 500 status) with
    // `continue_on_error: true` must still advance it, or a later workflow
    // gating on `metadata.progress.task_id` never sees that the failing task
    // ran at all.
    let wf_a = r#"{
        "id": "wf_a", "name": "A", "priority": 0, "condition": true,
        "tasks": [{
            "id": "boom", "name": "Boom", "continue_on_error": true,
            "function": { "name": "fail", "input": {} }
        }]
    }"#;
    let wf_b = r#"{
        "id": "wf_b", "name": "B", "priority": 1,
        "condition": { "==": [ { "var": "metadata.progress.task_id" }, "boom" ] },
        "tasks": [{
            "id": "map_b", "name": "B",
            "function": { "name": "map", "input": { "mappings": [ { "path": "data.b_ran", "logic": true } ] } }
        }]
    }"#;

    let workflows = vec![
        Workflow::from_json(wf_a).unwrap(),
        Workflow::from_json(wf_b).unwrap(),
    ];
    let engine = Engine::builder()
        .with_workflows(workflows)
        .register("fail", FailingTask)
        .build()
        .unwrap();

    let mut message = Message::from_value(&json!({}));
    let trace = engine
        .process_message_with_trace(&mut message)
        .await
        .unwrap();

    // The step for the failing task itself must already show the write —
    // its own message snapshot is captured right after `handle_task_result`.
    let boom_step = trace
        .steps
        .iter()
        .find(|s| s.task_id.as_deref() == Some("boom"))
        .expect("boom task should have an executed step (continue_on_error: true)");
    let boom_snapshot = boom_step.message.as_ref().expect("step carries a snapshot");
    assert_eq!(
        boom_snapshot.context["metadata"]["progress"]["task_id"],
        dv(json!("boom")),
        "metadata.progress must name the failing task right after it ran, not be left stale or absent"
    );
    assert_eq!(
        boom_snapshot.context["metadata"]["progress"]["status_code"],
        dv(json!(500))
    );

    // End-to-end proof: wf_b's condition gates on that same write and must
    // still see it and run, even though wf_a's task errored.
    assert_eq!(
        message.context["data"]["b_ran"],
        dv(json!(true)),
        "wf_b gates on wf_a's progress write and must still run"
    );
}

// =============================================================================
// DataflowError::Service — handler-owned error classification
// =============================================================================

/// Returns a service-classified error with operator-only detail.
struct ServiceFailingTask;

#[async_trait]
impl AsyncFunctionHandler for ServiceFailingTask {
    type Input = Value;

    async fn execute(&self, _ctx: &mut TaskContext<'_>, _input: &Value) -> Result<TaskOutcome> {
        Err(
            dataflow_rs::DataflowError::service("circuit_open", "upstream unavailable")
                .detail("connector 'billing' breaker open since 12:04")
                .retryable(true)
                .build(),
        )
    }
}

fn service_workflow(continue_on_error: bool) -> Workflow {
    Workflow::from_json(&format!(
        r#"{{ "id": "svc", "name": "svc", "priority": 0, "condition": true,
              "continue_on_error": {continue_on_error},
              "tasks": [ {{ "id": "boom", "name": "boom",
                            "continue_on_error": {continue_on_error},
                            "function": {{ "name": "svc_fail", "input": {{}} }} }} ] }}"#
    ))
    .unwrap()
}

#[tokio::test]
async fn a_service_error_lifts_its_kind_and_detail_onto_the_message() {
    let engine = Engine::builder()
        .with_workflow(service_workflow(false))
        .register("svc_fail", ServiceFailingTask)
        .build()
        .unwrap();

    let mut message = Message::from_value(&json!({}));
    assert!(engine.process_message(&mut message).await.is_err());

    // Assert the FULL code vec, so the decision to lift at the task site only —
    // and therefore to keep WORKFLOW_ERROR meaning "a workflow stopped" — is
    // part of the contract rather than incidental.
    let codes: Vec<&str> = message.errors().iter().map(|e| e.code.as_str()).collect();
    assert_eq!(codes, vec!["circuit_open", "WORKFLOW_ERROR"]);

    let task_err = &message.errors()[0];
    assert!(
        task_err.message.contains("upstream unavailable"),
        "the caller-safe text is carried, got: {}",
        task_err.message
    );
    assert!(
        !task_err.message.contains("breaker open"),
        "the operator-only detail must not leak into `message`, got: {}",
        task_err.message
    );
    assert_eq!(
        task_err.detail.as_deref(),
        Some("connector 'billing' breaker open since 12:04"),
        "the detail rides its own field"
    );
}

#[tokio::test]
async fn a_service_error_respects_continue_on_error_like_any_other() {
    // Control flow is untouched: `continue_on_error` still governs.
    let engine = Engine::builder()
        .with_workflow(service_workflow(true))
        .register("svc_fail", ServiceFailingTask)
        .build()
        .unwrap();

    let mut message = Message::from_value(&json!({}));
    engine
        .process_message(&mut message)
        .await
        .expect("continue_on_error: true still yields Ok");
    assert!(message.has_errors());
    assert_eq!(message.errors()[0].code, "circuit_open");
}

#[test]
fn the_service_builder_is_reachable_from_an_external_crate() {
    // `tests/` is a separate crate, so this proves the public path — including
    // that `ServiceErrorBuilder` is nameable at the crate root.
    let builder: dataflow_rs::ServiceErrorBuilder =
        dataflow_rs::DataflowError::service("rate_limited", "too many requests");
    let e = builder
        .detail("token bucket empty for tenant 42")
        .retryable(true)
        .build();

    assert_eq!(e.kind(), Some("rate_limited"));
    assert_eq!(e.detail(), Some("token bucket empty for tenant 42"));
    assert!(e.retryable());
    assert_eq!(e.to_string(), "too many requests");
    assert!(!e.to_string().contains("token bucket"));
}
