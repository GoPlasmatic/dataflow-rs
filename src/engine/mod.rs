/*!
# Engine Module

This module implements the core workflow engine for dataflow-rs. The engine provides
thread-safe, vertically-scalable message processing through workflows composed of tasks.

## Thread-Safety & Concurrency (v1.0)

The engine now features a unified concurrency model with:
- **Local DataLogic**: Each JSONLogic evaluation creates a local DataLogic instance
- **Arc-Swap Workflows**: Lock-free reads and atomic updates for workflow management
- **Unified Concurrency**: Single parameter controls both pool size and max concurrent messages
- **Zero Contention**: Pool size matches concurrent tasks to eliminate resource competition

## Key Components

- **Engine**: Thread-safe engine with configurable concurrency levels
- **Workflow**: Collection of tasks with JSONLogic conditions, stored using Arc-Swap
- **Task**: Individual processing unit that performs a specific function on a message
- **FunctionHandler**: Trait for custom processing logic
- **Message**: Data structure flowing through the engine

## Usage

```rust,no_run
use dataflow_rs::{Engine, Workflow, engine::message::Message};
use serde_json::json;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Define workflows
    let workflows = vec![
        Workflow::from_json(r#"{"id": "example", "name": "Example", "tasks": []}"#)?
    ];

    // Create engine with defaults (built-ins enabled)
    let engine = Engine::new(workflows.clone(), None, None, None, None);

    // Process messages
    let mut message = Message::new(&json!({}));
    engine.process_message(&mut message)?;

    Ok(())
}
```
*/

pub mod error;
pub mod functions;
pub mod message;
pub mod task;
pub mod workflow;

// Re-export key types for easier access
pub use error::{DataflowError, ErrorInfo, Result};
pub use functions::{FunctionConfig, FunctionHandler};
pub use message::Message;
pub use task::Task;
pub use workflow::Workflow;

// Re-export the jsonlogic library under our namespace
pub use datalogic_rs as jsonlogic;
use datalogic_rs::DataLogic;

