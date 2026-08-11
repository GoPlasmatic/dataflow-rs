//! `TraceOptions` — per-step timing, diffs, snapshot budget, audit scope, and
//! redaction.

use dataflow_rs::engine::message::Message;
use dataflow_rs::engine::utils::set_nested_value;
use dataflow_rs::{Engine, ExecutionTrace, TraceOptions, Workflow};
use serde_json::json;

mod common;

use common::{LoggingTask, dv, n_map_task_workflow};

// =============================================================================
// TraceOptions — timing, per-step diff, snapshot budget, redaction
// =============================================================================

#[tokio::test]
async fn executed_steps_carry_timing_and_skipped_steps_do_not() {
    // Mixes a sync built-in stretch with a registered async handler so both
    // ExecutionStep sites are covered.
    let wf = Workflow::from_json(
        r#"{
        "id": "w", "name": "w", "priority": 0, "condition": true,
        "tasks": [
            { "id": "sync_map", "name": "sync_map", "function": {
                "name": "map",
                "input": { "mappings": [ { "path": "data.a", "logic": 1 } ] } } },
            { "id": "skipped", "name": "skipped",
              "condition": { "==": [1, 2] },
              "function": {
                "name": "map",
                "input": { "mappings": [ { "path": "data.never", "logic": 1 } ] } } },
            { "id": "async_task", "name": "async_task", "function": {
                "name": "logger", "input": {} } }
        ]
    }"#,
    )
    .unwrap();

    let engine = Engine::builder()
        .with_workflow(wf)
        .register("logger", LoggingTask)
        .build()
        .unwrap();

    let mut message = Message::from_value(&json!({}));
    let trace = engine
        .process_message_with_trace(&mut message)
        .await
        .unwrap();

    for step in &trace.steps {
        let id = step.task_id.as_deref().unwrap_or("<workflow>");
        match step.result {
            dataflow_rs::StepResult::Executed => {
                assert!(
                    step.started_at.is_some() && step.duration_us.is_some(),
                    "executed step '{id}' must carry timing"
                );
            }
            dataflow_rs::StepResult::Skipped => {
                assert!(
                    step.started_at.is_none() && step.duration_us.is_none(),
                    "skipped step '{id}' must not carry timing"
                );
            }
        }
    }

    // Both dispatch sites produced a timed step.
    let timed: Vec<&str> = trace
        .steps
        .iter()
        .filter(|s| s.duration_us.is_some())
        .map(|s| s.task_id.as_deref().unwrap())
        .collect();
    assert_eq!(timed, vec!["sync_map", "async_task"]);
}

#[tokio::test]
async fn the_non_trace_path_still_takes_one_clock_read_per_message() {
    // Trace mode adds clock reads per task; process_message must not.
    let engine = Engine::builder()
        .with_workflow(n_map_task_workflow(4))
        .build()
        .unwrap();

    let mut message = Message::from_value(&json!({}));
    engine.process_message(&mut message).await.unwrap();

    let stamps: Vec<_> = message.audit_trail().iter().map(|a| a.timestamp).collect();
    assert_eq!(stamps.len(), 4);
    assert!(
        stamps.windows(2).all(|w| w[0] == w[1]),
        "all audit timestamps share the single per-message Utc::now()"
    );
}

