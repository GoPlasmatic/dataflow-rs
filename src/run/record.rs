//! The recorder: what one completed call leaves on the message, and what
//! the run's observers are told of it.
//!
//! Once per completed call, from what the call's
//! [`Decision`](super::policy::Decision) and the call left ([`Completed`]):
//! when it has a status, the audit entry (through `AuditMode`'s trim, with the
//! call's [`Stamp`]), then the progress write, then the error record, if any,
//! with the workflow and task ids, the run's timestamp and the stamp's
//! element, then, with an error context, the mirror of every record the call
//! added; then, for a `Skip` too, the trace's step and the observers'
//! `TaskFinished`. This is v3's `handle_task_result`, in its order. A fan-out
//! records once per element it folds.
//!
//! The trace's step carries, while the trace keeps changes, the engine's
//! writes made for it (the capture's prelude), the call's changes, the
//! progress write's `Change` and the error context's, and for a `map`, where
//! each of its mappings ends in them; the audit entry carries the call's
//! changes alone, and only under `capture_audit`, so the engine's
//! writes never reach it. The step also carries every record the call
//! added to the message, its own and the recorder's, in order: the
//! records after `errors_before`, read once the call has returned.
//!
//! A workflow's own failure, which no task made, records one error record,
//! and the trace's `Failed` step for it, and nothing else:
//! [`workflow_failure`].
//!
//! Every clock reading for an elapsed time goes through [`now`] and
//! [`since`], and only for an interest in it: a call's for an observer of
//! the task events that asked `TIMINGS`, or a trace's timings; a workflow's
//! for an observer of the workflow events that asked it; the run's for any
//! `TIMINGS`. A step's `elapsed` is kept only for the trace's timings.

use super::engine::{metadata, slots};
use super::interp::RunEnv;
use super::message::{AuditEntry, keeps_entries, record_audit};
use super::policy::{Decision, Flow, RecordMeta};
use crate::base::{Change, Error, ErrorRecord, Instant};
use crate::expr::Path;
use crate::function::task_record;
use crate::observe::{Event, StepResult};
use crate::program::CompiledWorkflow;
use crate::state::RunContext;
use datavalue::{DataValue, NumberValue, OwnedDataValue};
use std::borrow::Cow;
use std::sync::{Arc, LazyLock};
use std::time::Duration;

/// `metadata.progress`, which the engine writes after every call that does
/// not skip. Cross-workflow chaining reads it, so it is never gated.
pub(super) static PROGRESS: LazyLock<Path> = LazyLock::new(|| metadata(slots::PROGRESS));

/// The task a record is for: its ids, as the program owns them.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TaskIds<'p> {
    pub(crate) workflow: &'p Arc<str>,
    pub(crate) task: &'p Arc<str>,
}

/// Where a completed call sits in the run's iterations: the sweep's loop
/// counter, and the element's index in a fan-out. The audit entry carries
/// both, and the call's error record the element.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Stamp {
    /// The loop counter of the sweep the call ran in; `None` outside a
    /// loop's sweeps.
    pub(crate) loop_counter: Option<i64>,
    /// The element the call ran for, in a fan-out.
    pub(crate) element_index: Option<usize>,
}

impl Stamp {
    /// A call outside a fan-out, in the sweep the frame is in.
    pub(crate) fn sweep(env: &RunEnv<'_, '_>) -> Self {
        Self {
            loop_counter: env.frame.loop_counter,
            element_index: None,
        }
    }
}

/// The run's monotonic clock, read only when a deadline or timing is active:
/// natively Tokio's clock (so paused time governs both deadlines and elapsed
/// timings), and on wasm the system clock.
pub(crate) fn now() -> Instant {
    #[cfg(test)]
    super::tests::on_clock_read();
    #[cfg(not(target_arch = "wasm32"))]
    {
        tokio::time::Instant::now().into_std()
    }
    #[cfg(target_arch = "wasm32")]
    {
        Instant::now()
    }
}

