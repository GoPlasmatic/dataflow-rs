//! The interpreter: one `async fn` over a workflow's ops. Nothing else
//! interprets steps.
//!
//! A built-in runs inline with no `.await`; a host call awaits through
//! [`call_host`](super::host::call_host), and a fan-out through
//! [`fan_out`](super::fan_out::fan_out), which runs its elements' calls
//! before it folds them. Every completed call goes through
//! [`decide`](super::policy::decide), the one definition of failed, rejected
//! and halted, and then [`record`](super::record::record).
//!
//! A loop is two ops. `LoopInit` evaluates `over` once, writes
//! the counter's `init`, checks the bounds, and in bounds writes the bindings
//! and falls into the first sweep; `LoopNext` advances the counter and does
//! the same, jumping back to the sweep's first op. The engine owns the
//! counter: it lives in the [`Frame`], and the context only gets a copy.
//!
//! The run's [`Limits`] are checked every [`CHECK_EVERY`] ops and as each
//! sweep starts, when one is set. A stop between ops ends the run with
//! nothing recorded;
//! one as a sweep starts, after the counter's write, with the engine's step
//! for that write, so a trace still replays to the message's context.
//!
//! A workflow's `WorkflowStarted` goes to the run's observers once, when it
//! is known to be selected: as it starts when it has no condition, or when
//! the first evaluation of its condition admits it.
//!
//! A traced run's steps that are not a call's are made here: the
//! workflow's `Skipped` step when its condition refuses it on the first
//! judgment, one `Skipped` step per call a group's or a task's condition
//! refuses, the engine's step as a sweep is entered or a loop exits, and,
//! through the recorder, a workflow's own `Failed` step.

use super::cancel::Limits;
use super::error_context::ErrorContext;
use super::fan_out::fan_out;
use super::host::call_host;
use super::message::{AuditEntry, AuditMode};
use super::observers::Observers;
use super::policy::{self, Flow};
use super::record::{self, Completed, Stamp, TaskIds};
use super::trace::Capture;
use crate::base::{Change, Error, ErrorKind, ErrorRecord, Instant, ProgramId, TaskIx, WorkflowIx};
use crate::def::WorkflowFailurePolicy;
use crate::expr::path::kind;
use crate::expr::{Expr, ExprEngine, Path};
use crate::function::builtins::Builtin;
use crate::function::{CallLayer, SyncCtx, eval, test};
use crate::observe::{Event, Selection, StepResult, TraceStep};
use crate::program::{CallSite, Callee, CompiledWorkflow, CondKind, LoopSite, Op};
use crate::state::{Owner, RunContext};
use chrono::{DateTime, Utc};
use datavalue::{DataValue, OwnedDataValue};
use std::borrow::Cow;
use std::fmt;
use std::sync::Arc;

/// How one workflow's ops ended.
#[derive(Debug)]
pub(crate) enum Exit {
    /// The ops ran to their end.
    Completed,
    /// A task, a terminal group or a `filter` ended the workflow on purpose.
    Halted,
    /// The ops ran to their end, past a failure `on_failure: continue`
    /// stepped over.
    Failed,
    /// A failure stops the run with this error: a task's, under
    /// `on_failure: fail` in a workflow whose own is `fail`, or a step the
    /// interpreter refuses.
    Stopped(Error),
    /// The run's deadline passed or its token was cancelled, as this error
    /// of kind `TimedOut` or `Cancelled` says: at a check between ops, as a
    /// sweep starts, or cutting a call short. Only this exit makes the
    /// run `TimedOut` or `Cancelled`; an `Err` of either kind a function
    /// returned is a task's failure like any other.
    Limited(Error),
}

