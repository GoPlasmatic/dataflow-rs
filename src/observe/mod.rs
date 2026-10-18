//! What a run reports about itself: as it goes, to an [`Observer`], and
//! once it ends, on its [`Run`](crate::Run), with its [`Trace`] when one was
//! asked for.

mod observer;
pub(crate) mod status;
mod trace;

use crate::base::{Error, WorkflowIx};
pub use observer::{Event, Interest, Observer};
use serde::ser::SerializeStruct;
use serde::{Serialize, Serializer};
pub(crate) use status::RunStatus;
pub use trace::{StepResult, Trace, TraceOptions, TraceStep};

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
    /// How it ended, if it ran.
    pub exit: Option<WorkflowExit>,
}

/// Writes the `selection` string of a workflow's record: `ran`,
/// `skipped_by_condition`, `condition_failed` or `skipped_by_rollout`. The
/// error of a failed condition is a key of the record, which
/// [`WorkflowRecord`] writes.
impl Serialize for Selection {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(match self {
            Self::Ran => "ran",
            Self::SkippedByCondition => "skipped_by_condition",
            Self::ConditionFailed(_) => "condition_failed",
            Self::SkippedByRollout => "skipped_by_rollout",
        })
    }
}

/// Writes how a workflow ended as a lowercase string: `completed`,
/// `halted`, `failed` or `stopped`.
impl Serialize for WorkflowExit {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(match self {
            Self::Completed => "completed",
            Self::Halted => "halted",
            Self::Failed => "failed",
            Self::Stopped => "stopped",
        })
    }
}

/// Writes a workflow's record as `{"id", "index", "selection", "error"?,
/// "sweeps", "exit"}`: `error` only for a condition that failed to
/// evaluate, and `exit` `null` for a workflow that did not run.
impl Serialize for WorkflowRecord<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let error = match self.selection {
            Selection::ConditionFailed(e) => Some(e),
            _ => None,
        };
        let mut o = s.serialize_struct("WorkflowRecord", 6)?;
        o.serialize_field("id", self.id)?;
        o.serialize_field("index", &self.index)?;
        o.serialize_field("selection", self.selection)?;
        match error {
            Some(e) => o.serialize_field("error", e)?,
            None => o.skip_field("error")?,
        }
        o.serialize_field("sweeps", &self.sweeps)?;
        o.serialize_field("exit", &self.exit)?;
        o.end()
    }
}

#[cfg(feature = "ts")]
crate::base::ts::ts_decl!(
    Selection,
    "Selection",
    [
        " Why a workflow did or did not run: a failed condition's error is its",
        " record's `error`.",
    ],
    |_cfg| crate::base::ts::literals(&[
        "ran",
        "skipped_by_condition",
        "condition_failed",
        "skipped_by_rollout",
    ])
);

#[cfg(feature = "ts")]
crate::base::ts::ts_decl!(
    WorkflowExit,
    "WorkflowExit",
    [" How a workflow that ran ended."],
    |_cfg| crate::base::ts::literals(&["completed", "halted", "failed", "stopped"])
);

#[cfg(feature = "ts")]
crate::base::ts::ts_decl!(
    WorkflowRecord<'_>,
    "WorkflowRecord",
    [" One workflow's part in a run."],
    |cfg| {
        use crate::base::ts::{Prop, object};
        object(&[
            Prop::new("id", "string", &[" The workflow's id."]),
            Prop::new(
                "index",
                WorkflowIx::name(cfg),
                &[" The workflow's position in the program."],
            ),
            Prop::new(
                "selection",
                Selection::name(cfg),
                &[" Whether it was selected."],
            ),
            Prop::optional(
                "error",
                Error::name(cfg),
                &[
                    " Why its condition failed to evaluate; present only when `selection` is",
                    " `condition_failed`.",
                ],
            ),
            Prop::new(
                "sweeps",
                "number",
                &[
                    " The sweeps begun: one for a workflow without a loop that ran, and for",
                    " a loop each sweep its condition admitted.",
                ],
            ),
            Prop::new(
                "exit",
                Option::<WorkflowExit>::name(cfg),
                &[" How it ended; `null` when it did not run."],
            ),
        ])
    }
);
