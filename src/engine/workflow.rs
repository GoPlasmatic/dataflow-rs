use crate::engine::error::{DataflowError, Result};
use crate::engine::task::Task;
use serde::Deserialize;
use serde_json::Value;
use std::fs;
use std::path::Path;

/// Workflow represents a collection of tasks that execute sequentially
#[derive(Clone, Debug, Deserialize)]
pub struct Workflow {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub priority: u32,
    pub description: Option<String>,
    #[serde(default = "default_condition")]
    pub condition: Value,
    #[serde(skip)]
    pub condition_index: Option<usize>,
    pub tasks: Vec<Task>,
}

fn default_condition() -> Value {
    Value::Bool(true)
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
            name: String::new(),
            priority: 0,
            description: None,
            condition: Value::Bool(true),
            condition_index: None,
            tasks: Vec::new(),
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
