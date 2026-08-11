//! A workflow's `loop` config: the task list runs as a bounded `for` loop, with
//! the counter owned by the engine.

use async_trait::async_trait;
use dataflow_rs::engine::functions::AsyncFunctionHandler;
use dataflow_rs::engine::message::Message;
use dataflow_rs::{Engine, Result, TaskContext, TaskOutcome, Workflow};
use datavalue::OwnedDataValue;
use serde_json::{Value, json};
use std::sync::Arc;

mod common;

use common::{RecordingObserver, dv};

// =============================================================================
// Workflow loop — bounded per-sweep re-execution of a task list
// =============================================================================

/// Counts one async handler call per loop sweep, so the per-item test proves
/// an async task in a loop body runs once per item — not just sync built-ins.
#[derive(Debug, Default)]
struct CallCounter {
    calls: Arc<std::sync::atomic::AtomicUsize>,
}

#[async_trait]
impl AsyncFunctionHandler for CallCounter {
    type Input = Value;

    async fn execute(&self, ctx: &mut TaskContext<'_>, _input: &Value) -> Result<TaskOutcome> {
        let n = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        // Write through TaskContext so the audit trail records the change.
        ctx.set("temp_data.calls", OwnedDataValue::from_i64(n as i64));
        Ok(TaskOutcome::Success)
    }
}

#[tokio::test]
async fn loop_workflow_processes_each_item_of_an_array() {
    // The per-item pattern: a setup workflow counts the items, then a looping
    // workflow picks `items[i]`, calls an async handler for it, and appends one
    // output per item. `reduce`, `<`, `+`, `cat`, `merge` and computed-path
    // `val` are all core operators, so this runs under default features too.
    let workflows = vec![
        Workflow::from_json(
            r#"{
                "id": "setup", "name": "Setup", "priority": 0,
                "tasks": [{ "id": "count", "name": "Count items",
                    "function": { "name": "map", "input": { "mappings": [
                        { "path": "temp_data.n",
                          "logic": {"reduce": [{"var": "data.items"},
                                               {"+": [{"var": "accumulator"}, 1]}, 0]} },
                        { "path": "data.processed", "logic": [] }
                    ]}}}]
            }"#,
        )
        .unwrap(),
        Workflow::from_json(
            r#"{
                "id": "per_item", "name": "Per item", "priority": 1,
                "condition": {"<": [{"var": "temp_data.i"}, {"var": "temp_data.n"}]},
                "loop": { "counter": "i", "max": 100 },
                "tasks": [
                  { "id": "pick", "name": "Pick the item at i",
                    "function": { "name": "map", "input": { "mappings": [
                        { "path": "temp_data.item",
                          "logic": {"val": [["data", "items", {"var": "temp_data.i"}]]} }
                    ]}}},
                  { "id": "call", "name": "Async call for this item",
                    "function": { "name": "call_counter", "input": {} } },
                  { "id": "collect", "name": "Collect the result",
                    "function": { "name": "map", "input": { "mappings": [
                        { "path": "data.processed",
                          "logic": {"merge": [{"var": "data.processed"},
                                              [{"cat": ["item-", {"var": "temp_data.item.id"}]}]]} }
                    ]}}}
                ]
            }"#,
        )
        .unwrap(),
    ];

    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let engine = Engine::builder()
        .with_workflows(workflows)
        .register(
            "call_counter",
            CallCounter {
                calls: Arc::clone(&calls),
            },
        )
        .build()
        .expect("engine should build");

    // Seed `data`, not the payload: the JSONLogic eval context is
    // {data, metadata, temp_data}, so `data.items` is what conditions see.
    let mut message = Message::builder()
        .data(dv(
            json!({ "items": [{"id": "a"}, {"id": "b"}, {"id": "c"}] }),
        ))
        .build();
    engine
        .process_message(&mut message)
        .await
        .expect("processing should succeed");

    assert_eq!(
        Value::from(&message.context["data"]["processed"]),
        json!(["item-a", "item-b", "item-c"]),
        "one output per input item, in order"
    );
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        3,
        "the async body task ran once per item"
    );

    // One audit entry per task per sweep, each carrying the index it processed.
    let per_item: Vec<Option<i64>> = message
        .audit_trail()
        .iter()
        .filter(|entry| entry.workflow_id.as_ref() == "per_item")
        .map(|entry| entry.loop_counter)
        .collect();
    assert_eq!(
        per_item,
        vec![
            Some(0),
            Some(0),
            Some(0),
            Some(1),
            Some(1),
            Some(1),
            Some(2),
            Some(2),
            Some(2),
        ],
        "three tasks per sweep, three sweeps"
    );

    // The setup workflow's own entries are unstamped.
    assert!(
        message
            .audit_trail()
            .iter()
            .filter(|entry| entry.workflow_id.as_ref() == "setup")
            .all(|entry| entry.loop_counter.is_none()),
        "a non-looping workflow records no loop counter"
    );
}

