/*!
# Engine Module

This module implements the core workflow engine for dataflow-rs. The engine processes
messages through workflows composed of tasks, providing a flexible and extensible
data processing pipeline.

## Key Components

- **Engine**: The main engine that processes messages through workflows
- **Workflow**: A collection of tasks with conditions that determine when they should be applied
- **Task**: An individual processing unit that performs a specific function on a message
- **AsyncFunctionHandler**: A trait implemented by task handlers to define custom async processing logic
- **Message**: The data structure that flows through the engine, with data, metadata, and processing results
*/

pub mod error;
pub mod functions;
pub mod message;
pub mod task;
pub mod workflow;

// Re-export key types for easier access
pub use error::{DataflowError, ErrorInfo, Result};
pub use functions::AsyncFunctionHandler;
pub use message::Message;
pub use task::Task;
pub use workflow::Workflow;

// Re-export the jsonlogic library under our namespace
pub use datalogic_rs as jsonlogic;

use chrono::Utc;
use datalogic_rs::DataLogic;
use log::{debug, error, info, warn};
use message::AuditTrail;
use serde_json::{json, Map, Number, Value};
use std::{cell::RefCell, collections::HashMap};
use tokio::time::sleep;

// Thread-local DataLogic instance to avoid mutex contention
thread_local! {
    static THREAD_LOCAL_DATA_LOGIC: RefCell<DataLogic> = RefCell::new(DataLogic::new());
}

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

/// Engine that processes messages through workflows using non-blocking async IO.
///
/// This engine is optimized for IO-bound workloads like HTTP requests, database access,
/// and file operations. It uses Tokio for efficient async task execution.
///
/// Workflows are processed sequentially to ensure that later workflows can depend
/// on the results of earlier workflows.
pub struct Engine {
    /// Registry of available workflows
    workflows: HashMap<String, Workflow>,
    /// Registry of function handlers that can be executed by tasks
    task_functions: HashMap<String, Box<dyn AsyncFunctionHandler + Send + Sync>>,
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
    pub fn add_workflow(&mut self, workflow: &Workflow) {
        if workflow.validate().is_ok() {
            self.workflows.insert(workflow.id.clone(), workflow.clone());
        } else {
            error!("Invalid workflow: {}", workflow.id);
        }
    }

    /// Registers a custom function handler with the engine.
    ///
    /// # Arguments
    ///
    /// * `name` - The name of the function handler
    /// * `handler` - The function handler implementation
    pub fn register_task_function(
        &mut self,
        name: String,
        handler: Box<dyn AsyncFunctionHandler + Send + Sync>,
    ) {
        self.task_functions.insert(name, handler);
    }

    /// Check if a function with the given name is registered
    pub fn has_function(&self, name: &str) -> bool {
        self.task_functions.contains_key(name)
    }

    /// Processes a message through workflows that match their conditions.
    ///
    /// This async method:
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
    pub async fn process_message(&self, message: &mut Message) -> Result<()> {
        debug!(
            "Processing message {} sequentially through workflows",
            message.id
        );

        // Collect and sort workflows by ID to ensure deterministic execution order
        // This prevents non-deterministic behavior caused by HashMap iteration order
        let mut sorted_workflows: Vec<_> = self.workflows.iter().collect();
        sorted_workflows.sort_by_key(|(id, _)| id.as_str());

        // Process workflows sequentially in sorted order, evaluating conditions just before execution
        for (_, workflow) in sorted_workflows {
            // Evaluate workflow condition using current message state
            let condition = workflow.condition.clone().unwrap_or(Value::Bool(true));

            if !self
                .evaluate_condition(&condition, &message.metadata)
                .await?
            {
                debug!("Workflow {} skipped - condition not met", workflow.id);
                continue;
            }

            info!("Processing workflow {}", workflow.id);

            // Execute this workflow and merge results back into the message
            let (workflow_id, workflow_message) =
                Self::process_workflow(workflow.clone(), message.clone(), &self.task_functions)
                    .await;

            // Merge workflow results back into the original message
            message.data = workflow_message.data;
            message.metadata = workflow_message.metadata;
            message.temp_data = workflow_message.temp_data;
            message.audit_trail.extend(workflow_message.audit_trail);
            message.errors.extend(workflow_message.errors);

            info!("Completed processing workflow {}", workflow_id);

            // If there were errors in this workflow, we may want to decide whether to continue
            // For now, we continue processing remaining workflows even if one fails
        }

        debug!(
            "Completed processing all workflows for message {}",
            message.id
        );
        Ok(())
    }

