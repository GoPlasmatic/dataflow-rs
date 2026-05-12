//! # Task outcome
//!
//! Type returned by every task execution. Replaces the historical
//! `(usize, Vec<Change>)` tuple — the bare `usize` doubled as both an HTTP-like
//! status (200) *and* an in-band control-flow signal (skip / halt) which made
//! the contract impossible to type-check. `TaskOutcome` makes the four cases
//! explicit; the audit-trail status code is derived from the variant.
//!
//! The `Vec<Change>` is no longer part of the return value — sync built-ins
//! emit changes via the executor's per-task buffer, async handlers via
//! `TaskContext`. The audit trail is recorded by the workflow executor based
//! on the variant returned here.

/// Status code recorded on the audit trail when a task halts the workflow.
///
/// Preserved from v3 (`FILTER_STATUS_HALT = 299`) so existing audit-log
/// consumers (e.g. dataflow-ui) keep parsing halt entries unchanged.
pub const HALT_STATUS_CODE: u16 = 299;

/// Outcome of a single task execution.
///
/// Returned by `AsyncFunctionHandler::execute` and by the engine's internal
/// sync-builtin dispatch path. The workflow executor uses the variant to
/// decide whether to continue, skip the audit trail, or halt the workflow.
///
/// HTTP-like status semantics (preserved from v3 and earlier):
/// - status `200` — normal completion
/// - status `400..500` — logged as a warning, workflow continues
/// - status `500..` — recorded; fails the workflow unless the task or
///   workflow has `continue_on_error = true`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[must_use]
pub enum TaskOutcome {
    /// Normal completion. Audit trail records status `200` and the workflow
    /// continues with the next task.
    #[default]
    Success,

    /// Completion with an explicit HTTP-like status code. Audit trail records
    /// the supplied status. Codes in the `400..500` range log a warning;
    /// codes `>= 500` fail the workflow unless `continue_on_error` is set.
    Status(u16),

    /// Skip recording an audit trail entry for this task; continue with the
    /// next task. Used by filter gates that intentionally no-op when their
    /// condition does not match.
    Skip,

    /// Record the audit trail (status [`HALT_STATUS_CODE`]) and stop further
    /// tasks in the current workflow. Subsequent workflows registered on the
    /// same engine still process this message normally.
    Halt,
}

impl TaskOutcome {
    /// HTTP-like status code that will be stamped on the audit trail entry
    /// for this outcome. `Skip` returns `None` since no audit entry is
    /// recorded in that case.
    #[inline]
    pub fn audit_status(self) -> Option<u16> {
        match self {
            TaskOutcome::Success => Some(200),
            TaskOutcome::Status(s) => Some(s),
            TaskOutcome::Skip => None,
            TaskOutcome::Halt => Some(HALT_STATUS_CODE),
        }
    }

    /// Whether the workflow executor should halt further tasks in this
    /// workflow after observing this outcome.
    #[inline]
    pub fn halts_workflow(self) -> bool {
        matches!(self, TaskOutcome::Halt)
    }
}
