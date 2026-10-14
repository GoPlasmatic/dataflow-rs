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
use super::message::{AuditEntry, keeps_entries, record_audit};
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
    // A mode that keeps nothing builds no entry for `record_audit` to drop.
    if keeps_entries(env.mode) {
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
    } else {
        env.changes.clear();
    }
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

    /// `metadata.progress.<key>`: where the view's in-place update marks
    /// this key's value.
    fn leaf(self) -> &'static Path {
        fn parse(key: Key) -> Path {
            format!("metadata.progress.{}", key.name())
                .parse()
                .expect("a progress key is a path")
        }
        static WORKFLOW: LazyLock<Path> = LazyLock::new(|| parse(Key::Workflow));
        static TASK: LazyLock<Path> = LazyLock::new(|| parse(Key::Task));
        static STATUS: LazyLock<Path> = LazyLock::new(|| parse(Key::Status));
        match self {
            Self::Workflow => &WORKFLOW,
            Self::Task => &TASK,
            Self::Status => &STATUS,
        }
    }

    /// The value this key takes, borrowed from the program.
    fn value<'p>(self, workflow: &'p str, task: &'p str, status: u16) -> DataValue<'p> {
        match self {
            Self::Workflow => DataValue::String(workflow),
            Self::Task => DataValue::String(task),
            Self::Status => DataValue::Number(NumberValue::Integer(i64::from(status))),
        }
    }
}

/// The key order of a fresh progress object. v3 overwrites a progress
/// object of exactly three keys, each one of the three, in place, so its
/// keys keep their order; anything else it replaces with a fresh object in
/// this order.
const FRESH: [Key; 3] = [Key::Workflow, Key::Task, Key::Status];

/// The keys of an object v3 overwrites in place, in its order: exactly
/// three, each one of the three. `None` for any other object.
fn in_place<'k>(keys: impl ExactSizeIterator<Item = &'k str>) -> Option<[Key; 3]> {
    if keys.len() != 3 {
        return None;
    }
    let mut order = FRESH;
    for (slot, key) in order.iter_mut().zip(keys) {
        *slot = Key::of(key)?;
    }
    Some(order)
}

/// v3's `overwrite_progress_in_place`: an object [`in_place`] accepts takes
/// the three values where they are, reusing its strings' buffers, so its
/// keys keep their order and nothing is allocated once the ids fit. `false`
/// for anything else, which the caller replaces.
fn overwrite_in_place(slot: &mut OwnedDataValue, workflow: &str, task: &str, status: u16) -> bool {
    let OwnedDataValue::Object(pairs) = slot else {
        return false;
    };
    let Some(order) = in_place(pairs.iter().map(|(k, _)| k.as_str())) else {
        return false;
    };
    for ((_, value), key) in pairs.iter_mut().zip(order) {
        match key {
            Key::Workflow => overwrite_str(value, workflow),
            Key::Task => overwrite_str(value, task),
            Key::Status => *value = OwnedDataValue::from(status),
        }
    }
    true
}

/// A leaf set to the string `text`, in its own buffer when it holds a
/// string: v3's `overwrite_str_in_place`.
fn overwrite_str(slot: &mut OwnedDataValue, text: &str) {
    match slot {
        OwnedDataValue::String(s) => {
            if s != text {
                s.clear();
                s.push_str(text);
            }
        }
        _ => *slot = OwnedDataValue::String(text.to_owned()),
    }
}

/// Whether the view's `old` already is `new`, exactly: a string of the same
/// text, or the same integer. A float of an integer's value is not it, since
/// v3 writes the integer over it.
fn unchanged(old: DataValue<'_>, new: DataValue<'_>) -> bool {
    match (old, new) {
        (DataValue::String(a), DataValue::String(b)) => a == b,
        (
            DataValue::Number(NumberValue::Integer(a)),
            DataValue::Number(NumberValue::Integer(b)),
        ) => a == b,
        _ => false,
    }
}

