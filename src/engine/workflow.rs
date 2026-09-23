use crate::engine::error::{DataflowError, Result};
use crate::engine::functions::{ConnectorName, FunctionConfig};
use crate::engine::task::Task;
use chrono::{DateTime, Utc};
use datalogic_rs::Logic;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::path::Path;
use std::sync::Arc;

pub use crate::engine::rollout::{Rollout, RolloutError};

/// Engine-managed `for` loop over a workflow's task list.
///
/// A workflow carrying a `loop` runs its task list repeatedly — one *sweep*
/// per iteration — rather than once. Per sweep the engine writes the counter
/// into `temp_data` (when [`counter`](Self::counter) names it), checks
/// `counter < max`, then re-evaluates the workflow `condition`; the sweep runs
/// only if both hold. Afterwards the counter advances by `increment`.
///
/// The bound is half-open, matching [`Rollout`]: `init: 0, max: n` yields
/// counter values `0..n-1` — exactly array indices.
///
/// Reaching `max` is normal completion, never an error: `max` is always
/// author-supplied, so hitting it is the stated bound rather than a runaway.
/// To stop mid-body, use a `filter` task with `on_reject: halt` — that breaks
/// the whole loop, not just the current sweep.
///
/// # Iterating an array
///
/// [`setup`](Self::setup), [`over`](Self::over), [`item`](Self::item) (`as`)
/// and [`scratch`](Self::scratch) turn the counter into a batch loop: setup
/// runs once, `over` is evaluated once after it, and each iteration sees
/// `over[counter]` in `temp_data.<as>` and a fresh `{}` in
/// `temp_data.<scratch>`. A loop carrying none of them behaves exactly as a
/// plain counter loop always has.
///
/// # Example
///
/// ```json
/// {
///     "id": "per_item",
///     "condition": {"<": [{"var": "temp_data.i"}, {"var": "temp_data.n"}]},
///     "loop": { "counter": "i", "max": 10000 },
///     "tasks": [ ... ]
/// }
/// ```
///
/// `#[non_exhaustive]`: construct through [`LoopConfig::bounded`] and assign
/// the public fields you need. The compiled fields are engine internals a
/// struct literal would otherwise have to name. Not `PartialEq`: `setup` holds
/// [`Task`]s, which are not comparable, and neither is the compiled `over`.
#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
pub struct LoopConfig {
    /// `temp_data` field the engine maintains as the induction variable —
    /// `"i"` means `temp_data.i`, and dot-paths nest (`"cursor.index"` →
    /// `temp_data.cursor.index`).
    ///
    /// `None` still bounds the loop by `max`; the count is simply not exposed
    /// to conditions or tasks. The engine tracks it either way, so the audit
    /// trail carries it regardless.
    ///
    /// The engine owns this field: it is rewritten before every sweep, so a
    /// body task writing the same path is overwritten at the next increment.
    #[serde(default)]
    pub counter: Option<String>,

    /// First counter value. Defaults to `0`.
    #[serde(default)]
    pub init: i64,

    /// Added to the counter after each sweep. Defaults to `1`; must be `>= 1`,
    /// so the counter strictly increases and the loop cannot stall.
    #[serde(default = "default_increment")]
    pub increment: i64,

    /// Required upper bound — sweeps run while `counter < max`. There is no
    /// default: an unbounded loop is never what the author meant, and the
    /// bound is what makes termination structural rather than a property of
    /// the condition being written correctly.
    pub max: i64,

    /// Steps run **once**, before the first iteration, in the normal step
    /// grammar: groups are allowed, and `terminal` ends the workflow before
    /// any iteration. Their audit entries carry no `loop_counter`, because
    /// setup is not a sweep. Shares the step id namespace with the workflow's
    /// `tasks`.
    ///
    /// The workflow `condition` gates setup: a false result skips setup and
    /// the loop alike.
    #[serde(default, deserialize_with = "crate::engine::steps::flatten")]
    pub setup: Vec<Task>,

    /// JSONLogic evaluated **once, after setup**. It must yield an array;
    /// anything else, `null` included, is a workflow error naming
    /// `loop.over`. An empty array runs zero iterations.
    ///
    /// The counter indexes the array: iteration `k` holds `over[k]` in
    /// `temp_data.<as>`, and the loop stops at `max` or at the array's end,
    /// whichever comes first. `init` is therefore a starting offset and
    /// `increment` a stride, and `init` must be `>= 0`.
    ///
    /// An explicit `"over": null` is kept as `Some(Value::Null)` rather than
    /// read as absence, so it is refused like any other literal that can never
    /// be an array instead of silently turning the loop into a counter loop.
    #[serde(default, deserialize_with = "present_value")]
    pub over: Option<Value>,

    /// `temp_data` field holding the current element of [`Self::over`]. The
    /// JSON key is `as`. Optional: without it the element is not exposed, but
    /// the loop is still bounded by the array's length. Requires `over`.
    #[serde(default, rename = "as")]
    pub item: Option<String>,

    /// `temp_data` field reset to `{}` at the start of **every** iteration,
    /// before the condition, so per-item state cannot leak from one element
    /// to the next. Left holding the last iteration's state after the loop,
    /// like the counter. Independent of `over`.
    #[serde(default)]
    pub scratch: Option<String>,

    /// Engine-internal: `["temp_data", ..counter segments]`, populated by
    /// `LogicCompiler`. Empty when `counter` is `None`. Not part of the stable
    /// API.
    #[doc(hidden)]
    #[serde(skip)]
    pub counter_parts: Arc<[Arc<str>]>,

    /// Engine-internal: pre-split `temp_data.{as}`. Not part of the stable API.
    #[doc(hidden)]
    #[serde(skip)]
    pub item_parts: Arc<[Arc<str>]>,

    /// Engine-internal: pre-split `temp_data.{scratch}`. Not part of the
    /// stable API.
    #[doc(hidden)]
    #[serde(skip)]
    pub scratch_parts: Arc<[Arc<str>]>,

