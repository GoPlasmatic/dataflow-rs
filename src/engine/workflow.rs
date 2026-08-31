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
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
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

    /// Engine-internal: `["temp_data", ..counter segments]`, populated by
    /// `LogicCompiler`. Empty when `counter` is `None`. Not part of the stable
    /// API.
    #[doc(hidden)]
    #[serde(skip)]
    pub counter_parts: Arc<[Arc<str>]>,
}

fn default_increment() -> i64 {
    1
}

impl LoopConfig {
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
        if let Some(counter) = &self.counter {
            if counter.is_empty() || counter.split('.').any(str::is_empty) {
                return Err(DataflowError::Workflow(format!(
                    "Workflow {workflow_id}: loop counter must be a non-empty \
                     temp_data field path, got {counter:?}"
                )));
            }
        }
        Ok(())
    }

    /// Pre-split `temp_data.{counter}` into the path parts the executor writes
    /// through, so a sweep never re-splits the path. Populated by
    /// `LogicCompiler`; the executor falls back to splitting on the fly for
    /// workflows constructed directly rather than through `Engine::builder`.
    #[doc(hidden)]
    pub fn precompute_counter_path(&mut self) {
        self.counter_parts = match &self.counter {
            Some(counter) => crate::engine::utils::compute_path_parts("temp_data", counter),
            None => Arc::from([] as [Arc<str>; 0]),
        };
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
        for task in &self.tasks {
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
    /// Every connector reference in this workflow, in task order.
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
        self.tasks.iter().filter_map(move |task| {
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
            cfg.precompute_counter_path();
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
        cfg.precompute_counter_path();
        let first: Vec<Arc<str>> = cfg.counter_parts.to_vec();
        cfg.precompute_counter_path();
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

        cfg.precompute_counter_path();

        let parts: Vec<&str> = cfg.counter_parts.iter().map(Arc::as_ref).collect();
        assert_eq!(parts, ["temp_data", "cursor", "index"]);
    }

    #[test]
    fn precompute_counter_path_is_empty_without_a_counter_name() {
        let mut cfg = loop_wf(r#"{"max": 5}"#)
            .expect("valid loop")
            .loop_config
            .expect("loop config present");
        cfg.precompute_counter_path();
        assert!(cfg.counter_parts.is_empty());
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
