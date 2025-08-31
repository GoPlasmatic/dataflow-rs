use crate::engine::error::{DataflowError, Result};
use crate::engine::functions::FunctionConfig;
use crate::engine::task::{RawTask, Task};
use serde::Deserialize;
use serde_json::Value;
use std::fs;
use std::path::Path;

#[derive(Clone, Debug)]
pub struct Workflow {
    pub id: String,
    pub name: String,
    pub priority: u32,
    pub description: Option<String>,
    pub condition: Option<Value>,
    pub tasks: Vec<Task>,
}

/// Raw workflow structure for deserialization
#[derive(Deserialize, Clone, Debug)]
struct RawWorkflow {
    pub id: String,
    pub name: String,
    pub priority: u32,
    pub description: Option<String>,
    pub condition: Option<Value>,
    pub tasks: Vec<RawTask>,
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
            condition: None,
            tasks: Vec::new(),
        }
    }

    /// Load workflow from JSON string
    pub fn from_json(json_str: &str) -> Result<Self> {
        // First deserialize to raw workflow
        let raw_workflow: RawWorkflow =
            serde_json::from_str(json_str).map_err(DataflowError::from_serde)?;

        // Convert raw workflow to parsed workflow with pre-parsed task configs
        let mut parsed_tasks = Vec::new();
        for raw_task in raw_workflow.tasks {
            // Pre-parse the function configuration based on function name
            let function_config = FunctionConfig::from_function_input(
                &raw_task.function.name,
                &raw_task.function.input,
            )?;

            // Create parsed task with pre-parsed config
            let task = Task::new(
                raw_task.id,
                raw_task.name,
                raw_task.description,
                raw_task.condition,
                raw_task.function.name,
                function_config,
            );
            parsed_tasks.push(task);
        }

        Ok(Workflow {
            id: raw_workflow.id,
            name: raw_workflow.name,
            priority: raw_workflow.priority,
            description: raw_workflow.description,
            condition: raw_workflow.condition,
            tasks: parsed_tasks,
        })
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