    /// Engine-internal: compiled `over`, populated by `LogicCompiler`. Not
    /// part of the stable API.
    #[doc(hidden)]
    #[serde(skip)]
    pub compiled_over: Option<Arc<Logic>>,
}

fn default_increment() -> i64 {
    1
}

/// `deserialize_with` target that keeps an explicit JSON `null` as
/// `Some(Value::Null)`. Absence still gives `None`, through `#[serde(default)]`.
fn present_value<'de, D>(deserializer: D) -> std::result::Result<Option<Value>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Value::deserialize(deserializer).map(Some)
}

/// The pre-split form of an unnamed slot: `set_nested_value_parts` treats an
/// empty slice as a no-op write.
fn no_parts() -> Arc<[Arc<str>]> {
    Arc::from([] as [Arc<str>; 0])
}

/// Whether `path` is a well-formed `temp_data` field path: non-empty, with no
/// empty segment. The one rule `counter`, `as` and `scratch` share.
pub(crate) fn is_slot_path(path: &str) -> bool {
    !path.is_empty() && !path.split('.').any(str::is_empty)
}

/// Whether two slot paths name the same key or one lies inside the other:
/// `it` and `it.item` overlap, `it` and `item` do not. A reset of the outer
/// slot would wipe the inner one, so the engine refuses to own both.
pub(crate) fn slots_overlap(a: &str, b: &str) -> bool {
    fn within(outer: &str, inner: &str) -> bool {
        inner
            .strip_prefix(outer)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('.'))
    }
    within(a, b) || within(b, a)
}

/// Why a literal `over` could never yield an array, if it is one.
///
/// An object is an expression and an array is itself; any other JSON literal
/// is a constant that is not an array, so the loop would fail on every
/// message. Shared by [`LoopConfig`]'s validation and the authoring check.
pub(crate) fn over_literal_problem(over: &Value) -> Option<String> {
    let kind = match over {
        Value::Object(_) | Value::Array(_) => return None,
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
    };
    Some(format!(
        "loop over must be a JSONLogic expression or an array literal — \
         {kind} can never evaluate to an array"
    ))
}

impl LoopConfig {
    /// A loop bounded at `max`, every other field at its default: counter
    /// unnamed, `init: 0`, `increment: 1`, no setup, no `over`.
    ///
    /// ```
    /// use dataflow_rs::engine::workflow::LoopConfig;
    ///
    /// let mut config = LoopConfig::bounded(10);
    /// config.counter = Some("i".to_string());
    /// assert_eq!((config.init, config.increment, config.max), (0, 1, 10));
    /// ```
    pub fn bounded(max: i64) -> Self {
        Self {
            counter: None,
            init: 0,
            increment: default_increment(),
            max,
            setup: Vec::new(),
            over: None,
            item: None,
            scratch: None,
            counter_parts: no_parts(),
            item_parts: no_parts(),
            scratch_parts: no_parts(),
            compiled_over: None,
        }
    }

    /// Structural validation, run from [`Workflow::validate`] at
    /// `Engine::build()` time. Every rule here rejects a config that could
    /// only fail — or spin — at runtime.
    fn validate(&self, workflow_id: &str) -> Result<()> {
        if self.increment < 1 {
            return Err(DataflowError::Workflow(format!(
                "Workflow {workflow_id}: loop increment must be >= 1, got {} \
                 (a non-advancing counter would never reach max)",
                self.increment
            )));
        }
        if self.max <= self.init {
            return Err(DataflowError::Workflow(format!(
                "Workflow {workflow_id}: loop max ({}) must be greater than init ({}) — \
                 the bound is half-open, so this could never run a sweep",
                self.max, self.init
            )));
        }
        for (field, slot) in [
            ("counter", &self.counter),
            ("as", &self.item),
            ("scratch", &self.scratch),
        ] {
            if let Some(path) = slot
                && !is_slot_path(path)
            {
                return Err(DataflowError::Workflow(format!(
                    "Workflow {workflow_id}: loop {field} must be a non-empty \
                     temp_data field path, got {path:?}"
                )));
            }
        }
        if self.item.is_some() && self.over.is_none() {
            return Err(DataflowError::Workflow(format!(
                "Workflow {workflow_id}: loop `as` names a slot for the current element, \
                 but there is no `over` to take elements from"
            )));
        }
        if let Some(over) = &self.over {
            if let Some(problem) = over_literal_problem(over) {
                return Err(DataflowError::Workflow(format!(
                    "Workflow {workflow_id}: {problem}"
                )));
            }
            if self.init < 0 {
                return Err(DataflowError::Workflow(format!(
                    "Workflow {workflow_id}: loop init must be >= 0 when over is set — \
                     the counter indexes the array, got {}",
                    self.init
                )));
            }
        }
        // Each engine-owned slot is its own path: a per-iteration reset of
        // `scratch` that also wiped `as`, or a counter written over the
        // element, would be exactly the silent leak this exists to end.
        let slots = [
            ("counter", self.counter.as_deref()),
            ("as", self.item.as_deref()),
            ("scratch", self.scratch.as_deref()),
        ];
        for (i, (name_a, a)) in slots.iter().enumerate() {
            for (name_b, b) in &slots[i + 1..] {
                if let (Some(a), Some(b)) = (a, b)
                    && slots_overlap(a, b)
                {
                    return Err(DataflowError::Workflow(format!(
                        "Workflow {workflow_id}: loop {name_a} ({a:?}) and {name_b} ({b:?}) \
                         overlap — each engine-owned slot must be its own temp_data path"
                    )));
                }
            }
        }
        Ok(())
    }

    /// Pre-split the three `temp_data.{…}` slots into the path parts the
    /// executor writes through, so an iteration never re-splits a path.
    /// Populated by `LogicCompiler`; the executor falls back to splitting on
    /// the fly for workflows constructed directly rather than through
    /// `Engine::builder`. An unnamed slot yields an empty slice, which
    /// `set_nested_value_parts` treats as a no-op.
    #[doc(hidden)]
    pub fn precompute_paths(&mut self) {
        fn parts(name: Option<&str>) -> Arc<[Arc<str>]> {
            match name {
                Some(name) => crate::engine::utils::compute_path_parts("temp_data", name),
                None => no_parts(),
            }
        }
        self.counter_parts = parts(self.counter.as_deref());
        self.item_parts = parts(self.item.as_deref());
        self.scratch_parts = parts(self.scratch.as_deref());
    }
}

