//! What a run reports about itself.

use crate::base::{Error, WorkflowIx};

/// How a run ended.
#[derive(Debug)]
#[non_exhaustive]
pub enum RunStatus {
    /// Every considered workflow finished and the message holds no errors.
    Completed,
    /// Every considered workflow finished, but the message holds error records.
    CompletedWithErrors,
    /// A workflow stopped the run with this error. An error of kind
    /// `TimedOut` or `Cancelled` a function returned, a child run's
    /// propagated, stops the run here: only the run's own limits give the
    /// two statuses below.
    Stopped(Error),
    /// The run's deadline passed: at a check between ops or at a sweep, or
    /// while a call it then cut short ran.
    TimedOut,
    /// The run's token, or one of its ancestors, was cancelled.
    Cancelled,
}

/// Why a workflow did or did not run.
#[derive(Debug)]
#[non_exhaustive]
pub enum Selection {
    /// The workflow was selected and ran.
    Ran,
    /// Its condition was false.
    SkippedByCondition,
    /// Its condition failed to evaluate, which counts as false.
    ConditionFailed(Error),
    /// The message's routing bucket is outside its rollout.
    SkippedByRollout,
}

/// How a workflow that ran ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum WorkflowExit {
    /// It ran to its end.
    Completed,
    /// A task or a halt ended it early, on purpose.
    Halted,
    /// A task failed and `on_failure: continue` stepped past it.
    Failed,
    /// It stopped the run.
    Stopped,
}

/// One workflow's part in a run.
#[derive(Debug)]
#[non_exhaustive]
pub struct WorkflowRecord<'r> {
    /// The workflow's id.
    pub id: &'r str,
    /// The workflow's position in the program.
    pub index: WorkflowIx,
    /// Whether it was selected.
    pub selection: &'r Selection,
    /// The sweeps begun: one for a workflow without a loop that ran, and for
    /// a loop each sweep its condition admitted.
    pub sweeps: u32,
    /// How it ended; `None` when it did not run.
    pub exit: Option<WorkflowExit>,
}
