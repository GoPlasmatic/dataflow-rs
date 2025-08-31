/*!
# Engine Module

This module implements the core workflow engine for dataflow-rs. The engine provides
thread-safe, vertically-scalable message processing through workflows composed of tasks.

## Thread-Safety & Concurrency (v1.0)

The engine now features a unified concurrency model with:
- **DataLogic Pool**: Thread-safe pool of DataLogic instances for JSONLogic evaluation
- **Arc-Swap Workflows**: Lock-free reads and atomic updates for workflow management
- **Unified Concurrency**: Single parameter controls both pool size and max concurrent messages
- **Zero Contention**: Pool size matches concurrent tasks to eliminate resource competition

## Key Components

- **Engine**: Thread-safe engine with configurable concurrency levels
- **Workflow**: Collection of tasks with JSONLogic conditions, stored using Arc-Swap
- **Task**: Individual processing unit that performs a specific function on a message
- **AsyncFunctionHandler**: Trait for custom async processing logic (now receives DataLogic parameter)
- **Message**: Data structure flowing through the engine, with dedicated DataLogic instance per workflow
- **DataLogicPool**: Pool of DataLogic instances for concurrent message processing

## Usage

```rust,no_run
use dataflow_rs::{Engine, engine::message::Message};
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create engine with default concurrency (CPU count)
    let engine = Engine::new();

    // Or specify custom concurrency level
    let engine = Engine::with_concurrency(32);

    // Process messages concurrently
    let mut message = Message::new(&json!({}));
    engine.process_message_concurrent(&mut message).await?;

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
pub use functions::AsyncFunctionHandler;
pub use message::Message;
pub use task::Task;
pub use workflow::Workflow;

// Re-export the jsonlogic library under our namespace
pub use datalogic_rs as jsonlogic;

use arc_swap::ArcSwap;
use chrono::Utc;
use datalogic_rs::DataLogic;
use log::{debug, error, info, warn};
use message::AuditTrail;
use serde_json::{Map, Number, Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::{Mutex, Semaphore};
use tokio::time::sleep;

/// DataLogic pool for thread-safe concurrent access
pub struct DataLogicPool {
    instances: Vec<Arc<Mutex<DataLogic>>>,
    next: AtomicUsize,
}

impl DataLogicPool {
    pub fn new(size: usize) -> Self {
        let instances = (0..size)
            .map(|_| Arc::new(Mutex::new(DataLogic::with_preserve_structure())))
            .collect();

        Self {
            instances,
            next: AtomicUsize::new(0),
        }
    }

    pub async fn with_instance<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut DataLogic) -> R,
    {
        // Round-robin selection for load distribution
        let index = self.next.fetch_add(1, Ordering::Relaxed) % self.instances.len();
        let instance = &self.instances[index];

        let mut guard = instance.lock().await;
        guard.reset_arena(); // Clean state before use
        f(&mut guard)
    }

    /// Get a DataLogic instance for exclusive use within a workflow
    pub fn get_instance(&self) -> Arc<Mutex<DataLogic>> {
        // Round-robin selection for load distribution
        let index = self.next.fetch_add(1, Ordering::Relaxed) % self.instances.len();
        self.instances[index].clone()
    }
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

/// Thread-safe engine that processes messages through workflows using non-blocking async IO.
///
/// ## Architecture
///
/// The engine is optimized for both IO-bound and CPU-bound workloads, featuring:
/// - **Vertical Scalability**: Automatically utilizes all available CPU cores
/// - **Thread-Safe Design**: All components are Send + Sync for concurrent access
/// - **Unified Concurrency**: Single parameter controls both DataLogic pool size and max concurrent messages
///
/// ## Concurrency Model
///
/// Each message receives exclusive access to a DataLogic instance for its entire workflow execution,
/// eliminating lock contention between tasks while maintaining thread-safety across messages.
///
/// ## Performance
///
/// The engine achieves linear scalability with CPU cores, capable of processing millions of
/// messages per second with appropriate concurrency settings.
pub struct Engine {
    /// Registry of available workflows (using ArcSwap for atomic reloads)
    workflows: Arc<ArcSwap<HashMap<String, Workflow>>>,
    /// Registry of function handlers that can be executed by tasks
    task_functions: HashMap<String, Box<dyn AsyncFunctionHandler + Send + Sync>>,
    /// DataLogic pool for thread-safe access
    datalogic_pool: Arc<DataLogicPool>,
    /// Semaphore to limit concurrent message processing
    concurrency_limiter: Arc<Semaphore>,
    /// Maximum concurrency level (pool size and max concurrent messages)
    concurrency: usize,
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
        // Default concurrency based on available CPU cores
        let concurrency = std::thread::available_parallelism()
            .map(|n| n.get() * 2) // 2x CPU cores
            .unwrap_or(8); // Fallback to 8

        Self::with_concurrency(concurrency)
    }

    /// Create a new engine with a specific concurrency level.
    /// This sets both the DataLogic pool size and the maximum concurrent messages.
    ///
    /// # Arguments
    /// * `concurrency` - Maximum number of messages that can be processed concurrently
    pub fn with_concurrency(concurrency: usize) -> Self {
        let mut engine = Self {
            workflows: Arc::new(ArcSwap::from_pointee(HashMap::new())),
            task_functions: HashMap::new(),
            datalogic_pool: Arc::new(DataLogicPool::new(concurrency)),
            concurrency_limiter: Arc::new(Semaphore::new(concurrency)),
            concurrency,
            retry_config: RetryConfig::default(),
        };

        // Register built-in function handlers
        for (name, handler) in functions::builtins::get_all_functions() {
            engine.register_task_function(name, handler);
        }

        engine
    }

    /// Create a new engine with a specific pool size (deprecated, use with_concurrency)
    #[deprecated(since = "1.0.0", note = "Use with_concurrency instead")]
    pub fn with_pool_size(pool_size: usize) -> Self {
        Self::with_concurrency(pool_size)
    }

    /// Create a new engine instance without any pre-registered functions
    pub fn new_empty() -> Self {
        let concurrency = std::thread::available_parallelism()
            .map(|n| n.get() * 2)
            .unwrap_or(8);

        Self {
            workflows: Arc::new(ArcSwap::from_pointee(HashMap::new())),
            task_functions: HashMap::new(),
            datalogic_pool: Arc::new(DataLogicPool::new(concurrency)),
            concurrency_limiter: Arc::new(Semaphore::new(concurrency)),
            concurrency,
            retry_config: RetryConfig::default(),
        }
    }

    /// Get the configured concurrency level
    pub fn concurrency(&self) -> usize {
        self.concurrency
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
    pub fn add_workflow(&self, workflow: &Workflow) {
        if workflow.validate().is_ok() {
            let current = self.workflows.load();
            let mut new_workflows = HashMap::clone(&current);
            new_workflows.insert(workflow.id.clone(), workflow.clone());
            self.workflows.store(Arc::new(new_workflows));
        } else {
            error!("Invalid workflow: {}", workflow.id);
        }
    }

    /// Reload all workflows atomically
    pub fn reload_workflows(&self, new_workflows: Vec<Workflow>) -> Result<()> {
        let mut workflow_map = HashMap::new();

        for workflow in new_workflows {
            if workflow.validate().is_ok() {
                workflow_map.insert(workflow.id.clone(), workflow);
            } else {
                warn!("Skipping invalid workflow: {}", workflow.id);
            }
        }

        // Atomic swap for zero-cost reads
        self.workflows.store(Arc::new(workflow_map));
        info!("Reloaded {} workflows", self.workflows.load().len());
        Ok(())
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

        // Load workflows atomically and sort by ID to ensure deterministic execution order
        // This prevents non-deterministic behavior caused by HashMap iteration order
        let workflows = self.workflows.load();
        let mut sorted_workflows: Vec<_> = workflows.iter().collect();
        sorted_workflows.sort_by_key(|(id, workflow)| (workflow.priority, id.as_str()));

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

            Self::process_workflow(
                workflow,
                message,
                &self.task_functions,
                &self.datalogic_pool,
                &self.retry_config,
            )
            .await;

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

    /// Process a message with automatic concurrency control.
    /// This method will wait if the maximum concurrency level is reached.
    ///
    /// Use this when spawning concurrent tasks to ensure the engine's
    /// concurrency limit is respected.
    ///
    /// # Example
    /// ```ignore
    /// use tokio::task::JoinSet;
    /// let mut tasks = JoinSet::new();
    ///
    /// for msg in messages {
    ///     let engine = engine.clone();
    ///     tasks.spawn(async move {
    ///         engine.process_message_concurrent(msg).await
    ///     });
    /// }
    /// ```
    pub async fn process_message_concurrent(&self, message: &mut Message) -> Result<()> {
        // Acquire a permit from the semaphore to limit concurrency
        let _permit = self.concurrency_limiter.acquire().await.map_err(|e| {
            DataflowError::Unknown(format!("Failed to acquire concurrency permit: {}", e))
        })?;

        // Process the message while holding the permit
        // The permit is automatically released when dropped
        self.process_message(message).await
    }

    /// Process a single workflow with sequential task execution
    async fn process_workflow(
        workflow: &Workflow,
        message: &mut Message,
        task_functions: &HashMap<String, Box<dyn AsyncFunctionHandler + Send + Sync>>,
        datalogic_pool: &DataLogicPool,
        retry_config: &RetryConfig,
    ) {
        let workflow_id = workflow.id.clone();
        let mut workflow_errors = Vec::new();

        // Acquire a DataLogic instance for the entire workflow processing
        // This avoids repeated lock acquisitions for each task
        let data_logic_instance = datalogic_pool.get_instance();
        let mut data_logic = data_logic_instance.lock().await;

        // Process tasks SEQUENTIALLY within this workflow
        // IMPORTANT: Task order matters! Results from previous tasks are used by subsequent tasks.
        // We intentionally process tasks one after another rather than concurrently.
        for task in &workflow.tasks {
            let task_condition = task.condition.clone().unwrap_or(Value::Bool(true));

            // Evaluate task condition using the acquired DataLogic instance
            let should_execute = data_logic
                .evaluate_json(&task_condition, &message.metadata, None)
                .map_err(|e| {
                    DataflowError::LogicEvaluation(format!("Error evaluating condition: {e}"))
                })
                .map(|result| result.as_bool().unwrap_or(false));

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
                    message,
                    &function_input,
                    function.as_ref(),
                    &mut data_logic,
                    retry_config,
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
    }

    /// Static helper method to execute a task with retries
    async fn execute_task_static(
        task_id: &str,
        workflow_id: &str,
        message: &mut Message,
        input_json: &Value,
        function: &dyn AsyncFunctionHandler,
        data_logic: &mut DataLogic,
        retry_config: &RetryConfig,
    ) -> Result<()> {
        info!("Executing task {} in workflow {}", task_id, workflow_id);

        let mut last_error = None;
        let mut retry_count = 0;

        // Try executing the task with retries
        while retry_count <= retry_config.max_retries {
            match function.execute(message, input_json, data_logic).await {
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
                    if retry_count < retry_config.max_retries && e.retryable() {
                        warn!(
                            "Task {} execution failed with retryable error, retry {}/{}: {:?}",
                            task_id,
                            retry_count + 1,
                            retry_config.max_retries,
                            e
                        );

                        // Calculate delay with optional exponential backoff
                        let delay = if retry_config.use_backoff {
                            retry_config.retry_delay_ms * (2_u64.pow(retry_count))
                        } else {
                            retry_config.retry_delay_ms
                        };

                        // Use tokio's non-blocking sleep
                        sleep(std::time::Duration::from_millis(delay)).await;

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

    /// Evaluates a condition using DataLogic
    async fn evaluate_condition(&self, condition: &Value, data: &Value) -> Result<bool> {
        // For simple boolean conditions, short-circuit
        if let Value::Bool(b) = condition {
            return Ok(*b);
        }

        // Use pooled DataLogic instance
        self.datalogic_pool
            .with_instance(|data_logic| {
                data_logic
                    .evaluate_json(condition, data, None)
                    .map_err(|e| {
                        DataflowError::LogicEvaluation(format!("Error evaluating condition: {e}"))
                    })
                    .map(|result| result.as_bool().unwrap_or(false))
            })
            .await
    }
}
