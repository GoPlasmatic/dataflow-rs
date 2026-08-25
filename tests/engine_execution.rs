//! Task and workflow execution: the async handler path, the sync stretch for
//! self-contained built-ins, and consecutive fully-sync workflows sharing one
//! arena.

use dataflow_rs::engine::functions::{AsyncFunctionHandler, FunctionConfig};
use dataflow_rs::engine::message::Message;
use dataflow_rs::{Engine, Task, TaskContext, TaskOutcome, Workflow};
use serde_json::json;
use std::sync::Arc;

mod common;

use common::{AsyncLoggingTask, LoggingTask, dv};

#[tokio::test]
async fn test_async_task_execution() {
    // Drive the handler directly via `TaskContext` — exercises the trait
    // surface without going through `Engine::process_message`.
    let task = LoggingTask;

    let mut message = Message::from_value(&json!({}));
    let datalogic = Arc::new(
        datalogic_rs::Engine::builder()
            .with_templating(true)
            .build(),
    );

    let mut ctx = TaskContext::new(&mut message, &datalogic);
    let outcome = task.execute(&mut ctx, &json!({})).await;

    assert!(outcome.is_ok(), "Task execution should succeed");
    assert_eq!(outcome.unwrap(), TaskOutcome::Success);
}

#[tokio::test]
async fn test_workflow_execution() {
    // Create a workflow
    let workflow = custom_task_workflow(
        "test_workflow",
        "Test Workflow",
        "log_task",
        "Log Task",
        "log",
        false,
    );

    // Create engine with the workflow and custom function
    let engine = Engine::builder()
        .with_workflow(workflow)
        .register("log", LoggingTask)
        .build()
        .unwrap();

    // Create a dummy message
    let mut message = Message::from_value(&json!({}));

    // Process the message
    let result = engine.process_message(&mut message).await;

    match &result {
        Ok(_) => println!("Workflow executed successfully"),
        Err(e) => println!("Workflow execution failed: {e:?}"),
    }

    assert!(result.is_ok(), "Workflow execution should succeed");

    // Verify the message was processed correctly
    assert_eq!(
        message.audit_trail().len(),
        1,
        "Message should have one audit trail entry"
    );
    assert_eq!(
        message.audit_trail()[0].task_id.as_ref(),
        "log_task",
        "Audit trail should contain the executed task"
    );
}

#[tokio::test]
async fn test_async_workflow_execution() {
    // Create a workflow with async task
    let workflow = custom_task_workflow(
        "async_workflow",
        "Async Test Workflow",
        "async_log_task",
        "Async Log Task",
        "async_log",
        false,
    );

    // Create engine with the workflow and custom function
    let engine = Engine::builder()
        .with_workflow(workflow)
        .register("async_log", AsyncLoggingTask)
        .build()
        .unwrap();

    // Create a dummy message
    let mut message = Message::from_value(&json!({}));

    // Process the message
    let result = engine.process_message(&mut message).await;

    assert!(result.is_ok(), "Async workflow execution should succeed");

    // Verify the message was processed correctly
    assert_eq!(
        message.audit_trail().len(),
        1,
        "Message should have one audit trail entry"
    );
    assert_eq!(
        message.audit_trail()[0].task_id.as_ref(),
        "async_log_task",
        "Audit trail should contain the executed async task"
    );
}

// =============================================================================
// Log/Filter in sync stretch — regression coverage
// =============================================================================
//
// Both built-ins ship `execute_in_arena` variants that reuse the workflow's
// outer `ArenaContext` instead of opening their own `with_arena` scope. That
// fixes the re-entrant `RefCell::borrow_mut` panic that the sync-stretch
// dispatch previously triggered, and as a side effect lets Log/Filter reuse
// the depth-2 arena cache (no per-call `to_arena` walk of `data.input`).

#[tokio::test]
async fn log_builtin_runs_in_sync_stretch() {
    let workflow_json = r#"{
        "id": "log_only",
        "name": "Log Only",
        "tasks": [
            {
                "id": "log_task",
                "name": "Log",
                "function": {
                    "name": "log",
                    "input": {
                        "message": "hello"
                    }
                }
            }
        ]
    }"#;

    let workflow = Workflow::from_json(workflow_json).unwrap();
    let engine = Engine::builder().with_workflow(workflow).build().unwrap();
    let mut message = Message::from_value(&json!({}));
    engine.process_message(&mut message).await.unwrap();
    // Audit entry recorded with status 200.
    assert_eq!(message.audit_trail().len(), 1);
    assert_eq!(message.audit_trail()[0].status, 200);
    assert_eq!(message.audit_trail()[0].task_id.as_ref(), "log_task");
}

