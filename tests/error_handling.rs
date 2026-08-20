//! The single error channel — every failure reaches `message.errors()`, whether
//! or not `process_message` also returns `Err` — plus `DataflowError::Service`
//! classification.

use async_trait::async_trait;
use dataflow_rs::engine::functions::{AsyncFunctionHandler, FunctionConfig};
use dataflow_rs::engine::message::Message;
use dataflow_rs::{Engine, Result, Task, TaskContext, TaskOutcome, Workflow};
use serde_json::{Value, json};

mod common;

use common::{AddErrorTask, FailingTask, FivehundredTask, TimingOutTask, dv};

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

// =============================================================================
// Error-code classification on the live path
// =============================================================================

#[tokio::test]
async fn an_engine_owned_variant_records_its_own_code_not_a_blanket_task_error() {
    // Before 3.5.0 the live path ran every non-`Service` variant through a flat
    // `TASK_ERROR` fallback, so a timeout, a dropped connection and a rejected
    // request were indistinguishable to a workflow author — the variant->code
    // table existed but had no non-test caller. This pins that the table is on
    // the live path.
    let workflow = r#"{
        "id": "wf", "name": "W", "priority": 0, "condition": true,
        "tasks": [{
            "id": "slow", "name": "Slow", "continue_on_error": true,
            "function": { "name": "timeout", "input": {} }
        }]
    }"#;
    let engine = Engine::builder()
        .with_workflow(Workflow::from_json(workflow).unwrap())
        .register("timeout", TimingOutTask)
        .build()
        .unwrap();

    let mut message = Message::from_value(&json!({}));
    engine.process_message(&mut message).await.unwrap();

    assert_eq!(
        message.errors()[0].code,
        "TIMEOUT_ERROR",
        "the live path must classify by variant, not collapse to TASK_ERROR"
    );
}

// =============================================================================
// Error context path — per-task failure codes mirrored into the context
// =============================================================================

/// A workflow whose single `validation` task fails two rules and returns
/// `Status(400)`. That path pushes straight to `message.errors` and lands in
/// `handle_task_result`'s *success* arm, so it reaches neither failure arm.
fn failing_validation_workflow(id: &str, continue_on_error: bool) -> Workflow {
    Workflow::from_json(&format!(
        r#"{{
            "id": "{id}", "name": "V", "priority": 0, "condition": true,
            "continue_on_error": true,
            "tasks": [{{
                "id": "check", "name": "Check", "continue_on_error": {continue_on_error},
                "function": {{ "name": "validation", "input": {{ "rules": [
                    {{ "logic": false, "message": "first rule failed" }},
                    {{ "logic": false, "message": "second rule failed" }}
                ] }} }}
            }}]
        }}"#
    ))
    .unwrap()
}

/// The records written at `metadata.errors`, as plain JSON.
fn records(message: &Message) -> Vec<Value> {
    let ctx: Value = serde_json::to_value(&message.context).unwrap();
    ctx.get("metadata")
        .and_then(|m| m.get("errors"))
        .and_then(|e| e.as_array())
        .cloned()
        .unwrap_or_default()
}

#[tokio::test]
async fn off_by_default_writes_nothing() {
    let engine = Engine::builder()
        .with_workflow(failing_validation_workflow("wf", true))
        .build()
        .unwrap();

    let mut message = Message::from_value(&json!({}));
    engine.process_message(&mut message).await.unwrap();

    // The failure is on the message either way — only the context is untouched.
    assert_eq!(message.errors().len(), 2);
    let ctx: Value = serde_json::to_value(&message.context).unwrap();
    assert!(
        ctx["metadata"].get("errors").is_none(),
        "no path configured must mean no write at all, got {ctx}"
    );
}

