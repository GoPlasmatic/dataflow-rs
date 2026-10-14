//! The interpreter: one `async fn` over a workflow's ops (05). Nothing else
//! interprets steps.
//!
//! A built-in runs inline with no `.await`; only host calls and fan-outs will
//! await, from phase 5 (I65). Every completed call goes through
//! [`decide`](super::policy::decide), the one definition of failed, rejected
//! and halted, and then [`record`](super::record::record).

use super::message::{AuditEntry, AuditMode};
use super::policy::{self, Flow};
use super::record::{self, TaskIds};
use crate::base::{Change, Error, ErrorKind, ErrorRecord};
use crate::def::WorkflowFailurePolicy;
use crate::expr::ExprEngine;
use crate::function::builtins::Builtin;
use crate::function::{SyncCtx, test};
use crate::observe::Selection;
use crate::program::{CallSite, Callee, CompiledWorkflow, CondKind, Op};
use crate::state::RunContext;
use chrono::{DateTime, Utc};
use datavalue::OwnedDataValue;
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

/// The loop frame: the sweep the workflow is in. Task 7's loops fill it.
#[derive(Debug, Default)]
pub(crate) struct Frame {
    /// The counter of the sweep running, stamped on its audit entries.
    pub(crate) loop_counter: Option<i64>,
    /// The sweeps begun: one for a workflow without a loop that ran.
    pub(crate) sweeps: u32,
}

/// What the interpreter carries through a run, beside the run context: the
/// run's facts and the message's other parts, and the current workflow's
/// selection, exit and frame.
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
    /// The current workflow's selection, once it is known.
    pub(crate) selection: Option<Selection>,
    /// Whether `on_failure: continue` stepped past a failure in it.
    pub(crate) failed: bool,
    /// The current workflow's loop frame.
    pub(crate) frame: Frame,
}

impl RunEnv<'_, '_> {
    /// Sets the workflow's selection. A workflow without a loop that ran has
    /// begun its one sweep.
    fn select(&mut self, selection: Selection, wf: &CompiledWorkflow) {
        if matches!(selection, Selection::Ran) && wf.loops.is_empty() {
            self.frame.sweeps = 1;
        }
        self.selection = Some(selection);
    }
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

/// Runs one workflow's ops over the run context.
///
/// A workflow condition's first evaluation sets the selection: `Ran`,
/// `SkippedByCondition`, or `ConditionFailed`, which jumps as a false one
/// does; a later one, which ends a loop, records nothing. A group's or a
/// task's condition only jumps. A workflow without a condition is `Ran` when
/// it starts.
pub(crate) async fn run_workflow<'p>(
    rc: &mut RunContext<'_, 'p>,
    wf: &'p CompiledWorkflow,
    env: &mut RunEnv<'_, 'p>,
) -> Exit {
    // The workflow's condition is compiled first, so it is the first row
    // when it has one; a group's or task's names its step.
    if !wf.conds.first().is_some_and(|c| c.step.is_none()) {
        env.select(Selection::Ran, wf);
    }
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
                match (kind, env.selection.is_none()) {
                    (CondKind::Workflow, true) => {
                        let (selection, flow) = match passed {
                            Ok(true) => (Selection::Ran, Flow::Next),
                            Ok(false) => (Selection::SkippedByCondition, Flow::Jump(else_to)),
                            Err(e) => (Selection::ConditionFailed(e), Flow::Jump(else_to)),
                        };
                        env.select(selection, wf);
                        flow
                    }
                    _ if matches!(passed, Ok(true)) => Flow::Next,
                    _ => Flow::Jump(else_to),
                }
            }
            Op::Call { site, .. } => {
                let site = &wf.calls[site as usize];
                match &site.callee {
                    Callee::Builtin(builtin) => call(rc, env, wf, site, builtin),
                    Callee::Host(_) => return Exit::Stopped(not_yet()),
                }
            }
            Op::FanOut { .. } => return Exit::Stopped(not_yet()),
            Op::LoopInit { .. } | Op::LoopNext { .. } => unreachable!("Task 7"),
            Op::Halt => Flow::Halt,
        };
        pc = match flow {
            Flow::Next => pc + 1,
            Flow::Jump(to) => to as usize,
            Flow::Halt => return Exit::Halted,
            Flow::Fail(e) => match wf.on_failure {
                WorkflowFailurePolicy::Continue => {
                    env.failed = true;
                    fail_to(op) as usize
                }
                WorkflowFailurePolicy::Fail => return Exit::Stopped(e),
            },
        };
    }
    if env.failed {
        Exit::Failed
    } else {
        Exit::Completed
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