/// The time since `start`, on [`now`]'s clock.
pub(crate) fn since(start: Instant) -> Duration {
    now().saturating_duration_since(start)
}

/// One recorded call: what the call left, beside its decision, for the
/// recorder's outputs.
pub(crate) struct Completed {
    /// The task's row in the workflow, for its ids, `TaskIx` and the
    /// function's name.
    pub(crate) task: u32,
    /// Where the call sits in the run's iterations.
    pub(crate) stamp: Stamp,
    /// How long the call ran, when the run is timed.
    pub(crate) elapsed: Option<Duration>,
    /// `env.errors.len()` before the call: the error context mirrors what
    /// came after.
    pub(crate) errors_before: usize,
    /// A change of the call is the trace's alone ([`Change::for_trace`]) or
    /// says `replaced`: only then is the audit's copy of the changes made
    /// over ([`audited`]).
    pub(crate) marked: bool,
}

impl Completed {
    /// One completed call of `task`, in `env`'s sweep, at `element_index` in
    /// a fan-out.
    pub(crate) fn new(
        env: &RunEnv<'_, '_>,
        task: u32,
        element_index: Option<usize>,
        elapsed: Option<Duration>,
        errors_before: usize,
        marked: bool,
    ) -> Self {
        Self {
            task,
            stamp: Stamp {
                element_index,
                ..Stamp::sweep(env)
            },
            elapsed,
            errors_before,
            marked,
        }
    }
}

/// Records one completed call of `wf` under its `decision`, in this order:
/// the audit entry, with the call's changes, taken from `env.changes`, under
/// [`CaptureMode::audit`](super::interp::CaptureMode::audit) (through
/// `AuditMode`), the progress write, the error record, the error-context
/// record, the trace's step, and the observers' `TaskFinished`. A `Skip` gets
/// the last two only. The decision's flow.
///
/// The first three steps are [`record_core`], called by both paths. A call of
/// an unwatched run that marked no change has only those three to record,
/// through [`plain`]: the run's other outputs cost it one branch.
#[inline(always)]
pub(crate) fn record<'p>(
    rc: &mut RunContext<'_, 'p>,
    env: &mut RunEnv<'_, 'p>,
    wf: &'p CompiledWorkflow,
    done: Completed,
    decision: Decision,
) -> Flow {
    let Decision {
        status,
        record,
        meta,
        flow,
    } = decision;
    #[cfg(test)]
    super::tests::completed(env, &done, status, &flow);
    if env.capture.is_watched() || done.marked {
        // The decision's flow is `Fail`, `Abort` or `Stop`: the trace's `Failed`.
        let failed = matches!(flow, Flow::Fail(_) | Flow::Abort(_) | Flow::Stop(_));
        watched(rc, env, wf, done, status, record, meta, failed);
    } else {
        plain(rc, env, done.task, done.stamp, status, record, meta);
    }
    flow
}

/// The first three recorder steps for a call with `status`, in one order for
/// [`plain`] and [`watched`]:
///
/// 1. the audit entry (when `env.mode` keeps entries) with `audit_changes`;
/// 2. the progress write (saving the old value first when `env.traces_changes()`);
/// 3. the decision's error record onto `env.errors`.
///
/// Returns the `Change` of `metadata.progress` when the trace keeps changes.
#[inline(always)]
#[allow(clippy::too_many_arguments)]
fn record_core<'p>(
    rc: &mut RunContext<'_, 'p>,
    env: &mut RunEnv<'_, 'p>,
    ids: TaskIds<'p>,
    stamp: Stamp,
    status: u16,
    record: Option<(Cow<'static, str>, String)>,
    meta: Option<RecordMeta>,
    audit_changes: impl FnOnce(&mut RunEnv<'_, 'p>) -> Vec<Change>,
) -> Option<Change> {
    // A mode that keeps nothing builds no entry for `record_audit` to drop.
    if keeps_entries(env.mode) {
        let changes = audit_changes(env);
        audit(env, ids, stamp, status, changes);
    }
    let old = env.traces_changes().then(|| rc.get_or_null(&PROGRESS));
    write_progress(rc, ids.workflow, ids.task, status);
    let progress_change = old.map(|old| {
        Change::write(
            Arc::clone(PROGRESS.dotted()),
            old,
            rc.get_or_null(&PROGRESS),
        )
    });
    if let Some(record) = record {
        push_record(env, ids, stamp, record, meta);
    }
    progress_change
}

