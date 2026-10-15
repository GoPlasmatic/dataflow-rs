//! Observers: what a run tells a host as it goes, inline (R9).

use super::{RunStatus, WorkflowRecord};
use crate::base::{ProgramId, WorkflowIx};
use crate::function::CallInfo;
use std::fmt;
use std::ops::{BitOr, BitOrAssign};
use std::time::Duration;

/// What a host is told about a run as it goes: counters, histograms and
/// spans, kept by the host.
///
/// An engine has one ([`EngineConfig::observer`]) and a run may add one
/// beside it ([`RunOptions::observer`]). Each is called only with the events
/// its [`interest`](Self::interest) names, and the clock is read only when
/// one of them asks for [`Interest::TIMINGS`].
///
/// [`on`](Self::on) is called inline, synchronously, from the run: it must
/// not block, re-enter the engine or panic. Push to a channel or bump an
/// atomic and return.
///
/// ```
/// # extern crate dataflow_rs_v4 as dataflow_rs;
/// # tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
/// use dataflow_rs::observe::{Event, Observer};
/// use dataflow_rs::run::EngineConfig;
/// use dataflow_rs::{Engine, Message};
/// use serde_json::json;
/// use std::sync::Arc;
/// use std::sync::atomic::{AtomicU64, Ordering};
///
/// #[derive(Default)]
/// struct Calls(AtomicU64);
///
/// impl Observer for Calls {
///     fn on(&self, event: &Event<'_>) {
///         if let Event::TaskFinished { .. } = event {
///             self.0.fetch_add(1, Ordering::Relaxed);
///         }
///     }
/// }
///
/// let calls = Arc::new(Calls::default());
/// let engine = Engine::builder()
///     .config(EngineConfig::default().observer(calls.clone()))
///     .workflow_json(&json!({"id": "w", "tasks": [
///         {"id": "t", "function": {"name": "map", "input": {
///             "mappings": [{"path": "data.a", "logic": 1}]}}}
///     ]}))
///     .build()
///     .unwrap();
/// engine.run(&mut Message::from_json(&json!({}))).await;
/// assert_eq!(calls.0.load(Ordering::Relaxed), 1);
/// # });
/// ```
///
/// [`EngineConfig::observer`]: crate::run::EngineConfig::observer()
/// [`RunOptions::observer`]: crate::run::RunOptions::observer()
pub trait Observer: Send + Sync + 'static {
    /// The events this observer is called with. Read once per run, when the
    /// run starts. `WORKFLOWS | TASKS` by default, which reads no clock.
    fn interest(&self) -> Interest {
        Interest::WORKFLOWS | Interest::TASKS
    }

    /// One event of a run, inline and synchronously. It must not block or
    /// re-enter the engine; what it keeps, it copies.
    fn on(&self, event: &Event<'_>);
}

/// What an [`Observer`] asks for: a set of flags. `|` and `|=` unite two
/// sets, and [`contains`](Self::contains) tests one.
///
/// Every observer receives [`RunStarted`](Event::RunStarted) and
/// [`RunFinished`](Event::RunFinished).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Interest(u8);

impl Interest {
    /// No flag: the run's events only.
    pub const NONE: Self = Self(0);
    /// [`WorkflowStarted`](Event::WorkflowStarted) and
    /// [`WorkflowFinished`](Event::WorkflowFinished).
    pub const WORKFLOWS: Self = Self(1);
    /// [`TaskFinished`](Event::TaskFinished).
    pub const TASKS: Self = Self(1 << 1);
    /// Each `elapsed` this observer is given is `Some`: the clock is read.
    pub const TIMINGS: Self = Self(1 << 2);
    /// Every flag.
    pub const ALL: Self = Self(Self::WORKFLOWS.0 | Self::TASKS.0 | Self::TIMINGS.0);

    /// The flags' names, in `Debug`'s order.
    const NAMES: [(Self, &'static str); 3] = [
        (Self::WORKFLOWS, "WORKFLOWS"),
        (Self::TASKS, "TASKS"),
        (Self::TIMINGS, "TIMINGS"),
    ];

    /// Whether every flag of `other` is in this set: always for
    /// [`NONE`](Self::NONE).
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Whether the set holds no flag.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl BitOr for Interest {
    type Output = Self;

    /// The flags of either set.
    fn bitor(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

impl BitOrAssign for Interest {
    /// Adds `other`'s flags.
    fn bitor_assign(&mut self, other: Self) {
        self.0 |= other.0;
    }
}

impl fmt::Debug for Interest {
    /// The flags set, by name: `Interest(WORKFLOWS | TASKS)`, or
    /// `Interest(NONE)`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Interest(")?;
        if self.is_empty() {
            f.write_str("NONE")?;
        }
        let set = Self::NAMES.iter().filter(|(flag, _)| self.contains(*flag));
        for (i, (_, name)) in set.enumerate() {
            if i > 0 {
                f.write_str(" | ")?;
            }
            f.write_str(name)?;
        }
        f.write_str(")")
    }
}

/// One event of a run, borrowed for the call to [`Observer::on`]: an
/// observer copies out what it keeps.
///
/// An `elapsed` is `Some` only for an observer whose interest holds
/// [`Interest::TIMINGS`].
#[derive(Debug)]
#[non_exhaustive]
pub enum Event<'a> {
    /// The run starts, after its stamps, before any workflow.
    RunStarted {
        /// The program the run runs.
        program: ProgramId,
        /// The message's id.
        message_id: &'a str,
        /// The channel the run was started on.
        channel: Option<&'a str>,
        /// How many workflows the run considers: its channel's Active ones,
        /// before any rollout or condition is judged.
        considered: usize,
    },
    /// A workflow was selected: its condition admitted it, or it has none.
    /// Once per workflow, a loop's included. A workflow that ends `Ran`
    /// with no judgment of its condition reached (a loop that enters no
    /// sweep, or a run stopped before the condition) gets it as it ends,
    /// before its `WorkflowFinished`.
    WorkflowStarted {
        /// The workflow's position in the program.
        workflow: WorkflowIx,
        /// The workflow's id.
        id: &'a str,
    },
    /// A call was recorded: once per call, a fan-out's once per element it
    /// folds, and once with no element for a fan-out with nothing to call or
    /// cut short. A task its condition refused is not.
    TaskFinished {
        /// The call: its workflow, its task and the function's registered
        /// name, its loop counter and its element.
        info: &'a CallInfo<'a>,
        /// The status recorded; `None` for a `Skip`.
        status: Option<u16>,
        /// How long the call ran, the recording not included.
        elapsed: Option<Duration>,
    },
    /// A workflow that was selected ended. Its record says how, and how many
    /// sweeps it ran.
    WorkflowFinished {
        /// What the workflow did.
        record: &'a WorkflowRecord<'a>,
        /// How long since its [`WorkflowStarted`](Event::WorkflowStarted).
        elapsed: Option<Duration>,
    },
    /// The run ended, whatever its status.
    RunFinished {
        /// The message's id, as its `RunStarted` named it: what ties the end
        /// to the start when one observer sees runs overlap.
        message_id: &'a str,
        /// How the run ended.
        status: &'a RunStatus,
        /// How many error records the message holds.
        errors: usize,
        /// How long the run took.
        elapsed: Option<Duration>,
    },
}