#[tokio::test]
async fn filter_builtin_runs_in_sync_stretch() {
    let workflow_json = r#"{
        "id": "filter_only",
        "name": "Filter Only",
        "tasks": [
            {
                "id": "filter_task",
                "name": "Filter",
                "function": {
                    "name": "filter",
                    "input": {
                        "condition": true,
                        "on_reject": "halt"
                    }
                }
            }
        ]
    }"#;

    let workflow = Workflow::from_json(workflow_json).unwrap();
    let engine = Engine::builder().with_workflow(workflow).build().unwrap();
    let mut message = Message::from_value(&json!({}));
    engine.process_message(&mut message).await.unwrap();
    // Condition was true → status 200 (FILTER_STATUS_PASS).
    assert_eq!(message.audit_trail().len(), 1);
    assert_eq!(message.audit_trail()[0].status, 200);
}

#[tokio::test]
async fn filter_halt_in_sync_stretch_short_circuits_workflow() {
    let workflow_json = r#"{
        "id": "filter_halt",
        "name": "Filter Halt",
        "tasks": [
            {
                "id": "gate",
                "name": "Gate",
                "function": {
                    "name": "filter",
                    "input": {
                        "condition": false,
                        "on_reject": "halt"
                    }
                }
            },
            {
                "id": "after_halt",
                "name": "After Halt",
                "function": {
                    "name": "map",
                    "input": {
                        "mappings": [
                            { "path": "data.should_not_run", "logic": true }
                        ]
                    }
                }
            }
        ]
    }"#;

    let workflow = Workflow::from_json(workflow_json).unwrap();
    let engine = Engine::builder().with_workflow(workflow).build().unwrap();
    let mut message = Message::from_value(&json!({}));
    engine.process_message(&mut message).await.unwrap();

    // Only the gate's audit entry should exist. The map task never ran, so
    // `data.should_not_run` must be absent. `HALT_STATUS_CODE` reachable from
    // the crate root, not just `dataflow_rs::engine::task_outcome::..`.
    assert_eq!(message.audit_trail().len(), 1);
    assert_eq!(message.audit_trail()[0].task_id.as_ref(), "gate");
    assert_eq!(
        message.audit_trail()[0].status,
        usize::from(dataflow_rs::HALT_STATUS_CODE)
    );
    assert!(message.context["data"].get("should_not_run").is_none());
}

#[tokio::test]
async fn log_filter_chained_with_map_share_one_arena() {
    // map → filter → map → log in one sync stretch. Pre-fix this would have
    // panicked at the filter step. Now everything runs in one arena scope.
    let workflow_json = r#"{
        "id": "mixed_sync",
        "name": "Mixed Sync Stretch",
        "tasks": [
            {
                "id": "set_amount",
                "name": "Set Amount",
                "function": {
                    "name": "map",
                    "input": {
                        "mappings": [
                            { "path": "data.amount", "logic": 100 }
                        ]
                    }
                }
            },
            {
                "id": "gate",
                "name": "Amount > 0",
                "function": {
                    "name": "filter",
                    "input": {
                        "condition": { ">": [ { "var": "data.amount" }, 0 ] },
                        "on_reject": "halt"
                    }
                }
            },
            {
                "id": "double_amount",
                "name": "Double Amount",
                "function": {
                    "name": "map",
                    "input": {
                        "mappings": [
                            {
                                "path": "data.amount",
                                "logic": { "*": [ { "var": "data.amount" }, 2 ] }
                            }
                        ]
                    }
                }
            },
            {
                "id": "log_result",
                "name": "Log Result",
                "function": {
                    "name": "log",
                    "input": {
                        "message": { "cat": [ "doubled=", { "var": "data.amount" } ] }
                    }
                }
            }
        ]
    }"#;

    let workflow = Workflow::from_json(workflow_json).unwrap();
    let engine = Engine::builder().with_workflow(workflow).build().unwrap();
    let mut message = Message::from_value(&json!({}));
    engine.process_message(&mut message).await.unwrap();

    assert_eq!(message.context["data"]["amount"], dv(json!(200)));
    assert_eq!(message.audit_trail().len(), 4);
    let task_ids: Vec<&str> = message
        .audit_trail()
        .iter()
        .map(|a| a.task_id.as_ref())
        .collect();
    assert_eq!(
        task_ids,
        vec!["set_amount", "gate", "double_amount", "log_result"]
    );
}