/// Workflow lifecycle status
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowStatus {
    #[default]
    Active,
    Paused,
    Archived,
}

/// Workflow represents a collection of tasks that execute sequentially (also known as a Rule in rules-engine terminology).
///
/// Conditions are evaluated against the full message context, including `data`, `metadata`, and `temp_data` fields.
/// A collection of tasks with a condition, executed against a message.
///
/// `#[non_exhaustive]`: construct through [`Workflow::new`], [`Workflow::rule`]
/// or [`Workflow::from_json`] and assign the public fields you need. Field
/// reads and writes are unaffected.
///
/// Same reason as [`Task`]: several fields are engine internals
/// marked *not part of the stable API*, and struct-literal construction forced
/// callers to name them.
#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
pub struct Workflow {
    pub id: String,
    /// Engine-internal: `Arc<str>` mirror of `id`, populated by
    /// `LogicCompiler::compile_workflows`. Cloning is a refcount bump; per-message
    /// `AuditTrail` entries reuse it instead of allocating from `&id` each time.
    /// Not part of the stable API.
    #[doc(hidden)]
    #[serde(skip)]
    pub id_arc: Arc<str>,
    pub name: String,
    #[serde(default)]
    pub priority: u32,
    pub description: Option<String>,
    #[serde(default = "crate::engine::utils::default_condition")]
    pub condition: Value,
    /// Engine-internal: pre-compiled JSONLogic for `condition`, populated by
    /// `LogicCompiler`. `None` is treated as "no condition / always run" by
    /// the executor. Not part of the stable API.
    #[doc(hidden)]
    #[serde(skip)]
    pub compiled_condition: Option<Arc<Logic>>,
    /// Engine-internal: `true` when every task is a synchronous built-in
    /// (`is_sync_builtin`), so the whole workflow can run inside a shared
    /// `with_arena` scope with no `.await`. Populated by `LogicCompiler`; the
    /// `false` default means an uncompiled workflow conservatively takes the
    /// async path. Not part of the stable API.
    #[doc(hidden)]
    #[serde(skip, default)]
    pub fully_sync: bool,
    /// The workflow's steps, flattened.
    ///
    /// The JSON `tasks` array holds *steps*: an element carrying a `tasks` key
    /// is a [`TaskGroup`](crate::TaskGroup), anything else is a [`Task`]. The
    /// parser flattens the
    /// tree in document order and records each group's span on the task that
    /// opens it ([`Task::group_starts`]), so this stays a flat list and the
    /// executor keeps walking `&[Task]` slices.
    #[serde(deserialize_with = "crate::engine::steps::flatten")]
    pub tasks: Vec<Task>,
    #[serde(default)]
    pub continue_on_error: bool,
    /// Channel for routing (default: "default")
    #[serde(default = "default_channel")]
    pub channel: String,
    /// Version number for rule versioning (default: 1)
    #[serde(default = "default_version")]
    pub version: u32,
    /// Workflow status — Active, Paused, or Archived (default: Active)
    #[serde(default)]
    pub status: WorkflowStatus,
    /// Traffic split for this workflow. `None` (the default) means the workflow
    /// is not part of a split and runs for every message.
    ///
    /// A workflow with a rollout is skipped when the message's
    /// [`crate::Message::routing_bucket`] falls outside the range. A message with
    /// **no** bucket is admitted — see [`Rollout`].
    #[serde(default)]
    pub rollout: Option<Rollout>,
    /// Engine-managed loop over this workflow's task list. `None` (the
    /// default) runs the task list exactly once — the historical behaviour, on
    /// a code path that carries no loop overhead.
    ///
    /// See [`LoopConfig`] for the per-sweep contract.
    #[serde(default, rename = "loop")]
    pub loop_config: Option<LoopConfig>,
    /// Tags for categorization and filtering
    #[serde(default)]
    pub tags: Vec<String>,
    /// Creation timestamp
    #[serde(default)]
    pub created_at: Option<DateTime<Utc>>,
    /// Last update timestamp
    #[serde(default)]
    pub updated_at: Option<DateTime<Utc>>,
}

fn default_channel() -> String {
    "default".to_string()
}

fn default_version() -> u32 {
    1
}

impl Default for Workflow {
    fn default() -> Self {
        Self::new()
    }
}

impl Workflow {
    pub fn new() -> Self {
        Self {
            id: String::new(),
            id_arc: Arc::from(""),
            name: String::new(),
            priority: 0,
            description: None,
            condition: Value::Bool(true),
            compiled_condition: None,
            fully_sync: false,
            tasks: Vec::new(),
            continue_on_error: false,
            channel: default_channel(),
            version: 1,
            status: WorkflowStatus::Active,
            rollout: None,
            loop_config: None,
            tags: Vec::new(),
            created_at: None,
            updated_at: None,
        }
    }

    /// Create a workflow (rule) with a condition and tasks.
    ///
    /// This is a convenience constructor for the IFTTT-style rules engine pattern:
    /// **IF** `condition` **THEN** execute `tasks`.
    ///
    /// # Arguments
    /// * `id` - Unique identifier for the rule
    /// * `name` - Human-readable name
    /// * `condition` - JSONLogic condition evaluated against the full context (data, metadata, temp_data)
    /// * `tasks` - Actions to execute when the condition is met
    pub fn rule(id: &str, name: &str, condition: Value, tasks: Vec<Task>) -> Self {
        Self {
            id: id.to_string(),
            id_arc: Arc::from(id),
            name: name.to_string(),
            priority: 0,
            description: None,
            condition,
            compiled_condition: None,
            fully_sync: false,
            tasks,
            continue_on_error: false,
            channel: default_channel(),
            version: 1,
            status: WorkflowStatus::Active,
            rollout: None,
            loop_config: None,
            tags: Vec::new(),
            created_at: None,
            updated_at: None,
        }
    }

