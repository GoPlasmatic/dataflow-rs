//! An observer that keeps every event as v3's driver recorded it
//! (`corpus/v3/src/observe.rs`): v3's five events, from v4's [`Event`]s, so
//! a case's `observer` array is compared with v3's verbatim (I109).
//!
//! v3's driver built each event with `serde_json::json!`, whose objects sort
//! their keys, so each object here is written with its keys in that sorted
//! order: the order the recordings hold. No duration is kept, as v3's
//! recorder kept none, so the interest is `WORKFLOWS | TASKS` and no clock is
//! read.

use dataflow_corpus::Json;
use dataflow_rs::Observer;
use dataflow_rs::observe::{Event, Interest, RunStatus, WorkflowExit};
use std::sync::Mutex;

/// The recording observer: one per engine, drained per run step.
#[derive(Debug, Default)]
pub struct Recorder(Mutex<Vec<Json>>);

impl Recorder {
    /// The events since the last call, in the order they fired.
    pub fn take(&self) -> Vec<Json> {
        std::mem::take(&mut *self.0.lock().expect("the recorder lock"))
    }
}

/// An object with these keys, in this order.
fn object(pairs: Vec<(&str, Json)>) -> Json {
    Json::Object(pairs.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
}

fn number(n: impl Into<serde_json::Number>) -> Json {
    Json::Number(n.into())
}

impl Observer for Recorder {
    fn interest(&self) -> Interest {
        Interest::WORKFLOWS | Interest::TASKS
    }

    fn on(&self, event: &Event<'_>) {
        let json = match event {
            Event::RunStarted {
                message_id,
                considered,
                ..
            } => object(vec![
                ("event", Json::str("message_started")),
                ("message_id", Json::str(*message_id)),
                ("workflows_considered", number(*considered)),
            ]),
            Event::WorkflowStarted { id, .. } => object(vec![
                ("event", Json::str("workflow_started")),
                ("workflow_id", Json::str(*id)),
            ]),
            Event::TaskFinished { info, status, .. } => object(vec![
                ("event", Json::str("task_finished")),
                ("function", Json::str(info.function)),
                ("status", status.map_or(Json::Null, number)),
                ("task_id", Json::str(info.task_id)),
                ("workflow_id", Json::str(info.workflow_id)),
            ]),
            Event::WorkflowFinished { record, .. } => object(vec![
                ("event", Json::str("workflow_finished")),
                (
                    "halted",
                    Json::Bool(record.exit == Some(WorkflowExit::Halted)),
                ),
                ("sweeps", number(record.sweeps)),
                ("workflow_id", Json::str(record.id)),
            ]),
            Event::RunFinished {
                message_id,
                status,
                errors,
                ..
            } => object(vec![
                ("errors", number(*errors)),
                ("event", Json::str("message_finished")),
                ("message_id", Json::str(*message_id)),
                (
                    "stopped_early",
                    Json::Bool(matches!(
                        status,
                        RunStatus::Stopped(_) | RunStatus::TimedOut | RunStatus::Cancelled
                    )),
                ),
            ]),
            _ => return,
        };
        self.0.lock().expect("the recorder lock").push(json);
    }
}