/// The loop frame: the sweep the workflow is in.
#[derive(Debug, Default)]
pub(crate) struct Frame {
    /// The counter of the sweep running, stamped on its audit entries:
    /// `None` until a loop's first sweep, so setup's entries carry none.
    pub(crate) loop_counter: Option<i64>,
    /// The sweeps begun: one for a workflow without a loop that ran, and for
    /// a loop each sweep its condition admitted.
    pub(crate) sweeps: u32,
    /// The loop's counter, owned by the engine: the sweep's, or once the loop
    /// is out of bounds the first counter past them.
    counter: i64,
    /// `loop.over`'s elements, evaluated once, after setup. Each sweep moves
    /// its element out, so the array is never in the run arena, where a
    /// borrow held across sweeps would forbid every checkpoint.
    items: Option<Vec<OwnedDataValue>>,
    /// Whether the workflow's `WorkflowStarted` went out.
    begun: bool,
    /// When it went out, in a timed run: where the workflow's elapsed time
    /// starts.
    pub(crate) started: Option<Instant>,
}

impl Frame {
    /// Whether the ops running are a sweep's: always for a workflow without
    /// a loop, and for a loop from its first sweep on.
    fn in_sweep(&self, wf: &CompiledWorkflow) -> bool {
        wf.loops.is_empty() || self.loop_counter.is_some()
    }
}

/// What the interpreter carries through a run, beside the run context: the
/// run's facts and the message's other parts, and the current workflow's
/// failure flag and loop frame.
pub(crate) struct RunEnv<'r, 'p> {
    /// The program's identity, for a host call's `CallInfo`.
    pub(crate) program: ProgramId,
    /// The current workflow's position in the program.
    pub(crate) workflow_ix: WorkflowIx,
    /// The program's `TaskIx` of the current workflow's first task.
    pub(crate) task_base: u32,
    /// The channel the run was started on.
    pub(crate) channel: Option<&'r str>,
    /// The run's routing bucket, at most 99.
    pub(crate) routing_bucket: Option<u8>,
    /// The run's layers, outermost first, around every host call.
    pub(crate) layers: &'r [Arc<dyn CallLayer>],
    /// The run's deadline and token.
    pub(crate) limits: Limits<'r>,
    /// The ops the run has begun while a limit is set, across its
    /// workflows: every [`CHECK_EVERY`]th checks the limits.
    pub(crate) ops: u32,
    /// The expression engine the program was compiled with.
    pub(crate) exprs: &'p ExprEngine,
    /// The message's payload.
    pub(crate) payload: &'r OwnedDataValue,
    /// The message's error list.
    pub(crate) errors: &'r mut Vec<ErrorRecord>,
    /// The message's audit trail.
    pub(crate) audit: &'r mut Vec<AuditEntry>,
    /// Which audit entries the message keeps.
    pub(crate) mode: AuditMode,
    /// Whether a call records its `Change`s: the audit or the trace wants
    /// them.
    pub(crate) capture: bool,
    /// Whether the audit entry gets the call's `Change`s: change capture is
    /// on and the mode keeps entries.
    pub(crate) capture_audit: bool,
    /// What the run reports beyond the message, held by `run_with`.
    pub(crate) watch: &'r mut Watch<'p>,
    /// Whether a call is recorded beyond the message: the run is traced,
    /// mirrors its records into an error context, or tells an observer of
    /// the task events ([`Watch::records_calls`]). An unwatched call takes
    /// the plain recorder, so the run's other outputs cost it one branch.
    pub(crate) watched: bool,
    /// The run's one clock reading.
    pub(crate) now: DateTime<Utc>,
    /// The same reading, RFC 3339, for error records: one `Arc<str>` per run,
    /// shared by every record it stamps.
    pub(crate) now_text: &'r Arc<str>,
    /// The changes of the call running, read by the recorder.
    pub(crate) changes: Vec<Change>,
    /// The current workflow's id.
    pub(crate) workflow_id: &'p Arc<str>,
    /// The current workflow's task ids, by task row.
    pub(crate) task_ids: &'p [Arc<str>],
    /// Whether `on_failure: continue` stepped past a failure in it.
    pub(crate) failed: bool,
    /// The current workflow's loop frame.
    pub(crate) frame: Frame,
}