    /// Load workflow from JSON string
    pub fn from_json(json_str: &str) -> Result<Self> {
        serde_json::from_str(json_str).map_err(DataflowError::from_serde)
    }

    /// Load workflow from JSON file
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let json_str = fs::read_to_string(path).map_err(DataflowError::from_io)?;

        Self::from_json(&json_str)
    }

    /// Validate the workflow structure
    pub fn validate(&self) -> Result<()> {
        // Check required fields
        if self.id.is_empty() {
            return Err(DataflowError::Workflow(
                "Workflow id cannot be empty".to_string(),
            ));
        }

        if self.name.is_empty() {
            return Err(DataflowError::Workflow(
                "Workflow name cannot be empty".to_string(),
            ));
        }

        // Check tasks
        if self.tasks.is_empty() {
            return Err(DataflowError::Workflow(
                "Workflow must have at least one task".to_string(),
            ));
        }

        // Validate that task and group IDs are unique. Groups share the task
        // id namespace: both name a step, both surface in traces and error
        // messages, and a collision would make either ambiguous.
        let mut step_ids = std::collections::HashSet::new();
        for task in self.all_tasks() {
            for group in &task.group_starts {
                if !step_ids.insert(group.id.as_str()) {
                    return Err(DataflowError::Workflow(format!(
                        "Duplicate step ID '{}' in workflow — task group IDs share the task ID namespace",
                        group.id
                    )));
                }
            }
            if !step_ids.insert(task.id.as_str()) {
                return Err(DataflowError::Workflow(format!(
                    "Duplicate task ID '{}' in workflow",
                    task.id
                )));
            }
        }

        // Setup steps included: a `for_each` that breaks its rules is refused
        // wherever it sits. The rules live once, in `ForEach::problem`.
        for task in self.all_tasks() {
            if let Some(for_each) = &task.for_each
                && let Some(problem) = for_each.problem(&task.function)
            {
                return Err(DataflowError::Workflow(format!(
                    "Task '{}': {}",
                    task.id, problem.message
                )));
            }
        }

        // A loop whose bounds could never advance is rejected at build time
        // rather than spinning — or silently doing nothing — on the first
        // message.
        if let Some(loop_config) = &self.loop_config {
            loop_config.validate(&self.id)?;
        }

        Ok(())
    }
}

/// One task's connector reference, located within a workflow.
///
/// `Copy`: every field is a shared borrow. `config` is carried so callers can
/// apply cross-field rules — "a task on this kind of connector also needs
/// `input.database`" — without re-parsing the task.
///
/// Not `Serialize`: [`FunctionConfig`] is deserialize-only, so callers that emit
/// JSON diagnostics build their own shape from these fields.
#[derive(Debug, Clone, Copy)]
pub struct ConnectorRef<'a> {
    /// `id` of the owning workflow.
    pub workflow_id: &'a str,
    /// `id` of the referencing task.
    pub task_id: &'a str,
    /// Canonical function name, as [`FunctionConfig::function_name`].
    pub function: &'a str,
    /// The connector name when authored as a literal, or the expression when
    /// computed. See [`ConnectorName`].
    pub connector: ConnectorName<'a>,
    /// The whole function config, for cross-field rules.
    pub config: &'a FunctionConfig,
}

impl Workflow {
    /// Every task this workflow can run, in execution order: the loop's
    /// `setup` list first, then the body.
    ///
    /// The one iteration a pass over "all the tasks" should use. `tasks`
    /// alone misses setup, and a check that reads only `tasks` (a handler
    /// lookup, a secret scan, a lint) would silently pass a setup step it
    /// should have refused.
    pub fn all_tasks(&self) -> impl Iterator<Item = &Task> {
        self.loop_config
            .iter()
            .flat_map(|l| l.setup.iter())
            .chain(self.tasks.iter())
    }

    /// Mutable [`Self::all_tasks`], for the compiler's per-task passes.
    pub(crate) fn all_tasks_mut(&mut self) -> impl Iterator<Item = &mut Task> {
        self.loop_config
            .iter_mut()
            .flat_map(|l| l.setup.iter_mut())
            .chain(self.tasks.iter_mut())
    }