#[tokio::test]
async fn nothing_is_written_when_no_task_fails() {
    // The key is *absent*, not an empty array: a clean message keeps the exact
    // wire shape it had before the option existed.
    let workflow = r#"{
        "id": "wf", "name": "W", "priority": 0, "condition": true,
        "tasks": [{
            "id": "ok", "name": "Ok",
            "function": { "name": "map", "input": { "mappings": [
                { "path": "data.ran", "logic": true }
            ] } }
        }]
    }"#;
    let engine = Engine::builder()
        .with_workflow(Workflow::from_json(workflow).unwrap())
        .with_error_context_path("metadata.errors")
        .build()
        .unwrap();

    let mut message = Message::from_value(&json!({}));
    engine.process_message(&mut message).await.unwrap();

    let ctx: Value = serde_json::to_value(&message.context).unwrap();
    assert!(
        ctx["metadata"].get("errors").is_none(),
        "a clean run must leave the key absent, not an empty array, got {ctx}"
    );
}

#[tokio::test]
async fn validation_failures_are_recorded_with_the_executors_own_ids_and_status() {
    // Three things at once: the sync stretch, the `Status(400)` path that
    // reaches neither failure arm, and that ids come from the executor rather
    // than the `ErrorInfo` (validation builds its entries with `simple_ref`,
    // whose `task_id` is `None`).
    let engine = Engine::builder()
        .with_workflow(failing_validation_workflow("wf", true))
        .with_error_context_path("metadata.errors")
        .build()
        .unwrap();

    let mut message = Message::from_value(&json!({}));
    engine.process_message(&mut message).await.unwrap();

    assert_eq!(
        records(&message),
        vec![
            json!({ "workflow_id": "wf", "task_id": "check",
                    "code": "VALIDATION_ERROR", "status": 400 }),
            json!({ "workflow_id": "wf", "task_id": "check",
                    "code": "VALIDATION_ERROR", "status": 400 }),
        ],
        "one record per failed rule, carrying the task's own 400"
    );
    assert!(
        message.errors()[0].task_id.is_none(),
        "guard: validation's own ErrorInfo has no task_id, so the record's must \
         come from the executor"
    );
}

#[tokio::test]
async fn a_later_task_in_the_same_sync_stretch_sees_the_record() {
    // The intra-stretch arena refresh. Both tasks are sync built-ins, so they
    // share one ArenaContext; without refreshing the configured path the second
    // task's condition reads a stale snapshot and never fires.
    let workflow = r#"{
        "id": "wf", "name": "W", "priority": 0, "condition": true,
        "tasks": [
            { "id": "check", "name": "Check", "continue_on_error": true,
              "function": { "name": "validation", "input": { "rules": [
                  { "logic": false, "message": "nope" }
              ] } } },
            { "id": "react", "name": "React",
              "condition": { "==": [ { "var": "metadata.errors.0.code" }, "VALIDATION_ERROR" ] },
              "function": { "name": "map", "input": { "mappings": [
                  { "path": "data.reacted", "logic": true }
              ] } } }
        ]
    }"#;
    let engine = Engine::builder()
        .with_workflow(Workflow::from_json(workflow).unwrap())
        .with_error_context_path("metadata.errors")
        .build()
        .unwrap();

    let mut message = Message::from_value(&json!({}));
    engine.process_message(&mut message).await.unwrap();

    assert_eq!(
        message.context["data"]["reacted"],
        dv(json!(true)),
        "the next task in the same stretch must see the record"
    );
}

#[tokio::test]
async fn a_downstream_workflow_branches_on_the_recorded_code() {
    // The issue's use case, end to end: three failure modes that used to be
    // indistinguishable now route differently. Both workflows are fully sync,
    // so they share one arena across the workflow boundary.
    let wf_a = failing_validation_workflow("wf_a", true);
    let wf_b = Workflow::from_json(
        r#"{
        "id": "wf_b", "name": "B", "priority": 1,
        "condition": { "in": [ { "var": "metadata.errors.0.code" },
                               ["VALIDATION_ERROR", "TIMEOUT_ERROR"] ] },
        "tasks": [{
            "id": "retry", "name": "Retry",
            "function": { "name": "map", "input": { "mappings": [
                { "path": "data.queued_for_retry", "logic": true }
            ] } }
        }]
    }"#,
    )
    .unwrap();

    let engine = Engine::builder()
        .with_workflows(vec![wf_a, wf_b])
        .with_error_context_path("metadata.errors")
        .build()
        .unwrap();

    let mut message = Message::from_value(&json!({}));
    engine.process_message(&mut message).await.unwrap();

    assert_eq!(
        message.context["data"]["queued_for_retry"],
        dv(json!(true)),
        "a downstream workflow must be able to gate on why the task failed"
    );
}