/// What a run reports beyond the message, which the interpreter reads
/// rarely: its observers, its trace and its error context, and whether it
/// reads the clock. One per run, held by `run_with`, so the run's future
/// carries it once and each workflow's [`RunEnv`] one reference to it.
pub(crate) struct Watch<'p> {
    /// The run's observers.
    pub(crate) observers: Observers<'p>,
    /// Whether the run reads the clock around each call: an observer of the
    /// task events asked for `TIMINGS`, or the trace for timings.
    pub(crate) timed: bool,
    /// The run's trace, when it is traced. Boxed: the run's future carries
    /// a pointer.
    pub(crate) trace: Option<Box<Capture>>,
    /// Where the recorder mirrors each call's records, when the engine has
    /// an error context.
    pub(crate) error_context: Option<&'p ErrorContext>,
}

impl Watch<'_> {
    /// Whether a recorded call has anything to report beyond the message:
    /// a trace step, an error-context mirror or a `TaskFinished`. Fixed for
    /// the run: a trace stays, its budget crossed or not.
    pub(crate) fn records_calls(&self) -> bool {
        self.trace.is_some() || self.error_context.is_some() || self.observers.tasks()
    }
}

/// How many ops run between two checks of the limits, when one is set.
///
/// The interpreter checks a deadline and a cancellation token at every host
/// call, every fan-out element's start and every sweep, and here: at every
/// `CHECK_EVERY`th op, so a loop of pure built-ins still stops. The number is
/// public so a test can derive where a run stops instead of spelling `63`.
pub const CHECK_EVERY: u32 = 64;

impl<'p> RunEnv<'_, 'p> {
    /// At an op boundary: the run context's checkpoint when due, then, with
    /// a limit set, the limits at every 64th op.
    fn tick(&mut self, rc: &mut RunContext<'_, '_>) -> Result<(), Error> {
        rc.tick();
        if self.limits.is_set() {
            self.ops = self.ops.wrapping_add(1);
            if self.ops.is_multiple_of(CHECK_EVERY) {
                self.limits.check()?;
            }
        }
        Ok(())
    }

    /// Whether the trace keeps changes: only then are the engine's writes
    /// recorded, and their old values read.
    pub(crate) fn traces_changes(&self) -> bool {
        self.watch.trace.as_ref().is_some_and(|t| t.changes())
    }

    /// The ids of the current workflow's task at row `task`, as the program
    /// owns them.
    pub(crate) fn ids(&self, task: u32) -> TaskIds<'p> {
        TaskIds {
            workflow: self.workflow_id,
            task: &self.task_ids[task as usize],
        }
    }

    /// The record of the current workflow's own `error`: its code and
    /// message, and the workflow's id.
    pub(crate) fn workflow_record(&self, error: &Error) -> ErrorRecord {
        let mut record = ErrorRecord::from(error);
        record.workflow_id = Some(Arc::clone(self.workflow_id));
        record
    }

    /// The current workflow's own step.
    pub(crate) fn workflow_step(&self, result: StepResult) -> TraceStep {
        TraceStep::new(self.workflow_ix, self.workflow_id, result)
    }

    /// A step of the current workflow's task at row `task`, in the sweep
    /// the frame is in.
    pub(crate) fn task_step(&self, task: u32, result: StepResult) -> TraceStep {
        let mut step = self.workflow_step(result);
        step.task = Some(TaskIx::new(self.task_base + task));
        step.task_id = Some(Arc::clone(&self.task_ids[task as usize]));
        step.loop_counter = self.frame.loop_counter;
        step
    }

    /// Pushes `step` onto the trace, if the run is traced. Calls record
    /// their changes from then on only while the audit or the trace wants
    /// them: a budget crossed leaves only the audit's.
    pub(crate) fn push_step(&mut self, step: TraceStep) {
        if let Some(trace) = self.watch.trace.as_deref_mut() {
            trace.push(step);
            self.capture = captures(self.capture_audit, Some(trace));
        }
    }
}

/// Whether a call records its `Change`s: the audit wants them, or the trace
/// keeps them.
pub(crate) fn captures(capture_audit: bool, trace: Option<&Capture>) -> bool {
    capture_audit || trace.is_some_and(Capture::changes)
}