    /// Every connector reference in this workflow, in task order.
    ///
    /// Setup steps of a `loop` come first, as they run first.
    ///
    /// Tasks whose function names no connector are skipped. One item is yielded
    /// per *task*, not per distinct connector: two tasks on the same connector
    /// yield two items. Callers wanting a distinct set collect one themselves.
    ///
    /// Does not require a compiled workflow — this reads only deserialized
    /// fields, so it works on the output of [`Workflow::from_json`] before the
    /// engine has compiled it.
    ///
    /// Which configs carry a connector is this crate's fact; deriving it here
    /// rather than reimplementing the set downstream is the point.
    pub fn connector_refs(&self) -> impl Iterator<Item = ConnectorRef<'_>> {
        // `move` is load-bearing: it copies the `&Workflow` into the closure so
        // the returned iterator does not borrow a local.
        self.all_tasks().filter_map(move |task| {
            task.function.connector().map(|connector| ConnectorRef {
                workflow_id: &self.id,
                task_id: &task.id,
                function: task.function.function_name(),
                connector,
                config: &task.function,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wf(tasks_json: &str) -> Workflow {
        Workflow::from_json(&format!(
            r#"{{ "id": "w", "name": "w", "priority": 0, "condition": true,
                  "tasks": [{tasks_json}] }}"#
        ))
        .expect("workflow should parse")
    }

    const HTTP: &str = r#"{ "id": "call", "name": "call", "function": {
        "name": "http_call", "input": { "connector": "user_service" } } }"#;
    const KAFKA: &str = r#"{ "id": "pub", "name": "pub", "function": {
        "name": "publish_kafka",
        "input": { "connector": "events", "topic": "t" } } }"#;
    const MAP: &str = r#"{ "id": "m", "name": "m", "function": {
        "name": "map", "input": { "mappings": [] } } }"#;
    const LOG: &str = r#"{ "id": "l", "name": "l", "function": {
        "name": "log", "input": { "message": "hi" } } }"#;

    #[test]
    fn connector_refs_yields_only_connector_tasks_in_task_order() {
        let workflow = wf(&format!("{MAP},{HTTP},{LOG},{KAFKA}"));
        let refs: Vec<_> = workflow.connector_refs().collect();

        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].task_id, "call");
        assert_eq!(refs[0].function, "http_call");
        assert_eq!(refs[0].connector.as_static(), Some("user_service"));
        assert_eq!(refs[1].task_id, "pub");
        assert_eq!(refs[1].function, "publish_kafka");
        assert_eq!(refs[1].connector.as_static(), Some("events"));
    }

    #[test]
    fn connector_refs_carries_the_owning_workflow_id() {
        let workflow = wf(HTTP);
        assert!(workflow.connector_refs().all(|r| r.workflow_id == "w"));

        // Including the empty-id case from `Workflow::new()`.
        let empty = Workflow::new();
        assert_eq!(empty.id, "");
        assert_eq!(empty.connector_refs().count(), 0);
    }

    #[test]
    fn connector_refs_is_empty_for_no_tasks() {
        // `validate` rejects an empty task list, but `connector_refs` must not
        // assume `validate` ran — `Workflow::new()` has empty tasks.
        assert_eq!(Workflow::new().connector_refs().count(), 0);
    }

    #[test]
    fn connector_refs_does_not_deduplicate() {
        let a = r#"{ "id": "a", "name": "a", "function": {
            "name": "http_call", "input": { "connector": "same" } } }"#;
        let b = r#"{ "id": "b", "name": "b", "function": {
            "name": "enrich",
            "input": { "connector": "same", "merge_path": "data.out" } } }"#;
        let workflow = wf(&format!("{a},{b}"));

        let refs: Vec<_> = workflow.connector_refs().collect();
        assert_eq!(refs.len(), 2, "one item per task, not a distinct set");
        assert!(refs.iter().all(|r| r.connector.as_static() == Some("same")));
    }

    #[test]
    fn connector_refs_works_on_an_uncompiled_workflow() {
        // Straight from `from_json`, before any engine construction: `id_arc` and
        // `compiled_condition` are still unset.
        let workflow = wf(HTTP);
        assert!(workflow.compiled_condition.is_none());
        assert_eq!(workflow.connector_refs().count(), 1);
    }

    #[test]
    fn connector_ref_is_copy() {
        let workflow = wf(HTTP);
        let r = workflow.connector_refs().next().unwrap();
        let copied = r;
        // Reading both without cloning only compiles if `ConnectorRef` is `Copy`.
        assert_eq!(r.connector, copied.connector);
        assert_eq!(r.task_id, copied.task_id);
    }

    #[test]
    fn connector_ref_config_supports_a_cross_field_rule() {
        // Proves `config` is load-bearing rather than decorative: read another
        // key out of the same task's input.
        let custom = r#"{ "id": "db", "name": "db", "function": {
            "name": "pg_query",
            "input": { "connector": "pg_main", "database": "orders" } } }"#;
        let workflow = wf(custom);

        let r = workflow.connector_refs().next().expect("custom connector");
        assert_eq!(r.connector.as_static(), Some("pg_main"));
        match r.config {
            FunctionConfig::Custom { input, .. } => {
                assert_eq!(
                    input.get("database").and_then(|v| v.as_str()),
                    Some("orders")
                );
            }
            other => panic!("expected Custom, got {other:?}"),
        }
    }

    #[test]
    fn rollout_defaults_to_none_on_every_construction_path() {
        assert_eq!(Workflow::new().rollout, None);
        assert_eq!(Workflow::default().rollout, None);
        assert_eq!(
            Workflow::rule("r", "r", Value::Bool(true), Vec::new()).rollout,
            None
        );
        assert_eq!(wf(MAP).rollout, None, "absent JSON key gives None");
    }

    // -----------------------------------------------------------------
    // LoopConfig
    // -----------------------------------------------------------------

    /// Build a one-task workflow carrying `loop_json`, then validate it.
    fn loop_wf(loop_json: &str) -> Result<Workflow> {
        let workflow = Workflow::from_json(&format!(
            r#"{{ "id": "w", "name": "w", "loop": {loop_json}, "tasks": [{MAP}] }}"#
        ))?;
        workflow.validate()?;
        Ok(workflow)
    }

    #[test]
    fn loop_config_defaults_init_zero_increment_one() {
        let cfg = loop_wf(r#"{"max": 5}"#)
            .expect("valid loop")
            .loop_config
            .expect("loop config present");
        assert_eq!(cfg.init, 0);
        assert_eq!(cfg.increment, 1);
        assert_eq!(cfg.max, 5);
        assert_eq!(cfg.counter, None);
    }

    #[test]
    fn loop_config_is_absent_on_every_construction_path() {
        assert!(wf(MAP).loop_config.is_none(), "absent JSON key gives None");
        assert!(Workflow::new().loop_config.is_none());
        assert!(Workflow::default().loop_config.is_none());
        assert!(
            Workflow::rule("r", "r", Value::Bool(true), Vec::new())
                .loop_config
                .is_none()
        );
    }

    #[test]
    fn loop_config_rejects_a_bound_that_could_never_run_a_sweep() {
        // Half-open: sweeps run while `counter < max`, so max == init is zero
        // sweeps and max < init is worse.
        assert!(loop_wf(r#"{"max": 0}"#).is_err());
        assert!(loop_wf(r#"{"init": 5, "max": 5}"#).is_err());
        assert!(loop_wf(r#"{"init": 5, "max": 2}"#).is_err());
    }

    #[test]
    fn loop_config_rejects_a_non_advancing_increment() {
        assert!(loop_wf(r#"{"max": 5, "increment": 0}"#).is_err());
        assert!(loop_wf(r#"{"max": 5, "increment": -1}"#).is_err());
    }

    #[test]
    fn loop_config_rejects_an_empty_counter_path() {
        assert!(loop_wf(r#"{"max": 5, "counter": ""}"#).is_err());
        assert!(loop_wf(r#"{"max": 5, "counter": "a..b"}"#).is_err());
        assert!(loop_wf(r#"{"max": 5, "counter": "a."}"#).is_err());
    }

    #[test]
    fn loop_config_requires_max() {
        // No default: an unbounded loop is never what the author meant, so it
        // fails to deserialize rather than picking a bound on their behalf.
        assert!(
            Workflow::from_json(r#"{ "id": "w", "name": "w", "loop": {}, "tasks": [] }"#).is_err()
        );
    }

    #[test]
    fn loop_config_deserializes_every_combination_of_optional_fields() {
        // `max` is the only required field; the other three are independently
        // optional, so all eight presence combinations must land on the
        // documented defaults for whatever is absent.
        for (json, counter, init, increment) in [
            (r#"{"max": 9}"#, None, 0, 1),
            (r#"{"max": 9, "counter": "i"}"#, Some("i"), 0, 1),
            (r#"{"max": 9, "init": 4}"#, None, 4, 1),
            (r#"{"max": 9, "increment": 3}"#, None, 0, 3),
            (r#"{"max": 9, "counter": "i", "init": 4}"#, Some("i"), 4, 1),
            (
                r#"{"max": 9, "counter": "i", "increment": 3}"#,
                Some("i"),
                0,
                3,
            ),
            (r#"{"max": 9, "init": 4, "increment": 3}"#, None, 4, 3),
            (
                r#"{"max": 9, "counter": "i", "init": 4, "increment": 3}"#,
                Some("i"),
                4,
                3,
            ),
        ] {
            let cfg = loop_wf(json)
                .unwrap_or_else(|e| panic!("{json} should be valid: {e}"))
                .loop_config
                .expect("loop config present");
            assert_eq!(cfg.counter.as_deref(), counter, "counter for {json}");
            assert_eq!(cfg.init, init, "init for {json}");
            assert_eq!(cfg.increment, increment, "increment for {json}");
            assert_eq!(cfg.max, 9, "max for {json}");
        }
    }

    #[test]
    fn loop_config_validation_matrix_over_init_increment_and_max() {
        // The full accept/reject table for the three numeric fields. `max` must
        // be strictly above `init` (half-open bound) and `increment` at least
        // 1 (the counter must advance).
        for (init, increment, max, valid) in [
            // Ordinary forward ranges.
            (0_i64, 1_i64, 1_i64, true),
            (0, 1, 100, true),
            (0, 7, 3, true), // one sweep, then the increment overshoots
            (5, 1, 6, true),
            // Negative and mixed-sign ranges are fine as long as max > init.
            (-5, 1, 0, true),
            (-5, 2, -4, true),
            (-1, 1, 1, true),
            // Empty or inverted bounds.
            (0, 1, 0, false),
            (5, 1, 5, false),
            (5, 1, 4, false),
            (0, 1, -1, false),
            (-5, 1, -5, false),
            // Non-advancing increments, independent of the bound.
            (0, 0, 10, false),
            (0, -1, 10, false),
            (0, -100, 10, false),
        ] {
            let json = format!(r#"{{"init": {init}, "increment": {increment}, "max": {max}}}"#);
            assert_eq!(
                loop_wf(&json).is_ok(),
                valid,
                "init={init} increment={increment} max={max} should be {}",
                if valid { "accepted" } else { "rejected" }
            );
        }
    }

    #[test]
    fn loop_config_counter_path_matrix() {
        // Accepted and rejected counter spellings, including the `#` escape the
        // rest of the path vocabulary uses for numerically-named keys.
        for (counter, valid) in [
            ("i", true),
            ("index", true),
            ("cursor.index", true),
            ("a.b.c.d", true),
            ("#7", true), // escaped numeric object key, same as elsewhere
            ("", false),
            (".", false),
            ("a.", false),
            (".a", false),
            ("a..b", false),
        ] {
            let json = format!(r#"{{"max": 5, "counter": "{counter}"}}"#);
            assert_eq!(
                loop_wf(&json).is_ok(),
                valid,
                "counter {counter:?} should be {}",
                if valid { "accepted" } else { "rejected" }
            );
        }
    }

    #[test]
    fn precompute_counter_path_matrix() {
        for (counter, expected) in [
            ("i", vec!["temp_data", "i"]),
            ("cursor.index", vec!["temp_data", "cursor", "index"]),
            ("a.b.c", vec!["temp_data", "a", "b", "c"]),
            // The `#` prefix is preserved here and stripped at write time,
            // exactly as `MapMapping::path_parts` treats it.
            ("#7", vec!["temp_data", "#7"]),
        ] {
            let mut cfg = loop_wf(&format!(r#"{{"max": 5, "counter": "{counter}"}}"#))
                .expect("valid loop")
                .loop_config
                .expect("loop config present");
            cfg.precompute_paths();
            let parts: Vec<&str> = cfg.counter_parts.iter().map(Arc::as_ref).collect();
            assert_eq!(parts, expected, "for counter {counter:?}");
        }
    }

    #[test]
    fn precompute_counter_path_is_idempotent() {
        // The compiler runs once, but a hot reload recompiles the same config;
        // calling twice must not accumulate segments.
        let mut cfg = loop_wf(r#"{"max": 5, "counter": "cursor.index"}"#)
            .expect("valid loop")
            .loop_config
            .expect("loop config present");
        cfg.precompute_paths();
        let first: Vec<Arc<str>> = cfg.counter_parts.to_vec();
        cfg.precompute_paths();
        assert_eq!(cfg.counter_parts.to_vec(), first);
    }

    #[test]
    fn loop_config_rejects_a_non_object_and_a_non_numeric_max() {
        for json in [r#""five""#, "5", "[]", r#"{"max": "5"}"#, "true"] {
            assert!(loop_wf(json).is_err(), "{json} is not a valid loop config");
        }
    }

    #[test]
    fn an_explicit_null_loop_means_no_loop() {
        // `Option<LoopConfig>` takes an explicit JSON null as absence, so a
        // caller emitting `"loop": null` for "no loop" gets the single-pass
        // workflow they meant rather than a deserialization error.
        let workflow = loop_wf("null").expect("explicit null should be accepted");
        assert!(workflow.loop_config.is_none());
    }

    #[test]
    fn a_workflow_with_a_loop_still_validates_its_other_rules() {
        // Loop validation is additive: the pre-existing rules still fire, and
        // an otherwise-invalid workflow is not rescued by a valid loop.
        let duplicate_tasks = Workflow::from_json(
            r#"{ "id": "w", "name": "w", "loop": {"max": 5}, "tasks": [
                 {"id": "t", "name": "t", "function": {"name": "map", "input": {"mappings": []}}},
                 {"id": "t", "name": "t", "function": {"name": "map", "input": {"mappings": []}}}] }"#,
        )
        .expect("should parse");
        assert!(duplicate_tasks.validate().is_err(), "duplicate task ids");

        let no_tasks =
            Workflow::from_json(r#"{ "id": "w", "name": "w", "loop": {"max": 5}, "tasks": [] }"#)
                .expect("should parse");
        assert!(no_tasks.validate().is_err(), "empty task list");
    }

    #[test]
    fn loop_config_coexists_with_every_other_workflow_field() {
        // `loop` is orthogonal to the rest of the schema — nothing it adds
        // shadows or disturbs a neighbouring field.
        let workflow = Workflow::from_json(&format!(
            r#"{{ "id": "w", "name": "w", "priority": 7, "description": "d",
                  "condition": {{"==": [1, 1]}},
                  "loop": {{"counter": "i", "max": 5}},
                  "continue_on_error": true, "channel": "c", "version": 3,
                  "status": "paused",
                  "rollout": {{"bucket_start": 0, "bucket_end": 50}},
                  "tags": ["x"], "tasks": [{MAP}] }}"#
        ))
        .expect("should parse");
        workflow.validate().expect("should validate");

        assert_eq!(workflow.priority, 7);
        assert_eq!(workflow.channel, "c");
        assert_eq!(workflow.version, 3);
        assert_eq!(workflow.status, WorkflowStatus::Paused);
        assert!(workflow.continue_on_error);
        assert_eq!(
            workflow.rollout,
            Some(Rollout {
                bucket_start: 0,
                bucket_end: 50
            })
        );
        assert_eq!(workflow.tags, ["x"]);
        assert_eq!(
            workflow
                .loop_config
                .expect("loop present")
                .counter
                .as_deref(),
            Some("i")
        );
    }

    #[test]
    fn loop_config_accepts_a_valid_counter() {
        let cfg = loop_wf(r#"{"max": 5, "counter": "cursor.index"}"#)
            .expect("valid loop")
            .loop_config
            .expect("loop config present");
        assert_eq!(cfg.counter.as_deref(), Some("cursor.index"));
    }

    #[test]
    fn precompute_counter_path_prefixes_temp_data() {
        let mut cfg = loop_wf(r#"{"max": 5, "counter": "cursor.index"}"#)
            .expect("valid loop")
            .loop_config
            .expect("loop config present");
        assert!(
            cfg.counter_parts.is_empty(),
            "uncompiled workflows start with no pre-split path"
        );

        cfg.precompute_paths();

        let parts: Vec<&str> = cfg.counter_parts.iter().map(Arc::as_ref).collect();
        assert_eq!(parts, ["temp_data", "cursor", "index"]);
    }

    #[test]
    fn precompute_counter_path_is_empty_without_a_counter_name() {
        let mut cfg = loop_wf(r#"{"max": 5}"#)
            .expect("valid loop")
            .loop_config
            .expect("loop config present");
        cfg.precompute_paths();
        assert!(cfg.counter_parts.is_empty());
    }

    #[test]
    fn loop_config_parses_setup_over_as_and_scratch() {
        let cfg = loop_wf(
            r#"{"counter": "i", "max": 64, "as": "item", "scratch": "it",
                "over": {"var": "temp_data.batch.items"},
                "setup": [
                  {"id": "claim", "name": "claim", "function": {"name": "map", "input": {"mappings": []}}},
                  {"id": "read", "condition": true, "tasks": [
                    {"id": "batch", "name": "batch", "function": {"name": "map", "input": {"mappings": []}}}
                  ]}
                ]}"#,
        )
        .expect("valid loop")
        .loop_config
        .expect("loop config present");
        let setup_ids: Vec<&str> = cfg.setup.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(
            setup_ids,
            ["claim", "batch"],
            "setup is flattened like tasks"
        );
        assert_eq!(
            cfg.setup[1].group_starts.len(),
            1,
            "the group span is recorded"
        );
        assert_eq!(cfg.item.as_deref(), Some("item"));
        assert_eq!(cfg.scratch.as_deref(), Some("it"));
        assert!(cfg.over.is_some());
        assert!(
            cfg.compiled_over.is_none(),
            "uncompiled until the engine builds"
        );
    }

    #[test]
    fn a_counter_only_loop_has_no_setup_over_as_or_scratch() {
        let cfg = loop_wf(r#"{"max": 5}"#).unwrap().loop_config.unwrap();
        assert!(cfg.setup.is_empty());
        assert!(cfg.over.is_none());
        assert!(cfg.item.is_none());
        assert!(cfg.scratch.is_none());

        let bounded = LoopConfig::bounded(5);
        assert_eq!(
            (bounded.init, bounded.increment, bounded.max),
            (0, 1, 5),
            "bounded() lands on the serde defaults"
        );
        assert!(bounded.setup.is_empty() && bounded.over.is_none());
        assert!(bounded.counter.is_none() && bounded.item.is_none() && bounded.scratch.is_none());
    }

    #[test]
    fn loop_config_rejects_as_without_over() {
        let err = loop_wf(r#"{"max": 5, "as": "item"}"#).expect_err("as needs over");
        assert!(err.to_string().contains("no `over`"), "got: {err}");
        assert!(loop_wf(r#"{"max": 5, "over": {"var": "data.items"}, "as": "item"}"#).is_ok());
        assert!(
            loop_wf(r#"{"max": 5, "over": {"var": "data.items"}}"#).is_ok(),
            "as is optional"
        );
    }

    #[test]
    fn loop_config_rejects_a_scalar_over_literal() {
        for over in [r#""data.items""#, "5", "true", "null"] {
            let json = format!(r#"{{"max": 5, "over": {over}}}"#);
            assert!(loop_wf(&json).is_err(), "{json} must be rejected");
        }
        for over in [r#"{"var": "data.items"}"#, "[1, 2, 3]", "[]"] {
            let json = format!(r#"{{"max": 5, "over": {over}}}"#);
            assert!(
                loop_wf(&json).is_ok(),
                "{json} is an expression or an array literal"
            );
        }
    }

    #[test]
    fn loop_config_rejects_a_negative_init_with_over() {
        assert!(loop_wf(r#"{"init": -1, "max": 5, "over": []}"#).is_err());
        assert!(
            loop_wf(r#"{"init": -1, "max": 5}"#).is_ok(),
            "fine without over, as today"
        );
        assert!(
            loop_wf(r#"{"init": 2, "max": 5, "over": []}"#).is_ok(),
            "an offset is fine"
        );
    }

    #[test]
    fn loop_config_slot_path_and_collision_matrix() {
        for (counter, item, scratch, valid) in [
            ("i", "item", "it", true),
            ("i", "it.item", "scratch", true),
            ("a.b", "a.c", "a.d", true),
            ("i", "i", "it", false),        // as == counter
            ("i", "item", "item", false),   // scratch == as
            ("it", "it.item", "x", false),  // as inside the counter
            ("i", "item", "item.x", false), // scratch inside as
            ("it.x", "item", "it", false),  // counter inside scratch
        ] {
            let json = format!(
                r#"{{"max": 5, "over": [], "counter": "{counter}", "as": "{item}", "scratch": "{scratch}"}}"#
            );
            assert_eq!(loop_wf(&json).is_ok(), valid, "{json}");
        }
        for (field, path) in [
            ("as", ""),
            ("as", "a..b"),
            ("scratch", "."),
            ("scratch", "a."),
        ] {
            let json = format!(r#"{{"max": 5, "over": [], "{field}": "{path}"}}"#);
            assert!(loop_wf(&json).is_err(), "{json} is a malformed slot path");
        }
    }

    #[test]
    fn slots_overlap_is_segment_aware() {
        assert!(slots_overlap("it", "it"));
        assert!(slots_overlap("it", "it.item"));
        assert!(slots_overlap("it.item", "it"));
        assert!(
            !slots_overlap("it", "item"),
            "a shared prefix is not nesting"
        );
        assert!(!slots_overlap("a.b", "a.c"));
    }

    #[test]
    fn a_setup_step_sharing_an_id_with_a_body_task_is_rejected() {
        let workflow = Workflow::from_json(&format!(
            r#"{{ "id": "w", "name": "w",
                  "loop": {{"max": 5, "setup": [{MAP}]}},
                  "tasks": [{MAP}] }}"#
        ))
        .expect("parses");
        let err = workflow
            .validate()
            .expect_err("setup and body share one id namespace");
        assert!(err.to_string().contains("Duplicate"), "got: {err}");
    }

    #[test]
    fn a_setup_group_carrying_halt_on_is_refused_at_parse() {
        let err = Workflow::from_json(&format!(
            r#"{{ "id": "w", "name": "w",
                  "loop": {{"max": 5, "setup": [
                    {{"id": "g", "halt_on": "failure", "tasks": [{MAP}]}}]}},
                  "tasks": [{LOG}] }}"#
        ))
        .expect_err("the step grammar applies to setup");
        assert!(err.to_string().contains("halt_on"), "got: {err}");
    }

    #[test]
    fn all_tasks_yields_setup_then_body_and_connector_refs_covers_setup() {
        let workflow = Workflow::from_json(&format!(
            r#"{{ "id": "w", "name": "w",
                  "loop": {{"max": 5, "setup": [{HTTP}]}},
                  "tasks": [{MAP}, {KAFKA}] }}"#
        ))
        .expect("parses");
        let ids: Vec<&str> = workflow.all_tasks().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["call", "m", "pub"]);
        let refs: Vec<&str> = workflow.connector_refs().map(|r| r.task_id).collect();
        assert_eq!(
            refs,
            ["call", "pub"],
            "the setup connector is reported first"
        );
        assert_eq!(wf(MAP).all_tasks().count(), 1, "no loop: just the body");
    }

    #[test]
    fn precompute_paths_fills_all_three_slots() {
        let mut cfg =
            loop_wf(r#"{"max": 5, "counter": "i", "over": [], "as": "it.item", "scratch": "scr"}"#)
                .unwrap()
                .loop_config
                .unwrap();
        assert!(cfg.item_parts.is_empty() && cfg.scratch_parts.is_empty());
        cfg.precompute_paths();
        let parts = |p: &Arc<[Arc<str>]>| p.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(parts(&cfg.counter_parts), ["temp_data", "i"]);
        assert_eq!(parts(&cfg.item_parts), ["temp_data", "it", "item"]);
        assert_eq!(parts(&cfg.scratch_parts), ["temp_data", "scr"]);
    }

    #[test]
    fn rollout_deserializes_from_json() {
        let workflow = Workflow::from_json(
            r#"{ "id": "w", "name": "w", "condition": true,
                 "rollout": { "bucket_start": 0, "bucket_end": 50 },
                 "tasks": [] }"#,
        )
        .unwrap();
        assert_eq!(
            workflow.rollout,
            Some(Rollout {
                bucket_start: 0,
                bucket_end: 50
            })
        );
    }
}
