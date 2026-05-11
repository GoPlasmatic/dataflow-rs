use crate::engine::error::{DataflowError, Result};
use crate::engine::task::Task;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::path::Path;
use std::sync::Arc;

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
    /// `Arc<str>` mirror of `id`, populated by `LogicCompiler::compile_workflows`.
    /// Cloning this is a refcount bump; per-message `AuditTrail` entries reuse
    /// it instead of allocating a fresh `Arc<str>` from `&id` each time.
    #[serde(skip)]
    pub id_arc: Arc<str>,
    pub name: String,
    #[serde(default)]
    pub priority: u32,
    pub description: Option<String>,
    #[serde(default = "default_condition")]
    pub condition: Value,
    #[serde(skip)]
    pub condition_index: Option<usize>,
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
            condition_index: None,
            tasks: Vec::new(),
            continue_on_error: false,
            channel: default_channel(),
            version: 1,
            status: WorkflowStatus::Active,
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
            condition_index: None,
            tasks,
            continue_on_error: false,
            channel: default_channel(),
            version: 1,
            status: WorkflowStatus::Active,
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
