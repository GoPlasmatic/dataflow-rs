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

pub mod error;
pub mod functions;
pub mod message;
pub mod task;
pub mod workflow;

// Re-export key types for easier access
pub use error::{DataflowError, ErrorInfo, Result};
pub use functions::FunctionHandler;
pub use message::Message;
pub use task::Task;
pub use workflow::Workflow;

// Re-export the jsonlogic library under our namespace
pub use datalogic_rs as jsonlogic;

use log::{debug, error, info, warn};
use message::AuditTrail;
use std::sync::{Arc, Mutex};

use chrono::Utc;
use datalogic_rs::DataLogic;
use serde_json::{json, Map, Number, Value};
use std::collections::HashMap;

/// Configuration for retry behavior
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum number of retries
    pub max_retries: u32,
    /// Delay between retries in milliseconds
    pub retry_delay_ms: u64,
    /// Whether to use exponential backoff
    pub use_backoff: bool,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            retry_delay_ms: 1000,
            use_backoff: true,
        }
    }
}

/// Main engine that processes messages through workflows
pub struct Engine {
    /// Registry of available workflows
    workflows: HashMap<String, Workflow>,
    /// Registry of function handlers that can be executed by tasks
    task_functions: HashMap<String, Box<dyn FunctionHandler + Send + Sync>>,
    /// DataLogic instance for evaluating conditions
    data_logic: Arc<Mutex<DataLogic>>,
    /// Configuration for retry behavior
    retry_config: RetryConfig,
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
            data_logic: Arc::new(Mutex::new(DataLogic::new())),
            retry_config: RetryConfig::default(),
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
            data_logic: Arc::new(Mutex::new(DataLogic::new())),
            retry_config: RetryConfig::default(),
        }
    }

    /// Configure retry behavior
    pub fn with_retry_config(mut self, config: RetryConfig) -> Self {
        self.retry_config = config;
        self
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
    /// use dataflow_rs::engine::error::Result;
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
    ///     ) -> Result<(usize, Vec<Change>)> {
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
    /// # Returns
    ///
    /// * `Result<()>` - Success or an error if processing failed
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
    /// engine.process_message(&mut message).unwrap();
    /// ```
    pub fn process_message(&self, message: &mut Message) -> error::Result<()> {
        debug!("Processing message {}", message.id);

        // Process each workflow
        for workflow in self.workflows.values() {
            // Check if workflow should process this message based on condition
            let condition = workflow.condition.clone().unwrap_or(Value::Bool(true));

            match self.eval_condition(&condition, &message.metadata) {
                Ok(should_process) => {
                    if !should_process {
                        debug!("Workflow {} skipped - condition not met", workflow.id);
                        continue;
                    }

                    info!("Processing workflow {}", workflow.id);

                    // Process each task in the workflow
                    for task in &workflow.tasks {
                        let task_condition = task.condition.clone().unwrap_or(Value::Bool(true));

                        match self.eval_condition(&task_condition, &message.metadata) {
                            Ok(should_execute) => {
                                if !should_execute {
                                    debug!("Task {} skipped - condition not met", task.id);
                                    continue;
                                }

                                // Execute task if we have a handler
                                if let Some(function) = self.task_functions.get(&task.function.name)
                                {
                                    self.execute_task_with_retry(
                                        &task.id,
                                        &workflow.id,
                                        message,
                                        &task.function.input,
                                        &**function,
                                    )?;
                                } else {
                                    let error = DataflowError::Workflow(format!(
                                        "Function '{}' not found",
                                        task.function.name
                                    ));
                                    message.add_error(ErrorInfo::new(
                                        Some(workflow.id.clone()),
                                        Some(task.id.clone()),
                                        error.clone(),
                                    ));

                                    return Err(error);
                                }
                            }
                            Err(e) => {
                                let error = DataflowError::LogicEvaluation(format!(
                                    "Failed to evaluate task condition: {}",
                                    e
                                ));
                                message.add_error(ErrorInfo::new(
                                    Some(workflow.id.clone()),
                                    Some(task.id.clone()),
                                    error.clone(),
                                ));

                                return Err(error);
                            }
                        }
                    }
                }
                Err(e) => {
                    let error = DataflowError::LogicEvaluation(format!(
                        "Failed to evaluate workflow condition: {}",
                        e
                    ));
                    message.add_error(ErrorInfo::new(
                        Some(workflow.id.clone()),
                        None,
                        error.clone(),
                    ));

                    return Err(error);
                }
            }
        }

        Ok(())
    }

    /// Evaluates a condition using DataLogic and returns the result
    fn eval_condition(&self, condition: &Value, data: &Value) -> error::Result<bool> {
        // Use mutex to safely access DataLogic
        let data_logic = self
            .data_logic
            .lock()
            .map_err(|_| DataflowError::Unknown("Failed to acquire data_logic lock".to_string()))?;

        data_logic
            .evaluate_json(condition, data, None)
            .map_err(|e| {
                DataflowError::LogicEvaluation(format!("Error evaluating condition: {}", e))
            })
            .map(|result| result.as_bool().unwrap_or(false))
    }

    /// Execute a task with a retry mechanism based on the engine's retry configuration
    fn execute_task_with_retry(
        &self,
        task_id: &str,
        workflow_id: &str,
        message: &mut Message,
        input_json: &Value,
        function: &dyn FunctionHandler,
    ) -> error::Result<()> {
        info!("Executing task {} in workflow {}", task_id, workflow_id);

        let mut last_error = None;
        let mut retry_count = 0;

        // Try executing the task up to max_retries + 1 times (initial attempt + retries)
        while retry_count <= self.retry_config.max_retries {
            match function.execute(message, input_json) {
                Ok((status_code, changes)) => {
                    // Create a new audit trail entry
                    message.audit_trail.push(AuditTrail {
                        workflow_id: workflow_id.to_string(),
                        task_id: task_id.to_string(),
                        timestamp: Utc::now().to_rfc3339(),
                        changes,
                        status_code,
                    });

                    info!("Task {} completed with status {}", task_id, status_code);

                    // Add the progress to metadata
                    let mut progress = Map::new();
                    progress.insert("task_id".to_string(), Value::String(task_id.to_string()));
                    progress.insert(
                        "workflow_id".to_string(),
                        Value::String(workflow_id.to_string()),
                    );
                    progress.insert(
                        "status_code".to_string(),
                        Value::Number(Number::from(status_code)),
                    );
                    progress.insert(
                        "timestamp".to_string(),
                        Value::String(Utc::now().to_rfc3339()),
                    );

                    // If we had retries, record that too
                    if retry_count > 0 {
                        progress.insert(
                            "retries".to_string(),
                            Value::Number(Number::from(retry_count)),
                        );
                    }

                    message.metadata["progress"] = json!(progress);

                    return Ok(());
                }
                Err(e) => {
                    last_error = Some(e.clone());

                    if retry_count < self.retry_config.max_retries {
                        warn!(
                            "Task {} execution failed, retry {}/{}: {:?}",
                            task_id,
                            retry_count + 1,
                            self.retry_config.max_retries,
                            e
                        );

                        // Calculate delay with optional exponential backoff
                        let delay = if self.retry_config.use_backoff {
                            self.retry_config.retry_delay_ms * (2_u64.pow(retry_count))
                        } else {
                            self.retry_config.retry_delay_ms
                        };

                        // Sleep for the calculated delay
                        std::thread::sleep(std::time::Duration::from_millis(delay));

                        retry_count += 1;
                    } else {
                        break;
                    }
                }
            }
        }

        // If we're here, all retries failed
        let error = last_error.unwrap_or_else(|| {
            DataflowError::Unknown("Unknown error during task execution".to_string())
        });

        error!(
            "Task {} execution failed after {} retries: {:?}",
            task_id, retry_count, error
        );

        // Record the error in the message
        message.add_error(ErrorInfo::new(
            Some(workflow_id.to_string()),
            Some(task_id.to_string()),
            error.clone(),
        ));

        Err(DataflowError::function_execution(
            format!("Task '{}' failed after {} retries", task_id, retry_count),
            Some(error),
        ))
    }
}
