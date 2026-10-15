//! The observers attached to one run (I104): the engine's and the run's,
//! each with its interest read once, and the union of the two, which decides
//! what the run computes for them, the clock reads included.

use crate::observe::{Event, Interest, Observer};

/// The attached observers, their interests read once, and the union.
///
/// Not `Copy`: `run_with` moves it into its `Watch`, so the run's future
/// holds one.
pub(crate) struct Observers<'r> {
    /// The engine's, then the run's.
    slots: [Option<(&'r dyn Observer, Interest)>; 2],
    union: Interest,
}

impl<'r> Observers<'r> {
    /// The engine's observer and the run's, if any, each asked its interest
    /// once.
    pub(crate) fn attach(engine: Option<&'r dyn Observer>, run: Option<&'r dyn Observer>) -> Self {
        let slots = [engine, run].map(|o| o.map(|o| (o, o.interest())));
        let union = slots
            .iter()
            .flatten()
            .fold(Interest::NONE, |union, &(_, interest)| union | interest);
        Self { slots, union }
    }

    /// Whether an observer asks for timings: only then is the clock read.
    pub(crate) fn timed(&self) -> bool {
        self.union.contains(Interest::TIMINGS)
    }

    /// Whether an observer asks for the workflow events.
    pub(crate) fn workflows(&self) -> bool {
        self.union.contains(Interest::WORKFLOWS)
    }

    /// Whether an observer asks for the task events.
    pub(crate) fn tasks(&self) -> bool {
        self.union.contains(Interest::TASKS)
    }

    /// Each slot gets the event if its interest admits it; `elapsed` is
    /// masked to `None` for a slot without `TIMINGS`. A run event goes to
    /// every slot.
    pub(crate) fn emit(&self, event: Event<'_>) {
        let wants = match event {
            Event::RunStarted { .. } | Event::RunFinished { .. } => Interest::NONE,
            Event::WorkflowStarted { .. } | Event::WorkflowFinished { .. } => Interest::WORKFLOWS,
            Event::TaskFinished { .. } => Interest::TASKS,
        };
        for &(observer, interest) in self.slots.iter().flatten() {
            if !interest.contains(wants) {
                continue;
            }
            if interest.contains(Interest::TIMINGS) {
                observer.on(&event);
            } else {
                observer.on(&untimed(&event));
            }
        }
    }
}

/// `event` with its `elapsed`, if it carries one, `None`.
fn untimed<'a>(event: &Event<'a>) -> Event<'a> {
    match *event {
        Event::RunStarted {
            program,
            message_id,
            channel,
            considered,
        } => Event::RunStarted {
            program,
            message_id,
            channel,
            considered,
        },
        Event::WorkflowStarted { workflow, id } => Event::WorkflowStarted { workflow, id },
        Event::TaskFinished { info, status, .. } => Event::TaskFinished {
            info,
            status,
            elapsed: None,
        },
        Event::WorkflowFinished { record, .. } => Event::WorkflowFinished {
            record,
            elapsed: None,
        },
        Event::RunFinished { status, errors, .. } => Event::RunFinished {
            status,
            errors,
            elapsed: None,
        },
    }
}