// =============================================================================
// Cross-workflow shared-arena run — regression coverage
// =============================================================================
//
// Consecutive fully-sync workflows execute inside ONE shared `with_arena` scope
// (`execute_sync_workflow_run`): the arena form of `message.context` is built
// once and carried across workflow boundaries. These tests pin the observable
// contract — chained `metadata.progress` conditions and cross-workflow data
// visibility must behave exactly as if each workflow rebuilt its own arena.

#[tokio::test]
async fn chained_fully_sync_workflows_advance_through_shared_arena() {
    // Three fully-sync workflows, each (after the first) gated on the previous
    // workflow's `metadata.progress.workflow_id`, and each reading the `data.*`
    // the previous one wrote. If the carried `ArenaContext` failed to reflect
    // either the `metadata.progress` write or the prior `data` write across a
    // workflow boundary, a later workflow's condition would not match (skipped)
    // or its map would read null — both caught below.
    let wf_a = r#"{
        "id": "wf_a",
        "name": "A",
        "priority": 0,
        "condition": true,
        "tasks": [{
            "id": "map_a", "name": "A",
            "function": { "name": "map", "input": { "mappings": [ { "path": "data.a", "logic": 1 } ] } }
        }]
    }"#;
    let wf_b = r#"{
        "id": "wf_b",
        "name": "B",
        "priority": 1,
        "condition": { "==": [ { "var": "metadata.progress.workflow_id" }, "wf_a" ] },
        "tasks": [{
            "id": "map_b", "name": "B",
            "function": { "name": "map", "input": { "mappings": [ { "path": "data.b", "logic": { "+": [ { "var": "data.a" }, 1 ] } } ] } }
        }]
    }"#;
    let wf_c = r#"{
        "id": "wf_c",
        "name": "C",
        "priority": 2,
        "condition": { "==": [ { "var": "metadata.progress.workflow_id" }, "wf_b" ] },
        "tasks": [{
            "id": "map_c", "name": "C",
            "function": { "name": "map", "input": { "mappings": [ { "path": "data.c", "logic": { "+": [ { "var": "data.b" }, 1 ] } } ] } }
        }]
    }"#;

    let workflows = vec![
        Workflow::from_json(wf_a).unwrap(),
        Workflow::from_json(wf_b).unwrap(),
        Workflow::from_json(wf_c).unwrap(),
    ];
    let engine = Engine::builder().with_workflows(workflows).build().unwrap();
    let mut message = Message::from_value(&json!({}));
    engine.process_message(&mut message).await.unwrap();

    // Every workflow ran (condition matched via the carried arena) and each map
    // saw the previous workflow's write.
    assert_eq!(message.context["data"]["a"], dv(json!(1)));
    assert_eq!(message.context["data"]["b"], dv(json!(2)));
    assert_eq!(message.context["data"]["c"], dv(json!(3)));

    // One audit entry per workflow's single task, in order.
    let task_ids: Vec<&str> = message
        .audit_trail()
        .iter()
        .map(|a| a.task_id.as_ref())
        .collect();
    assert_eq!(task_ids, vec!["map_a", "map_b", "map_c"]);

    // Progress reflects the last workflow to run.
    assert_eq!(
        message.context["metadata"]["progress"]["workflow_id"],
        dv(json!("wf_c"))
    );
}

