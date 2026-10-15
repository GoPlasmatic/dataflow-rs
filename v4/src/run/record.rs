//! The recorder: what one completed call leaves on the message, and what
//! the run's observers are told of it.
//!
//! Once per completed call, from what the call's
//! [`Decision`](super::policy::Decision) and the call left ([`Completed`]):
//! when it has a status, the audit entry (through `AuditMode`'s trim, with the
//! call's [`Stamp`]), then the progress write, then the error record, if any,
//! with the workflow and task ids, the run's timestamp and the stamp's
//! element; then, for a `Skip` too, the trace's step and the observers'
//! `TaskFinished`. This is v3's `handle_task_result`, in its order. A fan-out
//! records once per element it folds.
//!
//! The trace's step carries, while the trace keeps changes, the engine's
//! writes made for it (`RunEnv::trace_prelude`), the call's changes, and the
//! progress write's `Change`; the audit entry carries the call's changes
//! alone, and only under `capture_audit` (I107), so the engine's writes never
//! reach it.
//!
//! A workflow's own failure, which no task made, records one error record,
//! and the trace's `Failed` step for it, and nothing else (I61):
//! [`workflow_failure`].
//!
//! Every clock reading for an elapsed time goes through [`now`] and
//! [`since`], and only in a timed run: one an observer asked `TIMINGS` of,
//! or a trace its timings. A step's `elapsed` is kept only for the latter.

use super::interp::{RunEnv, old_value};
use super::message::{AuditEntry, keeps_entries, record_audit};
use crate::base::{Change, Error, ErrorRecord, Instant, TaskIx};
use crate::expr::Path;
use crate::function::{CallInfo, task_record};
use crate::observe::{Event, StepResult};
use crate::program::CompiledWorkflow;
use crate::state::RunContext;
use datavalue::{DataValue, NumberValue, OwnedDataValue};
use std::borrow::Cow;
use std::sync::{Arc, LazyLock};
use std::time::Duration;

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

/// The clock, read for an elapsed time: only when the run is timed.
pub(crate) fn now() -> Instant {
    #[cfg(test)]
    tests::CLOCK_READS.with(|reads| reads.set(reads.get() + 1));
    Instant::now()
}

/// The time since `start`, on [`now`]'s clock.
pub(crate) fn since(start: Instant) -> Duration {
    now().saturating_duration_since(start)
}

/// One recorded call: what the decision and the call left, for the
/// recorder's outputs.
pub(crate) struct Completed<'p> {
    /// The workflow's and the task's ids.
    pub(crate) ids: TaskIds<'p>,
    /// The task's row in the workflow, for `TaskIx` and the function's name.
    pub(crate) task: u32,
    /// Where the call sits in the run's iterations.
    pub(crate) stamp: Stamp,
    /// The decision's status; `None` for a `Skip`.
    pub(crate) status: Option<u16>,
    /// The decision's error record: (code, message).
    pub(crate) record: Option<(Cow<'static, str>, String)>,
    /// The decision's flow is `Fail` or `Stop`: the trace's `Failed` (I105).
    pub(crate) failed: bool,
    /// How long the call ran, when the run is timed.
    pub(crate) elapsed: Option<Duration>,
    /// `env.errors.len()` before the call: the error context mirrors what
    /// came after.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "read by phase 6's Task 3 (the error context)")
    )]
    pub(crate) errors_before: usize,
}