#[tokio::test]
async fn an_async_handler_error_is_recorded_with_status_500() {
    // The async call site, which has no live arena.
    let workflow = r#"{
        "id": "wf", "name": "W", "priority": 0, "condition": true,
        "tasks": [{
            "id": "boom", "name": "Boom", "continue_on_error": true,
            "function": { "name": "fail", "input": {} }
        }]
    }"#;
    let engine = Engine::builder()
        .with_workflow(Workflow::from_json(workflow).unwrap())
        .register("fail", FailingTask)
        .with_error_context_path("metadata.errors")
        .build()
        .unwrap();

    let mut message = Message::from_value(&json!({}));
    engine.process_message(&mut message).await.unwrap();

    assert_eq!(
        records(&message),
        vec![json!({ "workflow_id": "wf", "task_id": "boom",
                     "code": "TASK_ERROR", "status": 500 })],
    );
}

#[tokio::test]
async fn a_five_hundred_outcome_is_recorded_as_task_status_error() {
    let workflow = r#"{
        "id": "wf", "name": "W", "priority": 0, "condition": true,
        "tasks": [{
            "id": "five", "name": "Five", "continue_on_error": true,
            "function": { "name": "five", "input": {} }
        }]
    }"#;
    let engine = Engine::builder()
        .with_workflow(Workflow::from_json(workflow).unwrap())
        .register("five", FivehundredTask)
        .with_error_context_path("metadata.errors")
        .build()
        .unwrap();

    let mut message = Message::from_value(&json!({}));
    engine.process_message(&mut message).await.unwrap();

    assert_eq!(
        records(&message),
        vec![json!({ "workflow_id": "wf", "task_id": "five",
                     "code": "TASK_STATUS_ERROR", "status": 500 })],
    );
}

#[tokio::test]
async fn a_handler_recorded_error_carries_the_outcome_status() {
    // `TaskContext::add_error` on a task that *succeeds*. `status` is the task's
    // own outcome status, not a blanket 500 — the distinction
    // `metadata.progress` cannot make.
    let workflow = r#"{
        "id": "wf", "name": "W", "priority": 0, "condition": true,
        "tasks": [{
            "id": "noted", "name": "Noted",
            "function": { "name": "note", "input": {} }
        }]
    }"#;
    let engine = Engine::builder()
        .with_workflow(Workflow::from_json(workflow).unwrap())
        .register("note", AddErrorTask)
        .with_error_context_path("metadata.errors")
        .build()
        .unwrap();

    let mut message = Message::from_value(&json!({}));
    engine.process_message(&mut message).await.unwrap();

    assert_eq!(
        records(&message),
        vec![json!({ "workflow_id": "wf", "task_id": "noted",
                     "code": "CUSTOM_CODE", "status": 200 })],
    );
}

