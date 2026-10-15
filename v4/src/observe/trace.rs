//! The trace (R17, I105): a run's initial context, then one step per
//! recorded call, skipped step and engine write, each with its changes, so a
//! replay from `initial` rebuilds the context after every step.

use crate::base::{Change, ErrorRecord, TaskIx, WorkflowIx};
use crate::expr::{Path, path};
use crate::run::Context;
use datavalue::OwnedDataValue;
use serde::{Serialize, Serializer};
use std::sync::Arc;
use std::time::Duration;

/// What a run's trace records. A run is traced when
/// [`RunOptions::trace`](crate::RunOptions::trace()) is given these.
///
/// ```
/// # extern crate dataflow_rs_v4 as dataflow_rs;
/// use dataflow_rs::observe::TraceOptions;
///
/// let opts = TraceOptions::default()
///     .timings(true)
///     .redact(vec!["data.card".parse().unwrap()])
///     .max_bytes(1 << 20);
/// assert!(opts.changes && opts.timings);
/// ```
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct TraceOptions {
    /// Time each call, into its step's `elapsed`. `false` by default: no
    /// clock is read for a trace without it.
    pub timings: bool,
    /// Record each step's changes. `true` by default. A trace records its
    /// own changes whatever `RunOptions::capture_changes` says, which governs
    /// only the audit trail (R45). With `false` the steps carry statuses and
    /// timings, for profiling, and cannot be replayed.
    pub changes: bool,
    /// Prefixes whose values read `null` in `initial` and in every change,
    /// applied as each is captured; the message keeps the real values. A
    /// change at or below one is recorded as a write of `null` at it, a
    /// removal's included, so no key below it reaches the trace and a replay
    /// reads `null` there from the first write on. A path that resolves
    /// nothing in `initial` creates nothing there. Empty by default.
    pub redact: Vec<Path>,
    /// A ceiling on what the trace holds, on v3's scale: one machine word
    /// per node, plus each string's and each key's length, counting
    /// `initial` and every change. The step that would cross it, and every
    /// later one, keeps everything but its changes, and
    /// [`Trace::truncated`] is set. `None`, the default, is unbounded.
    pub max_bytes: Option<usize>,
}

impl Default for TraceOptions {
    fn default() -> Self {
        Self {
            timings: false,
            changes: true,
            redact: Vec::new(),
            max_bytes: None,
        }
    }
}

impl TraceOptions {
    /// Times each call, or not.
    #[must_use]
    pub fn timings(mut self, on: bool) -> Self {
        self.timings = on;
        self
    }

    /// Records each step's changes, or not.
    #[must_use]
    pub fn changes(mut self, on: bool) -> Self {
        self.changes = on;
        self
    }

    /// Redacts `paths`, as the `redact` field says.
    #[must_use]
    pub fn redact(mut self, paths: Vec<Path>) -> Self {
        self.redact = paths;
        self
    }

    /// Bounds the trace at `max` bytes, as the `max_bytes` field says.
    #[must_use]
    pub fn max_bytes(mut self, max: usize) -> Self {
        self.max_bytes = Some(max);
        self
    }
}

/// A run's trace: [`initial`](Self::initial), then each step's changes.
///
/// Serialized as `{initial, steps, truncated}`.
#[derive(Clone, Debug, Serialize)]
#[non_exhaustive]
pub struct Trace {
    /// The context after the run's stamps and the error context's reset,
    /// before any workflow, redacted.
    pub initial: Context,
    /// Every step, in the order the run made them.
    pub steps: Vec<TraceStep>,
    /// The byte budget was crossed: from the step that crossed it on, every
    /// step's changes are empty, so the contexts replayed from there on are
    /// not whole.
    pub truncated: bool,
}

impl Trace {
    /// The context after each step, replaying the steps' changes in order
    /// from `initial`: a removal removes its path; a change at a root merges
    /// its object's keys into the root, unless it says
    /// [`replaced`](Change::replaced); anything else is set at its path by
    /// [`Path::set`]'s rules, so a hop that cannot continue writes nothing
    /// and a segment that is an index on an array names its element. A
    /// replay elsewhere, such as the UI's, follows the same rules. Each
    /// context yielded is a whole copy.
    pub fn contexts(&self) -> impl Iterator<Item = Context> + '_ {
        let mut root = self.initial.as_value().clone();
        self.steps.iter().map(move |step| {
            for change in &step.changes {
                apply(&mut root, change);
            }
            Context::from_root(root.clone())
        })
    }
}

