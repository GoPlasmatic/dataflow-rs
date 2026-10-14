//! The interpreter: one `async fn` over a workflow's ops (05). Nothing else
//! interprets steps.
//!
//! A built-in runs inline with no `.await`; only host calls and fan-outs will
//! await, from phase 5 (I65). Every completed call goes through
//! [`decide`](super::policy::decide), the one definition of failed, rejected
//! and halted, and then [`record`](super::record::record).
//!
//! A loop is two ops, in 05's order. `LoopInit` evaluates `over` once, writes
//! the counter's `init`, checks the bounds, and in bounds writes the bindings
//! and falls into the first sweep; `LoopNext` advances the counter and does
//! the same, jumping back to the sweep's first op. The engine owns the
//! counter: it lives in the [`Frame`], and the context only gets a copy.

use super::message::{AuditEntry, AuditMode};
use super::policy::{self, Flow};
use super::record::{self, TaskIds};
use crate::base::{Change, Error, ErrorKind, ErrorRecord};
use crate::def::WorkflowFailurePolicy;
use crate::expr::path::kind;
use crate::expr::{Expr, ExprEngine};
use crate::function::builtins::Builtin;
use crate::function::{SyncCtx, eval, test};
use crate::observe::Selection;
use crate::program::{CallSite, Callee, CompiledWorkflow, CondKind, LoopSite, Op};
use crate::state::RunContext;
use chrono::{DateTime, Utc};
use datavalue::{DataValue, OwnedDataValue};
use std::borrow::Cow;
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
    /// The workflow stops the run with this error.
    Stopped(Error),
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
    /// Whether a call records its `Change`s (I67).
    pub(crate) capture: bool,
    /// The run's one clock reading.
    pub(crate) now: DateTime<Utc>,
    /// The same reading, RFC 3339, for error records.
    pub(crate) now_text: &'r str,
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

/// I65: until phase 5, a host call or a fan-out stops the run.
fn not_yet() -> Error {
    Error::new(
        ErrorKind::Config,
        "host functions and fan-outs run from phase 5",
    )
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
pub(crate) async fn run_workflow<'p>(
    rc: &mut RunContext<'_, 'p>,
    wf: &'p CompiledWorkflow,
    env: &mut RunEnv<'_, 'p>,
) -> (Exit, Selection) {
    let mut selection = Selection::Ran;
    // The workflow's condition is compiled first, so it is the first row
    // when it has one; a group's or task's names its step.
    let mut unjudged = wf.conds.first().is_some_and(|c| c.step.is_none());
    env.frame.sweeps = u32::from(wf.loops.is_empty());
    let mut pc = 0;
    while let Some(&op) = wf.ops.get(pc) {
        rc.tick();
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
                            Ok(true) => Selection::Ran,
                            Ok(false) => Selection::SkippedByCondition,
                            Err(e) => Selection::ConditionFailed(e),
                        };
                    }
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
                    Callee::Host(_) => return (Exit::Stopped(not_yet()), selection),
                }
            }
            Op::FanOut { .. } => return (Exit::Stopped(not_yet()), selection),
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
            Flow::Halt => return (Exit::Halted, selection),
            Flow::Fail(e) => match wf.on_failure {
                WorkflowFailurePolicy::Continue => {
                    env.failed = true;
                    fail_to(op) as usize
                }
                WorkflowFailurePolicy::Fail => return (Exit::Stopped(e), selection),
            },
        };
    }
    let exit = if env.failed {
        Exit::Failed
    } else {
        Exit::Completed
    };
    (exit, selection)
}

/// `LoopInit`: `over`, once, then the counter's `init` and the first sweep.
///
/// An `over` that does not give an array fails the workflow before the
/// counter is written (I61): one record, whatever the workflow's policy,
/// which then decides; under `continue` the failure goes to the loop's exit.
fn loop_init<'p>(
    rc: &mut RunContext<'_, 'p>,
    env: &mut RunEnv<'_, 'p>,
    site: &'p LoopSite,
    exit_to: u32,
) -> Flow {
    if let Some(over) = &site.over {
        match elements(rc, env, over) {
            Ok(items) => env.frame.items = Some(items),
            Err(e) => {
                record::workflow_failure(env, &e);
                return Flow::Fail(e);
            }
        }
    }
    env.frame.counter = site.init;
    enter(rc, &mut env.frame, site, Flow::Next, exit_to)
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
    enter(rc, &mut env.frame, site, Flow::Jump(body_to), exit_to)
}