#[tokio::test]
async fn records_accumulate_across_workflows_and_exclude_the_workflow_wrapper() {
    let wf_a = failing_validation_workflow("wf_a", true);
    let wf_b = Workflow::from_json(
        r#"{
        "id": "wf_b", "name": "B", "priority": 1, "condition": true,
        "continue_on_error": true,
        "tasks": [{
            "id": "boom", "name": "Boom", "continue_on_error": false,
            "function": { "name": "fail", "input": {} }
        }]
    }"#,
    )
    .unwrap();

    let engine = Engine::builder()
        .with_workflows(vec![wf_a, wf_b])
        .register("fail", FailingTask)
        .with_error_context_path("metadata.errors")
        .build()
        .unwrap();

    let mut message = Message::from_value(&json!({}));
    engine.process_message(&mut message).await.unwrap();

    // Four entries on the message: two validation rules, the task error, and
    // the workflow wrapper. Only three records — the wrapper re-reports the
    // same underlying failure, so mirroring it would double-count.
    let codes: Vec<&str> = message.errors().iter().map(|e| e.code.as_str()).collect();
    assert_eq!(
        codes,
        vec![
            "VALIDATION_ERROR",
            "VALIDATION_ERROR",
            "TASK_ERROR",
            "WORKFLOW_ERROR"
        ]
    );
    assert_eq!(
        records(&message),
        vec![
            json!({ "workflow_id": "wf_a", "task_id": "check",
                    "code": "VALIDATION_ERROR", "status": 400 }),
            json!({ "workflow_id": "wf_a", "task_id": "check",
                    "code": "VALIDATION_ERROR", "status": 400 }),
            json!({ "workflow_id": "wf_b", "task_id": "boom",
                    "code": "TASK_ERROR", "status": 500 }),
        ],
        "each record carries its own workflow_id; WORKFLOW_ERROR is excluded"
    );
}

#[tokio::test]
async fn a_looping_workflow_records_one_entry_per_failing_sweep() {
    let workflow = r#"{
        "id": "wf", "name": "W", "priority": 0, "condition": true,
        "continue_on_error": true,
        "loop": { "counter": "i", "max": 3 },
        "tasks": [{
            "id": "boom", "name": "Boom", "continue_on_error": true,
            "function": { "name": "fail", "input": {} }
        }]
    }"#;
    let engine = Engine::builder()
        .with_workflow(Workflow::from_json(workflow).unwrap())
        .register("fail", FailingTask)
        .with_error_context_path("metadata.errors")
        .build()
        .unwrap();

    let mut message = Message::from_value(&json!({}));
    engine.process_message(&mut message).await.unwrap();

    assert_eq!(records(&message).len(), 3, "one record per sweep");
}

#[tokio::test]
async fn the_cap_keeps_the_most_recent_records() {
    let workflow = r#"{
        "id": "wf", "name": "W", "priority": 0, "condition": true,
        "continue_on_error": true,
        "loop": { "counter": "i", "max": 10 },
        "tasks": [{
            "id": "boom", "name": "Boom", "continue_on_error": true,
            "function": { "name": "fail", "input": {} }
        }]
    }"#;
    let engine = Engine::builder()
        .with_workflow(Workflow::from_json(workflow).unwrap())
        .register("fail", FailingTask)
        .with_error_context_path("metadata.errors")
        .with_error_context_limit(4)
        .build()
        .unwrap();

    let mut message = Message::from_value(&json!({}));
    engine.process_message(&mut message).await.unwrap();

    // Ten sweeps, four retained — the memory cost is bounded by the limit, not
    // by the loop's iteration count.
    assert_eq!(records(&message).len(), 4);
    assert_eq!(message.errors().len(), 10, "the message keeps all of them");
}

#[tokio::test]
async fn the_record_shape_is_exactly_four_keys_in_a_fixed_order() {
    // Wire-shape guard. The string assertion is load-bearing: `OwnedDataValue`'s
    // object equality is key-lookup based and therefore order-insensitive, so
    // comparing values alone would not pin the key order.
    let engine = Engine::builder()
        .with_workflow(failing_validation_workflow("wf", true))
        .with_error_context_path("metadata.errors")
        .build()
        .unwrap();

    let mut message = Message::from_value(&json!({}));
    engine.process_message(&mut message).await.unwrap();

    let ctx = serde_json::to_string(&message.context).unwrap();
    assert!(
        ctx.contains(
            r#"{"workflow_id":"wf","task_id":"check","code":"VALIDATION_ERROR","status":400}"#
        ),
        "record shape/order changed: {ctx}"
    );
}

