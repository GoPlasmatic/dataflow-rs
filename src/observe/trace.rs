//! The trace: a run's initial context, then one step per
//! recorded call, skipped step and engine write, each with its changes, so a
//! replay from `initial` rebuilds the context after every step.

use crate::base::{Change, Context, Error, ErrorKind, ErrorRecord, TaskIx, WorkflowIx};
use crate::expr::{Path, path};
use datavalue::OwnedDataValue;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::sync::Arc;
use std::time::Duration;

/// What a run's trace records. A run is traced when
/// [`RunOptions::trace`](crate::RunOptions::trace()) is given these.
///
/// ```
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
    /// only the audit trail, so turning that off cannot empty a debugger's
    /// replay. With `false` the steps carry statuses and timings, for
    /// profiling, and cannot be replayed.
    pub changes: bool,
    /// Prefixes whose values read `null` in `initial` and in every change,
    /// applied as each is captured; the message keeps the real values. A
    /// change at or below one is recorded as a write of `null` at it, a
    /// removal's included, so no key below it reaches the trace and a replay
    /// reads `null` there from the first write on. A path that resolves
    /// nothing in `initial` creates nothing there. Empty by default.
    ///
    /// It masks values at those paths and nothing else: a step's `errors`,
    /// like a value derived from a redacted one and written elsewhere, are
    /// carried as the message keeps them, so a record whose message quotes
    /// a redacted value quotes it in the trace too.
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
/// Written as `{initial, steps, truncated}`, and read back from what it
/// writes: a trace read back writes the text it was read from, and replays
/// to the original's contexts.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
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
    /// [`replaced`](Change::replaced); anything else is set at its path as
    /// [`Path::set`] sets it, except that a root takes any value whole (a
    /// redacted root's `null`, which `Path::set` refuses, included): a hop
    /// that cannot continue writes nothing, and a segment that is an index
    /// on an array names its element. A replay elsewhere, such as the UI's,
    /// follows the same rules. Each context yielded is a whole copy.
    pub fn contexts(&self) -> impl Iterator<Item = Context> + '_ {
        let mut root = self.initial.as_value().clone();
        self.steps.iter().map(move |step| {
            for change in &step.changes {
                apply(&mut root, change);
            }
            Context::from_root(root.clone())
        })
    }

    /// The context after each mapping of the `map` step at `step`, replaying
    /// `initial` through the steps before it and then the step's changes up
    /// to each of its [`mapping_ends`](TraceStep::mapping_ends), by
    /// [`contexts`](Self::contexts)'s rules: one context per mapping, in
    /// order, a mapping that wrote nothing repeating the one before it. The
    /// last is the context after the last mapping and before the step's own
    /// trailing changes (the progress write, and the error context's), so it
    /// differs from the context [`contexts`](Self::contexts) yields for the
    /// step by those alone. `None` when `step` is past the last step, when
    /// the step has no `mapping_ends`, or when an end exceeds the step's
    /// `changes` or precedes the end before it, as a trace edited or read
    /// from another build may have.
    pub fn mapping_contexts(&self, step: usize) -> Option<Vec<Context>> {
        let at = self.steps.get(step)?;
        if at.mapping_ends.is_empty() || !valid_mapping_ends(at) {
            return None;
        }
        let mut root = self.initial.as_value().clone();
        for earlier in &self.steps[..step] {
            for change in &earlier.changes {
                apply(&mut root, change);
            }
        }
        let mut out = Vec::with_capacity(at.mapping_ends.len());
        let mut from = 0usize;
        for &end in &at.mapping_ends {
            let end = end as usize;
            for change in &at.changes[from..end] {
                apply(&mut root, change);
            }
            out.push(Context::from_root(root.clone()));
            from = end;
        }
        Some(out)
    }

    /// Checks what a replay needs and a trace read from elsewhere may lack:
    /// every step's [`mapping_ends`](TraceStep::mapping_ends) fits its
    /// changes, every change's path parses, and no change leaves a root
    /// anything but an object, or the `null` a redaction writes.
    ///
    /// [`contexts`](Self::contexts) and [`mapping_contexts`](Self::mapping_contexts)
    /// trust their trace: a change that does not parse is skipped and a
    /// root written whole stands, as the splice and `Ctx::set` allow. A host
    /// that reads a trace from a caller (the wasm bindings do) calls this
    /// first and reports a failure as `INVALID_TRACE`.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Config`] naming the step and what is wrong.
    pub fn validate(&self) -> crate::base::Result<()> {
        match self.initial.as_value() {
            OwnedDataValue::Object(roots) => {
                for (root, value) in roots {
                    check_root(root, value)?;
                }
            }
            _ => {
                return Err(Error::new(
                    ErrorKind::Config,
                    "a trace's initial context is not an object of roots",
                ));
            }
        }
        for (i, step) in self.steps.iter().enumerate() {
            if !valid_mapping_ends(step) {
                return Err(Error::new(
                    ErrorKind::Config,
                    format!(
                        "step {i}'s mapping_ends {:?} do not fit its {} change(s)",
                        step.mapping_ends,
                        step.changes.len()
                    ),
                ));
            }
            for change in &step.changes {
                let at = change.path.parse::<Path>().map_err(|_| {
                    Error::new(
                        ErrorKind::Config,
                        format!(
                            "step {i}'s change path {:?} is not a context path",
                            change.path
                        ),
                    )
                })?;
                if !at.is_root() {
                    continue;
                }
                if change.removed {
                    return Err(Error::new(
                        ErrorKind::Config,
                        format!(
                            "step {i} removes the root `{}`, which a context always keeps",
                            change.path
                        ),
                    ));
                }
                check_root(&change.path, &change.new_value)?;
            }
        }
        Ok(())
    }
}