    /// Process a single workflow with sequential task execution
    async fn process_workflow(
        workflow: Workflow,
        mut message: Message,
        task_functions: &HashMap<String, Box<dyn AsyncFunctionHandler + Send + Sync>>,
    ) -> (String, Message) {
        let workflow_id = workflow.id.clone();
        let mut workflow_errors = Vec::new();

        // Process tasks SEQUENTIALLY within this workflow
        // IMPORTANT: Task order matters! Results from previous tasks are used by subsequent tasks.
        // We intentionally process tasks one after another rather than concurrently.
        for task in &workflow.tasks {
            let task_condition = task.condition.clone().unwrap_or(Value::Bool(true));

            // Evaluate task condition using thread-local DataLogic
            let should_execute = THREAD_LOCAL_DATA_LOGIC.with(|data_logic_cell| {
                let data_logic = data_logic_cell.borrow_mut();
                data_logic
                    .evaluate_json(&task_condition, &message.metadata, None)
                    .map_err(|e| {
                        DataflowError::LogicEvaluation(format!("Error evaluating condition: {}", e))
                    })
                    .map(|result| result.as_bool().unwrap_or(false))
            });

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
            if let Some(function) = task_functions.get(&task.function.name) {
                let task_id = task.id.clone();
                let function_input = task.function.input.clone();

                // Execute this task (with retries)
                match Self::execute_task_static(
                    &task_id,
                    &workflow_id,
                    &mut message,
                    &function_input,
                    function.as_ref(),
                )
                .await
                {
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
                    DataflowError::Workflow(format!("Function '{}' not found", task.function.name));

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

        // Return the processed message for this workflow
        (workflow_id, message)
    }

    /// Static helper method to execute a task with retries
    async fn execute_task_static(
        task_id: &str,
        workflow_id: &str,
        message: &mut Message,
        input_json: &Value,
        function: &dyn AsyncFunctionHandler,
    ) -> Result<()> {
        info!("Executing task {} in workflow {}", task_id, workflow_id);

        let mut last_error = None;
        let mut retry_count = 0;
        let max_retries = 3; // Default max retries
        let retry_delay_ms = 1000; // Default retry delay in ms
        let use_backoff = true; // Default backoff behavior

        // Try executing the task with retries
        while retry_count <= max_retries {
            match function.execute(message, input_json).await {
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

                    if retry_count < max_retries {
                        warn!(
                            "Task {} execution failed, retry {}/{}: {:?}",
                            task_id,
                            retry_count + 1,
                            max_retries,
                            e
                        );

                        // Calculate delay with optional exponential backoff
                        let delay = if use_backoff {
                            retry_delay_ms * (2_u64.pow(retry_count))
                        } else {
                            retry_delay_ms
                        };

                        // Use tokio's non-blocking sleep
                        sleep(std::time::Duration::from_millis(delay)).await;

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
            "Task {} in workflow {} failed after {} retries: {:?}",
            task_id, workflow_id, retry_count, error
        );

        Err(error)
    }

    /// Evaluates a condition using DataLogic
    async fn evaluate_condition(&self, condition: &Value, data: &Value) -> Result<bool> {
        // For simple boolean conditions, short-circuit
        if let Value::Bool(b) = condition {
            return Ok(*b);
        }

        // Use thread-local DataLogic instance instead of mutex-protected one
        THREAD_LOCAL_DATA_LOGIC.with(|data_logic_cell| {
            let data_logic = data_logic_cell.borrow_mut();
            data_logic
                .evaluate_json(condition, data, None)
                .map_err(|e| {
                    DataflowError::LogicEvaluation(format!("Error evaluating condition: {}", e))
                })
                .map(|result| result.as_bool().unwrap_or(false))
        })
    }
}
