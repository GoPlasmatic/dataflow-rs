//! A workflow's `loop` iterating an array: `setup` once, `over` evaluated
//! after it, `as` holding the element, `scratch` reset per iteration — the
//! shape #60 replaces four hand-built idioms with.

use async_trait::async_trait;
use dataflow_rs::engine::functions::AsyncFunctionHandler;
use dataflow_rs::engine::message::Message;
use dataflow_rs::{
    Engine, ExecutionObserver, IssueCode, Result, TaskContext, TaskEvent, TaskOutcome, Workflow,
    WorkflowFinished,
};
use datavalue::OwnedDataValue;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

mod common;

use common::dv;

/// Appends the element it was called for, and the iteration, to `data.calls`,
/// so a test sees both the order of visits and what `temp_data.item` held.
#[derive(Debug, Default)]
struct Visit {
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl AsyncFunctionHandler for Visit {
    type Input = Value;

    async fn execute(&self, ctx: &mut TaskContext<'_>, _input: &Value) -> Result<TaskOutcome> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let item = Value::from(&ctx.message().context["temp_data"]["item"]);
        let counter = ctx.loop_counter();
        let mut seen = Value::from(&ctx.message().context["data"]["calls"]);
        if !seen.is_array() {
            seen = json!([]);
        }
        seen.as_array_mut()
            .expect("just made an array")
            .push(json!({"item": item, "i": counter}));
        ctx.set("data.calls", OwnedDataValue::from(&seen));
        Ok(TaskOutcome::Success)
    }
}

/// The issue's shape: claim, read the batch, iterate its items. `mark` writes
/// a per-item flag into scratch only for an item that asks to retry; `record`
/// reads it back for every item, so a leak would show up on the next one.
const BATCH_WORKFLOW: &str = r#"{
    "id": "batch", "name": "Batch",
    "loop": {
        "setup": [
            { "id": "claim", "name": "claim",
              "function": { "name": "map", "input": { "mappings": [
                  { "path": "temp_data.claimed", "logic": true } ] } } },
            { "id": "read", "name": "read",
              "function": { "name": "map", "input": { "mappings": [
                  { "path": "temp_data.batch", "logic": { "var": "data.input" } } ] } } }
        ],
        "over": { "var": "temp_data.batch.items" },
        "as": "item",
        "counter": "i",
        "max": 64,
        "scratch": "it"
    },
    "tasks": [
        { "id": "mark", "name": "mark",
          "condition": { "==": [{ "var": "temp_data.item.retry" }, true] },
          "function": { "name": "map", "input": { "mappings": [
              { "path": "temp_data.it.retrying", "logic": true } ] } } },
        { "id": "visit", "name": "visit", "function": { "name": "visit", "input": {} } },
        { "id": "record", "name": "record",
          "function": { "name": "map", "input": { "mappings": [
              { "path": { "cat": ["data.retrying.#", { "var": "temp_data.i" }] },
                "logic": { "!!": [{ "var": "temp_data.it.retrying" }] } } ] } } }
    ]
}"#;

fn batch_builder(calls: &Arc<AtomicUsize>) -> dataflow_rs::EngineBuilder {
    Engine::builder()
        .with_workflows(vec![Workflow::from_json(BATCH_WORKFLOW).unwrap()])
        .register(
            "visit",
            Visit {
                calls: Arc::clone(calls),
            },
        )
}

fn batch_message() -> Message {
    Message::builder()
        .data(dv(json!({
            "input": { "items": [
                { "id": "a" }, { "id": "b", "retry": true }, { "id": "c" } ] }
        })))
        .build()
}

#[tokio::test]
async fn the_batch_pattern_needs_no_guard_no_halt_filter_no_pick_and_no_clear() {
    let calls = Arc::new(AtomicUsize::new(0));
    let engine = batch_builder(&calls).build().expect("engine should build");
    let mut message = batch_message();

    engine.process_message(&mut message).await.expect("ok");

    assert_eq!(
        calls.load(Ordering::SeqCst),
        3,
        "the async body ran once per element"
    );
    assert_eq!(
        Value::from(&message.context["data"]["calls"]),
        json!([
            {"item": {"id": "a"}, "i": 0},
            {"item": {"id": "b", "retry": true}, "i": 1},
            {"item": {"id": "c"}, "i": 2}
        ]),
        "each iteration saw its own element and index"
    );
    assert_eq!(
        Value::from(&message.context["data"]["retrying"]),
        json!({"0": false, "1": true, "2": false}),
        "the scratch flag written for `b` did not leak into `c`"
    );
    assert_eq!(
        Value::from(&message.context["temp_data"]["claimed"]),
        json!(true),
        "setup ran"
    );

    // claim and read (setup, unstamped), then per iteration: mark (only for
    // `b` — a skipped task records nothing), visit, record.
    let stamps: Vec<Option<i64>> = message
        .audit_trail()
        .iter()
        .map(|e| e.loop_counter)
        .collect();
    assert_eq!(
        stamps,
        vec![
            None,
            None,
            Some(0),
            Some(0),
            Some(1),
            Some(1),
            Some(1),
            Some(2),
            Some(2)
        ]
    );
    assert!(message.errors().is_empty());
}