/// Always fails, to drive the loop's error paths.
#[derive(Debug)]
struct AlwaysFails;

#[async_trait]
impl AsyncFunctionHandler for AlwaysFails {
    type Input = Value;

    async fn execute(&self, _ctx: &mut TaskContext<'_>, _input: &Value) -> Result<TaskOutcome> {
        Err(dataflow_rs::engine::error::DataflowError::Task(
            "boom".to_string(),
        ))
    }
}

/// A three-sweep loop whose body is `tasks`, wired to the given handlers.
fn loop_engine(
    tasks: &str,
    workflow_extra: &str,
    handlers: Vec<(
        &str,
        Box<dyn dataflow_rs::engine::functions::DynAsyncFunctionHandler>,
    )>,
) -> Engine {
    let workflow = Workflow::from_json(&format!(
        r#"{{ "id": "w", "name": "w", {workflow_extra}
              "loop": {{"counter": "i", "max": 3}}, "tasks": [{tasks}] }}"#
    ))
    .expect("workflow should parse");
    let mut builder = Engine::builder().with_workflows(vec![workflow]);
    for (name, handler) in handlers {
        builder = builder.register_boxed(name, handler);
    }
    builder.build().expect("engine should build")
}

/// Loop counters recorded for `workflow_id`, in order.
fn loop_counters(message: &Message, workflow_id: &str) -> Vec<Option<i64>> {
    message
        .audit_trail()
        .iter()
        .filter(|entry| entry.workflow_id.as_ref() == workflow_id)
        .map(|entry| entry.loop_counter)
        .collect()
}

#[tokio::test]
async fn a_loop_body_mixing_sync_and_async_tasks_runs_every_task_each_sweep() {
    // Exercises the sync-stretch / async-boundary split inside a sweep: a
    // leading sync stretch, an async task, then a trailing sync task.
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let engine = loop_engine(
        r#"{"id": "pre", "name": "pre", "function": {"name": "map",
             "input": {"mappings": [{"path": "temp_data.pre", "logic": {"var": "temp_data.i"}}]}}},
           {"id": "mid", "name": "mid", "function": {"name": "call_counter", "input": {}}},
           {"id": "post", "name": "post", "function": {"name": "map",
             "input": {"mappings": [{"path": "data.post", "logic": {"var": "temp_data.pre"}}]}}}"#,
        "",
        vec![(
            "call_counter",
            Box::new(CallCounter {
                calls: Arc::clone(&calls),
            }),
        )],
    );

    let mut message = Message::builder().build();
    engine.process_message(&mut message).await.unwrap();

    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 3);
    assert_eq!(
        loop_counters(&message, "w"),
        vec![
            Some(0),
            Some(0),
            Some(0),
            Some(1),
            Some(1),
            Some(1),
            Some(2),
            Some(2),
            Some(2),
        ],
        "all three tasks ran on all three sweeps"
    );
    assert_eq!(Value::from(&message.context["data"]["post"]), json!(2));
}

#[tokio::test]
async fn a_loop_body_that_starts_with_an_async_task_still_sweeps() {
    // The `first_boundary == 0` path in execute_pass: no leading sync stretch
    // to fold the workflow condition into.
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let engine = loop_engine(
        r#"{"id": "first", "name": "first", "function": {"name": "call_counter", "input": {}}}"#,
        "",
        vec![(
            "call_counter",
            Box::new(CallCounter {
                calls: Arc::clone(&calls),
            }),
        )],
    );

    let mut message = Message::builder().build();
    engine.process_message(&mut message).await.unwrap();

    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 3);
    assert_eq!(
        loop_counters(&message, "w"),
        vec![Some(0), Some(1), Some(2)]
    );
}

#[tokio::test]
async fn a_loop_with_a_condition_and_a_leading_async_task_re_checks_every_sweep() {
    // Condition + async-first body: the condition is evaluated on the owned
    // context each sweep rather than folded into an arena scope.
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let engine = loop_engine(
        r#"{"id": "first", "name": "first", "function": {"name": "call_counter", "input": {}}}"#,
        r#""condition": {"<": [{"var": "temp_data.i"}, 2]},"#,
        vec![(
            "call_counter",
            Box::new(CallCounter {
                calls: Arc::clone(&calls),
            }),
        )],
    );

    let mut message = Message::builder().build();
    engine.process_message(&mut message).await.unwrap();

    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "the condition stopped the loop before its bound"
    );
    assert_eq!(loop_counters(&message, "w"), vec![Some(0), Some(1)]);
}

