//! The recorder: what one completed call leaves on the message.
//!
//! Once per completed call, from the call's [`Decision`](super::policy::Decision):
//! when it has a status, the audit entry (through `AuditMode`'s trim, with the
//! sweep's `loop_counter`), then the progress write, then the error record, if
//! any, with the workflow and task ids and the run's timestamp. A `Skip`
//! records nothing. This is v3's `handle_task_result`, in its order.
//!
//! A workflow's own failure, which no task made, records one error record
//! and nothing else (I61): [`workflow_failure`].

use super::interp::RunEnv;
use super::message::{AuditEntry, record_audit};
use crate::base::{Error, ErrorRecord};
use crate::expr::Path;
use crate::function::task_record;
use crate::state::RunContext;
use datavalue::{DataValue, NumberValue, OwnedDataValue};
use std::borrow::Cow;
use std::sync::{Arc, LazyLock};

/// `metadata.progress`, which the engine writes after every call that does
/// not skip. Cross-workflow chaining reads it, so it is never gated.
static PROGRESS: LazyLock<Path> = LazyLock::new(|| {
    "metadata.progress"
        .parse()
        .expect("`metadata.progress` is a path")
});

/// The task a record is for: its ids, as the program owns them.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TaskIds<'p> {
    pub(crate) workflow: &'p Arc<str>,
    pub(crate) task: &'p Arc<str>,
}

/// Records one completed call: `status` and `record` are its decision's. The
/// call's changes are taken from `env.changes`.
pub(crate) fn record<'p>(
    rc: &mut RunContext<'_, 'p>,
    env: &mut RunEnv<'_, 'p>,
    ids: TaskIds<'p>,
    status: Option<u16>,
    record: Option<(Cow<'static, str>, String)>,
) {
    let Some(status) = status else {
        env.changes.clear();
        return;
    };
    record_audit(
        env.audit,
        env.mode,
        AuditEntry {
            workflow_id: Arc::clone(ids.workflow),
            task_id: Arc::clone(ids.task),
            timestamp: env.now,
            changes: std::mem::take(&mut env.changes),
            status: usize::from(status),
            loop_counter: env.frame.loop_counter,
            element_index: None,
        },
    );
    write_progress(rc, ids.workflow, ids.task, status);
    if let Some((code, message)) = record {
        let (workflow, task) = (&**ids.workflow, &**ids.task);
        env.errors
            .push(task_record(&code, message, workflow, task, env.now_text));
    }
}

/// Records a workflow's own failure (I61), whatever its policy: one record
/// with the error's code and message, the workflow's id, no task id, and the
/// run's timestamp. No audit entry and no progress write: no task completed.
pub(crate) fn workflow_failure(env: &mut RunEnv<'_, '_>, error: &Error) {
    let mut record = ErrorRecord::from(error);
    record.workflow_id = Some(env.workflow_id.to_string());
    record.timestamp = Some(env.now_text.to_owned());
    env.errors.push(record);
}

/// One of the progress object's keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Key {
    Workflow,
    Task,
    Status,
}

impl Key {
    fn of(key: &str) -> Option<Self> {
        match key {
            "workflow_id" => Some(Self::Workflow),
            "task_id" => Some(Self::Task),
            "status_code" => Some(Self::Status),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Workflow => "workflow_id",
            Self::Task => "task_id",
            Self::Status => "status_code",
        }
    }
}

/// The key order a progress write keeps. v3 overwrites a progress object of
/// exactly three keys, each one of the three, in place, so its keys keep
/// their order; anything else it replaces with a fresh object in this order.
const FRESH: [Key; 3] = [Key::Workflow, Key::Task, Key::Status];

/// The order of `keys`, by v3's rule.
fn order<'k>(keys: impl ExactSizeIterator<Item = &'k str>) -> [Key; 3] {
    if keys.len() != 3 {
        return FRESH;
    }
    let mut order = FRESH;
    for (slot, key) in order.iter_mut().zip(keys) {
        match Key::of(key) {
            Some(key) => *slot = key,
            None => return FRESH,
        }
    }
    order
}

/// The key order the progress write keeps: read from the view when there is
/// one, which builds nothing, and from the owned tree otherwise.
fn current_order(rc: &mut RunContext<'_, '_>) -> [Key; 3] {
    if rc.has_view() {
        rc.read(|_, root| {
            let progress = root
                .as_object()
                .and_then(|roots| roots.iter().find(|(k, _)| *k == "metadata"))
                .and_then(|(_, metadata)| metadata.as_object())
                .and_then(|keys| keys.iter().find(|(k, _)| *k == "progress"));
            match progress.and_then(|(_, p)| p.as_object()) {
                Some(pairs) => order(pairs.iter().map(|(k, _)| *k)),
                None => FRESH,
            }
        })
    } else {
        match rc.get(&PROGRESS).as_deref() {
            Some(OwnedDataValue::Object(pairs)) => order(pairs.iter().map(|(k, _)| k.as_str())),
            _ => FRESH,
        }
    }
}

/// The progress write, `{workflow_id, task_id, status_code}` in the order
/// [`current_order`] gives: owned when no view exists, otherwise spliced into
/// the view from the program's strings, converting nothing until a
/// checkpoint.
fn write_progress<'p>(rc: &mut RunContext<'_, 'p>, workflow: &'p str, task: &'p str, status: u16) {
    let order = current_order(rc);
    rc.engine_set(
        &PROGRESS,
        || {
            OwnedDataValue::Object(
                order
                    .iter()
                    .map(|key| {
                        let value = match key {
                            Key::Workflow => OwnedDataValue::String(workflow.to_owned()),
                            Key::Task => OwnedDataValue::String(task.to_owned()),
                            Key::Status => OwnedDataValue::from(status),
                        };
                        (key.name().to_owned(), value)
                    })
                    .collect(),
            )
        },
        |owner| {
            let value = |key: Key| match key {
                Key::Workflow => DataValue::String(workflow),
                Key::Task => DataValue::String(task),
                Key::Status => DataValue::Number(NumberValue::Integer(i64::from(status))),
            };
            DataValue::Object(
                owner
                    .arena
                    .alloc_slice_copy(&order.map(|key| (key.name(), value(key)))),
            )
        },
    );
}