/// One change applied to `root`, the object holding the three roots, by
/// [`Trace::contexts`]'s rule.
fn apply(root: &mut OwnedDataValue, change: &Change) {
    let Ok(at) = change.path.parse::<Path>() else {
        return;
    };
    if change.removed {
        path::remove(root, at.segs());
        return;
    }
    if at.is_root()
        && !change.replaced
        && let OwnedDataValue::Object(new) = &change.new_value
        && let Some(OwnedDataValue::Object(pairs)) = path::get_mut(root, at.segs())
    {
        // `map`'s merge, as the splice does it: a key there is replaced
        // where it is, a new one appended.
        for (key, value) in new {
            match pairs.iter_mut().find(|(k, _)| k == key) {
                Some((_, slot)) => *slot = value.clone(),
                None => pairs.push((key.clone(), value.clone())),
            }
        }
        return;
    }
    at.assign(root, change.new_value.clone());
}

/// What a step was.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum StepResult {
    /// A call ran, or the engine wrote, and the run went on: a failure its
    /// task's `on_failure: continue` or `halt` stepped past included, with
    /// its status and error.
    Executed,
    /// A condition or a rollout refused the task, the group's tasks or the
    /// workflow.
    Skipped,
    /// The failure went to the workflow's policy, or stopped the run: the
    /// step v3 dropped (break 5). A workflow's own failure is one too.
    Failed,
}

/// One step of a [`Trace`].
///
/// Serialized with its keys in field order; `elapsed` as whole microseconds
/// under `elapsed_us`, and it, `error`, `loop_counter` and `element_index`
/// only when present.
#[derive(Clone, Debug, Serialize)]
#[non_exhaustive]
pub struct TraceStep {
    /// The workflow's position in the program.
    pub workflow: WorkflowIx,
    /// The workflow's id.
    pub workflow_id: Arc<str>,
    /// The task's position in the program; `None` for the workflow's own
    /// step: its skip, its failure, or the engine's writes as a sweep is
    /// entered.
    pub task: Option<TaskIx>,
    /// The task's id; `None` with `task`.
    pub task_id: Option<Arc<str>>,
    /// What the step was.
    pub result: StepResult,
    /// The call's status; `None` for a skip, a `Skip` outcome and the
    /// engine's step.
    pub status: Option<u16>,
    /// The step's changes in order: the engine's writes made for it first (a
    /// fan-out's `into` pre-fill), then the call's own, then the progress
    /// write, then the error context's. Empty past the byte budget and when
    /// the trace records none.
    pub changes: Vec<Change>,
    /// The recorder's error record for the call, or the workflow's own
    /// failure, or the error its condition failed with.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorRecord>,
    /// How long the call ran, when the trace asked for timings.
    #[serde(
        rename = "elapsed_us",
        serialize_with = "micros",
        skip_serializing_if = "Option::is_none"
    )]
    pub elapsed: Option<Duration>,
    /// The loop counter of the sweep: the call's, or the one the engine's
    /// step wrote.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loop_counter: Option<i64>,
    /// The fan-out element the call ran for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub element_index: Option<usize>,
}

impl TraceStep {
    /// A step of workflow `workflow`, `id`, with nothing set: the workflow's
    /// own until its task is set.
    pub(crate) fn new(workflow: WorkflowIx, id: &Arc<str>, result: StepResult) -> Self {
        Self {
            workflow,
            workflow_id: Arc::clone(id),
            task: None,
            task_id: None,
            result,
            status: None,
            changes: Vec::new(),
            error: None,
            elapsed: None,
            loop_counter: None,
            element_index: None,
        }
    }
}

/// `elapsed` as whole microseconds.
fn micros<S: Serializer>(elapsed: &Option<Duration>, s: S) -> Result<S::Ok, S::Error> {
    match elapsed {
        Some(d) => s.serialize_u64(u64::try_from(d.as_micros()).unwrap_or(u64::MAX)),
        None => s.serialize_none(),
    }
}