#[tokio::test]
async fn a_failing_task_with_continue_on_error_lets_every_sweep_run() {
    // Task-level continue_on_error: the error never reaches the workflow, so
    // the sweep finishes and the loop runs to its bound.
    let engine = loop_engine(
        r#"{"id": "boom", "name": "boom", "continue_on_error": true,
             "function": {"name": "failing", "input": {}}}"#,
        "",
        vec![("failing", Box::new(AlwaysFails))],
    );

    let mut message = Message::builder().build();
    engine
        .process_message(&mut message)
        .await
        .expect("task-level continue_on_error swallows the error");

    assert_eq!(
        loop_counters(&message, "w"),
        vec![Some(0), Some(1), Some(2)],
        "every sweep ran and recorded its 500"
    );
    assert_eq!(message.errors().len(), 3, "one recorded error per sweep");
    assert!(message.audit_trail().iter().all(|e| e.status == 500));
}

#[tokio::test]
async fn a_failing_task_with_workflow_continue_on_error_advances_to_the_next_sweep() {
    // Workflow-level continue_on_error: the error propagates out of the sweep,
    // is recorded, and the loop advances rather than abandoning the rest —
    // item 7 failing must not stop item 8.
    let engine = loop_engine(
        r#"{"id": "boom", "name": "boom", "function": {"name": "failing", "input": {}}}"#,
        r#""continue_on_error": true,"#,
        vec![("failing", Box::new(AlwaysFails))],
    );

    let mut message = Message::builder().build();
    engine
        .process_message(&mut message)
        .await
        .expect("workflow-level continue_on_error keeps the message going");

    assert_eq!(
        loop_counters(&message, "w"),
        vec![Some(0), Some(1), Some(2)],
        "the loop advanced past each failing sweep"
    );
    // Each sweep records the task error and the workflow wrapper.
    assert_eq!(
        message
            .errors()
            .iter()
            .filter(|e| e.code == "WORKFLOW_ERROR")
            .count(),
        3
    );
}

#[tokio::test]
async fn a_failing_task_without_continue_on_error_stops_the_loop_on_the_first_sweep() {
    let engine = loop_engine(
        r#"{"id": "boom", "name": "boom", "function": {"name": "failing", "input": {}}}"#,
        "",
        vec![("failing", Box::new(AlwaysFails))],
    );

    let mut message = Message::builder().build();
    let result = engine.process_message(&mut message).await;

    assert!(result.is_err(), "the error stops the message");
    assert_eq!(
        loop_counters(&message, "w"),
        vec![Some(0)],
        "only the first sweep ran"
    );
}

#[tokio::test]
async fn trace_steps_carry_the_loop_counter_for_executed_and_skipped_tasks() {
    let workflow = Workflow::from_json(
        r#"{ "id": "w", "name": "w", "loop": {"counter": "i", "max": 3},
             "tasks": [
               {"id": "evens", "name": "evens",
                "condition": {"==": [{"%": [{"var": "temp_data.i"}, 2]}, 0]},
                "function": {"name": "map", "input": {"mappings": []}}},
               {"id": "always", "name": "always",
                "function": {"name": "map", "input": {"mappings": []}}}] }"#,
    )
    .unwrap();
    let engine = Engine::new(vec![workflow], std::collections::HashMap::new()).unwrap();

    let mut message = Message::builder().build();
    let trace = engine
        .process_message_with_trace(&mut message)
        .await
        .unwrap();

    let steps: Vec<(Option<&str>, &str, Option<i64>)> = trace
        .steps
        .iter()
        .map(|s| {
            (
                s.task_id.as_deref(),
                if s.result == dataflow_rs::StepResult::Executed {
                    "executed"
                } else {
                    "skipped"
                },
                s.loop_counter,
            )
        })
        .collect();

    assert_eq!(
        steps,
        [
            (Some("evens"), "executed", Some(0)),
            (Some("always"), "executed", Some(0)),
            (Some("evens"), "skipped", Some(1)),
            (Some("always"), "executed", Some(1)),
            (Some("evens"), "executed", Some(2)),
            (Some("always"), "executed", Some(2)),
        ],
        "skipped steps are stamped with their sweep too"
    );
}

