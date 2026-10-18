//! What a finished run reports.

use crate::base::{Error, ErrorKind, WorkflowIx};
use crate::observe::{RunStatus, Selection, Trace, WorkflowExit, WorkflowRecord};
use crate::program::Program;
use crate::state::Stats;
use serde::ser::SerializeStruct;
use serde::{Serialize, Serializer};

/// One workflow considered by a run, as the engine records it.
#[derive(Debug)]
pub(crate) struct Record {
    pub(crate) ix: WorkflowIx,
    pub(crate) selection: Selection,
    pub(crate) sweeps: u32,
    pub(crate) exit: Option<WorkflowExit>,
}

/// What the run's arena and view did, counted.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ArenaStats {
    /// Views built.
    pub views: u32,
    /// Checkpoints taken.
    pub checkpoints: u32,
    /// The largest view spine, in bytes, right after a build.
    pub live_max: usize,
    /// The largest run-arena bytes in use.
    pub peak_arena: usize,
}

/// The report of one run: how it ended, what each workflow did, and its
/// trace when one was asked for.
pub struct Run {
    program: Program,
    status: RunStatus,
    records: Vec<Record>,
    stats: Stats,
    /// Boxed: the report is the run future's output, and most runs are
    /// not traced.
    trace: Option<Box<Trace>>,
}

impl Run {
    pub(crate) fn new(
        program: Program,
        status: RunStatus,
        records: Vec<Record>,
        stats: Stats,
        trace: Option<Box<Trace>>,
    ) -> Self {
        Self {
            program,
            status,
            records,
            stats,
            trace,
        }
    }

    /// The run's trace, when [`RunOptions::trace`](crate::RunOptions::trace())
    /// asked for one: on every status, the step that stopped the run
    /// included. A run stopped between ops made no step for the stop, and
    /// one stopped as a sweep starts only the engine's step for the counter
    /// it wrote; either way the trace replays to the message's context.
    pub fn trace(&self) -> Option<&Trace> {
        self.trace.as_deref()
    }

    /// How the run ended.
    pub fn status(&self) -> &RunStatus {
        &self.status
    }

    /// Every workflow considered, in `WorkflowIx` order, up to and including
    /// the one that stopped the run.
    pub fn workflows(&self) -> impl Iterator<Item = WorkflowRecord<'_>> {
        self.records.iter().map(|r| WorkflowRecord {
            id: &self.program.workflows()[r.ix.index() as usize].id,
            index: r.ix,
            selection: &r.selection,
            sweeps: r.sweeps,
            exit: r.exit,
        })
    }

    /// How many workflows were selected and ran.
    pub fn matched(&self) -> usize {
        self.records
            .iter()
            .filter(|r| matches!(r.selection, Selection::Ran))
            .count()
    }

    /// May running the message again succeed? True for a timeout and for a
    /// stop whose error says so.
    pub fn retryable(&self) -> bool {
        match &self.status {
            RunStatus::Stopped(e) => e.is_retryable(),
            RunStatus::TimedOut => true,
            _ => false,
        }
    }

    /// Did the run end before every workflow was considered?
    pub fn partial(&self) -> bool {
        matches!(
            self.status,
            RunStatus::Stopped(_) | RunStatus::TimedOut | RunStatus::Cancelled
        )
    }

    /// The run's trace, taken out of the report, which keeps none after.
    /// Take it before [`into_result`](Self::into_result) when the trace is
    /// wanted on every status: `into_result` drops the report, the trace
    /// with it, on exactly the runs that stopped.
    pub fn take_trace(&mut self) -> Option<Trace> {
        self.trace.take().map(|trace| *trace)
    }

    /// `Ok(self)` unless the run was stopped, timed out or cancelled, which
    /// become the error, the report and its trace dropped; take the trace
    /// first with [`take_trace`](Self::take_trace) to keep it.
    pub fn into_result(self) -> crate::Result<Self> {
        match self.status {
            RunStatus::Stopped(e) => Err(e),
            RunStatus::TimedOut => Err(Error::new(ErrorKind::TimedOut, "the run timed out")),
            RunStatus::Cancelled => Err(Error::new(ErrorKind::Cancelled, "the run was cancelled")),
            RunStatus::Completed | RunStatus::CompletedWithErrors => Ok(self),
        }
    }

    /// The run's arena counters.
    #[doc(hidden)]
    pub fn arena_stats(&self) -> ArenaStats {
        ArenaStats {
            views: self.stats.views,
            checkpoints: self.stats.checkpoints,
            live_max: self.stats.live_max,
            peak_arena: self.stats.peak_arena,
        }
    }
}

/// Writes a run as `{"status", "error"?, "retryable", "partial", "matched",
/// "workflows"}`: `error` only for a stopped run, `workflows` the
/// records of [`Run::workflows`]. Neither the trace nor the arena counters
/// are part of it. A `Run` has no `Deserialize`, since it holds its
/// program.
impl Serialize for Run {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        /// The records, written as a sequence.
        struct Records<'r>(&'r Run);
        impl Serialize for Records<'_> {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.collect_seq(self.0.workflows())
            }
        }

        let mut o = s.serialize_struct("Run", 6)?;
        o.serialize_field("status", &self.status)?;
        match &self.status {
            RunStatus::Stopped(e) => o.serialize_field("error", e)?,
            _ => o.skip_field("error")?,
        }
        o.serialize_field("retryable", &self.retryable())?;
        o.serialize_field("partial", &self.partial())?;
        o.serialize_field("matched", &self.matched())?;
        o.serialize_field("workflows", &Records(self))?;
        o.end()
    }
}