/// [`record`] for a call of an unwatched run that marked no change: the
/// audit entry, with every change the call recorded, the progress write and
/// the error record, through [`record_core`]. A `Skip` records nothing.
#[inline]
fn plain<'p>(
    rc: &mut RunContext<'_, 'p>,
    env: &mut RunEnv<'_, 'p>,
    task: u32,
    stamp: Stamp,
    status: Option<u16>,
    record: Option<(Cow<'static, str>, String)>,
    meta: Option<RecordMeta>,
) {
    if let Some(status) = status {
        let ids = env.ids(task);
        record_core(rc, env, ids, stamp, status, record, meta, |env| {
            std::mem::take(&mut env.changes)
        });
    }
    env.changes.clear();
}

/// The call's audit entry, with `changes`, through `AuditMode`'s trim.
#[inline(always)]
fn audit(
    env: &mut RunEnv<'_, '_>,
    ids: TaskIds<'_>,
    stamp: Stamp,
    status: u16,
    changes: Vec<Change>,
) {
    record_audit(
        env.audit,
        env.mode,
        AuditEntry {
            workflow_id: Arc::clone(ids.workflow),
            task_id: Arc::clone(ids.task),
            timestamp: env.now,
            changes,
            status: usize::from(status),
            loop_counter: stamp.loop_counter,
            element_index: stamp.element_index,
        },
    );
}

