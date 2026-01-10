//! # Task Module
//!
//! This module defines the `Task` structure, which represents a single
//! processing unit within a workflow. Tasks are the fundamental building
//! blocks of data processing pipelines.

use crate::engine::functions::FunctionConfig;
use serde::Deserialize;
use serde_json::Value;

/// A single processing unit within a workflow.
///
/// Tasks execute functions with optional conditions and error handling.
/// They are processed sequentially within a workflow, allowing later tasks
/// to depend on results from earlier ones.
///
/// # Example JSON Definition
///
/// ```json
/// {
///     "id": "validate_user",
///     "name": "Validate User Data",
///     "description": "Ensures user data meets requirements",
///     "condition": {"==": [{"var": "metadata.type"}, "user"]},
///     "function": {
///         "name": "validation",
///         "input": { "rules": [...] }
///     },
///     "continue_on_error": false
/// }
/// ```
#[derive(Clone, Debug, Deserialize)]
pub struct Task {
    /// Unique identifier for the task within the workflow.
    pub id: String,

    /// Human-readable name for the task.
    pub name: String,

    /// Optional description explaining what the task does.
    pub description: Option<String>,

    /// JSONLogic condition that determines if the task should execute.
    /// Conditions can only access `metadata` fields, not `data` fields.
    /// Defaults to `true` (always execute).
    #[serde(default = "default_condition")]
    pub condition: Value,

    /// Index into the compiled logic cache for this task's condition.
    /// Set during workflow compilation; not serialized.
    #[serde(skip)]
    pub condition_index: Option<usize>,

    /// The function configuration specifying what operation to perform.
    /// Can be a built-in function (map, validation) or a custom function.
    pub function: FunctionConfig,

    /// Whether to continue workflow execution if this task fails.
    /// When `true`, errors are recorded but don't stop the workflow.
    /// Defaults to `false`.
    #[serde(default)]
    pub continue_on_error: bool,
}

/// Returns the default condition value (always true).
fn default_condition() -> Value {
    Value::Bool(true)
}