/// Whether every end in `step.mapping_ends` is non-decreasing and fits
/// within `step.changes`.
pub(super) fn valid_mapping_ends(step: &TraceStep) -> bool {
    let mut before = 0u32;
    for &end in &step.mapping_ends {
        if end < before || usize::try_from(end).map_or(true, |e| e > step.changes.len()) {
            return false;
        }
        before = end;
    }
    true
}

/// Checks a context root's value: an object, or the `null` a redaction
/// writes. `Path::set` refuses anything else at a root, so a trace holding
/// one did not come from a run.
fn check_root(name: &str, value: &OwnedDataValue) -> crate::base::Result<()> {
    if matches!(value, OwnedDataValue::Object(_) | OwnedDataValue::Null) {
        return Ok(());
    }
    Err(Error::new(
        ErrorKind::Config,
        format!("`{name}` is a context root and holds neither an object nor null"),
    ))
}

/// One change applied to `root`, the object holding the three roots, by
/// [`Trace::contexts`]'s rule.
pub(super) fn apply(root: &mut OwnedDataValue, change: &Change) {
    let Ok(at) = change.path.parse::<Path>() else {
        return;
    };
    if change.removed {
        path::remove(root, at.segs());
        return;
    }
    if at.is_root() && !change.replaced {
        // `map`'s merge, as the splice does it: a key there is replaced
        // where it is, a new one appended; a non-object replaces the slot.
        path::merge_root(root, at.root().as_str(), change.new_value.clone());
        return;
    }
    at.assign(root, change.new_value.clone());
}

/// What a step was. Serialized, and read, as its lowercase name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum StepResult {
    /// A call ran, or the engine wrote, and the run went on: a failure its
    /// task's `on_failure: continue` or `halt` stepped past included, with
    /// its status and its records.
    Executed,
    /// A condition or a rollout refused the task, the group's tasks or the
    /// workflow.
    Skipped,
    /// The failure went to the workflow's policy, or stopped the run: the
    /// step v3 dropped. A workflow's own failure is one too.
    Failed,
}

/// One step of a [`Trace`].
///
/// Written with its keys in this order. `mapping_ends` and `errors` are
/// absent when empty, and `elapsed_us` (in whole microseconds),
/// `loop_counter` and `element_index` when there is none.
/// Read back from what it writes: `elapsed_us` as the whole microseconds
/// written, and an absent `mapping_ends` or `errors` as empty.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[non_exhaustive]
pub struct TraceStep {
    /// The workflow's position in the program.
    pub workflow: WorkflowIx,
    /// The workflow's id.
    pub workflow_id: Arc<str>,
    /// The task's position in the program; `null` for the workflow's own
    /// step: its skip, its failure, or the engine's writes as a sweep is
    /// entered.
    pub task: Option<TaskIx>,
    /// The task's id; `null` when `task` is.
    pub task_id: Option<Arc<str>>,
    /// What the step was.
    pub result: StepResult,
    /// The call's status; `null` for a skip, a call whose outcome was a
    /// skip, and the engine's step.
    pub status: Option<u16>,
    /// The step's changes in order: the engine's writes made for it first (a
    /// fan-out's `into` pre-fill), then the call's own, then the progress
    /// write, then the error context's. Empty past the byte budget and when
    /// the trace records none.
    pub changes: Vec<Change>,
    /// Where each of a `map`'s mappings ends in `changes`: one entry per
    /// mapping, in order, the index just past its last change, so mapping
    /// `i`'s changes run from the end before it (0 for the first) up to its
    /// own. A mapping that wrote nothing repeats the end before it.
    /// Replaying the changes before mapping `i`'s end gives the context
    /// after mapping `i`. Empty for every other step, and whenever `changes`
    /// is.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mapping_ends: Vec<u32>,
    /// Every record the step added to the message, in the message's order:
    /// the call's own, then the engine's; for a workflow's own failure, its
    /// record. A workflow whose condition failed to evaluate adds none to
    /// the message, and its `skipped` step holds that failure's record.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<ErrorRecord>,
    /// How long the call ran, written in whole microseconds, when the trace
    /// asked for timings.
    #[serde(
        rename = "elapsed_us",
        default,
        serialize_with = "micros",
        deserialize_with = "from_micros",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(feature = "ts", ts(type = "number", optional))]
    pub elapsed: Option<Duration>,
    /// The loop counter of the sweep: the call's, or the one the engine's
    /// step wrote.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub loop_counter: Option<i64>,
    /// The fan-out element the call ran for.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
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
            mapping_ends: Vec::new(),
            errors: Vec::new(),
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