#[cfg(feature = "ts")]
crate::base::ts::ts_decl!(
    Run,
    "Run",
    [
        " A run as JSON: how it ended and what each workflow did. Neither the trace",
        " nor the arena counters are part of it.",
    ],
    |cfg| {
        use crate::base::ts::{Prop, object};
        object(&[
            Prop::new("status", RunStatus::name(cfg), &[" How the run ended."]),
            Prop::optional(
                "error",
                Error::name(cfg),
                &[" The error that stopped the run; present only when `status` is `stopped`."],
            ),
            Prop::new(
                "retryable",
                "boolean",
                &[
                    " May running the message again succeed? True for a timeout and for a",
                    " stop whose error says so.",
                ],
            ),
            Prop::new(
                "partial",
                "boolean",
                &[" Did the run end before every workflow was considered?"],
            ),
            Prop::new(
                "matched",
                "number",
                &[" How many workflows were selected and ran."],
            ),
            Prop::new(
                "workflows",
                Vec::<WorkflowRecord<'static>>::name(cfg),
                &[
                    " Every workflow considered, in program order, up to and including the",
                    " one that stopped the run.",
                ],
            ),
        ])
    }
);

impl std::fmt::Debug for Run {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Run")
            .field("status", &self.status)
            .field("records", &self.records)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Registry;
    use crate::def::{ParseOptions, parse};
    use crate::program::CompileOptions;
    use serde_json::json;

    fn program() -> Program {
        let compiler = Registry::default().compiler(CompileOptions::default());
        let ws = ["a", "b", "c", "d", "e"].map(|id| {
            let json = json!({"id": id, "tasks": [
                {"id": "t", "function": {"name": "log", "input": {"message": "hi"}}}
            ]});
            let def = parse(&json, ParseOptions::default()).def.unwrap();
            compiler.compile_one(&def).unwrap().workflow
        });
        compiler.link(ws)
    }

    fn record(i: u32, selection: Selection, exit: Option<WorkflowExit>) -> Record {
        Record {
            ix: WorkflowIx::new(i),
            selection,
            sweeps: u32::from(exit.is_some()),
            exit,
        }
    }

    fn records() -> Vec<Record> {
        vec![
            record(0, Selection::Ran, Some(WorkflowExit::Completed)),
            record(1, Selection::SkippedByCondition, None),
            record(
                2,
                Selection::ConditionFailed(Error::new(ErrorKind::Eval, "x")),
                None,
            ),
            record(3, Selection::SkippedByRollout, None),
            record(4, Selection::Ran, Some(WorkflowExit::Stopped)),
        ]
    }

    fn run(status: RunStatus) -> Run {
        Run::new(program(), status, records(), Stats::default(), None)
    }

    #[test]
    fn a_run_reports_its_workflows_and_their_exits() {
        let r = run(RunStatus::Completed);
        assert_eq!(r.matched(), 2);
        assert!(!r.retryable() && !r.partial());
        let seen: Vec<_> = r.workflows().map(|w| (w.id, w.sweeps, w.exit)).collect();
        assert_eq!(seen[0], ("a", 1, Some(WorkflowExit::Completed)));
        assert_eq!(seen[1], ("b", 0, None));
        assert_eq!(seen[4], ("e", 1, Some(WorkflowExit::Stopped)));
        assert!(matches!(
            r.workflows().nth(2).unwrap().selection,
            Selection::ConditionFailed(_)
        ));
        assert!(r.into_result().is_ok());
        assert!(run(RunStatus::CompletedWithErrors).into_result().is_ok());
        assert!(!run(RunStatus::CompletedWithErrors).partial());

        let r = run(RunStatus::TimedOut);
        assert!(r.retryable() && r.partial());
        let e = r.into_result().unwrap_err();
        assert_eq!(e.code(), "TIMEOUT_ERROR");

        let r = run(RunStatus::Cancelled);
        assert!(!r.retryable() && r.partial());
        assert_eq!(r.into_result().unwrap_err().code(), "CANCELLED");

        let r = run(RunStatus::Stopped(
            Error::service("X", "m").with_retryable(true),
        ));
        assert!(r.retryable() && r.partial());
        assert_eq!(r.into_result().unwrap_err().code(), "X");
        let r = run(RunStatus::Stopped(Error::service("X", "m")));
        assert!(!r.retryable() && r.partial());
    }

    #[test]
    fn arena_stats_come_from_the_state_stats() {
        let stats = Stats {
            views: 2,
            checkpoints: 3,
            live_max: 40,
            peak_arena: 50,
            ..Stats::default()
        };
        let r = Run::new(program(), RunStatus::Completed, Vec::new(), stats, None);
        let a = r.arena_stats();
        assert_eq!(
            (a.views, a.checkpoints, a.live_max, a.peak_arena),
            (2, 3, 40, 50)
        );
    }
}