/// Records one completed call of `wf`, in this order: the audit entry, with
/// the call's changes, taken from `env.changes`, under `capture_audit`
/// (through `AuditMode`), the progress write, the error record, the trace's
/// step, and the observers' `TaskFinished`. A `Skip` gets the last two only.
pub(crate) fn record<'p>(
    rc: &mut RunContext<'_, 'p>,
    env: &mut RunEnv<'_, 'p>,
    wf: &'p CompiledWorkflow,
    done: Completed<'p>,
) {
    #[cfg(test)]
    tests::completed(&done);
    let Completed {
        ids,
        task,
        stamp,
        status,
        record,
        failed,
        elapsed,
        ..
    } = done;
    // The step's changes, while the trace keeps them: the engine's writes
    // made for it first, then the call's, then the progress write.
    let traced = env.traces_changes();
    let mut changes = if traced {
        std::mem::take(&mut env.trace_prelude)
    } else {
        Vec::new()
    };
    let mut error = None;
    if let Some(status) = status {
        // A mode that keeps nothing builds no entry for `record_audit` to
        // drop.
        if keeps_entries(env.mode) {
            let changes = match (env.capture_audit, traced) {
                (false, _) => Vec::new(),
                // Both want them.
                (true, true) => env.changes.clone(),
                (true, false) => std::mem::take(&mut env.changes),
            };
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
        let old = traced.then(|| old_value(rc, &PROGRESS));
        write_progress(rc, ids.workflow, ids.task, status);
        if let Some(old) = old {
            changes.append(&mut env.changes);
            changes.push(Change::write(
                Arc::clone(PROGRESS.dotted()),
                old,
                old_value(rc, &PROGRESS),
            ));
        }
        if let Some((code, message)) = record {
            let (workflow, task) = (&**ids.workflow, &**ids.task);
            let mut record = task_record(&code, message, workflow, task, env.now_text);
            record.element_index = stamp.element_index;
            if env.trace.is_some() {
                error = Some(record.clone());
            }
            env.errors.push(record);
        }
    } else if traced {
        // A `Skip`'s writes stand: its step carries them.
        changes.append(&mut env.changes);
    }
    env.changes.clear();
    env.trace_prelude.clear();
    if let Some(trace) = env.trace.as_deref() {
        let timings = trace.timings();
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
        step.error = error;
        step.elapsed = elapsed.filter(|_| timings);
        step.loop_counter = stamp.loop_counter;
        step.element_index = stamp.element_index;
        env.push_step(step);
    }
    if env.observers.tasks() {
        let meta = &wf.tasks[task as usize];
        let info = CallInfo {
            program: env.program,
            workflow: env.workflow_ix,
            task: TaskIx::new(env.task_base + task),
            function: &meta.function,
            workflow_id: &wf.id,
            task_id: &meta.id,
            channel: env.channel,
            loop_counter: stamp.loop_counter,
            element_index: stamp.element_index,
        };
        env.observers.emit(Event::TaskFinished {
            info: &info,
            status,
            elapsed,
        });
    }
}

/// Records a workflow's own failure (I61), whatever its policy: one record
/// with the error's code and message, the workflow's id, no task id, and the
/// run's timestamp, and in a traced run the workflow's `Failed` step, with
/// the record as its error. No audit entry and no progress write: no task
/// completed.
pub(crate) fn workflow_failure(env: &mut RunEnv<'_, '_>, error: &Error) {
    let mut record = ErrorRecord::from(error);
    record.workflow_id = Some(env.workflow_id.to_string());
    record.timestamp = Some(env.now_text.to_owned());
    if env.trace.is_some() {
        let mut step = env.workflow_step(StepResult::Failed);
        step.error = Some(record.clone());
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
    pub(super) fn completed(done: &Completed<'_>) {
        COMPLETED.with_borrow_mut(|kept| {
            if let Some(kept) = kept {
                kept.push(Done {
                    task: done.ids.task.to_string(),
                    element: done.stamp.element_index,
                    status: done.status,
                    failed: done.failed,
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

        // Timed, every call reads it twice, and each end once. A workflow's
        // start is read only for an observer of workflows.
        let e = engine(&workflows, Some(Interest::TASKS));
        for (run, expected) in [
            (Interest::TIMINGS, 2 + 8 * 2),
            (Interest::WORKFLOWS | Interest::TIMINGS, 2 + 2 + 8 * 2),
        ] {
            let observer = Quiet(run);
            let before = reads();
            let opts = RunOptions::default().observer(&observer);
            let done = collect(&e, json!({}), &opts).await;
            assert_eq!(reads() - before, expected, "{run:?}");
            assert!(done.iter().all(|d| d.timed), "{run:?}: {done:?}");
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
                ("h", None, Some(500), false, 1),
                ("f", Some(0), Some(200), false, 2),
                ("f", Some(1), Some(200), false, 2),
                ("e", None, Some(200), false, 2),
                ("n", None, Some(500), false, 2),
                ("stop", None, Some(500), true, 3),
                // The first element fails the task, and the fold stops.
                ("g", Some(0), Some(500), true, 4),
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