/// Where a failure `on_failure: continue` steps past goes from `op`.
fn fail_to(op: Op) -> u32 {
    match op {
        Op::Call { fail_to, .. } | Op::FanOut { fail_to, .. } => fail_to,
        Op::LoopInit { exit_to, .. } => exit_to,
        Op::Cond { .. } | Op::LoopNext { .. } | Op::Halt => {
            unreachable!("only a call, a fan-out or a loop's start fails")
        }
    }
}

/// Runs one workflow's ops over the run context: how they ended, and the
/// workflow's selection.
///
/// A workflow is `Ran` unless the first evaluation of its condition refuses
/// it: false gives `SkippedByCondition`, and an evaluation error
/// `ConditionFailed`, which jumps as a false one does. A later evaluation,
/// which ends a loop, records nothing. So a workflow without a condition is
/// `Ran` when it starts. A group's or a task's condition only jumps.
///
/// A workflow without a loop begins its one sweep when it starts, and a loop
/// each sweep its `LoopInit` or `LoopNext` enters. A workflow condition that
/// refuses the sweep it guards takes it back: a sweep counts once its
/// condition admits it, as v3's `sweeps_run`.
///
/// The workflow's `WorkflowStarted` goes out as it starts when no workflow
/// condition judges it first, else once that judgment admits it, and in
/// any case before it ends `Ran`: a loop whose condition no sweep reached,
/// or a run stopped before its condition, is `Ran` too.
pub(crate) async fn run_workflow<'p>(
    rc: &mut RunContext<'_, 'p>,
    wf: &'p CompiledWorkflow,
    env: &mut RunEnv<'_, 'p>,
) -> (Exit, Selection) {
    let mut selection = Selection::Ran;
    // Only a workflow condition's `Cond` judges the selection, and only the
    // first one evaluated.
    let mut unjudged = true;
    env.frame.sweeps = u32::from(wf.loops.is_empty());
    if !wf.judged_first() {
        begin(env, wf);
    }
    let mut pc = 0;
    let exit = loop {
        let Some(&op) = wf.ops.get(pc) else {
            break if env.failed {
                Exit::Failed
            } else {
                Exit::Completed
            };
        };
        if let Err(e) = env.tick(rc) {
            break Exit::Limited(e);
        }
        let flow = match op {
            Op::Cond {
                cond,
                kind,
                else_to,
            } => {
                let passed = test(rc, env.exprs, &wf.conds[cond as usize].expr);
                let admitted = matches!(passed, Ok(true));
                if kind == CondKind::Workflow {
                    if !admitted && env.frame.in_sweep(wf) {
                        env.frame.sweeps -= 1;
                    }
                    if std::mem::take(&mut unjudged) {
                        selection = match passed {
                            Ok(true) => {
                                begin(env, wf);
                                Selection::Ran
                            }
                            Ok(false) => {
                                skipped_workflow(env, None);
                                Selection::SkippedByCondition
                            }
                            Err(e) => {
                                skipped_workflow(env, Some(&e));
                                Selection::ConditionFailed(e)
                            }
                        };
                    }
                } else if !admitted && env.watch.trace.is_some() {
                    skipped_calls(env, wf, kind, pc, else_to);
                }
                if admitted {
                    Flow::Next
                } else {
                    Flow::Jump(else_to)
                }
            }
            Op::Call { site, .. } => {
                let site = &wf.calls[site as usize];
                match &site.callee {
                    Callee::Builtin(builtin) => call(rc, env, wf, site, builtin),
                    Callee::Host(bound) => call_host(rc, env, wf, site, &**bound).await,
                }
            }
            Op::FanOut { site, fan, .. } => {
                let site = &wf.calls[site as usize];
                let Callee::Host(bound) = &site.callee else {
                    unreachable!("compile_one checks that a fan-out calls a host function")
                };
                // Boxed: a fan-out's state stays out of the run's future,
                // which a host spawns once per message.
                Box::pin(fan_out(rc, env, wf, site, &wf.fans[fan as usize], &**bound)).await
            }
            Op::LoopInit { lp, exit_to } => loop_init(rc, env, &wf.loops[lp as usize], exit_to),
            Op::LoopNext {
                lp,
                body_to,
                exit_to,
            } => loop_next(rc, env, &wf.loops[lp as usize], body_to, exit_to),
            Op::Halt => Flow::Halt,
        };
        pc = match flow {
            Flow::Next => pc + 1,
            Flow::Jump(to) => to as usize,
            Flow::Halt => break Exit::Halted,
            Flow::Stop(e) => break Exit::Limited(e),
            Flow::Fail(e) => match wf.on_failure {
                WorkflowFailurePolicy::Continue => {
                    env.failed = true;
                    fail_to(op) as usize
                }
                WorkflowFailurePolicy::Fail => break Exit::Stopped(e),
            },
        };
    };
    if matches!(selection, Selection::Ran) {
        begin(env, wf);
    }
    (exit, selection)
}

