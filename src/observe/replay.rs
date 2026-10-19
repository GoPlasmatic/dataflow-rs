//! On-demand trace replay, retaining one working context rather than every step.

use super::Trace;
use super::trace::{apply, valid_mapping_ends};
use crate::base::Context;

/// A trace replayed on demand, with one working context.
///
/// Forward seeks apply only the intervening steps. A backward seek starts
/// again from the initial context. Memory use does not grow with the number
/// of steps visited. Like [`Trace::contexts`], this trusts its trace; call
/// [`Trace::validate`] first when reading an externally supplied trace.
#[derive(Debug)]
pub struct Replay {
    trace: Trace,
    context: Context,
    /// The next step to apply: zero means the initial context.
    next: usize,
}

impl Replay {
    /// Takes ownership of a trace and starts at its initial context.
    pub fn new(trace: Trace) -> Self {
        Self {
            context: trace.initial.clone(),
            trace,
            next: 0,
        }
    }

    /// The trace being replayed.
    pub fn trace(&self) -> &Trace {
        &self.trace
    }

    /// The context after `step`, or the initial context for `None`.
    /// Returns `None` for a step outside the trace, leaving the cursor unchanged.
    pub fn context_at(&mut self, step: Option<usize>) -> Option<&Context> {
        let end = match step {
            Some(step) => step.checked_add(1)?,
            None => 0,
        };
        if end > self.trace.steps.len() {
            return None;
        }
        if end < self.next {
            self.context = self.trace.initial.clone();
            self.next = 0;
        }
        for step in &self.trace.steps[self.next..end] {
            for change in &step.changes {
                apply(&mut self.context.root, change);
            }
        }
        self.next = end;
        Some(&self.context)
    }

    /// The context after one mapping of `step`, before its trailing engine
    /// writes. Returns `None` for a missing step or mapping, or malformed
    /// mapping boundaries. Only the requested mapping's context is copied.
    pub fn mapping_context_at(&mut self, step: usize, mapping: usize) -> Option<Context> {
        let at = self.trace.steps.get(step)?;
        if !valid_mapping_ends(at) {
            return None;
        }
        let end = *at.mapping_ends.get(mapping)? as usize;
        let mut context = self.context_at(step.checked_sub(1))?.clone();
        for change in &self.trace.steps[step].changes[..end] {
            apply(&mut context.root, change);
        }
        Some(context)
    }
}
