//! What a run reports about itself: as it goes, to an [`Observer`], and
//! once it ends, on its [`Run`](crate::Run), with its [`Trace`] when one was
//! asked for.

mod observer;
mod replay;
pub(crate) mod status;
mod trace;

pub use observer::{Event, Interest, Observer};
pub use replay::Replay;
pub(crate) use status::{RunStatus, Selection, WorkflowExit, WorkflowRecord};
pub use trace::{StepResult, Trace, TraceOptions, TraceStep};