/// [`overwrite_in_place`] against the view: an object [`in_place`] accepts
/// keeps its keys and their order and takes the values that differ. A value
/// already equal, as the workflow id and the status usually are, is neither
/// written nor marked, and when one value changes, as the task id alone
/// usually does, the checkpoint assigns it alone, where it is
/// (`Edit::overwrite`). `false` for anything else, which the caller
/// replaces.
fn overwrite_in_view<'p>(
    rc: &mut RunContext<'_, 'p>,
    workflow: &'p str,
    task: &'p str,
    status: u16,
) -> bool {
    rc.edit(|e| {
        e.overwrite(&PROGRESS, |pairs| {
            let order = in_place(pairs.iter().map(|(k, _)| *k))?;
            Some(std::array::from_fn::<_, 3, _>(|i| {
                let new = order[i].value(workflow, task, status);
                (
                    order[i].leaf(),
                    (!unchanged(pairs[i].1, new)).then_some(new),
                )
            }))
        })
    })
}

/// The progress write, `{workflow_id, task_id, status_code}`, by v3's rule
/// on both sides of the view: a progress object of exactly its three keys is
/// updated in place ([`overwrite_in_place`] with no view,
/// [`overwrite_in_view`] with one), and anything else is replaced by a fresh
/// object in the [`FRESH`] order: built owned with no view, and with one
/// spliced from the program's strings, converting nothing until a
/// checkpoint, which converts it.
fn write_progress<'p>(rc: &mut RunContext<'_, 'p>, workflow: &'p str, task: &'p str, status: u16) {
    if rc.engine_update(&PROGRESS, |slot| {
        overwrite_in_place(slot, workflow, task, status)
    }) {
        return;
    }
    if rc.has_view() && overwrite_in_view(rc, workflow, task, status) {
        return;
    }
    rc.engine_set(
        &PROGRESS,
        || {
            OwnedDataValue::Object(
                FRESH
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
            DataValue::Object(owner.arena.alloc_slice_copy(
                &FRESH.map(|key| (key.name(), key.value(workflow, task, status))),
            ))
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Tuning;

    fn text(v: &OwnedDataValue) -> String {
        serde_json::to_string(v).unwrap()
    }

    /// A context with `progress` (JSON text, its key order kept) seeded.
    fn context(progress: &str) -> OwnedDataValue {
        let text =
            format!(r#"{{"data":{{}},"metadata":{{"progress":{progress}}},"temp_data":{{}}}}"#);
        serde_json::from_str(&text).unwrap()
    }

    /// The buffer of the string at `metadata.progress.<key>`.
    fn buffer(ctx: &OwnedDataValue, key: &str) -> *const u8 {
        match &ctx["metadata"]["progress"][key] {
            OwnedDataValue::String(s) => s.as_ptr(),
            other => panic!("{key} holds {other:?}"),
        }
    }

    #[test]
    fn with_no_view_a_progress_of_its_three_keys_is_updated_where_it_is() {
        let mut ctx = context(r#"{"task_id":"a_long_task","status_code":1,"workflow_id":"wf"}"#);
        let before = (buffer(&ctx, "task_id"), buffer(&ctx, "workflow_id"));
        let mut rc = RunContext::new(&mut ctx, Tuning::default());
        write_progress(&mut rc, "wf", "t", 200);
        assert!(!rc.has_view(), "the write built no view");
        rc.finish();
        assert_eq!(
            text(&ctx["metadata"]["progress"]),
            r#"{"task_id":"t","status_code":200,"workflow_id":"wf"}"#,
            "the keys keep their order"
        );
        assert_eq!(
            (buffer(&ctx, "task_id"), buffer(&ctx, "workflow_id")),
            before,
            "both ids were written into the strings already there"
        );
    }

    /// The buffer of the progress object's pairs.
    fn pairs(ctx: &OwnedDataValue) -> *const (String, OwnedDataValue) {
        match &ctx["metadata"]["progress"] {
            OwnedDataValue::Object(pairs) => pairs.as_ptr(),
            other => panic!("progress holds {other:?}"),
        }
    }

    #[test]
    fn with_a_view_a_progress_of_its_three_keys_is_updated_where_it_is() {
        let mut ctx = context(r#"{"task_id":"a_long_task","status_code":200,"workflow_id":"wf"}"#);
        let before = (pairs(&ctx), buffer(&ctx, "workflow_id"));
        let mut rc = RunContext::new(&mut ctx, Tuning::default());
        rc.read(|_, _| ());
        write_progress(&mut rc, "wf", "t", 200);
        let stats = rc.finish();
        assert_eq!(
            text(&ctx["metadata"]["progress"]),
            r#"{"task_id":"t","status_code":200,"workflow_id":"wf"}"#,
            "the keys keep their order"
        );
        assert_eq!(stats.converted, 1, "only the task id changed: {stats:?}");
        assert_eq!(
            (pairs(&ctx), buffer(&ctx, "workflow_id")),
            before,
            "the object and the id that did not change stayed where they were"
        );

        // The same write again finds every value already there.
        let mut rc = RunContext::new(&mut ctx, Tuning::default());
        rc.read(|_, _| ());
        write_progress(&mut rc, "wf", "t", 200);
        assert!(
            matches!(rc.get(&PROGRESS), Some(Cow::Borrowed(_))),
            "nothing was marked"
        );
        assert_eq!(rc.finish().converted, 0);
    }

    #[test]
    fn with_a_view_the_progress_write_leaves_what_it_leaves_without_one() {
        // A key that comes twice is overwritten twice, as v3's loop over the
        // pairs does, and the object is then marked as a whole.
        for seeded in [
            r#"{"workflow_id":"wf","task_id":"t0","status_code":200}"#,
            r#"{"status_code":1,"workflow_id":"w0","task_id":"t"}"#,
            r#"{"task_id":5,"workflow_id":null,"status_code":"s"}"#,
            r#"{"task_id":"t","status_code":200.5,"workflow_id":"wf"}"#,
            r#"{"task_id":"y","task_id":"z","status_code":1}"#,
            r#"{"task_id":"y","status_code":1}"#,
            r#"{"task_id":"y","status_code":1,"other":0}"#,
            r#"{"task_id":"y","status_code":1,"workflow_id":"x","extra":0}"#,
            "[1,2,3]",
            "null",
        ] {
            let run = |viewed: bool| {
                let mut ctx = context(seeded);
                let mut rc = RunContext::new(&mut ctx, Tuning::default());
                let mut seen = Vec::new();
                for (workflow, task, status) in [
                    ("wf", "t", 200),
                    ("wf", "a_longer_task", 404),
                    ("w2", "a_longer_task", 404),
                    ("w2", "a_longer_task", 404),
                ] {
                    if viewed {
                        rc.read(|_, _| ());
                    }
                    write_progress(&mut rc, workflow, task, status);
                    seen.push(text(&rc.get(&PROGRESS).expect("a progress was written")));
                }
                assert_eq!(rc.has_view(), viewed);
                rc.finish();
                seen.push(text(&ctx["metadata"]["progress"]));
                seen
            };
            assert_eq!(run(true), run(false), "{seeded}");
        }
    }

    #[test]
    fn any_other_progress_is_replaced_in_the_fresh_order() {
        for seeded in [
            r#"{"task_id":"y","status_code":1}"#,
            r#"{"task_id":"y","status_code":1,"other":0}"#,
            r#"{"task_id":"y","status_code":1,"workflow_id":"x","extra":0}"#,
            "[1,2,3]",
        ] {
            let mut ctx = context(seeded);
            let mut rc = RunContext::new(&mut ctx, Tuning::default());
            write_progress(&mut rc, "wf", "t", 200);
            rc.finish();
            assert_eq!(
                text(&ctx["metadata"]["progress"]),
                r#"{"workflow_id":"wf","task_id":"t","status_code":200}"#,
                "{seeded}"
            );
        }
    }
}