#[tokio::test]
async fn skip_step_reports_its_own_empty_diff_not_the_previous_tasks() {
    // The reported mis-attribution, asserted fixed. `filter` with
    // on_reject: "skip" returns TaskOutcome::Skip, which records no audit entry.
    let wf = Workflow::from_json(
        r#"{
        "id": "skip_attribution", "name": "Skip Attribution", "condition": true,
        "tasks": [
            { "id": "write_a", "name": "Write A", "function": {
                "name": "map",
                "input": { "mappings": [ { "path": "data.a", "logic": 1 } ] } } },
            { "id": "gate", "name": "Gate", "function": {
                "name": "filter",
                "input": { "condition": false, "on_reject": "skip" } } }
        ]
    }"#,
    )
    .unwrap();

    let engine = Engine::builder().with_workflow(wf).build().unwrap();
    let mut message = Message::from_value(&json!({}));
    let trace = engine
        .process_message_with_trace_options(
            &mut message,
            TraceOptions {
                changes: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();

    assert_eq!(trace.steps.len(), 2);
    assert_eq!(trace.steps[1].task_id.as_deref(), Some("gate"));

    // The write belongs to write_a...
    let a_changes = trace.steps[0].changes.as_ref().unwrap();
    assert_eq!(a_changes.len(), 1);
    assert_eq!(&*a_changes[0].path, "data.a");

    // ...and the Skip step reports its own empty diff, not write_a's.
    assert!(
        trace.steps[1].changes.as_ref().unwrap().is_empty(),
        "a Skip step must not inherit the previous task's changes"
    );

    // The old heuristic — audit_trail.last() on the step's own snapshot — is
    // what mis-attributed; confirm the trap is still there so the fix matters.
    let snapshot = trace.steps[1].message.as_ref().unwrap();
    assert_eq!(
        snapshot.audit_trail().last().unwrap().task_id.as_ref(),
        "write_a"
    );
}

#[tokio::test]
async fn changes_flag_reports_the_diff_but_does_not_enable_capture() {
    let engine = Engine::builder()
        .with_workflow(n_map_task_workflow(2))
        .build()
        .unwrap();

    let mut message = dataflow_rs::MessageBuilder::new()
        .payload_json(&json!({}))
        .capture_changes(false)
        .build();

    let trace = engine
        .process_message_with_trace_options(
            &mut message,
            TraceOptions {
                changes: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();

    for step in &trace.steps {
        assert!(
            step.changes.as_ref().unwrap().is_empty(),
            "capture_changes(false) means there is no diff to report"
        );
    }
}

#[tokio::test]
async fn changes_default_off_is_absent_from_the_serialized_step() {
    let engine = Engine::builder()
        .with_workflow(n_map_task_workflow(1))
        .build()
        .unwrap();
    let mut message = Message::from_value(&json!({}));
    let trace = engine
        .process_message_with_trace(&mut message)
        .await
        .unwrap();

    assert!(trace.steps[0].changes.is_none());
    let serialized = serde_json::to_value(&trace.steps[0]).unwrap();
    assert!(serialized.get("changes").is_none());
}

#[tokio::test]
async fn timings_only_drops_snapshots_and_degrades_the_accessors() {
    let engine = Engine::builder()
        .with_workflow(n_map_task_workflow(3))
        .build()
        .unwrap();
    let mut message = Message::from_value(&json!({}));
    let trace = engine
        .process_message_with_trace_options(&mut message, TraceOptions::timings_only())
        .await
        .unwrap();

    assert_eq!(trace.executed_count(), 3);
    for step in &trace.steps {
        assert!(step.message.is_none(), "no snapshots under timings_only");
        assert!(step.mapping_contexts.is_none());
        assert!(step.duration_us.is_some(), "timing survives");
    }
    assert!(trace.final_message().is_none());
    assert!(trace.is_success(), "documented to degenerate to true");
    // Nothing was captured, so nothing was truncated.
    assert!(!trace.truncated());
}

#[tokio::test]
async fn a_snapshot_budget_truncates_later_steps_and_does_not_oscillate() {
    let engine = Engine::builder()
        .with_workflow(n_map_task_workflow(6))
        .build()
        .unwrap();
    let mut message = Message::from_value(&json!({}));
    set_nested_value(
        &mut message.context,
        "data.blob",
        dv(json!("x".repeat(4096))),
    );

    let trace = engine
        .process_message_with_trace_options(
            &mut message,
            TraceOptions {
                max_snapshot_bytes: 8192,
                ..Default::default()
            },
        )
        .await
        .unwrap();

    assert!(trace.truncated(), "the budget must be reported as exceeded");
    assert_eq!(trace.executed_count(), 6, "every step is still recorded");

    let with_snapshot: Vec<bool> = trace.steps.iter().map(|s| s.message.is_some()).collect();
    assert!(with_snapshot[0], "the first step is captured");
    assert!(
        !with_snapshot[with_snapshot.len() - 1],
        "later steps drop their snapshot"
    );
    // Monotone: once truncation starts it never recovers.
    let first_dropped = with_snapshot.iter().position(|c| !c).unwrap();
    assert!(
        with_snapshot[first_dropped..].iter().all(|c| !c),
        "no oscillation back to captured: {with_snapshot:?}"
    );

    // Timing survives truncation.
    assert!(trace.steps.last().unwrap().duration_us.is_some());
}

#[tokio::test]
async fn a_budget_smaller_than_the_first_snapshot_truncates_from_the_start() {
    let engine = Engine::builder()
        .with_workflow(n_map_task_workflow(3))
        .build()
        .unwrap();
    let mut message = Message::from_value(&json!({}));
    set_nested_value(
        &mut message.context,
        "data.blob",
        dv(json!("y".repeat(8192))),
    );

    let trace = engine
        .process_message_with_trace_options(
            &mut message,
            TraceOptions {
                max_snapshot_bytes: 1,
                ..Default::default()
            },
        )
        .await
        .unwrap();

    assert!(trace.truncated());
    assert!(
        trace.steps.iter().all(|s| s.message.is_none()),
        "no step recovers a snapshot"
    );
}

#[tokio::test]
async fn an_unbounded_budget_never_truncates() {
    let engine = Engine::builder()
        .with_workflow(n_map_task_workflow(6))
        .build()
        .unwrap();
    let mut message = Message::from_value(&json!({}));
    set_nested_value(
        &mut message.context,
        "data.blob",
        dv(json!("z".repeat(65536))),
    );
    let trace = engine
        .process_message_with_trace(&mut message)
        .await
        .unwrap();

    assert_eq!(trace.options().max_snapshot_bytes, 0);
    assert!(!trace.truncated());
    assert!(trace.steps.iter().all(|s| s.message.is_some()));
}

#[tokio::test]
async fn audit_trail_scope_controls_the_quadratic_term() {
    let n = 6usize;

    let total_audit_entries = |trace: &ExecutionTrace| -> usize {
        trace
            .steps
            .iter()
            .filter_map(|s| s.message.as_ref())
            .map(|m| m.audit_trail().len())
            .sum()
    };

    let engine = Engine::builder()
        .with_workflow(n_map_task_workflow(n))
        .build()
        .unwrap();

    // Full — the historical behaviour, N*(N+1)/2.
    let mut m1 = Message::from_value(&json!({}));
    let full = engine.process_message_with_trace(&mut m1).await.unwrap();
    assert_eq!(total_audit_entries(&full), n * (n + 1) / 2);

    // Own — linear, one per executed non-Skip step.
    let mut m2 = Message::from_value(&json!({}));
    let own = engine
        .process_message_with_trace_options(
            &mut m2,
            TraceOptions {
                snapshot_audit_trail: dataflow_rs::AuditTrailScope::Own,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(total_audit_entries(&own), n);

    // None — empty in every snapshot.
    let mut m3 = Message::from_value(&json!({}));
    let none = engine
        .process_message_with_trace_options(
            &mut m3,
            TraceOptions {
                snapshot_audit_trail: dataflow_rs::AuditTrailScope::None,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(total_audit_entries(&none), 0);
    // Snapshots are still present — only the audit term was dropped.
    assert!(none.steps.iter().all(|s| s.message.is_some()));
}

#[tokio::test]
async fn own_scope_does_not_leak_across_workflows_sharing_a_task_id() {
    // Two workflows reuse the same task id ("step1"). wf_a's step1 runs and
    // writes data.a; wf_b's step1 is skipped via `filter`. Regression for a
    // bug where the per-step diff (`changes: true` / `AuditTrailScope::Own`)
    // matched the "this task's own audit entry" lookup on `task_id` alone —
    // since wf_b's Skip pushes no entry, `audit_trail.last()` was still
    // wf_a's, and the matching task id made it look like a match. The skipped
    // step in wf_b must report an empty diff, not wf_a's `data.a` change.
    let wf_a = r#"{
        "id": "wf_a", "name": "A", "priority": 0, "condition": true,
        "tasks": [{
            "id": "step1", "name": "A1",
            "function": { "name": "map", "input": { "mappings": [ { "path": "data.a", "logic": 1 } ] } }
        }]
    }"#;
    let wf_b = r#"{
        "id": "wf_b", "name": "B", "priority": 1, "condition": true,
        "tasks": [{
            "id": "step1", "name": "B1",
            "function": { "name": "filter", "input": { "condition": false, "on_reject": "skip" } }
        }]
    }"#;

    let workflows = vec![
        Workflow::from_json(wf_a).unwrap(),
        Workflow::from_json(wf_b).unwrap(),
    ];
    let engine = Engine::builder().with_workflows(workflows).build().unwrap();
    let mut message = Message::from_value(&json!({}));
    let trace = engine
        .process_message_with_trace_options(
            &mut message,
            TraceOptions {
                changes: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let wf_b_step = trace
        .steps
        .iter()
        .find(|s| s.workflow_id == "wf_b" && s.task_id.as_deref() == Some("step1"))
        .expect("wf_b's step1 should still produce a step, even though it was skipped");
    assert_eq!(
        wf_b_step.changes.as_ref().map(Vec::len),
        Some(0),
        "a skipped step in wf_b must not inherit wf_a's same-named task's diff"
    );
}

#[tokio::test]
async fn redaction_nulls_the_snapshot_but_not_the_live_message() {
    let wf = Workflow::from_json(
        r#"{
        "id": "w", "name": "w", "priority": 0, "condition": true,
        "tasks": [
            { "id": "seed", "name": "seed", "function": {
                "name": "map",
                "input": { "mappings": [ { "path": "data.secret", "logic": "s3cret" } ] } } },
            { "id": "reader", "name": "reader", "function": {
                "name": "map",
                "input": { "mappings": [
                    { "path": "data.copied", "logic": { "var": "data.secret" } } ] } } }
        ]
    }"#,
    )
    .unwrap();

    let engine = Engine::builder().with_workflow(wf).build().unwrap();
    let mut message = Message::from_value(&json!({}));
    let trace = engine
        .process_message_with_trace_options(
            &mut message,
            TraceOptions {
                redact_paths: vec!["data.secret".to_string()],
                ..Default::default()
            },
        )
        .await
        .unwrap();

    // Every snapshot has the subtree nulled.
    for step in &trace.steps {
        let snap = step.message.as_ref().unwrap();
        assert_eq!(
            snap.context["data"]["secret"],
            dv(json!(null)),
            "the snapshot must not carry the secret"
        );
    }

    // The live message kept the real value, and the later task read it.
    assert_eq!(message.context["data"]["secret"], dv(json!("s3cret")));
    assert_eq!(
        message.context["data"]["copied"],
        dv(json!("s3cret")),
        "redaction must not affect what later tasks read"
    );
}

#[tokio::test]
async fn redaction_also_applies_to_mapping_contexts() {
    let wf = Workflow::from_json(
        r#"{
        "id": "w", "name": "w", "priority": 0, "condition": true,
        "tasks": [
            { "id": "seed", "name": "seed", "function": {
                "name": "map",
                "input": { "mappings": [ { "path": "data.secret", "logic": "s3cret" } ] } } },
            { "id": "multi", "name": "multi", "function": {
                "name": "map",
                "input": { "mappings": [
                    { "path": "data.one", "logic": 1 },
                    { "path": "data.two", "logic": 2 } ] } } }
        ]
    }"#,
    )
    .unwrap();

    let engine = Engine::builder().with_workflow(wf).build().unwrap();
    let mut message = Message::from_value(&json!({}));
    let trace = engine
        .process_message_with_trace_options(
            &mut message,
            TraceOptions {
                redact_paths: vec!["data.secret".to_string()],
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let contexts = trace.steps[1]
        .mapping_contexts
        .as_ref()
        .expect("multi-mapping map task snapshots its per-mapping contexts");
    assert_eq!(contexts.len(), 2);
    for ctx in contexts {
        assert_eq!(
            ctx["data"]["secret"],
            json!(null),
            "mapping contexts are whole-context clones and must be redacted too"
        );
    }
}

#[tokio::test]
async fn mapping_contexts_can_be_switched_off_while_the_map_still_writes() {
    let engine = Engine::builder()
        .with_workflow(
            Workflow::from_json(
                r#"{
        "id": "w", "name": "w", "priority": 0, "condition": true,
        "tasks": [ { "id": "multi", "name": "multi", "function": {
            "name": "map",
            "input": { "mappings": [
                { "path": "data.one", "logic": 1 },
                { "path": "data.two", "logic": 2 } ] } } } ]
    }"#,
            )
            .unwrap(),
        )
        .build()
        .unwrap();

    let mut message = Message::from_value(&json!({}));
    let trace = engine
        .process_message_with_trace_options(
            &mut message,
            TraceOptions {
                mapping_contexts: false,
                ..Default::default()
            },
        )
        .await
        .unwrap();

    assert!(trace.steps[0].mapping_contexts.is_none());
    // The map's writes still land.
    assert_eq!(message.context["data"]["one"], dv(json!(1)));
    assert_eq!(message.context["data"]["two"], dv(json!(2)));
}

#[tokio::test]
async fn the_budget_accounts_for_mapping_contexts_on_their_own() {
    // A single map task with several mappings over a large context can exceed
    // the budget by itself.
    let engine = Engine::builder()
        .with_workflow(
            Workflow::from_json(
                r#"{
        "id": "w", "name": "w", "priority": 0, "condition": true,
        "tasks": [
            { "id": "multi", "name": "multi", "function": {
                "name": "map",
                "input": { "mappings": [
                    { "path": "data.one", "logic": 1 },
                    { "path": "data.two", "logic": 2 },
                    { "path": "data.three", "logic": 3 } ] } } },
            { "id": "after", "name": "after", "function": {
                "name": "map",
                "input": { "mappings": [ { "path": "data.four", "logic": 4 } ] } } }
        ]
    }"#,
            )
            .unwrap(),
        )
        .build()
        .unwrap();

    let mut message = Message::from_value(&json!({}));
    set_nested_value(
        &mut message.context,
        "data.blob",
        dv(json!("q".repeat(4096))),
    );
    let trace = engine
        .process_message_with_trace_options(
            &mut message,
            TraceOptions {
                max_snapshot_bytes: 6000,
                ..Default::default()
            },
        )
        .await
        .unwrap();

    assert!(
        trace.truncated(),
        "three whole-context mapping snapshots plus the step snapshot exceed 6000"
    );
    assert!(
        trace.steps[1].message.is_none(),
        "the following step is past the budget"
    );
}

#[tokio::test]
async fn truncated_can_be_true_from_mapping_contexts_alone_with_snapshots_off() {
    // `truncated()` is one flag shared by two budget terms: message snapshots
    // and mapping contexts. With `snapshots: false`, no step was ever going to
    // carry a `message` — but a large-enough mapping context can still trip
    // the same shared budget on its own.
    let engine = Engine::builder()
        .with_workflow(
            Workflow::from_json(
                r#"{
        "id": "w", "name": "w", "priority": 0, "condition": true,
        "tasks": [
            { "id": "multi", "name": "multi", "function": {
                "name": "map",
                "input": { "mappings": [
                    { "path": "data.one", "logic": 1 },
                    { "path": "data.two", "logic": 2 },
                    { "path": "data.three", "logic": 3 } ] } } }
        ]
    }"#,
            )
            .unwrap(),
        )
        .build()
        .unwrap();

    let mut message = Message::from_value(&json!({}));
    set_nested_value(
        &mut message.context,
        "data.blob",
        dv(json!("q".repeat(4096))),
    );
    let trace = engine
        .process_message_with_trace_options(
            &mut message,
            TraceOptions {
                snapshots: false,
                mapping_contexts: true,
                max_snapshot_bytes: 6000,
                ..Default::default()
            },
        )
        .await
        .unwrap();

    assert!(
        trace.truncated(),
        "three whole-context mapping snapshots alone exceed 6000, even with snapshots off"
    );
    assert!(
        trace.steps.iter().all(|s| s.message.is_none()),
        "snapshots: false means no step ever carries a message, truncated or not"
    );
}

#[tokio::test]
async fn default_options_keep_the_serialized_trace_shape() {
    let engine = Engine::builder()
        .with_workflow(n_map_task_workflow(2))
        .build()
        .unwrap();
    let mut message = Message::from_value(&json!({}));
    let trace = engine
        .process_message_with_trace(&mut message)
        .await
        .unwrap();

    let serialized = serde_json::to_value(&trace).unwrap();
    // No `truncated` on a complete trace, and `steps` is still the only key.
    assert_eq!(
        serialized.as_object().unwrap().keys().collect::<Vec<_>>(),
        vec!["steps"]
    );

    // Steps keep their historical keys, plus timing.
    let step = &serialized["steps"][0];
    for key in ["workflow_id", "task_id", "result", "message"] {
        assert!(step.get(key).is_some(), "missing historical key '{key}'");
    }
    assert!(step.get("started_at").is_some());
    assert!(step.get("duration_us").is_some());
    assert!(step.get("changes").is_none(), "changes is off by default");
}
