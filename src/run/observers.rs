//! The observers attached to one run: the engine's and the run's,
//! each with its interest read once, and the union of the two, which decides
//! what the run computes for them. The clock is read per kind of event: a
//! call's only for a slot that holds both `TASKS` and `TIMINGS`, a
//! workflow's only for one that holds `WORKFLOWS` and `TIMINGS`, and the
//! run's for any `TIMINGS`, which every slot's run events then carry.

use crate::observe::{Event, Interest, Observer};

/// The attached observers, their interests read once, and the union.
///
/// Not `Copy`: `run_with` moves it into its `Watch`, so the run's future
/// holds one.
pub(crate) struct Observers<'r> {
    /// The engine's, then the run's.
    slots: [Option<(&'r dyn Observer, Interest)>; 2],
    union: Interest,
    /// Some slot holds `TASKS` and `TIMINGS`.
    timed_tasks: bool,
    /// Some slot holds `WORKFLOWS` and `TIMINGS`.
    timed_workflows: bool,
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
        let timed = |kind: Interest| {
            let both = kind | Interest::TIMINGS;
            slots.iter().flatten().any(|&(_, i)| i.contains(both))
        };
        Self {
            timed_tasks: timed(Interest::TASKS),
            timed_workflows: timed(Interest::WORKFLOWS),
            slots,
            union,
        }
    }

    /// Whether an observer asks for timings: the run's own elapsed time is
    /// read then.
    pub(crate) fn timed(&self) -> bool {
        self.union.contains(Interest::TIMINGS)
    }

    /// Whether an observer of the task events asks for timings: only then
    /// is a call timed for an observer (A M-2).
    pub(crate) fn timed_tasks(&self) -> bool {
        self.timed_tasks
    }

    /// Whether an observer of the workflow events asks for timings: only
    /// then is a workflow timed.
    pub(crate) fn timed_workflows(&self) -> bool {
        self.timed_workflows
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
    /// every slot. A run without observers builds no event.
    #[inline]
    pub(crate) fn emit(&self, event: Event<'_>) {
        if self.slots.iter().any(Option::is_some) {
            self.deliver(event);
        }
    }

    /// [`emit`](Self::emit)'s delivery, to every slot.
    fn deliver(&self, event: Event<'_>) {
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
        Event::RunFinished {
            message_id,
            status,
            errors,
            ..
        } => Event::RunFinished {
            message_id,
            status,
            errors,
            elapsed: None,
        },
    }
}