/// The sweep at the frame's counter, in 05's order: the counter written,
/// then the bounds, `counter < max` and `counter < len(over)`, then in bounds
/// `over[counter]` moved into `as` and `{}` into `scratch`, before the
/// sweep's condition. Out of bounds the loop exits, leaving the first counter
/// past them in the context. None of these writes records a `Change`.
fn enter<'p>(
    rc: &mut RunContext<'_, 'p>,
    frame: &mut Frame,
    site: &'p LoopSite,
    sweep: Flow,
    exit_to: u32,
) -> Flow {
    let counter = frame.counter;
    if let Some(path) = &site.counter {
        rc.engine_set(
            path,
            || OwnedDataValue::from_i64(counter),
            |_| DataValue::from_i64(counter),
        );
    }
    if counter >= site.max {
        return Flow::Jump(exit_to);
    }
    if let Some(items) = &mut frame.items {
        let Some(item) = usize::try_from(counter).ok().and_then(|i| items.get_mut(i)) else {
            return Flow::Jump(exit_to);
        };
        // Moved, not cloned: `increment >= 1` visits no element twice.
        let item = std::mem::take(item);
        if let Some(path) = &site.r#as {
            rc.set(path, item, None);
        }
    }
    if let Some(path) = &site.scratch {
        rc.engine_set(
            path,
            || OwnedDataValue::Object(Vec::new()),
            |_| DataValue::Object(&[]),
        );
    }
    frame.loop_counter = Some(counter);
    frame.sweeps = frame.sweeps.saturating_add(1);
    sweep
}

/// `loop.over`'s elements, owned. A value that is not an array is I61's
/// [`ErrorKind::Eval`]; an evaluation error keeps its own kind, with v3's
/// text less v3's `WORKFLOW_ERROR` wrapper, as I61 words the other.
fn elements(
    rc: &mut RunContext<'_, '_>,
    env: &RunEnv<'_, '_>,
    over: &Expr,
) -> Result<Vec<OwnedDataValue>, Error> {
    let id = env.workflow_id;
    let value = eval(rc, env.exprs, over).map_err(|e| {
        Error::new(
            e.kind().clone(),
            format!(
                "Workflow {id}: loop.over failed to evaluate: {}",
                e.record_text()
            ),
        )
    })?;
    match value {
        Cow::Owned(OwnedDataValue::Array(items)) => Ok(items),
        Cow::Borrowed(OwnedDataValue::Array(items)) => Ok(items.clone()),
        other => Err(Error::new(
            ErrorKind::Eval,
            format!(
                "Workflow {id}: loop.over must evaluate to an array, got {}",
                kind(&other)
            ),
        )),
    }
}

/// One built-in call: the call, the decision, the recorder; the flow.
fn call<'p>(
    rc: &mut RunContext<'_, 'p>,
    env: &mut RunEnv<'_, 'p>,
    wf: &'p CompiledWorkflow,
    site: &'p CallSite,
    builtin: &'p Builtin,
) -> Flow {
    let ids = TaskIds {
        workflow: env.workflow_id,
        task: &env.task_ids[site.task as usize],
    };
    let result = builtin.call(&mut SyncCtx {
        rc: &mut *rc,
        exprs: env.exprs,
        payload: env.payload,
        workflow_id: &wf.id,
        task_id: &wf.tasks[site.task as usize].id,
        capture: env.capture,
        changes: &mut env.changes,
        errors: &mut *env.errors,
        now: env.now_text,
    });
    // An `Err` records no changes, as v3's recorder drops them; its writes
    // stand.
    if result.is_err() {
        env.changes.clear();
    }
    let decision = policy::decide(site, ids.task, result);
    record::record(rc, env, ids, decision.status, decision.record);
    decision.flow
}
