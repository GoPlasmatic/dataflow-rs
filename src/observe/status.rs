//! How a run ended.

use crate::base::Error;
use serde::{Serialize, Serializer};

/// How a run ended.
#[derive(Debug)]
#[non_exhaustive]
pub enum RunStatus {
    /// Every considered workflow finished and the message holds no errors.
    Completed,
    /// Every considered workflow finished, but the message holds error records.
    CompletedWithErrors,
    /// A workflow stopped the run with this error. A function's `Err` of
    /// kind `TimedOut` or `Cancelled`, such as a child run's propagated,
    /// ends the run here when its task's policies stop the run and it
    /// returned before the run's own deadline passed or its token was
    /// cancelled: only the run's own limits give the two statuses below.
    Stopped(Error),
    /// The run's deadline passed: at a check between ops or at a sweep, or
    /// while a call ran that it then cut short or that returned `Err`
    /// after it.
    TimedOut,
    /// The run's token, or one of its ancestors, was cancelled: before a
    /// check, or while a call ran that it then cut short or that returned
    /// `Err` after it.
    Cancelled,
}

/// Writes the `status` string of a run's JSON: `completed`,
/// `completed_with_errors`, `stopped`, `timed_out` or `cancelled`. The
/// `error` of a stopped run is a key of the run's object, which
/// [`Run`](crate::Run) writes.
impl Serialize for RunStatus {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(match self {
            Self::Completed => "completed",
            Self::CompletedWithErrors => "completed_with_errors",
            Self::Stopped(_) => "stopped",
            Self::TimedOut => "timed_out",
            Self::Cancelled => "cancelled",
        })
    }
}

#[cfg(feature = "ts")]
crate::base::ts::ts_decl!(
    RunStatus,
    "RunStatus",
    [" How a run ended: a stopped run's error is its `error`."],
    |_cfg| crate::base::ts::literals(&[
        "completed",
        "completed_with_errors",
        "stopped",
        "timed_out",
        "cancelled",
    ])
);
