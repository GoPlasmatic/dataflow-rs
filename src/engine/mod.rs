/*!
# Engine Module

This module implements the core workflow engine for dataflow-rs. The engine provides
high-performance, asynchronous message processing through workflows composed of tasks.

## Architecture

The engine features a modular architecture with clear separation of concerns:
- **Compiler**: Pre-compiles JSONLogic expressions for optimal runtime performance
- **Executor**: Handles internal function execution (map, validation) efficiently
- **Engine**: Orchestrates workflow processing with immutable, pre-configured workflows
- **Direct DataLogic**: Each engine instance has its own DataLogic for zero contention

## Key Components

- **Engine**: Single-threaded engine optimized for both IO-bound and CPU-bound workloads
- **LogicCompiler**: Compiles and caches JSONLogic expressions during initialization
- **InternalExecutor**: Executes built-in map and validation functions with compiled logic
- **Workflow**: Collection of tasks with JSONLogic conditions (metadata-only access)
- **Task**: Individual processing unit that performs a specific function on a message
- **FunctionHandler**: Trait for custom processing logic implementation
- **Message**: Data structure flowing through the engine with audit trail

## Performance Optimizations

- **Pre-compilation**: All JSONLogic expressions compiled at startup
- **Direct Instantiation**: DataLogic instances created directly, avoiding any locking
- **Immutable Workflows**: Workflows defined at initialization for predictable performance
- **Efficient Caching**: Compiled logic cached for fast repeated evaluations

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
    let mut engine = Engine::new(workflows.clone(), None, None);

    // Process messages
    let mut message = Message::new(&json!({}));
    engine.process_message(&mut message)?;

    Ok(())
}
```
*/

pub mod compiler;
pub mod error;
pub mod executor;
pub mod functions;
pub mod message;
pub mod rayon;
pub mod retry;
pub mod task;
pub mod threaded;
pub mod workflow;

// Re-export key types for easier access
pub use error::{DataflowError, ErrorInfo, Result};
pub use functions::{FunctionConfig, FunctionHandler};
pub use message::Message;
pub use rayon::RayonEngine;
pub use retry::RetryConfig;
pub use task::Task;
pub use threaded::ThreadedEngine;
pub use workflow::Workflow;

use chrono::Utc;
use datalogic_rs::{DataLogic, Logic};
use log::{debug, error, info, warn};
use message::{AuditTrail, Change};
use serde_json::{Map, Number, Value, json};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use compiler::LogicCompiler;
use executor::InternalExecutor;

/// High-performance workflow engine for message processing.
///
/// ## Architecture
///
/// The engine features a modular design optimized for both IO-bound and CPU-bound workloads:
/// - **Separation of Concerns**: Compiler handles pre-compilation, Executor handles runtime
/// - **Direct DataLogic**: Single DataLogic instance per engine for zero contention
/// - **Immutable Workflows**: All workflows compiled and cached at initialization
/// - **Pre-compiled Logic**: JSONLogic expressions compiled once for optimal performance
///
/// ## Performance Characteristics
///
/// - **Zero Runtime Compilation**: All logic compiled during initialization
/// - **Cache-Friendly**: Compiled logic stored in contiguous memory
/// - **Predictable Latency**: No runtime allocations for logic evaluation
/// - **Thread-Safe Design**: Applications can safely use multiple engine instances across threads
pub struct Engine {
    /// Registry of available workflows (immutable after initialization)
    workflows: Arc<HashMap<String, Workflow>>,
    /// Registry of function handlers that can be executed by tasks (immutable after initialization)
    task_functions: Arc<HashMap<String, Box<dyn FunctionHandler + Send + Sync>>>,
    /// DataLogic instance for JSONLogic evaluation (wrapped in RefCell for interior mutability)
    datalogic: RefCell<DataLogic<'static>>,
    /// Compiled logic cache
    logic_cache: Vec<Logic<'static>>,
    /// Configuration for retry behavior
    retry_config: RetryConfig,
}

