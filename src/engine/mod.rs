/*!
# Engine Module

This module implements the core workflow engine for dataflow-rs. The engine processes
messages through workflows composed of tasks, providing a flexible and extensible
data processing pipeline.

## Key Components

- **Engine**: The main engine that processes messages through workflows
- **Workflow**: A collection of tasks with conditions that determine when they should be applied
- **Task**: An individual processing unit that performs a specific function on a message
- **FunctionHandler**: A trait implemented by task handlers to define custom processing logic
- **Message**: The data structure that flows through the engine, with data, metadata, and processing results
*/

pub mod functions;
pub mod message;
pub mod task;
pub mod workflow;

// Re-export key types for easier access
pub use functions::FunctionHandler;
pub use message::Message;
pub use task::Task;
pub use workflow::Workflow;

// Re-export the jsonlogic library under our namespace
pub use datalogic_rs as jsonlogic;

use message::AuditTrail;

use chrono::Utc;
use datalogic_rs::DataLogic;
use serde_json::{json, Map, Number, Value};
use std::collections::HashMap;

/// Main engine that processes messages through workflows
pub struct Engine {
    /// Registry of available workflows
    workflows: HashMap<String, Workflow>,
    /// Registry of function handlers that can be executed by tasks
    task_functions: HashMap<String, Box<dyn FunctionHandler + Send + Sync>>,
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine {
    /// Creates a new Engine instance with built-in function handlers pre-registered.
    ///
    /// # Example
    ///
    /// ```
    /// use dataflow_rs::Engine;
    ///
    /// let engine = Engine::new();
    /// ```
    pub fn new() -> Self {
        let mut engine = Self {
            workflows: HashMap::new(),
            task_functions: HashMap::new(),
        };

        // Register built-in function handlers
        for (name, handler) in functions::builtins::get_all_functions() {
            engine.register_task_function(name, handler);
        }

        engine
    }

    /// Create a new engine instance without any pre-registered functions
    pub fn new_empty() -> Self {
        Self {
            task_functions: HashMap::new(),
            workflows: HashMap::new(),
        }
    }

    /// Adds a workflow to the engine.
    ///
    /// # Arguments
    ///
    /// * `workflow` - The workflow to add
    ///
    /// # Example
    ///
    /// ```
    /// use dataflow_rs::{Engine, Workflow};
    /// use serde_json::json;
    ///
    /// let mut engine = Engine::new();
    /// 
    /// // Example JSON string for a workflow
    /// let workflow_json_str = r#"
    /// {
    ///     "id": "example_workflow",
    ///     "name": "Example Workflow",
    ///     "tasks": []
    /// }
    /// "#;
    /// 
    /// let workflow = Workflow::from_json(workflow_json_str).unwrap();
    /// engine.add_workflow(&workflow);
    /// ```
    pub fn add_workflow(&mut self, workflow: &Workflow) {
        self.workflows.insert(workflow.id.clone(), workflow.clone());
    }

    /// Registers a custom function handler with the engine.
    ///
    /// # Arguments
    ///
    /// * `name` - The name of the function handler
    /// * `handler` - The function handler implementation
    ///
    /// # Example
    ///
    /// ```
    /// use dataflow_rs::{Engine, FunctionHandler};
    /// use dataflow_rs::engine::message::{Change, Message};
    /// use serde_json::Value;
    /// 
    /// // Example custom function implementation
    /// struct CustomFunction;
    /// 
    /// impl FunctionHandler for CustomFunction {
    ///     fn execute(
    ///         &self,
    ///         message: &mut Message,
    ///         _input: &Value,
    ///     ) -> Result<(usize, Vec<Change>), String> {
    ///         // Implementation would go here
    ///         Ok((200, vec![]))
    ///     }
    /// }
    /// 
    /// let mut engine = Engine::new();
    /// engine.register_task_function("custom".to_string(), Box::new(CustomFunction));
    /// ```
    pub fn register_task_function(
        &mut self,
        name: String,
        handler: Box<dyn FunctionHandler + Send + Sync>,
    ) {
        self.task_functions.insert(name, handler);
    }

    /// Check if a function with the given name is registered
    pub fn has_function(&self, name: &str) -> bool {
        self.task_functions.contains_key(name)
    }

    /// Processes a message through workflows that match their conditions.
    ///
    /// This method:
    /// 1. Evaluates conditions for each workflow
    /// 2. For matching workflows, executes each task in sequence
    /// 3. Updates the message with processing results and audit trail
    ///
    /// # Arguments
    ///
    /// * `message` - The message to process
    ///
    /// # Example
    ///
    /// ```
    /// use dataflow_rs::Engine;
    /// use dataflow_rs::engine::Message;
    /// use serde_json::json;
    ///
    /// let mut engine = Engine::new();
    /// // ... add workflows ...
    /// let mut message = Message::new(&json!({"some": "data"}));
    /// engine.process_message(&mut message);
    /// ```
    pub fn process_message(&self, message: &mut Message) {
        let data_logic = DataLogic::new();
        // Process each workflow
        for workflow in self.workflows.values() {
            // Check if workflow should process this message based on condition
            let condition = workflow.condition.clone().unwrap_or(Value::Bool(true));
            let should_process = eval_condition(&data_logic, &condition, &message.metadata);

            if !should_process {
                continue;
            }

            // Process each task in the workflow
            for task in &workflow.tasks {
                let task_condition = task.condition.clone().unwrap_or(Value::Bool(true));
                let should_execute =
                    eval_condition(&data_logic, &task_condition, &message.metadata);

                if !should_execute {
                    continue;
                }

                // Execute task if we have a handler
                if let Some(function) = self.task_functions.get(&task.function.name) {
                    // Execute the task with fresh arena data
                    execute_task(
                        task.id.clone(),
                        workflow.id.clone(),
                        message,
                        &task.function.input,
                        &**function,
                    );
                }
            }
        }
    }
}

/// Helper function to evaluate a condition using DataLogic
fn eval_condition(data_logic: &DataLogic, condition: &Value, data: &Value) -> bool {
    match data_logic.evaluate_json(condition, data, None) {
        Ok(result) => result.as_bool().unwrap_or(false),
        Err(e) => {
            println!("Error evaluating condition: {}", e);
            false
        }
    }
}

/// Execute a task with a fresh data arena
///
/// This creates a new temporary message to isolate the task execution,
/// then copies relevant changes back to the original message.
fn execute_task(
    task_id: String,
    workflow_id: String,
    message: &mut Message,
    input_json: &Value,
    function: &dyn FunctionHandler,
) {
    println!("Executing task {}", task_id);
    // Execute the function with a fresh arena
    if let Ok((response_code, changes)) = function.execute(message, input_json) {
        let mut progress = Map::new();
        progress.insert("task_id".to_string(), Value::String(task_id.clone()));
        progress.insert(
            "workflow_id".to_string(),
            Value::String(workflow_id.clone()),
        );
        progress.insert(
            "response_code".to_string(),
            Value::Number(Number::from(response_code)),
        );
        progress.insert(
            "timestamp".to_string(),
            Value::String(chrono::Utc::now().to_rfc3339()),
        );
        message.metadata["progress"] = json!(progress);

        // Create a new audit trail entry
        message.audit_trail.push(AuditTrail {
            workflow_id,
            task_id,
            timestamp: Utc::now().to_rfc3339(),
            changes,
        });
    } else {
        println!("Error executing task {}", task_id);
    }
}
