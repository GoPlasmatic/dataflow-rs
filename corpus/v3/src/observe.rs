//! An observer that keeps every event as JSON, without its durations, which
//! are the only part of an event that differs between two runs.

use dataflow_corpus::Json;
use dataflow_rs::{
    ExecutionObserver, MessageFinished, MessageStarted, TaskEvent, WorkflowFinished,
    WorkflowStarted,
};
use serde_json::json;
use std::sync::Mutex;

#[derive(Default)]
pub struct Recorder(Mutex<Vec<Json>>);

impl Recorder {
    /// The events since the last call, in the order they fired.
    pub fn take(&self) -> Vec<Json> {
        std::mem::take(&mut *self.0.lock().expect("the recorder lock"))
    }

    fn push(&self, event: &serde_json::Value) {
        self.0
            .lock()
            .expect("the recorder lock")
            .push(Json::from(event));
    }
}

impl ExecutionObserver for Recorder {
    fn message_started(&self, e: &MessageStarted<'_>) {
        self.push(&json!({
            "event": "message_started",
            "message_id": e.message_id,
            "workflows_considered": e.workflows_considered,
        }));
    }

    fn message_finished(&self, e: &MessageFinished<'_>) {
        self.push(&json!({
            "event": "message_finished",
            "message_id": e.message_id,
            "errors": e.errors,
            "stopped_early": e.stopped_early,
        }));
    }

    fn workflow_started(&self, e: &WorkflowStarted<'_>) {
        self.push(&json!({ "event": "workflow_started", "workflow_id": e.workflow_id }));
    }

    fn workflow_finished(&self, e: &WorkflowFinished<'_>) {
        self.push(&json!({
            "event": "workflow_finished",
            "workflow_id": e.workflow_id,
            "sweeps": e.sweeps,
            "halted": e.halted,
        }));
    }

    fn task_finished(&self, e: &TaskEvent<'_>) {
        self.push(&json!({
            "event": "task_finished",
            "workflow_id": e.workflow_id,
            "task_id": e.task_id,
            "function": e.function,
            "status": e.status,
        }));
    }
}