#[tokio::test]
async fn a_non_looping_trace_step_serializes_without_a_loop_counter_key() {
    let workflow = Workflow::from_json(
        r#"{ "id": "w", "name": "w", "tasks": [{"id": "t", "name": "t",
             "function": {"name": "map", "input": {"mappings": []}}}] }"#,
    )
    .unwrap();
    let engine = Engine::new(vec![workflow], std::collections::HashMap::new()).unwrap();

    let mut message = Message::builder().build();
    let trace = engine
        .process_message_with_trace(&mut message)
        .await
        .unwrap();

    let json = serde_json::to_value(&trace.steps[0]).unwrap();
    assert!(
        json.get("loop_counter").is_none(),
        "non-looping trace JSON is unchanged from before loops existed"
    );
    let audit = serde_json::to_value(&message.audit_trail()[0]).unwrap();
    assert!(audit.get("loop_counter").is_none());
}

#[tokio::test]
async fn an_observer_sees_one_event_per_task_per_sweep() {
    let observer = Arc::new(RecordingObserver::default());
    let workflow = Workflow::from_json(
        r#"{ "id": "w", "name": "w", "loop": {"counter": "i", "max": 3},
             "tasks": [
               {"id": "a", "name": "a", "function": {"name": "map", "input": {"mappings": []}}},
               {"id": "b", "name": "b", "function": {"name": "map", "input": {"mappings": []}}}] }"#,
    )
    .unwrap();
    let engine = Engine::builder()
        .with_workflows(vec![workflow])
        .with_observer(Arc::clone(&observer) as Arc<dyn dataflow_rs::ExecutionObserver>)
        .build()
        .unwrap();

    let mut message = Message::builder().build();
    engine.process_message(&mut message).await.unwrap();

    let events = observer.seen();
    assert_eq!(events.len(), 6, "two tasks x three sweeps");
    assert!(events.iter().all(|e| e.workflow_id == "w"));
    let ids: Vec<&str> = events.iter().map(|e| e.task_id.as_str()).collect();
    assert_eq!(ids, ["a", "b", "a", "b", "a", "b"]);
}

#[tokio::test]
async fn a_loop_workflow_survives_a_hot_reload() {
    let make = |max: i64| {
        Workflow::from_json(&format!(
            r#"{{ "id": "w", "name": "w", "loop": {{"counter": "i", "max": {max}}},
                  "tasks": [{{"id": "t", "name": "t",
                    "function": {{"name": "map", "input": {{"mappings": []}}}}}}] }}"#
        ))
        .unwrap()
    };

    let engine = Engine::new(vec![make(2)], std::collections::HashMap::new()).unwrap();
    let mut first = Message::builder().build();
    engine.process_message(&mut first).await.unwrap();
    assert_eq!(loop_counters(&first, "w"), vec![Some(0), Some(1)]);

    // The reloaded engine recompiles the loop config, including its counter
    // path — a stale or unpopulated path would silently stop writing it.
    let reloaded = engine.with_new_workflows(vec![make(4)]).unwrap();
    let mut second = Message::builder().build();
    reloaded.process_message(&mut second).await.unwrap();

    assert_eq!(
        loop_counters(&second, "w"),
        vec![Some(0), Some(1), Some(2), Some(3)]
    );
    assert_eq!(
        Value::from(&second.context["temp_data"]["i"]),
        json!(4),
        "the recompiled counter path is still written"
    );
}

#[tokio::test]
async fn a_downstream_workflow_chains_off_a_loop_via_metadata_progress() {
    // `metadata.progress` is written after every task of every sweep, so a
    // downstream workflow can still gate on the loop having run.
    let workflows = vec![
        Workflow::from_json(
            r#"{ "id": "looper", "name": "looper", "priority": 0,
                 "loop": {"counter": "i", "max": 3},
                 "tasks": [{"id": "t", "name": "t",
                   "function": {"name": "map", "input": {"mappings": [
                     {"path": "data.last", "logic": {"var": "temp_data.i"}}]}}}] }"#,
        )
        .unwrap(),
        Workflow::from_json(
            r#"{ "id": "after", "name": "after", "priority": 1,
                 "condition": {"==": [{"var": "metadata.progress.workflow_id"}, "looper"]},
                 "tasks": [{"id": "t", "name": "t",
                   "function": {"name": "map", "input": {"mappings": [
                     {"path": "data.chained", "logic": true}]}}}] }"#,
        )
        .unwrap(),
    ];
    let engine = Engine::new(workflows, std::collections::HashMap::new()).unwrap();

    let mut message = Message::builder().build();
    engine.process_message(&mut message).await.unwrap();

    assert_eq!(Value::from(&message.context["data"]["last"]), json!(2));
    assert_eq!(
        Value::from(&message.context["data"]["chained"]),
        json!(true),
        "the downstream workflow saw the loop's progress"
    );
    assert_eq!(loop_counters(&message, "after"), vec![None]);
}