impl Default for Engine {
    fn default() -> Self {
        Self::new(Vec::new(), None, None)
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
    /// let mut engine = Engine::new(workflows.clone(), None, None);
    ///
    /// ```
    pub fn new(
        workflows: Vec<Workflow>,
        custom_functions: Option<HashMap<String, Box<dyn FunctionHandler + Send + Sync>>>,
        retry_config: Option<RetryConfig>,
    ) -> Self {
        // Compile workflows
        let mut compiler = LogicCompiler::new();
        let workflow_map = compiler.compile_workflows(workflows);
        let (datalogic, logic_cache) = compiler.into_parts();

        let mut task_functions = custom_functions.unwrap_or_default();

        // Add built-in function handlers if requested (defaults to true)
        for (name, handler) in functions::builtins::get_all_functions() {
            task_functions.insert(name, handler);
        }

        Self {
            workflows: Arc::new(workflow_map),
            task_functions: Arc::new(task_functions),
            datalogic: RefCell::new(datalogic),
            logic_cache,
            retry_config: retry_config.unwrap_or_default(),
        }
    }

    /// Creates a new Engine instance with shared function handlers.
    /// This is useful when creating multiple engine instances that share the same function registry.
    ///
    /// # Arguments
    /// * `workflows` - The workflows to use for processing messages
    /// * `task_functions` - Shared function handlers wrapped in Arc
    /// * `retry_config` - Optional retry configuration (uses default if None)
    pub fn new_with_shared_functions(
        workflows: Vec<Workflow>,
        task_functions: Arc<HashMap<String, Box<dyn FunctionHandler + Send + Sync>>>,
        retry_config: Option<RetryConfig>,
    ) -> Self {
        // Compile workflows
        let mut compiler = LogicCompiler::new();
        let workflow_map = compiler.compile_workflows(workflows);
        let (datalogic, logic_cache) = compiler.into_parts();

        Self {
            workflows: Arc::new(workflow_map),
            task_functions,
            datalogic: RefCell::new(datalogic),
            logic_cache,
            retry_config: retry_config.unwrap_or_default(),
        }
    }

    /// Processes a message through workflows that match their conditions.
    ///
    /// This method:
    /// 1. Iterates through workflows sequentially in deterministic order (sorted by ID)
    /// 2. Evaluates conditions for each workflow right before execution
    /// 3. Executes matching workflows one after another (not concurrently)
    /// 4. Updates the message with processing results and audit trail
    /// 5. Clears the evaluation arena after processing to prevent memory leaks
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
    pub fn process_message(&mut self, message: &mut Message) -> Result<()> {
        debug!(
            "Processing message {} sequentially through workflows",
            message.id
        );

        // Sort workflows by priority and ID to ensure deterministic execution order
        let mut sorted_workflows: Vec<_> = self.workflows.iter().collect();
        sorted_workflows.sort_by_key(|(id, workflow)| (workflow.priority, id.as_str()));

        // Process workflows sequentially in sorted order
        for (_, workflow) in sorted_workflows {
            // Evaluate workflow condition using current message state
            let should_process = {
                let datalogic = self.datalogic.borrow();
                let executor = InternalExecutor::new(&datalogic, &self.logic_cache);
                executor.evaluate_condition(
                    workflow.condition_index,
                    &workflow.condition,
                    &message.metadata,
                )?
            };

            if !should_process {
                debug!("Workflow {} skipped - condition not met", workflow.id);
                continue;
            }

            info!("Processing workflow {}", workflow.id);
            self.process_workflow(workflow, message);
            info!("Completed processing workflow {}", workflow.id);
        }

        debug!(
            "Completed processing all workflows for message {}",
            message.id
        );

        // Clear the evaluation arena to free memory allocated during message processing
        // This prevents memory leaks from accumulating across multiple message processing calls
        self.datalogic.borrow_mut().reset_eval_arena();

        Ok(())
    }

    fn process_workflow(&self, workflow: &Workflow, message: &mut Message) {
        let workflow_id = workflow.id.clone();
        let mut workflow_errors = Vec::new();

        // Cache timestamp for this workflow execution to reduce clock_gettime calls
        let workflow_timestamp = Utc::now().to_rfc3339();

        // Process tasks SEQUENTIALLY within this workflow
        for task in &workflow.tasks {
            // Evaluate task condition
            let should_execute = {
                let datalogic = self.datalogic.borrow();
                let executor = InternalExecutor::new(&datalogic, &self.logic_cache);
                executor.evaluate_condition(
                    task.condition_index,
                    &task.condition,
                    &message.metadata,
                )
            };

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

            // Execute task based on its type
            let task_id = task.id.clone();
            let function_config = &task.function;

            let execution_result = match function_config {
                FunctionConfig::Map { input, .. } => {
                    let datalogic = self.datalogic.borrow();
                    let executor = InternalExecutor::new(&datalogic, &self.logic_cache);
                    executor.execute_map(message, input)
                }
                FunctionConfig::Validation { input, .. } => {
                    let datalogic = self.datalogic.borrow();
                    let executor = InternalExecutor::new(&datalogic, &self.logic_cache);
                    executor.execute_validate(message, input)
                }
                FunctionConfig::Custom { name, .. } => {
                    if let Some(function) = self.task_functions.get(name) {
                        self.execute_task(
                            &task_id,
                            &workflow_id,
                            message,
                            function_config,
                            function.as_ref(),
                        )
                    } else {
                        Err(DataflowError::Workflow(format!(
                            "Function '{}' not found",
                            name
                        )))
                    }
                }
            };

            // Handle execution result
            match execution_result {
                Ok((status_code, changes)) => {
                    debug!(
                        "Task {} completed successfully with status {}",
                        task_id, status_code
                    );

                    // Record audit trail using cached timestamp
                    message.audit_trail.push(AuditTrail {
                        workflow_id: workflow_id.to_string(),
                        task_id: task_id.to_string(),
                        timestamp: workflow_timestamp.clone(),
                        changes,
                        status_code,
                    });

                    // Add progress metadata with cached timestamp
                    self.update_progress_metadata_with_timestamp(
                        message,
                        &task_id,
                        &workflow_id,
                        status_code,
                        &workflow_timestamp,
                    );
                }
                Err(error) => {
                    workflow_errors.push(ErrorInfo::new(
                        Some(workflow_id.clone()),
                        Some(task_id.clone()),
                        error.clone(),
                    ));
                    break; // Break the task sequence if a task fails
                }
            }
        }

        // Add any errors encountered to the message
        message.errors.extend(workflow_errors);
    }

    /// Execute a custom task with retries
    fn execute_task(
        &self,
        task_id: &str,
        workflow_id: &str,
        message: &mut Message,
        config: &FunctionConfig,
        function: &dyn FunctionHandler,
    ) -> Result<(usize, Vec<Change>)> {
        info!("Executing task {} in workflow {}", task_id, workflow_id);

        let mut last_error = None;
        let mut retry_count = 0;

        // Try executing the task with retries
        while retry_count <= self.retry_config.max_retries {
            match function.execute(message, config, &self.datalogic.borrow()) {
                Ok((status_code, changes)) => {
                    info!("Task {} completed with status {}", task_id, status_code);

                    if retry_count > 0 {
                        self.update_progress_metadata_with_retries(
                            message,
                            task_id,
                            workflow_id,
                            status_code,
                            retry_count,
                        );
                    } else {
                        self.update_progress_metadata(message, task_id, workflow_id, status_code);
                    }

                    return Ok((status_code, changes));
                }
                Err(e) => {
                    last_error = Some(e.clone());

                    // Check if this error is retryable
                    if retry_count < self.retry_config.max_retries && e.retryable() {
                        warn!(
                            "Task {} execution failed with retryable error, retry {}/{}: {:?}",
                            task_id,
                            retry_count + 1,
                            self.retry_config.max_retries,
                            e
                        );

                        self.retry_config.sleep(retry_count);
                        retry_count += 1;
                    } else {
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

    /// Update progress metadata
    fn update_progress_metadata(
        &self,
        message: &mut Message,
        task_id: &str,
        workflow_id: &str,
        status_code: usize,
    ) {
        let timestamp = Utc::now().to_rfc3339();
        self.update_progress_metadata_with_timestamp(
            message,
            task_id,
            workflow_id,
            status_code,
            &timestamp,
        );
    }

    /// Update progress metadata with provided timestamp
    fn update_progress_metadata_with_timestamp(
        &self,
        message: &mut Message,
        task_id: &str,
        workflow_id: &str,
        status_code: usize,
        timestamp: &str,
    ) {
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
            Value::String(timestamp.to_string()),
        );
        message.metadata["progress"] = json!(progress);
    }

    /// Update progress metadata with retry count
    fn update_progress_metadata_with_retries(
        &self,
        message: &mut Message,
        task_id: &str,
        workflow_id: &str,
        status_code: usize,
        retry_count: u32,
    ) {
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
        progress.insert(
            "retries".to_string(),
            Value::Number(Number::from(retry_count)),
        );
        message.metadata["progress"] = json!(progress);
    }
}