#[tokio::test]
async fn cross_workflow_false_condition_skips_only_that_workflow() {
    // A real (non-matching) workflow condition skips only that workflow; the
    // shared arena stays intact and a later workflow still runs.
    let wf_x = r#"{
        "id": "wf_x", "name": "X", "priority": 0, "condition": true,
        "tasks": [{ "id": "map_x", "name": "X",
            "function": { "name": "map", "input": { "mappings": [ { "path": "data.x", "logic": 1 } ] } } }]
    }"#;
    let wf_y = r#"{
        "id": "wf_y", "name": "Y", "priority": 1,
        "condition": { "==": [ { "var": "data.x" }, 999 ] },
        "tasks": [{ "id": "map_y", "name": "Y",
            "function": { "name": "map", "input": { "mappings": [ { "path": "data.y", "logic": 1 } ] } } }]
    }"#;
    let wf_z = r#"{
        "id": "wf_z", "name": "Z", "priority": 2, "condition": true,
        "tasks": [{ "id": "map_z", "name": "Z",
            "function": { "name": "map", "input": { "mappings": [ { "path": "data.z", "logic": 1 } ] } } }]
    }"#;

    let workflows = vec![
        Workflow::from_json(wf_x).unwrap(),
        Workflow::from_json(wf_y).unwrap(),
        Workflow::from_json(wf_z).unwrap(),
    ];
    let engine = Engine::builder().with_workflows(workflows).build().unwrap();
    let mut message = Message::from_value(&json!({}));
    engine.process_message(&mut message).await.unwrap();

    assert_eq!(message.context["data"]["x"], dv(json!(1)));
    assert!(
        message.context["data"].get("y").is_none(),
        "wf_y condition was false — it must be skipped"
    );
    assert_eq!(message.context["data"]["z"], dv(json!(1)));

    // Only wf_x and wf_z executed.
    let task_ids: Vec<&str> = message
        .audit_trail()
        .iter()
        .map(|a| a.task_id.as_ref())
        .collect();
    assert_eq!(task_ids, vec!["map_x", "map_z"]);
}

#[tokio::test]
async fn a_task_error_still_advances_the_shared_arena_for_the_next_workflow() {
    // Regression: the sync stretch refreshed `metadata.progress` *after* the `?`
    // on `handle_task_result`, so a task returning `Err` skipped the refresh.
    //
    // That is reachable whenever the task has `continue_on_error: false` but its
    // workflow has `continue_on_error: true`: `record_workflow_error` then
    // returns `false` and `execute_sync_workflow_run` continues into the next
    // workflow carrying the *same* `ArenaContext`. Both workflows here are fully
    // sync, so they share one — and wf_b's condition would be evaluated against a
    // stale snapshot in which the failing task never ran.
    let wf_a = r#"{
        "id": "wf_a", "name": "A", "priority": 0, "condition": true,
        "continue_on_error": true,
        "tasks": [{
            "id": "parse", "name": "Parse", "continue_on_error": false,
            "function": { "name": "parse_xml",
                          "input": { "source": "payload.n", "target": "out" } }
        }]
    }"#;
    let wf_b = r#"{
        "id": "wf_b", "name": "B", "priority": 1,
        "condition": { "==": [ { "var": "metadata.progress.task_id" }, "parse" ] },
        "tasks": [{
            "id": "m", "name": "M",
            "function": { "name": "map", "input": { "mappings": [
                { "path": "data.b_ran", "logic": true }
            ] } }
        }]
    }"#;

    let engine = Engine::builder()
        .with_workflows(vec![
            Workflow::from_json(wf_a).unwrap(),
            Workflow::from_json(wf_b).unwrap(),
        ])
        .build()
        .unwrap();

    // `payload.n` is a number, so `parse_xml` returns `Err`.
    let mut message = Message::from_value(&json!({ "n": 42 }));
    engine.process_message(&mut message).await.unwrap();

    assert_eq!(
        message.context["data"]["b_ran"],
        dv(json!(true)),
        "wf_b gates on wf_a's progress write and shares its arena — it must still run"
    );
}

/// Build a single-custom-task workflow through the documented constructors.
///
/// Since `Task` and `Workflow` became `#[non_exhaustive]` this is the shape an
/// external caller writes: a constructor, then assignment of the public fields
/// it cares about. The engine internals — `id_arc`, `compiled_condition`,
/// `group_starts` — are set correctly by `Task::action` and `Workflow::new`,
/// and are no longer named here at all.
fn custom_task_workflow(
    workflow_id: &str,
    workflow_name: &str,
    task_id: &str,
    task_name: &str,
    function_name: &str,
    continue_on_error: bool,
) -> Workflow {
    let mut task = Task::action(
        task_id,
        task_name,
        FunctionConfig::Custom {
            name: function_name.to_string(),
            input: json!({}),
            compiled_input: None,
        },
    );
    task.continue_on_error = continue_on_error;

    let mut workflow = Workflow::new();
    workflow.id = workflow_id.to_string();
    workflow.name = workflow_name.to_string();
    workflow.tasks = vec![task];
    workflow.continue_on_error = continue_on_error;
    workflow
}
