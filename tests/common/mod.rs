//! Fixtures shared by the integration test binaries.
//!
//! Every file directly under `tests/` compiles as its own crate, so anything
//! more than one of them needs lives here and is pulled in with `mod common;`.
//! No single binary uses all of it — hence the module-wide dead-code allowance.

#![allow(dead_code)]

use async_trait::async_trait;
use dataflow_rs::engine::functions::AsyncFunctionHandler;
use dataflow_rs::{Result, TaskContext, TaskOutcome, Workflow};
use datavalue::OwnedDataValue;
use serde_json::Value;

/// Bridge helper for tests: build an `OwnedDataValue` from a `json!` literal.
pub fn dv(v: serde_json::Value) -> OwnedDataValue {
    OwnedDataValue::from(&v)
}

// A simple async task implementation
#[derive(Debug)]
pub struct LoggingTask;

#[async_trait]
impl AsyncFunctionHandler for LoggingTask {
    type Input = Value;

    async fn execute(&self, ctx: &mut TaskContext<'_>, _input: &Value) -> Result<TaskOutcome> {
        println!("Executed task for message: {}", ctx.message().id());
        Ok(TaskOutcome::Success)
    }
}

// Handler that always returns Err — used by the single-error-channel
// regression tests.
pub struct FailingTask;

#[async_trait]
impl AsyncFunctionHandler for FailingTask {
    type Input = Value;

    async fn execute(&self, _ctx: &mut TaskContext<'_>, _input: &Value) -> Result<TaskOutcome> {
        Err(dataflow_rs::DataflowError::Task("boom".to_string()))
    }
}

// Handler that returns a 500 status — used by the single-error-channel
// regression tests.
pub struct FivehundredTask;

#[async_trait]
impl AsyncFunctionHandler for FivehundredTask {
    type Input = Value;

    async fn execute(&self, _ctx: &mut TaskContext<'_>, _input: &Value) -> Result<TaskOutcome> {
        Ok(TaskOutcome::Status(500))
    }
}

// An async task implementation
pub struct AsyncLoggingTask;

#[async_trait]
impl AsyncFunctionHandler for AsyncLoggingTask {
    type Input = Value;

    async fn execute(&self, ctx: &mut TaskContext<'_>, _input: &Value) -> Result<TaskOutcome> {
        println!("Executed async task for message: {}", ctx.message().id());
        // Simulate async work
        tokio::time::sleep(tokio::time::Duration::from_millis(1)).await;
        Ok(TaskOutcome::Success)
    }
}

/// N map tasks, each writing one key, as one workflow.
pub fn n_map_task_workflow(n: usize) -> Workflow {
    let tasks: Vec<String> = (0..n)
        .map(|i| {
            format!(
                r#"{{ "id": "t{i}", "name": "t{i}", "function": {{
                       "name": "map",
                       "input": {{ "mappings": [ {{ "path": "data.k{i}", "logic": {i} }} ] }} }} }}"#
            )
        })
        .collect();
    Workflow::from_json(&format!(
        r#"{{ "id": "w", "name": "w", "priority": 0, "condition": true,
              "tasks": [{}] }}"#,
        tasks.join(",")
    ))
    .unwrap()
}

/// One recorded observer callback.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SeenEvent {
    pub workflow_id: String,
    pub task_id: String,
    pub function: String,
    pub status: Option<u16>,
}

/// Records every event it is handed. `Mutex` is fine for a test; a real observer
/// must not block, per the trait contract.
#[derive(Default)]
pub struct RecordingObserver {
    events: std::sync::Mutex<Vec<SeenEvent>>,
}

impl RecordingObserver {
    pub fn seen(&self) -> Vec<SeenEvent> {
        self.events.lock().unwrap().clone()
    }
}

impl dataflow_rs::ExecutionObserver for RecordingObserver {
    fn task_finished(&self, event: &dataflow_rs::TaskEvent<'_>) {
        self.events.lock().unwrap().push(SeenEvent {
            workflow_id: event.workflow_id.to_string(),
            task_id: event.task_id.to_string(),
            function: event.function.to_string(),
            status: event.status,
        });
    }
}
