//! What a run reports about itself: as it goes, to an [`Observer`], and
//! once it ends, on its [`Run`](crate::Run), with its [`Trace`] when one was
//! asked for.

mod observer;
pub(crate) mod status;
mod trace;

pub use observer::{Event, Interest, Observer};
pub(crate) use status::{RunStatus, Selection, WorkflowExit, WorkflowRecord};
pub use trace::{StepResult, Trace, TraceOptions, TraceStep};