/// The workflow's `WorkflowStarted`, once, and in a timed run the clock
/// reading its elapsed time starts from.
fn begin(env: &mut RunEnv<'_, '_>, wf: &CompiledWorkflow) {
    if !env.watch.observers.workflows() || std::mem::replace(&mut env.frame.begun, true) {
        return;
    }
    // Only an observer of the workflow events with `TIMINGS` times a
    // workflow: a trace's timings are its calls'.
    if env.watch.observers.timed_workflows() {
        env.frame.started = Some(record::now());
    }
    env.watch.observers.emit(Event::WorkflowStarted {
        workflow: env.workflow_ix,
        id: &wf.id,
    });
}

/// The workflow's own `Skipped` step, for a condition that refused it on
/// its first judgment: when it failed to evaluate, with the record of
/// the error it failed with and the workflow's id, so the step is told
/// apart from a plain `false`. The message never gets that record, so the
/// step's is the only one.
fn skipped_workflow(env: &mut RunEnv<'_, '_>, error: Option<&Error>) {
    if env.watch.trace.is_none() {
        return;
    }
    let mut step = env.workflow_step(StepResult::Skipped);
    if let Some(e) = error {
        #[cfg(test)]
        super::record::traced_work();
        step.errors.push(env.workflow_record(e));
    }
    env.push_step(step);
}

/// The `Skipped` steps of what a group's or a task's condition at `pc`
/// refused: one per call in the group's span, `pc + 1 .. else_to`, or the
/// task's one call, at `pc + 1`, in the sweep the frame is in.
fn skipped_calls(
    env: &mut RunEnv<'_, '_>,
    wf: &CompiledWorkflow,
    kind: CondKind,
    pc: usize,
    else_to: u32,
) {
    let span = match kind {
        CondKind::Group => wf.ops.get(pc + 1..else_to as usize),
        CondKind::Task => wf.ops.get(pc + 1..pc + 2),
        CondKind::Workflow => None,
    };
    for op in span.unwrap_or_default() {
        if let Op::Call { site, .. } | Op::FanOut { site, .. } = *op {
            let step = env.task_step(wf.calls[site as usize].task, StepResult::Skipped);
            env.push_step(step);
        }
    }
}

/// `LoopInit`: `over`, once, then the counter's `init` and the first sweep.
///
/// An `over` that does not give an array fails the workflow before the
/// counter is written: one record, whatever the workflow's policy,
/// which then decides; under `continue` the failure goes to the loop's exit.
fn loop_init<'p>(
    rc: &mut RunContext<'_, 'p>,
    env: &mut RunEnv<'_, 'p>,
    site: &'p LoopSite,
    exit_to: u32,
) -> Flow {
    if let Some(over) = &site.over {
        match elements(rc, env.exprs, over, Over::Loop(env.workflow_id)) {
            // A loop moves each element out: its own copy.
            Ok(items) => env.frame.items = Some(items.into_owned()),
            Err(e) => {
                record::workflow_failure(env, &e);
                return Flow::Fail(e);
            }
        }
    }
    env.frame.counter = site.init;
    enter(rc, env, site, Flow::Next, exit_to)
}

