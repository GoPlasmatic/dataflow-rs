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
use super::policy::{Decision, Flow};
use crate::base::{Change, Error, Instant};
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
static PROGRESS: LazyLock<Path> = LazyLock::new(|| metadata(slots::PROGRESS));

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
    tests::CLOCK_READS.with(|reads| reads.set(reads.get() + 1));
    #[cfg(not(target_arch = "wasm32"))]
    {
        tokio::time::Instant::now().into_std()
    }
    #[cfg(target_arch = "wasm32")]
    {
        Instant::now()
    }
}

/// How many times this thread read [`now`].
#[cfg(test)]
pub(crate) fn clock_reads() -> u32 {
    tests::CLOCK_READS.with(std::cell::Cell::get)
}

/// In the lib's own tests, `result` turned into an `Err` once the call has
/// written, while a test asks: a built-in that writes, then fails, which no
/// built-in does today.
#[cfg(test)]
pub(crate) fn failing_after_its_writes(
    result: crate::Result<crate::function::Outcome>,
) -> crate::Result<crate::function::Outcome> {
    if tests::FAIL_AFTER_WRITES.with(std::cell::Cell::get) {
        return Err(Error::service("BROKE", "after its writes"));
    }
    result
}

/// In the lib's own tests, one piece of the trace's own work done: a
/// step's records copied, or a mapping's end noted. A run without a trace
/// does none.
#[cfg(test)]
pub(crate) fn traced_work() {
    tests::TRACE_WORK.with(|work| work.set(work.get() + 1));
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
                loop_counter: env.frame.loop_counter,
                element_index,
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
        flow,
    } = decision;
    #[cfg(test)]
    tests::completed(env, &done, status, &flow);
    if env.capture.is_watched() || done.marked {
        // The decision's flow is `Fail` or `Stop`: the trace's `Failed`.
        let failed = matches!(flow, Flow::Fail(_) | Flow::Stop(_));
        watched(rc, env, wf, done, status, record, failed);
    } else {
        plain(rc, env, done.task, done.stamp, status, record);
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
fn record_core<'p>(
    rc: &mut RunContext<'_, 'p>,
    env: &mut RunEnv<'_, 'p>,
    ids: TaskIds<'p>,
    stamp: Stamp,
    status: u16,
    record: Option<(Cow<'static, str>, String)>,
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
        push_record(env, ids, stamp, record);
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
) {
    if let Some(status) = status {
        let ids = env.ids(task);
        record_core(rc, env, ids, stamp, status, record, |env| {
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
) {
    let mut record = task_record(code, message, ids.workflow, ids.task, env.now_text);
    record.element_index = stamp.element_index;
    env.errors.push(record);
}

/// [`record`] for a call of a watched run, or one that marked a change:
/// everything the recorder makes.
#[cold]
#[inline(never)]
fn watched<'p>(
    rc: &mut RunContext<'_, 'p>,
    env: &mut RunEnv<'_, 'p>,
    wf: &'p CompiledWorkflow,
    done: Completed,
    status: Option<u16>,
    record: Option<(Cow<'static, str>, String)>,
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
        let progress = record_core(rc, env, ids, stamp, status, record, |env| {
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
        traced_work();
        step.errors = env.errors[errors_before..].to_vec();
        step.elapsed = elapsed.filter(|_| timings);
        step.loop_counter = stamp.loop_counter;
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
    record.stamp(env.workflow_id, None, env.now_text, None);
    if env.watch.trace.is_some() {
        let mut step = env.workflow_step(StepResult::Failed);
        #[cfg(test)]
        traced_work();
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::CancelToken;
    use crate::function::{Ctx, Function, Outcome};
    use crate::observe::{Interest, Observer, TraceOptions};
    use crate::run::{EngineConfig, RunOptions};
    use crate::state::Tuning;
    use crate::{Engine, Message};
    use serde_json::{Value, json};
    use std::cell::{Cell, RefCell};
    use std::sync::Arc;

    thread_local! {
        /// How many times this thread read the recorder's clock.
        pub(super) static CLOCK_READS: Cell<u32> = const { Cell::new(0) };
        /// What each recorded call completed with, while a test collects it.
        static COMPLETED: RefCell<Option<Vec<Done>>> = const { RefCell::new(None) };
        /// Every built-in call on this thread fails once it has written.
        pub(super) static FAIL_AFTER_WRITES: Cell<bool> = const { Cell::new(false) };
        /// How much of the trace's own work this thread did: a
        /// step's records copied, or a mapping's end noted.
        pub(super) static TRACE_WORK: Cell<u32> = const { Cell::new(0) };
    }

    /// What one recorded call completed with, owned.
    #[derive(Debug, PartialEq)]
    struct Done {
        task: String,
        element: Option<usize>,
        status: Option<u16>,
        failed: bool,
        errors_before: usize,
        timed: bool,
    }

    /// `done`, kept while a test collects.
    pub(super) fn completed(
        env: &RunEnv<'_, '_>,
        done: &Completed,
        status: Option<u16>,
        flow: &Flow,
    ) {
        COMPLETED.with_borrow_mut(|kept| {
            if let Some(kept) = kept {
                kept.push(Done {
                    task: env.ids(done.task).task.to_string(),
                    element: done.stamp.element_index,
                    status,
                    failed: matches!(flow, Flow::Fail(_) | Flow::Stop(_)),
                    errors_before: done.errors_before,
                    timed: done.elapsed.is_some(),
                });
            }
        });
    }

    fn reads() -> u32 {
        CLOCK_READS.with(Cell::get)
    }

    /// Completes.
    struct Answer;

    impl Function for Answer {
        type Config = ();

        async fn call(&self, _ctx: &mut Ctx<'_>, _cfg: &()) -> crate::Result<Outcome> {
            Ok(Outcome::Ok)
        }
    }

    /// Fails.
    struct Down;

    impl Function for Down {
        type Config = ();

        async fn call(&self, _ctx: &mut Ctx<'_>, _cfg: &()) -> crate::Result<Outcome> {
            Err(Error::service("PARTNER_DOWN", "down"))
        }
    }

    /// An observer with `interest` that keeps nothing.
    struct Quiet(Interest);

    impl Observer for Quiet {
        fn interest(&self) -> Interest {
            self.0
        }

        fn on(&self, _event: &Event<'_>) {}
    }

    /// An engine over `workflows` with `answer` and `down` registered and
    /// `observer` its own.
    fn engine(workflows: &[Value], observer: Option<Interest>) -> Engine {
        let mut config = EngineConfig::default();
        if let Some(interest) = observer {
            config = config.observer(Arc::new(Quiet(interest)));
        }
        workflows
            .iter()
            .fold(
                Engine::builder()
                    .register("answer", Answer)
                    .register("down", Down)
                    .config(config),
                |b, w| b.workflow_json(w),
            )
            .build()
            .unwrap()
    }

    fn task(id: &str, function: &str, extra: Value) -> Value {
        let mut task = json!({"id": id, "function": {"name": function}});
        for (key, value) in extra.as_object().unwrap() {
            task[key] = value.clone();
        }
        task
    }

    fn map(id: &str, path: &str, mode: &str) -> Value {
        json!({"id": id, "function": {"name": "map", "input": {
            "mappings": [{"path": path, "logic": 1, "mode": mode}]}}})
    }

    /// What every call of one run of `e` completed with, over `data`.
    async fn collect(e: &Engine, data: Value, opts: &RunOptions<'_>) -> Vec<Done> {
        COMPLETED.with_borrow_mut(|kept| *kept = Some(Vec::new()));
        let mut message = Message::from_parts(
            OwnedDataValue::Null,
            OwnedDataValue::from(&data),
            OwnedDataValue::Null,
        );
        e.run_with(&mut message, opts).await;
        COMPLETED.with_borrow_mut(Option::take).unwrap()
    }

    #[tokio::test]
    async fn no_clock_is_read_without_an_interest_in_timings() {
        // Two sweeps of a built-in, a host call and a two-element fan-out.
        let workflows = [
            json!({"id": "w", "loop": {"counter": "i", "max": 2}, "tasks": [
                map("m", "data.a", "set"),
                task("h", "answer", json!({})),
                task("f", "answer", json!({"for_each": {"over": [1, 2], "as": "x"}})),
            ]}),
        ];
        let untimed = [
            None,
            Some(Interest::NONE),
            Some(Interest::TASKS),
            Some(Interest::WORKFLOWS | Interest::TASKS),
        ];
        for interest in untimed {
            let e = engine(&workflows, interest);
            let before = reads();
            let done = collect(&e, json!({}), &RunOptions::default()).await;
            assert_eq!(reads(), before, "{interest:?}");
            assert_eq!(done.len(), 8);
            assert!(done.iter().all(|d| !d.timed), "{interest:?}: {done:?}");
        }

        // Timed, per kind (A M-2): the run's ends once each for any
        // `TIMINGS`; the workflow's start and end only for a slot holding
        // `WORKFLOWS` and `TIMINGS`; every call twice only for one holding
        // `TASKS` and `TIMINGS`. A `TIMINGS` beside another slot's interest
        // times nothing for it.
        use Interest as I;
        for (engine_interest, run, expected, calls) in [
            (I::TASKS, I::TIMINGS, 2, false),
            (I::TASKS, I::WORKFLOWS | I::TIMINGS, 2 + 2, false),
            (I::WORKFLOWS, I::TASKS | I::TIMINGS, 2 + 8 * 2, true),
            (I::TASKS, I::ALL, 2 + 2 + 8 * 2, true),
        ] {
            let e = engine(&workflows, Some(engine_interest));
            let observer = Quiet(run);
            let before = reads();
            let opts = RunOptions::default().observer(&observer);
            let done = collect(&e, json!({}), &opts).await;
            assert_eq!(reads() - before, expected, "{engine_interest:?}, {run:?}");
            assert!(done.iter().all(|d| d.timed == calls), "{run:?}: {done:?}");
        }

        // A trace reads the clock only for its timings: two per call, and
        // neither the run's ends nor a workflow's start, which only an
        // observer's `TIMINGS` asks for.
        for observer in [None, Some(Interest::WORKFLOWS | Interest::TASKS)] {
            let e = engine(&workflows, observer);
            for (trace, expected) in [
                (TraceOptions::default(), 0),
                (TraceOptions::default().timings(true), 8 * 2),
            ] {
                let timed = trace.timings;
                let before = reads();
                let opts = RunOptions::default().trace(trace);
                let done = collect(&e, json!({}), &opts).await;
                assert_eq!(reads() - before, expected, "{observer:?}, timed {timed}");
                assert!(done.iter().all(|d| d.timed == timed), "{done:?}");
            }
        }
    }

    fn trace_work() -> u32 {
        TRACE_WORK.with(Cell::get)
    }

    #[tokio::test]
    async fn an_untraced_run_does_no_trace_work() {
        // A step's records and a map's mapping ends are made only
        // under a trace, so a run without one, watched or not, does none of
        // that work: every call site that adds a record, a map of two
        // mappings, a workflow's own failure and a condition that fails.
        let workflows = [
            json!({"id": "w", "on_failure": "continue", "loop": {"counter": "i", "max": 2},
            "tasks": [
                {"id": "m", "function": {"name": "map", "input": {"mappings": [
                    {"path": "data.a", "logic": 1},
                    {"path": "data.l", "logic": 1, "mode": "append"}]}}},
                {"id": "v", "function": {"name": "validation", "input": {"rules": [
                    {"logic": {"var": "data.missing"}, "message": "no"}]}}},
                task("h", "down", json!({"on_failure": "continue"})),
                task("f", "down", json!({"on_failure": "continue",
                    "for_each": {"over": [1, 2], "as": "x"}})),
                task("stop", "down", json!({})),
            ]}),
            json!({"id": "c", "priority": 1, "condition": {"/": [1, {"var": "data.zero"}]},
                "tasks": [map("cm", "data.c", "set")]}),
            json!({"id": "o", "priority": 2, "on_failure": "continue",
                "loop": {"max": 1, "over": {"var": "data.a"}, "as": "x"},
                "tasks": [map("om", "data.o", "set")]}),
        ];
        let failures: Path = "temp_data.failures".parse().unwrap();
        let build = |observer: Option<Interest>, mirrored: bool| {
            let mut config = EngineConfig::default();
            if let Some(interest) = observer {
                config = config.observer(Arc::new(Quiet(interest)));
            }
            if mirrored {
                let ec = crate::run::ErrorContext::new(failures.clone(), 8).unwrap();
                config = config.error_context(ec);
            }
            workflows
                .iter()
                .fold(
                    Engine::builder().register("down", Down).config(config),
                    |b, w| b.workflow_json(w),
                )
                .build()
                .unwrap()
        };
        let data = json!({"zero": 0});
        for (observer, mirrored) in [
            (None, false),
            (Some(Interest::ALL), false),
            (None, true),
            (Some(Interest::TASKS), true),
        ] {
            let e = build(observer, mirrored);
            let before = trace_work();
            let done = collect(&e, data.clone(), &RunOptions::default()).await;
            assert_eq!(done.len(), 12, "{done:?}");
            assert_eq!(trace_work(), before, "{observer:?}, mirrored {mirrored}");
        }
        // The same runs traced do it.
        let e = build(None, true);
        let before = trace_work();
        let opts = RunOptions::default().trace(TraceOptions::default());
        collect(&e, data, &opts).await;
        assert!(trace_work() > before, "a traced run copies its records");
    }

    /// `(task, element, status, failed, errors_before)`.
    type Site = (&'static str, Option<usize>, Option<u16>, bool, usize);

    fn sites(done: &[Done]) -> Vec<Site> {
        done.iter()
            .map(|d| {
                let task = ["m", "bad", "h", "f", "e", "n", "stop", "g", "c", "fc"]
                    .into_iter()
                    .find(|&t| t == d.task)
                    .unwrap_or_else(|| panic!("an unknown task {}", d.task));
                (task, d.element, d.status, d.failed, d.errors_before)
            })
            .collect()
    }

    #[tokio::test]
    async fn every_call_site_completes_with_its_failure_and_its_error_count() {
        let on_continue = json!({"on_failure": "continue"});
        let e = engine(
            &[
                json!({"id": "w", "on_failure": "continue", "tasks": [
                    map("m", "data.a", "set"),
                    // An append to a string fails the mapping: a 500.
                    {"id": "bad", "on_failure": "continue", "function": {"name": "map",
                        "input": {"mappings": [
                            {"path": "data.text", "logic": 1, "mode": "append"}]}}},
                    task("h", "down", on_continue.clone()),
                    task("f", "answer", json!({"for_each": {"over": [1, 2], "as": "x"}})),
                    task("e", "answer", json!({"for_each": {"over": [], "as": "x"}})),
                    task("n", "answer", json!({"on_failure": "continue",
                        "for_each": {"over": {"var": "data.text"}, "as": "x"}})),
                    // Its failure goes to the workflow, which steps past it.
                    task("stop", "down", json!({})),
                ]}),
                json!({"id": "x", "priority": 1, "tasks": [
                    task("g", "down", json!({"for_each": {"over": [1, 2], "as": "x"}})),
                ]}),
            ],
            Some(Interest::ALL),
        );
        let done = collect(&e, json!({"text": "t"}), &RunOptions::default()).await;
        assert_eq!(
            sites(&done),
            [
                ("m", None, Some(200), false, 0),
                ("bad", None, Some(500), false, 0),
                ("h", None, Some(500), false, 2),
                ("f", Some(0), Some(200), false, 3),
                ("f", Some(1), Some(200), false, 3),
                ("e", None, Some(200), false, 3),
                ("n", None, Some(500), false, 3),
                ("stop", None, Some(500), true, 4),
                // The first element fails the task, and the fold stops.
                ("g", Some(0), Some(500), true, 5),
            ]
        );
        assert!(done.iter().all(|d| d.timed), "{done:?}");

        // Cut short by the run's token: a host call, and a fan-out recorded
        // once, with no element.
        let token = CancelToken::new();
        token.cancel();
        let opts = RunOptions::default().cancel(&token);
        for (workflow, expected) in [
            (
                task("c", "answer", json!({})),
                ("c", None, Some(500), true, 0),
            ),
            (
                task(
                    "fc",
                    "answer",
                    json!({"for_each": {"over": [1, 2], "as": "x"}}),
                ),
                ("fc", None, Some(500), true, 0),
            ),
        ] {
            let e = engine(&[json!({"id": "w", "tasks": [workflow]})], None);
            let done = collect(&e, json!({}), &opts).await;
            assert_eq!(sites(&done), [expected]);
            assert!(!done[0].timed);
        }
    }

    fn text(v: &OwnedDataValue) -> String {
        serde_json::to_string(v).unwrap()
    }

    #[tokio::test]
    async fn a_builtin_that_writes_then_fails_gives_its_writes_to_the_trace_alone() {
        // No built-in writes, then returns `Err`, today, so the hook
        // makes `map` do it. Its write stands: its `Failed` step carries it
        // and the trace replays to the message. The audit entry gets none,
        // as v3's recorder dropped them, with a trace and without one.
        let e = engine(
            &[json!({"id": "w", "tasks": [map("m", "data.a", "set")]})],
            None,
        );
        let traced = RunOptions::default().trace(TraceOptions::default());
        for opts in [RunOptions::default(), traced] {
            let mut message = Message::from_parts(
                OwnedDataValue::Null,
                OwnedDataValue::from(&json!({})),
                OwnedDataValue::Null,
            );
            FAIL_AFTER_WRITES.set(true);
            let run = e.run_with(&mut message, &opts).await;
            FAIL_AFTER_WRITES.set(false);
            assert!(
                matches!(run.status(), crate::run::RunStatus::Stopped(_)),
                "{run:?}"
            );
            let context = message.context();
            assert_eq!(text(&context.data()["a"]), "1", "the write stands");
            let audit = message.audit();
            assert_eq!(audit.len(), 1);
            assert!(audit[0].changes.is_empty(), "{audit:?}");
            assert_eq!(audit[0].status, 500);
            let Some(trace) = run.trace() else {
                continue;
            };
            let step = &trace.steps[0];
            assert_eq!((step.result, step.status), (StepResult::Failed, Some(500)));
            let paths: Vec<&str> = step.changes.iter().map(|c| &*c.path).collect();
            assert_eq!(paths, ["data.a", "metadata.progress"]);
            let last = trace.contexts().last().expect("a step");
            assert_eq!(
                serde_json::to_string(&last).unwrap(),
                serde_json::to_string(context).unwrap(),
                "the trace replays to the message"
            );
        }
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
