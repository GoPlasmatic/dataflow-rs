//! `TaskContext::workflow_id` / `task_id` / `loop_counter` — the executing
//! identity a handler can read about itself.
//!
//! The point of these tests is that the ids are observed *from inside a
//! handler during a real run*, not asserted about the plumbing. A host that
//! has to re-derive them by pairing trace steps with recorded calls positionally
//! is doing so because this surface did not exist.

use async_trait::async_trait;
use dataflow_rs::engine::functions::AsyncFunctionHandler;
use dataflow_rs::engine::message::Message;
use dataflow_rs::{DataflowError, Engine, Result, TaskContext, TaskOutcome, Workflow};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

/// What one handler invocation saw about itself.
#[derive(Debug, Clone, PartialEq)]
struct Seen {
    workflow_id: Option<String>,
    task_id: Option<String>,
    loop_counter: Option<i64>,
}

/// Records the identity of every call, in order.
#[derive(Clone, Default)]
struct Recorder(Arc<Mutex<Vec<Seen>>>);

impl Recorder {
    fn seen(&self) -> Vec<Seen> {
        self.0.lock().unwrap().clone()
    }

    fn note(&self, ctx: &TaskContext<'_>) {
        self.0.lock().unwrap().push(Seen {
            workflow_id: ctx.workflow_id().map(str::to_string),
            task_id: ctx.task_id().map(str::to_string),
            loop_counter: ctx.loop_counter(),
        });
    }
}

#[async_trait]
impl AsyncFunctionHandler for Recorder {
    type Input = Value;

    async fn execute(&self, ctx: &mut TaskContext<'_>, _input: &Value) -> Result<TaskOutcome> {
        self.note(ctx);
        Ok(TaskOutcome::Success)
    }
}

/// Records identity, then fails — identity must be visible on the error path
/// too, since that is exactly when a host most wants to label the call.
#[derive(Clone)]
struct RecordThenFail(Recorder);

#[async_trait]
impl AsyncFunctionHandler for RecordThenFail {
    type Input = Value;

    async fn execute(&self, ctx: &mut TaskContext<'_>, _input: &Value) -> Result<TaskOutcome> {
        self.0.note(ctx);
        Err(DataflowError::Validation("deliberate".to_string()))
    }
}

async fn run(workflow: Value, recorder: Recorder) -> Vec<Seen> {
    let workflow = Workflow::from_json(&workflow.to_string()).expect("fixture parses");
    let engine = Engine::builder()
        .with_workflow(workflow)
        .register("record", recorder.clone())
        .build()
        .unwrap();

    let mut message = Message::from_value(&json!({}));
    let _ = engine.process_message(&mut message).await;
    recorder.seen()
}

fn record_task(id: &str) -> Value {
    json!({"id": id, "name": id, "function": {"name": "record", "input": {}}})
}

#[tokio::test]
async fn a_handler_sees_its_own_workflow_and_task_ids() {
    let seen = run(
        json!({
            "id": "checkout", "name": "checkout", "priority": 0,
            "tasks": [record_task("charge"), record_task("notify")]
        }),
        Recorder::default(),
    )
    .await;

    assert_eq!(
        seen,
        vec![
            Seen {
                workflow_id: Some("checkout".into()),
                task_id: Some("charge".into()),
                loop_counter: None
            },
            Seen {
                workflow_id: Some("checkout".into()),
                task_id: Some("notify".into()),
                loop_counter: None
            },
        ],
        "each call reports the task actually running, in order"
    );
}

#[tokio::test]
async fn a_handler_inside_a_group_reports_the_leaf_task_id() {
    // Handlers dispatch only on leaf tasks — a group is span bookkeeping on the
    // task that opens it, never a dispatch target — so a group id can never
    // reach `task_id()`.
    let seen = run(
        json!({
            "id": "w", "name": "w", "priority": 0,
            "tasks": [
                {"id": "have_user", "condition": true, "tasks": [record_task("greet")]}
            ]
        }),
        Recorder::default(),
    )
    .await;

    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].task_id.as_deref(), Some("greet"));
    assert_ne!(
        seen[0].task_id.as_deref(),
        Some("have_user"),
        "the enclosing group's id must never surface as the task id"
    );
}

#[tokio::test]
async fn identity_is_reported_even_when_the_handler_fails() {
    let recorder = Recorder::default();
    let workflow = Workflow::from_json(
        &json!({
            "id": "w", "name": "w", "priority": 0,
            "tasks": [{"id": "boom", "name": "boom", "continue_on_error": true,
                       "function": {"name": "record", "input": {}}}]
        })
        .to_string(),
    )
    .unwrap();

    let engine = Engine::builder()
        .with_workflow(workflow)
        .register("record", RecordThenFail(recorder.clone()))
        .build()
        .unwrap();

    let mut message = Message::from_value(&json!({}));
    let _ = engine.process_message(&mut message).await;

    assert_eq!(
        recorder.seen(),
        vec![Seen {
            workflow_id: Some("w".into()),
            task_id: Some("boom".into()),
            loop_counter: None
        }],
        "a failing call is exactly when a host wants the label"
    );
}

#[tokio::test]
async fn loop_counter_tracks_the_sweep() {
    let seen = run(
        json!({
            "id": "w", "name": "w", "priority": 0,
            "loop": {"counter": "i", "init": 0, "max": 3, "increment": 1},
            "tasks": [record_task("body")]
        }),
        Recorder::default(),
    )
    .await;

    assert_eq!(
        seen.iter().map(|s| s.loop_counter).collect::<Vec<_>>(),
        vec![Some(0), Some(1), Some(2)],
        "one call per sweep, counting the engine's own counter"
    );
    assert!(
        seen.iter().all(|s| s.task_id.as_deref() == Some("body")),
        "identity is stable across sweeps"
    );
}

#[tokio::test]
async fn loop_counter_is_reported_without_a_named_counter() {
    // A loop with no `counter` name writes the counter to no path at all, so
    // `temp_data` cannot carry it. This accessor is the only way to see it.
    let seen = run(
        json!({
            "id": "w", "name": "w", "priority": 0,
            "loop": {"init": 0, "max": 2, "increment": 1},
            "tasks": [record_task("body")]
        }),
        Recorder::default(),
    )
    .await;

    assert_eq!(
        seen.iter().map(|s| s.loop_counter).collect::<Vec<_>>(),
        vec![Some(0), Some(1)]
    );
}

#[tokio::test]
async fn loop_counter_is_none_outside_a_loop() {
    // Distinct from identity being unknown: both ids are present here.
    let seen = run(
        json!({
            "id": "w", "name": "w", "priority": 0,
            "tasks": [record_task("once")]
        }),
        Recorder::default(),
    )
    .await;

    assert_eq!(seen[0].loop_counter, None);
    assert!(seen[0].workflow_id.is_some() && seen[0].task_id.is_some());
}

#[test]
fn a_directly_constructed_context_reports_no_identity() {
    // `TaskContext::new` is the documented entry point for tests and benches
    // driving a handler outside a workflow run. Inventing ids there would be
    // worse than admitting their absence.
    let datalogic = Arc::new(datalogic_rs::Engine::new());
    let mut message = Message::from_value(&json!({}));
    let ctx = TaskContext::new(&mut message, &datalogic);

    assert_eq!(ctx.workflow_id(), None);
    assert_eq!(ctx.task_id(), None);
    assert_eq!(ctx.loop_counter(), None);
}