/// `LoopNext`: the counter advanced by the increment, then the next sweep at
/// `body_to`. The advance saturates, so a counter pinned at `i64::MAX` is out
/// of bounds for every `max`, as in v3.
fn loop_next<'p>(
    rc: &mut RunContext<'_, 'p>,
    env: &mut RunEnv<'_, 'p>,
    site: &'p LoopSite,
    body_to: u32,
    exit_to: u32,
) -> Flow {
    env.frame.counter = env.frame.counter.saturating_add(site.increment);
    enter(rc, env, site, Flow::Jump(body_to), exit_to)
}

/// The sweep at the frame's counter, in this order: the counter written,
/// then the bounds, `counter < max` and `counter < len(over)`, then in bounds
/// the run's limits, then `over[counter]` moved into `as` and `{}` into
/// `scratch`, before the sweep's condition. Out of bounds the loop exits,
/// leaving the first counter past them in the context; past a limit the run
/// stops, the sweep not begun.
///
/// None of these writes records a `Change` for the audit. A traced run
/// gets the engine's step for them, `task: None` with the counter written,
/// as each sweep is entered, as the loop exits on a counter it wrote, and as
/// a limit stops the run after the counter's write, with each
/// write's `Change` while the trace keeps changes.
fn enter<'p>(
    rc: &mut RunContext<'_, 'p>,
    env: &mut RunEnv<'_, 'p>,
    site: &'p LoopSite,
    sweep: Flow,
    exit_to: u32,
) -> Flow {
    let traced = env.traces_changes();
    let mut changes = Vec::new();
    let counter = env.frame.counter;
    if let Some(path) = &site.counter {
        engine_write(
            rc,
            path,
            traced,
            &mut changes,
            || OwnedDataValue::from_i64(counter),
            |_| DataValue::from_i64(counter),
        );
    }
    // The element's index, when the loop is over an array.
    let element = match &env.frame.items {
        None => None,
        Some(items) => usize::try_from(counter).ok().filter(|&i| i < items.len()),
    };
    let ends = if counter >= site.max || (env.frame.items.is_some() && element.is_none()) {
        Some(Flow::Jump(exit_to))
    } else {
        // Only the counter is written by now: its step, so a stopped run's
        // trace replays to its context.
        env.limits.check().err().map(Flow::Stop)
    };
    if let Some(flow) = ends {
        if site.counter.is_some() {
            sweep_step(env, counter, changes);
        }
        return flow;
    }
    if let Some(item) = element.and_then(|i| env.frame.items.as_mut()?.get_mut(i)) {
        // Moved, not cloned: `increment >= 1` visits no element twice.
        let item = std::mem::take(item);
        if let Some(path) = &site.r#as {
            rc.set_recorded(path, item, None, traced.then_some(&mut changes));
        }
    }
    if let Some(path) = &site.scratch {
        engine_write(
            rc,
            path,
            traced,
            &mut changes,
            || OwnedDataValue::Object(Vec::new()),
            |_| DataValue::Object(&[]),
        );
    }
    env.frame.loop_counter = Some(counter);
    env.frame.sweeps = env.frame.sweeps.saturating_add(1);
    sweep_step(env, counter, changes);
    sweep
}

/// An engine write at `path` ([`RunContext::engine_set`]), with its
/// `Change` onto `changes` while the trace keeps changes.
fn engine_write<'p>(
    rc: &mut RunContext<'_, 'p>,
    path: &Path,
    traced: bool,
    changes: &mut Vec<Change>,
    owned: impl Fn() -> OwnedDataValue,
    view: impl for<'q> FnOnce(&'q Owner<'p>) -> DataValue<'q>,
) {
    let old = traced.then(|| rc.get_or_null(path));
    rc.engine_set(path, &owned, view);
    if let Some(old) = old {
        changes.push(Change::write(Arc::clone(path.dotted()), old, owned()));
    }
}

/// The engine's step for a sweep entered or a loop exited at `counter`,
/// with its writes' `changes`, when the run is traced.
fn sweep_step(env: &mut RunEnv<'_, '_>, counter: i64, changes: Vec<Change>) {
    if env.watch.trace.is_none() {
        return;
    }
    let mut step = env.workflow_step(StepResult::Executed);
    step.changes = changes;
    step.loop_counter = Some(counter);
    env.push_step(step);
}

