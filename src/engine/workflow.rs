use crate::engine::error::{DataflowError, Result};
use crate::engine::functions::FunctionConfig;
use crate::engine::task::Task;
use chrono::{DateTime, Utc};
use datalogic_rs::Logic;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::path::Path;
use std::sync::Arc;

/// Half-open bucket range `[bucket_start, bucket_end)` over `0..100`, giving this
/// workflow a slice of the traffic on its channel.
///
/// Compared against [`crate::Message::routing_bucket`]. The engine does **not**
/// derive the bucket: how a caller maps to one — a sticky hash of some request
/// identity, a per-message random draw, round-robin — is entirely the caller's
/// policy and deliberately stays outside this crate.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Rollout {
    /// Inclusive lower bound.
    pub bucket_start: u8,
    /// Exclusive upper bound. `100` means "up to and including bucket 99".
    pub bucket_end: u8,
}

impl Rollout {
    /// Whether this range serves `bucket` (`0..=99`).
    ///
    /// `[0, 100)` accepts everything. An empty or inverted range
    /// (`bucket_end <= bucket_start`) accepts nothing.
    #[inline]
    pub fn accepts(&self, bucket: u8) -> bool {
        bucket >= self.bucket_start && bucket < self.bucket_end
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
#[derive(Clone, Debug, Deserialize)]
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
    #[serde(default = "default_condition")]
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

fn default_condition() -> Value {
    Value::Bool(true)
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
        Workflow {
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
        Workflow {
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

        // Validate that task IDs are unique
        let mut task_ids = std::collections::HashSet::new();
        for task in &self.tasks {
            if !task_ids.insert(&task.id) {
                return Err(DataflowError::Workflow(format!(
                    "Duplicate task ID '{}' in workflow",
                    task.id
                )));
            }
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
    /// The connector name, exactly as authored.
    pub connector: &'a str,
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
        assert_eq!(refs[0].connector, "user_service");
        assert_eq!(refs[1].task_id, "pub");
        assert_eq!(refs[1].function, "publish_kafka");
        assert_eq!(refs[1].connector, "events");
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
        assert!(refs.iter().all(|r| r.connector == "same"));
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
        assert_eq!(r.connector, "pg_main");
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
    fn rollout_accepts_is_a_half_open_range() {
        let all = Rollout {
            bucket_start: 0,
            bucket_end: 100,
        };
        assert!(all.accepts(0));
        assert!(all.accepts(99));

        let lower = Rollout {
            bucket_start: 0,
            bucket_end: 50,
        };
        assert!(lower.accepts(0));
        assert!(lower.accepts(49));
        assert!(!lower.accepts(50), "bucket_end is exclusive");
        assert!(!lower.accepts(99));

        // `start` inclusive, `end` exclusive — boundary exactness.
        let upper = Rollout {
            bucket_start: 50,
            bucket_end: 100,
        };
        assert!(upper.accepts(50), "bucket_start is inclusive");
        assert!(upper.accepts(99));
        assert!(!upper.accepts(49));

        // The two halves partition 0..=99 exactly.
        for b in 0u8..=99 {
            assert_ne!(
                lower.accepts(b),
                upper.accepts(b),
                "bucket {b} must be served by exactly one half"
            );
        }
    }

    #[test]
    fn rollout_empty_and_inverted_ranges_accept_nothing() {
        let empty = Rollout {
            bucket_start: 50,
            bucket_end: 50,
        };
        let inverted = Rollout {
            bucket_start: 60,
            bucket_end: 20,
        };
        for b in 0u8..=99 {
            assert!(!empty.accepts(b), "empty range accepted {b}");
            assert!(!inverted.accepts(b), "inverted range accepted {b}");
        }
    }

    #[test]
    fn rollout_end_of_100_is_representable_without_overflow() {
        // `bucket_end = 100` fits a u8 and `accepts` does no arithmetic on it.
        let r = Rollout {
            bucket_start: 99,
            bucket_end: 100,
        };
        assert!(r.accepts(99));
        assert!(!r.accepts(98));
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