#[tokio::test]
async fn an_over_loop_survives_a_hot_reload() {
    let calls = Arc::new(AtomicUsize::new(0));
    let engine = batch_builder(&calls).build().unwrap();
    let mut first = batch_message();
    engine.process_message(&mut first).await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 3);

    // A reload recompiles `over` and the slot paths; a stale or unpopulated
    // compiled form would surface as a `loop.over` workflow error here.
    let reloaded = engine
        .with_new_workflows(vec![Workflow::from_json(BATCH_WORKFLOW).unwrap()])
        .unwrap();
    let mut second = batch_message();
    reloaded.process_message(&mut second).await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 6);
    assert!(second.errors().is_empty());
}

#[tokio::test]
async fn trace_steps_stamp_setup_without_a_loop_counter() {
    let calls = Arc::new(AtomicUsize::new(0));
    let engine = batch_builder(&calls).build().unwrap();
    let mut message = batch_message();

    let trace = engine
        .process_message_with_trace(&mut message)
        .await
        .unwrap();

    let steps: Vec<(Option<&str>, Option<i64>)> = trace
        .steps
        .iter()
        .map(|s| (s.task_id.as_deref(), s.loop_counter))
        .collect();
    assert_eq!(&steps[..2], &[(Some("claim"), None), (Some("read"), None)]);
    assert!(
        steps[2..].iter().all(|(_, c)| c.is_some()),
        "every body step carries its iteration"
    );
    assert_eq!(
        steps[2],
        (Some("mark"), Some(0)),
        "a skipped body step is stamped too"
    );
}

/// Task events plus the one `workflow_finished` per workflow.
#[derive(Default)]
struct SpanObserver {
    tasks: Mutex<Vec<String>>,
    finished: Mutex<Vec<(String, u32, bool)>>,
}

impl ExecutionObserver for SpanObserver {
    fn task_finished(&self, event: &TaskEvent<'_>) {
        self.tasks.lock().unwrap().push(event.task_id.to_string());
    }

    fn workflow_finished(&self, event: &WorkflowFinished<'_>) {
        self.finished.lock().unwrap().push((
            event.workflow_id.to_string(),
            event.sweeps,
            event.halted,
        ));
    }
}

#[tokio::test]
async fn an_observer_sees_setup_task_events_and_counts_iterations_only() {
    let calls = Arc::new(AtomicUsize::new(0));
    let observer = Arc::new(SpanObserver::default());
    let engine = batch_builder(&calls)
        .with_observer(Arc::clone(&observer) as Arc<dyn ExecutionObserver>)
        .build()
        .unwrap();
    let mut message = batch_message();
    engine.process_message(&mut message).await.unwrap();

    let tasks = observer.tasks.lock().unwrap().clone();
    assert_eq!(
        &tasks[..2],
        &["claim", "read"],
        "setup tasks emit task events"
    );
    assert_eq!(
        tasks.len(),
        2 + 3 + 3 + 1,
        "then visit and record per element, plus mark once"
    );
    assert_eq!(
        observer.finished.lock().unwrap().clone(),
        vec![("batch".to_string(), 3, false)],
        "one span for the whole loop; the setup pass is not a sweep"
    );
}

#[tokio::test]
async fn build_refuses_an_unknown_function_in_setup() {
    let workflow = Workflow::from_json(
        r#"{ "id": "w", "name": "w",
             "loop": { "max": 3, "setup": [
               { "id": "s", "name": "s", "function": { "name": "no_such_handler", "input": {} } } ] },
             "tasks": [ { "id": "t", "name": "t",
               "function": { "name": "map", "input": { "mappings": [] } } } ] }"#,
    )
    .unwrap();
    let err = Engine::builder()
        .with_workflows(vec![workflow])
        .build()
        .err()
        .expect("a setup step is resolved like a body step");
    assert!(err.to_string().contains("no_such_handler"), "got: {err}");
}

#[tokio::test]
async fn check_workflow_reports_setup_steps_and_a_secret_in_over() {
    let workflow = Workflow::from_json(
        r#"{ "id": "w", "name": "w",
             "loop": { "max": 3, "as": "item",
               "over": { "secret": "batch" },
               "setup": [ { "id": "s", "name": "s",
                 "function": { "name": "http_call", "input": { "connector": "api" } } } ] },
             "tasks": [ { "id": "t", "name": "t",
               "function": { "name": "map", "input": { "mappings": [] } } } ] }"#,
    )
    .unwrap();
    let issues = Engine::builder()
        .with_secrets_json(&json!({"batch": "x"}))
        .check_workflow(&workflow);
    let codes: Vec<IssueCode> = issues.iter().map(|i| i.code).collect();
    assert!(
        codes.contains(&IssueCode::MissingHandler),
        "http_call in setup: {codes:?}"
    );
    let over = issues
        .iter()
        .find(|i| i.code == IssueCode::SecretInMessageWrite)
        .unwrap_or_else(|| {
            panic!("over's result lands in temp_data, so it may not read a secret: {codes:?}")
        });
    assert_eq!(over.path.as_deref(), Some("loop.over"));
}