/// Whose `over` is evaluated, which its errors name.
#[derive(Clone, Copy, Debug)]
pub(super) enum Over<'a> {
    /// A workflow's `loop.over`, by the workflow's id.
    Loop(&'a str),
    /// A task's `for_each.over`, by the task's id.
    FanOut(&'a str),
}

impl fmt::Display for Over<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Loop(id) => write!(f, "Workflow {id}: loop.over"),
            Self::FanOut(id) => write!(f, "Task {id}: for_each.over"),
        }
    }
}

/// An `over`'s elements: a loop's or a fan-out's, by one rule; a folded
/// constant's lent. A value that is not an array is [`ErrorKind::Eval`], as
/// a wrong-typed result is everywhere else; an evaluation error keeps its
/// own kind, with v3's text less v3's wrapper.
pub(super) fn elements<'e>(
    rc: &mut RunContext<'_, '_>,
    exprs: &ExprEngine,
    over: &'e Expr,
    owner: Over<'_>,
) -> Result<Cow<'e, [OwnedDataValue]>, Error> {
    let value = eval(rc, exprs, over).map_err(|e| {
        Error::new(
            e.kind().clone(),
            format!("{owner} failed to evaluate: {}", e.record_text()),
        )
    })?;
    match value {
        Cow::Owned(OwnedDataValue::Array(items)) => Ok(Cow::Owned(items)),
        Cow::Borrowed(OwnedDataValue::Array(items)) => Ok(Cow::Borrowed(items)),
        other => Err(Error::new(
            ErrorKind::Eval,
            format!("{owner} must evaluate to an array, got {}", kind(&other)),
        )),
    }
}

/// One built-in call: the call, timed in a timed run, the decision, the
/// recorder; the flow.
fn call<'p>(
    rc: &mut RunContext<'_, 'p>,
    env: &mut RunEnv<'_, 'p>,
    wf: &'p CompiledWorkflow,
    site: &'p CallSite,
    builtin: &'p Builtin,
) -> Flow {
    let errors_before = env.errors.len();
    // The program owns the ids, so copying the `Arc`s out borrows `env` only
    // for the two reads, before `mapping_ends` takes `env.watch` mutably.
    let workflow_id = Arc::clone(env.workflow_id);
    let task_id = Arc::clone(&env.task_ids[site.task as usize]);
    // An unwatched run neither traces nor times a call.
    let (traced, started) = if env.watched {
        (env.traces_changes(), env.watch.timed.then(record::now))
    } else {
        (false, None)
    };
    // Lent only to a call whose trace keeps changes.
    let mapping_ends = if traced {
        env.watch.trace.as_deref_mut().map(Capture::mapping_ends)
    } else {
        None
    };
    let mut cx = SyncCtx {
        rc: &mut *rc,
        exprs: env.exprs,
        payload: env.payload,
        workflow_id,
        task_id,
        capture: env.capture,
        traced,
        changes: &mut env.changes,
        marked: false,
        mapping_ends,
        errors: &mut *env.errors,
        now: Arc::clone(env.now_text),
    };
    let result = builtin.call(&mut cx);
    let mut marked = cx.marked;
    #[cfg(test)]
    let result = record::failing_after_its_writes(result);
    let elapsed = started.map(record::since);
    // An `Err` records no changes for the audit, as v3's recorder drops
    // them. Its writes stand, so a trace that keeps changes keeps them, for
    // itself alone: the call's `Failed` or `Executed` step carries them.
    if result.is_err() {
        if traced {
            for change in &mut env.changes {
                change.trace_only = true;
            }
            marked = true;
        } else {
            env.changes.clear();
        }
    }
    let decision = policy::decide(site.policy, env.ids(site.task).task, result);
    let done = Completed {
        task: site.task,
        stamp: Stamp::sweep(env),
        elapsed,
        errors_before,
        marked,
    };
    record::record(rc, env, wf, done, decision)
}