use chrono::Utc;
use log::{debug, error, info, warn};
use message::AuditTrail;
use serde_json::{Map, Number, Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

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

/// Single-threaded engine that processes messages through workflows.
///
/// ## Architecture
///
/// The engine is optimized for both IO-bound and CPU-bound workloads, featuring:
/// - **Single-Threaded**: Designed for simple synchronous processing
/// - **Direct DataLogic**: Uses a direct DataLogic instance for JSONLogic evaluation
/// - **Immutable Workflows**: Workflows are defined at initialization and cannot be changed
///
/// ## Performance
///
/// Applications can implement their own engine pooling across threads for parallel processing.
pub struct Engine {
    /// Registry of available workflows (immutable after initialization)
    workflows: Arc<HashMap<String, Workflow>>,
    /// Registry of function handlers that can be executed by tasks (immutable after initialization)
    task_functions: Arc<HashMap<String, Box<dyn FunctionHandler + Send + Sync>>>,
    /// DataLogic instance for JSONLogic evaluation
    datalogic: DataLogic<'static>,
    /// Configuration for retry behavior
    retry_config: RetryConfig,
}

impl Clone for Engine {
    fn clone(&self) -> Self {
        Self {
            workflows: Arc::clone(&self.workflows),
            task_functions: Arc::clone(&self.task_functions),
            // Create a new DataLogic instance for the cloned Engine
            datalogic: DataLogic::with_preserve_structure(),
            retry_config: self.retry_config.clone(),
        }
    }
}

impl Default for Engine {
    fn default() -> Self {
        Self::new(Vec::new(), None, None, None, None)
    }
}

impl Engine {
    /// Creates a new Engine instance with configurable parameters.
    ///
    /// # Arguments
    /// * `workflows` - The workflows to use for processing messages
    /// * `custom_functions` - Optional custom function handlers (None uses empty map)
    /// * `include_builtins` - Optional flag to include built-in functions (defaults to true if None)
    /// * `retry_config` - Optional retry configuration (uses default if None)
    ///
    /// # Example
    ///
    /// ```
    /// use dataflow_rs::{Engine, Workflow};
    ///
    /// let workflows = vec![Workflow::from_json(r#"{"id": "test", "name": "Test", "priority": 0, "tasks": []}"#).unwrap()];
    ///
    /// // Simple usage with defaults
    /// let engine = Engine::new(workflows.clone(), None, None, None, None);
    ///
    /// ```
    pub fn new(
        workflows: Vec<Workflow>,
        custom_functions: Option<HashMap<String, Box<dyn FunctionHandler + Send + Sync>>>,
        include_builtins: Option<bool>,
        _concurrency: Option<usize>,
        retry_config: Option<RetryConfig>,
    ) -> Self {
        let mut workflow_map = HashMap::new();

        // Validate and add all workflows
        for workflow in workflows {
            if let Err(e) = workflow.validate() {
                error!("Invalid workflow {}: {:?}", workflow.id, e);
                continue;
            }
            workflow_map.insert(workflow.id.clone(), workflow);
        }

        let mut task_functions = custom_functions.unwrap_or_default();

        // Add built-in function handlers if requested (defaults to true)
        if include_builtins.unwrap_or(true) {
            for (name, handler) in functions::builtins::get_all_functions() {
                task_functions.insert(name, handler);
            }
        }

        // Concurrency parameter is now ignored since engine is single-threaded

        Self {
            workflows: Arc::new(workflow_map),
            task_functions: Arc::new(task_functions),
            datalogic: DataLogic::with_preserve_structure(),
            retry_config: retry_config.unwrap_or_default(),
        }
    }

    /// Get the configured retry configuration
    pub fn retry_config(&self) -> &RetryConfig {
        &self.retry_config
    }

    /// Get the configured workflows
    pub fn workflows(&self) -> &HashMap<String, Workflow> {
        &self.workflows
    }

    /// Get the registered task functions
    pub fn task_functions(&self) -> &HashMap<String, Box<dyn FunctionHandler + Send + Sync>> {
        &self.task_functions
    }

    /// Check if a function with the given name is registered
    pub fn has_function(&self, name: &str) -> bool {
        self.task_functions.contains_key(name)
    }

    /// Processes a message through workflows that match their conditions.
    ///
    /// This method:
    /// 1. Iterates through workflows sequentially in deterministic order (sorted by ID)
    /// 2. Evaluates conditions for each workflow right before execution
    /// 3. Executes matching workflows one after another (not concurrently)
    /// 4. Updates the message with processing results and audit trail
    ///
    /// Workflows are executed sequentially because later workflows may depend
    /// on the results of earlier workflows, and their conditions may change
    /// based on modifications made by previous workflows.
    ///
    /// # Arguments
    ///
    /// * `message` - The message to process
    ///
    /// # Returns
    ///
    /// * `Result<()>` - Success or an error if processing failed
    pub fn process_message(&self, message: &mut Message) -> Result<()> {
        debug!(
            "Processing message {} sequentially through workflows",
            message.id
        );

        // Sort workflows by priority and ID to ensure deterministic execution order
        // This prevents non-deterministic behavior caused by HashMap iteration order
        let mut sorted_workflows: Vec<_> = self.workflows.iter().collect();
        sorted_workflows.sort_by_key(|(id, workflow)| (workflow.priority, id.as_str()));

        // Process workflows sequentially in sorted order, evaluating conditions just before execution
        for (_, workflow) in sorted_workflows {
            // Evaluate workflow condition using current message state
            if !self.evaluate_condition(&workflow.condition, &message.metadata)? {
                debug!("Workflow {} skipped - condition not met", workflow.id);
                continue;
            }

            info!("Processing workflow {}", workflow.id);

            self.process_workflow(workflow, message);

            info!("Completed processing workflow {}", workflow.id);

            // If there were errors in this workflow, we may want to decide whether to continue
            // For now, we continue processing remaining workflows even if one fails
        }

        debug!(
            "Completed processing all workflows for message {}",
            message.id
        );
        Ok(())
    }

    fn process_workflow(&self, workflow: &Workflow, message: &mut Message) {
        let workflow_id = workflow.id.clone();
        let mut workflow_errors = Vec::new();

        // Process tasks SEQUENTIALLY within this workflow
        // IMPORTANT: Task order matters! Results from previous tasks are used by subsequent tasks.
        // We intentionally process tasks one after another rather than concurrently.
        for task in &workflow.tasks {
            // Evaluate task condition
            let should_execute = self.evaluate_condition(&task.condition, &message.metadata);

            // Handle condition evaluation result
            let should_execute = match should_execute {
                Ok(result) => result,
                Err(e) => {
                    workflow_errors.push(ErrorInfo::new(
                        Some(workflow_id.clone()),
                        Some(task.id.clone()),
                        e.clone(),
                    ));
                    false
                }
            };

            if !should_execute {
                debug!("Task {} skipped - condition not met", task.id);
                continue;
            }

            // Execute task if we have a handler
            let function_name = task.function.function_name();
            if let Some(function) = self.task_functions.get(function_name) {
                let task_id = task.id.clone();
                let function_config = &task.function;

                // Execute this task (with retries)
                match self.execute_task(
                    &task_id,
                    &workflow_id,
                    message,
                    function_config,
                    function.as_ref(),
                ) {
                    Ok(_) => {
                        debug!("Task {} completed successfully", task_id);
                    }
                    Err(error) => {
                        workflow_errors.push(ErrorInfo::new(
                            Some(workflow_id.clone()),
                            Some(task_id.clone()),
                            error.clone(),
                        ));

                        // Break the task sequence if a task fails
                        break;
                    }
                }
            } else {
                let error =
                    DataflowError::Workflow(format!("Function '{}' not found", function_name));

                workflow_errors.push(ErrorInfo::new(
                    Some(workflow_id.clone()),
                    Some(task.id.clone()),
                    error,
                ));

                // Break the task sequence if a function is not found
                break;
            }
        }

        // Add any errors encountered to the message
        message.errors.extend(workflow_errors);
    }

    /// Helper method to execute a task with retries
    fn execute_task(
        &self,
        task_id: &str,
        workflow_id: &str,
        message: &mut Message,
        config: &FunctionConfig,
        function: &dyn FunctionHandler,
    ) -> Result<()> {
        info!("Executing task {} in workflow {}", task_id, workflow_id);

        let mut last_error = None;
        let mut retry_count = 0;

        // Try executing the task with retries
        while retry_count <= self.retry_config.max_retries {
            match function.execute(message, config, &self.datalogic) {
                Ok((status_code, changes)) => {
                    // Success! Record audit trail and return
                    message.audit_trail.push(AuditTrail {
                        workflow_id: workflow_id.to_string(),
                        task_id: task_id.to_string(),
                        timestamp: Utc::now().to_rfc3339(),
                        changes,
                        status_code,
                    });

                    info!("Task {} completed with status {}", task_id, status_code);

                    // Add progress metadata
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

                    // Check if this error is retryable before attempting retry
                    if retry_count < self.retry_config.max_retries && e.retryable() {
                        warn!(
                            "Task {} execution failed with retryable error, retry {}/{}: {:?}",
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

                        // Sleep using standard thread sleep
                        thread::sleep(Duration::from_millis(delay));

                        retry_count += 1;
                    } else {
                        // Either we've exhausted retries or the error is not retryable
                        if !e.retryable() {
                            info!(
                                "Task {} failed with non-retryable error, skipping retries: {:?}",
                                task_id, e
                            );
                        }
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
            "Task {} in workflow {} failed after {} retries: {:?}",
            task_id, workflow_id, retry_count, error
        );

        Err(error)
    }

    /// Evaluate a condition using the engine's DataLogic
    fn evaluate_condition(&self, condition: &Value, data: &Value) -> Result<bool> {
        // Short-circuit for simple boolean conditions
        if let Value::Bool(b) = condition {
            return Ok(*b);
        }

        self.datalogic
            .evaluate_json(condition, data)
            .map(|result| result.as_bool().unwrap_or(false))
            .map_err(|e| {
                DataflowError::LogicEvaluation(format!("Error evaluating condition: {}", e))
            })
    }
}