/// The decision's error record, with the ids, the run's timestamp and the
/// stamp's element, onto the message's records.
#[inline(always)]
fn push_record(
    env: &mut RunEnv<'_, '_>,
    ids: TaskIds<'_>,
    stamp: Stamp,
    (code, message): (Cow<'static, str>, String),
    meta: Option<RecordMeta>,
) {
    let now = env.now_text();
    let mut record = task_record(code, message, ids.workflow, ids.task, now);
    record.element_index = stamp.element_index;
    if let Some((cause, site)) = meta {
        record.cause = cause;
        record.site = site;
    }
    env.errors.push(record);
}

/// [`record`] for a call of a watched run, or one that marked a change:
/// everything the recorder makes.
#[cold]
#[inline(never)]
#[allow(clippy::too_many_arguments)]
fn watched<'p>(
    rc: &mut RunContext<'_, 'p>,
    env: &mut RunEnv<'_, 'p>,
    wf: &'p CompiledWorkflow,
    done: Completed,
    status: Option<u16>,
    record: Option<(Cow<'static, str>, String)>,
    meta: Option<RecordMeta>,
    failed: bool,
) {
    let Completed {
        task,
        stamp,
        elapsed,
        errors_before,
        marked,
    } = done;
    let ids = env.ids(task);
    // The step's changes, while the trace keeps them: the engine's writes
    // made for it first, then the call's, then the progress write, then the
    // error-context write.
    let traced = env.traces_changes();
    let mut changes = match env.watch.trace.as_deref_mut() {
        Some(trace) if traced => trace.take_prelude(),
        _ => Vec::new(),
    };
    // The call's own changes follow the prelude: its mapping ends move past
    // it.
    let prelude = changes.len();
    if let Some(status) = status {
        let progress = record_core(rc, env, ids, stamp, status, record, meta, |env| {
            if env.capture.audit() {
                // When the trace wants them too, the audit gets a copy.
                let mut changes = if traced {
                    env.changes.clone()
                } else {
                    std::mem::take(&mut env.changes)
                };
                // The audit's copy keeps v3's bytes.
                if marked {
                    changes.retain_mut(audited);
                }
                changes
            } else {
                Vec::new()
            }
        });
        if let Some(progress) = progress {
            changes.append(&mut env.changes);
            changes.push(progress);
        }
        // Every record the call added, its own and the recorder's.
        if let Some(ec) = env.watch.error_context
            && let Some(change) = ec.mirror(
                rc,
                &env.errors[errors_before..],
                ids.workflow,
                ids.task,
                status,
                traced,
            )
        {
            changes.push(change);
        }
    } else if traced {
        // A `Skip`'s writes stand: its step carries them.
        changes.append(&mut env.changes);
    }
    env.changes.clear();
    if let Some(trace) = env.watch.trace.as_deref_mut() {
        let timings = trace.timings();
        let mapping_ends = trace.take_mapping_ends(prelude);
        let mut step = env.task_step(
            task,
            if failed {
                StepResult::Failed
            } else {
                StepResult::Executed
            },
        );
        step.status = status;
        step.changes = changes;
        step.mapping_ends = mapping_ends;
        // Every record the call added, in the message's order.
        #[cfg(test)]
        super::tests::on_trace_work();
        step.errors = env.errors[errors_before..].to_vec();
        step.elapsed = elapsed.filter(|_| timings);
        step.element_index = stamp.element_index;
        env.push_step(step);
    }
    if env.watch.observers.tasks() {
        let info = env.call_info(wf, task, stamp);
        env.watch.observers.emit(Event::TaskFinished {
            info: &info,
            status,
            elapsed,
        });
    }
}

/// Whether a change of a call that marked one stays in its audit entry,
/// made as the entry carries it, so the entry is v3's bytes: a change the
/// trace alone keeps does not, and `replaced` is the trace's alone (the
/// trace is the replay surface, and the audit trail keeps v3's bytes), so
/// it is cleared.
fn audited(change: &mut Change) -> bool {
    change.replaced = false;
    !change.trace_only
}

/// Records a workflow's own failure, whatever its policy: one record
/// with the error's code and message, the workflow's id, no task id, and the
/// run's timestamp, and in a traced run the workflow's `Failed` step, with
/// the record as the one it added. No audit entry and no progress write: no
/// task completed.
pub(crate) fn workflow_failure(env: &mut RunEnv<'_, '_>, error: &Error) {
    let mut record = env.workflow_record(error);
    let workflow_id = env.workflow_id;
    let now = env.now_text();
    record.stamp(workflow_id, None, now, None);
    if env.watch.trace.is_some() {
        let mut step = env.workflow_step(StepResult::Failed);
        #[cfg(test)]
        super::tests::on_trace_work();
        step.errors = vec![record.clone()];
        env.push_step(step);
    }
    env.errors.push(record);
}

/// Records a task condition failure under `ConditionErrorPolicy::Stop`:
/// pushes an [`ErrorRecord`] stamped with the workflow and task ids, and in a
/// traced run pushes a `Failed` task step carrying that record, without
/// fabricating a task execution audit entry or progress write.
pub(crate) fn task_condition_failure(env: &mut RunEnv<'_, '_>, task: u32, error: &Error) {
    let mut record = ErrorRecord::from(error);
    let ids = env.ids(task);
    let workflow_id = Arc::clone(ids.workflow);
    let task_id = Arc::clone(ids.task);
    let now = env.now_text();
    record.stamp(&workflow_id, Some(&task_id), now, None);
    if env.watch.trace.is_some() {
        let mut step = env.task_step(task, StepResult::Failed);
        #[cfg(test)]
        super::tests::on_trace_work();
        step.errors = vec![record.clone()];
        env.push_step(step);
    }
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
pub(super) fn write_progress<'p>(
    rc: &mut RunContext<'_, 'p>,
    workflow: &'p str,
    task: &'p str,
    status: u16,
) {
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
                        let value = key.value(workflow, task, status).to_owned();
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
