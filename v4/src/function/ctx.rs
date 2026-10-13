//! `Ctx`: a function's view of one call, and the static facts about it.

use crate::base::{ProgramId, TaskIx, WorkflowIx};

/// Static facts about one call: where it sits in the program, and which
/// iteration it is.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct CallInfo<'a> {
    /// The program the call belongs to. It tells the indices of one build
    /// from another's across a reload.
    pub program: ProgramId,
    /// The workflow's position in the program.
    pub workflow: WorkflowIx,
    /// The task's position in the program.
    pub task: TaskIx,
    /// The name the function was called by.
    pub function: &'a str,
    /// The workflow's id.
    pub workflow_id: &'a str,
    /// The task's id.
    pub task_id: &'a str,
    /// The channel the run was started on; `None` when the run takes every
    /// active workflow.
    pub channel: Option<&'a str>,
    /// The loop counter, inside a workflow's loop.
    pub loop_counter: Option<i64>,
    /// The element's index, inside a `for_each` fan-out.
    pub element_index: Option<usize>,
}

/// A function's view of one call.
///
/// It holds only the call's [`CallInfo`] for now; reading the message and
/// writing to it come with the runtime. A host never builds one: the engine
/// hands it to [`Function::call`](super::Function::call) and to each
/// [`CallLayer`](super::CallLayer).
#[derive(Debug)]
pub struct Ctx<'a> {
    info: CallInfo<'a>,
}

impl<'a> Ctx<'a> {
    /// A context for one call, for the tests of the call chain.
    #[cfg(test)]
    pub(crate) fn new(info: CallInfo<'a>) -> Self {
        Self { info }
    }

    /// The static facts about this call.
    pub fn info(&self) -> &CallInfo<'a> {
        &self.info
    }
}