#[tokio::test]
async fn the_operator_only_detail_never_reaches_the_context() {
    // `ErrorInfo::detail` is documented as unsafe to hand to an untrusted
    // caller, and `Message.context` is serialized straight back to callers.
    let engine = Engine::builder()
        .with_workflow(service_workflow(true))
        .register("svc_fail", ServiceFailingTask)
        .with_error_context_path("metadata.errors")
        .build()
        .unwrap();

    let mut message = Message::from_value(&json!({}));
    engine.process_message(&mut message).await.unwrap();

    let ctx = serde_json::to_string(&message.context).unwrap();
    assert!(
        ctx.contains(r#""code":"circuit_open""#),
        "the service kind is the code: {ctx}"
    );
    assert!(
        !ctx.contains("detail") && !ctx.contains("breaker open"),
        "operator-only detail must not reach the context: {ctx}"
    );
    assert!(
        !ctx.contains("upstream unavailable"),
        "the error message is excluded too: {ctx}"
    );
}

#[tokio::test]
async fn a_non_array_already_at_the_path_is_replaced() {
    let workflow = r#"{
        "id": "wf", "name": "W", "priority": 0, "condition": true,
        "tasks": [
            { "id": "squat", "name": "Squat",
              "function": { "name": "map", "input": { "mappings": [
                  { "path": "metadata.errors", "logic": "oops" }
              ] } } },
            { "id": "check", "name": "Check", "continue_on_error": true,
              "function": { "name": "validation", "input": { "rules": [
                  { "logic": false, "message": "nope" }
              ] } } }
        ]
    }"#;
    let engine = Engine::builder()
        .with_workflow(Workflow::from_json(workflow).unwrap())
        .with_error_context_path("metadata.errors")
        .build()
        .unwrap();

    let mut message = Message::from_value(&json!({}));
    engine.process_message(&mut message).await.unwrap();

    assert_eq!(
        records(&message),
        vec![json!({ "workflow_id": "wf", "task_id": "check",
                     "code": "VALIDATION_ERROR", "status": 400 })],
        "the engine owns the configured path; a squatting scalar is replaced \
         rather than silently swallowing the records"
    );
}

#[tokio::test]
async fn the_option_survives_a_hot_reload_and_a_with_observer_rebuild() {
    // Both rebuild the executor from scratch. `with_observer` is the easy one
    // to miss: it is applied *after* the error context inside `build()`.
    let engine = Engine::builder()
        .with_workflow(failing_validation_workflow("wf", true))
        .with_error_context_path("metadata.errors")
        .with_observer(std::sync::Arc::new(common::RecordingObserver::default()))
        .build()
        .unwrap();

    let mut message = Message::from_value(&json!({}));
    engine.process_message(&mut message).await.unwrap();
    assert_eq!(
        records(&message).len(),
        2,
        "with_observer must not drop the error context path"
    );

    let reloaded = engine
        .with_new_workflows(vec![failing_validation_workflow("wf2", true)])
        .unwrap();
    let mut message = Message::from_value(&json!({}));
    reloaded.process_message(&mut message).await.unwrap();
    assert_eq!(
        records(&message).len(),
        2,
        "a hot reload must not drop the error context path"
    );
}

#[test]
fn build_rejects_a_path_the_eval_context_cannot_see() {
    for bad in [
        "",
        "errors",
        "payload.errors",
        "metadata.",
        "metadata.progress",
    ] {
        let result = Engine::builder()
            .with_workflow(failing_validation_workflow("wf", true))
            .with_error_context_path(bad)
            .build();
        assert!(
            result.is_err(),
            "{bad:?} must be rejected at build, not silently write nowhere"
        );
    }

    // A valid path still builds, and a zero limit does not.
    assert!(
        Engine::builder()
            .with_workflow(failing_validation_workflow("wf", true))
            .with_error_context_path("temp_data.failures")
            .build()
            .is_ok()
    );
    assert!(
        Engine::builder()
            .with_workflow(failing_validation_workflow("wf", true))
            .with_error_context_path("metadata.errors")
            .with_error_context_limit(0)
            .build()
            .is_err()
    );
}