/// `elapsed` from whole microseconds, as [`micros`] writes it.
fn from_micros<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Duration>, D::Error> {
    Ok(Option::<u64>::deserialize(d)?.map(Duration::from_micros))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn value(v: serde_json::Value) -> OwnedDataValue {
        OwnedDataValue::from(&v)
    }

    fn write(path: &str, old: serde_json::Value, new: serde_json::Value) -> Change {
        Change::write(path, value(old), value(new))
    }

    /// `initial` with an empty `data`, one earlier step writing `data.z`,
    /// then a `map` step of four mappings: a plain write, a skipped `null`,
    /// an `append` creating its array, and a root merge of two keys, ending
    /// at `[1, 1, 3, 4]`, followed by the recorder's progress write, which no
    /// mapping end covers.
    fn trace() -> Trace {
        let id: Arc<str> = Arc::from("w");
        let mut earlier = TraceStep::new(WorkflowIx::new(0), &id, StepResult::Executed);
        earlier.changes = vec![write("data.z", json!(null), json!(0))];
        let mut map = TraceStep::new(WorkflowIx::new(0), &id, StepResult::Executed);
        map.changes = vec![
            write("data.a", json!(null), json!(1)),
            write("data.list.0", json!(null), json!("x")),
            write("data.list.1", json!(null), json!("y")),
            write("data", json!({}), json!({"b": 2, "c": 3})),
            write(
                "metadata.progress",
                json!(null),
                json!({"workflow_id": "w", "task_id": "m", "status_code": 200}),
            ),
        ];
        map.mapping_ends = vec![1, 1, 3, 4];
        Trace {
            initial: Context::from_root(value(json!({
                "data": {}, "metadata": {}, "temp_data": {}
            }))),
            steps: vec![earlier, map],
            truncated: false,
        }
    }

    #[test]
    fn a_map_step_replays_mapping_by_mapping_from_the_steps_before_it() {
        let trace = trace();
        let contexts = trace.mapping_contexts(1).expect("a map step");
        let data: Vec<serde_json::Value> = contexts
            .iter()
            .map(|c| serde_json::to_value(c.data()).unwrap())
            .collect();
        assert_eq!(
            data,
            vec![
                json!({"z": 0, "a": 1}),
                json!({"z": 0, "a": 1}),
                json!({"z": 0, "a": 1, "list": ["x", "y"]}),
                json!({"z": 0, "a": 1, "list": ["x", "y"], "b": 2, "c": 3}),
            ]
        );
        // The last mapping context is the step's context without the
        // trailing progress write, which `contexts` includes.
        let after = trace.contexts().nth(1).unwrap();
        let last = contexts.last().unwrap();
        assert_eq!(last.data(), after.data());
        assert!(last.metadata().as_object().is_some_and(|m| m.is_empty()));
        assert_eq!(
            serde_json::to_value(after.metadata()).unwrap()["progress"]["status_code"],
            json!(200)
        );
    }

    #[test]
    fn a_step_without_mapping_ends_and_one_out_of_range_give_none() {
        let trace = trace();
        assert!(trace.mapping_contexts(0).is_none());
        assert!(trace.mapping_contexts(2).is_none());
    }

    #[test]
    fn a_malformed_mapping_end_gives_none() {
        let mut past = trace();
        past.steps[1].mapping_ends = vec![1, 6];
        assert!(past.mapping_contexts(1).is_none());
        let mut backwards = trace();
        backwards.steps[1].mapping_ends = vec![3, 1];
        assert!(backwards.mapping_contexts(1).is_none());
    }

    #[test]
    fn validate_refuses_what_a_replay_cannot_stand() {
        assert!(trace().validate().is_ok());

        // A root takes only an object in a run; a scalar or an array would
        // make the replayed context one no run produced.
        let mut scalar = trace();
        scalar.steps[1]
            .changes
            .push(Change::write("metadata", value(json!({})), value(json!(5))));
        assert!(scalar.validate().is_err());

        // A root cannot be removed, and `Path::set` says so.
        let mut removed = trace();
        removed.steps[0].changes = vec![Change::removal("data", value(json!({})))];
        assert!(removed.validate().is_err());

        // An unparsable path is refused, not silently skipped.
        let mut bad_path = trace();
        bad_path.steps[0].changes = vec![Change::write(
            "outside",
            value(json!(null)),
            value(json!(1)),
        )];
        assert!(bad_path.validate().is_err());

        // The initial context's roots are checked too.
        let mut bad_initial = trace();
        bad_initial.initial = Context::from_root(value(json!({
            "data": 5, "metadata": {}, "temp_data": {}
        })));
        assert!(bad_initial.validate().is_err());

        // A redacted root's `null` is legitimate.
        let mut redacted = trace();
        redacted.steps[0].changes =
            vec![Change::write("data", value(json!({})), value(json!(null)))];
        assert!(redacted.validate().is_ok());
    }
}
