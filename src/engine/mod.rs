/*!
# Engine Module

This module implements the core async workflow engine for dataflow-rs. The engine provides
high-performance, asynchronous message processing through workflows composed of tasks.

## Architecture

The engine features a clean async-first architecture with DataLogic v4:
- **Compiler**: Pre-compiles JSONLogic expressions using DataLogic v4's Arc<CompiledLogic>
- **Executor**: Handles internal function execution (map, validation) with async support
- **Engine**: Orchestrates workflow processing with shared compiled logic
- **Thread-Safe**: Single DataLogic instance with Arc-wrapped compiled logic for zero-copy sharing

## Key Components

- **Engine**: Async engine optimized for Tokio runtime with mixed I/O and CPU workloads
- **LogicCompiler**: Compiles and caches JSONLogic expressions during initialization
- **InternalExecutor**: Executes built-in map and validation functions with compiled logic
- **Workflow**: Collection of tasks with JSONLogic conditions (can access data, metadata, temp_data)
- **Task**: Individual processing unit that performs a specific function on a message
- **AsyncFunctionHandler**: Trait for custom async processing logic
- **Message**: Data structure flowing through the engine with audit trail

## Performance Optimizations

- **Pre-compilation**: All JSONLogic expressions compiled at startup
- **Arc-wrapped Logic**: Zero-copy sharing of compiled logic across async tasks
- **Spawn Blocking**: CPU-intensive JSONLogic evaluation in blocking tasks
- **True Async**: I/O operations remain fully async

## Usage

```rust,no_run
use dataflow_rs::{Engine, Workflow, engine::message::Message};
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Define workflows
    let workflows = vec![
        Workflow::from_json(r#"{"id": "example", "name": "Example", "tasks": [{"id": "task1", "name": "Task 1", "function": {"name": "map", "input": {"mappings": []}}}]}"#)?
    ];

    // Create engine with defaults
    let engine = Engine::new(workflows, None);

    // Process messages asynchronously
    let mut message = Message::from_value(&json!({}));
    engine.process_message(&mut message).await?;

    Ok(())
}
```
*/

pub mod compiler;
pub mod error;
pub mod executor;
pub mod functions;
pub mod message;
pub mod task;
pub mod task_executor;
pub mod trace;
pub mod utils;
pub mod workflow;
pub mod workflow_executor;

// Re-export key types for easier access
pub use error::{DataflowError, ErrorInfo, Result};
pub use functions::{AsyncFunctionHandler, FunctionConfig};
pub use message::Message;
pub use task::Task;
pub use trace::{ExecutionStep, ExecutionTrace, StepResult};
pub use workflow::{Workflow, WorkflowStatus};

use chrono::Utc;
use datalogic_rs::{Engine as DatalogicEngine, Logic};
use datavalue::OwnedDataValue;
use std::collections::HashMap;
use std::sync::Arc;

use compiler::LogicCompiler;
use executor::InternalExecutor;
use task_executor::TaskExecutor;
use utils::set_nested_value;
use workflow_executor::WorkflowExecutor;

/// High-performance async workflow engine for message processing.
///
/// ## Architecture
///
/// The engine is designed for async-first operation with Tokio:
/// - **Separation of Concerns**: Distinct executors for workflows and tasks
/// - **Shared DataLogic**: Single DataLogic instance with Arc for thread-safe sharing
/// - **Arc<CompiledLogic>**: Pre-compiled logic shared across all async tasks
/// - **Async Functions**: Native async support for I/O-bound operations
///
/// ## Performance Characteristics
///
/// - **Zero Runtime Compilation**: All logic compiled during initialization
/// - **Zero-Copy Sharing**: Arc-wrapped compiled logic shared without cloning
/// - **Optimal for Mixed Workloads**: Async I/O with blocking CPU evaluation
/// - **Thread-Safe by Design**: All components safe to share across Tokio tasks
pub struct Engine {
    /// Registry of available workflows, pre-sorted by priority (immutable after initialization)
    workflows: Arc<Vec<Workflow>>,
    /// Channel index: maps channel name -> indices into workflows vec (only Active workflows)
    channel_index: Arc<HashMap<String, Vec<usize>>>,
    /// Workflow executor for orchestrating workflow execution
    workflow_executor: Arc<WorkflowExecutor>,
    /// Shared datalogic v5 engine for JSONLogic evaluation (Send + Sync)
    datalogic: Arc<DatalogicEngine>,
    /// Compiled logic cache with Arc<Logic> for zero-copy cross-thread sharing
    logic_cache: Vec<Arc<Logic>>,
}

/// Build a channel index from pre-sorted workflows.
/// Maps channel name -> indices into workflows vec, only for Active workflows.
fn build_channel_index(workflows: &[Workflow]) -> HashMap<String, Vec<usize>> {
    let mut index: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, workflow) in workflows.iter().enumerate() {
        if workflow.status == WorkflowStatus::Active {
            index.entry(workflow.channel.clone()).or_default().push(i);
        }
    }
    index
}

impl Engine {
    /// Creates a new Engine instance with configurable parameters.
    ///
    /// # Arguments
    /// * `workflows` - The workflows to use for processing messages
    /// * `custom_functions` - Optional custom async function handlers
    ///
    /// # Example
    ///
    /// ```
    /// use dataflow_rs::{Engine, Workflow};
    ///
    /// let workflows = vec![Workflow::from_json(r#"{"id": "test", "name": "Test", "priority": 0, "tasks": [{"id": "task1", "name": "Task 1", "function": {"name": "map", "input": {"mappings": []}}}]}"#).unwrap()];
    ///
    /// // Simple usage with defaults
    /// let engine = Engine::new(workflows, None);
    /// ```
    pub fn new(
        workflows: Vec<Workflow>,
        custom_functions: Option<HashMap<String, Box<dyn AsyncFunctionHandler + Send + Sync>>>,
    ) -> Self {
        // Compile workflows with DataLogic v4 (sorted by priority at compile time)
        let mut compiler = LogicCompiler::new();
        let sorted_workflows = compiler.compile_workflows(workflows);
        let (datalogic, logic_cache) = compiler.into_parts();

        let mut task_functions = custom_functions.unwrap_or_default();

        // Add built-in async function handlers
        for (name, handler) in functions::builtins::get_all_functions() {
            task_functions.insert(name, handler);
        }

        // Create internal executor with shared DataLogic and compiled logic
        let internal_executor = Arc::new(InternalExecutor::new(
            Arc::clone(&datalogic),
            logic_cache.clone(),
        ));

        // Create task executor
        let task_executor = Arc::new(TaskExecutor::new(
            Arc::new(task_functions),
            Arc::clone(&internal_executor),
            Arc::clone(&datalogic),
        ));

        // Create workflow executor
        let workflow_executor = Arc::new(WorkflowExecutor::new(task_executor, internal_executor));

        // Build channel index for O(1) channel-based routing
        let channel_index = build_channel_index(&sorted_workflows);

        Self {
            workflows: Arc::new(sorted_workflows),
            channel_index: Arc::new(channel_index),
            workflow_executor,
            datalogic,
            logic_cache,
        }
    }

    /// Creates a new Engine with different workflows but the same custom function handlers.
    ///
    /// This is the hot-reload path. The existing engine remains valid for any
    /// in-flight `process_message` calls. The returned engine shares the same
    /// function registry (zero-copy Arc bump) but has freshly compiled logic
    /// for the new workflow set.
    ///
    /// # Arguments
    /// * `workflows` - The new set of workflows to compile and use
    pub fn with_new_workflows(&self, workflows: Vec<Workflow>) -> Self {
        // Extract the shared function registry from the existing executor
        let task_functions = self.workflow_executor.task_functions();

        // Compile new workflows with a fresh DataLogic instance
        let mut compiler = LogicCompiler::new();
        let sorted_workflows = compiler.compile_workflows(workflows);
        let (datalogic, logic_cache) = compiler.into_parts();

        // Rebuild the executor stack, reusing the existing function registry
        let internal_executor = Arc::new(InternalExecutor::new(
            Arc::clone(&datalogic),
            logic_cache.clone(),
        ));

        let task_executor = Arc::new(TaskExecutor::new(
            task_functions,
            Arc::clone(&internal_executor),
            Arc::clone(&datalogic),
        ));

        let workflow_executor = Arc::new(WorkflowExecutor::new(task_executor, internal_executor));

        // Build channel index for O(1) channel-based routing
        let channel_index = build_channel_index(&sorted_workflows);

        Self {
            workflows: Arc::new(sorted_workflows),
            channel_index: Arc::new(channel_index),
            workflow_executor,
            datalogic,
            logic_cache,
        }
    }

    /// Processes a message through workflows that match their conditions.
    ///
    /// This async method:
    /// 1. Iterates through workflows sequentially in priority order (pre-sorted at construction)
    /// 2. Delegates workflow execution to the WorkflowExecutor
    /// 3. Updates message metadata
    ///
    /// # Arguments
    /// * `message` - The message to process through workflows
    ///
    /// # Returns
    /// * `Result<()>` - Ok(()) if processing succeeded, Err if a fatal error occurred
    pub async fn process_message(&self, message: &mut Message) -> Result<()> {
        // Set processing metadata
        set_processing_metadata(&mut message.context, None);

        // Process each workflow in priority order (pre-sorted at construction)
        for workflow in self.workflows.iter() {
            self.workflow_executor.execute(workflow, message).await?;
        }

        Ok(())
    }

    /// Processes a message through workflows with step-by-step tracing.
    ///
    /// This method is similar to `process_message` but captures an execution trace
    /// that can be used for debugging and step-by-step visualization.
    ///
    /// # Arguments
    /// * `message` - The message to process through workflows
    ///
    /// # Returns
    /// * `Result<ExecutionTrace>` - The execution trace with message snapshots
    pub async fn process_message_with_trace(
        &self,
        message: &mut Message,
    ) -> Result<ExecutionTrace> {
        use trace::ExecutionTrace;

        // Set processing metadata
        set_processing_metadata(&mut message.context, None);

        let mut trace = ExecutionTrace::new();

        // Process each workflow in priority order (pre-sorted at construction)
        for workflow in self.workflows.iter() {
            self.workflow_executor
                .execute_with_trace(workflow, message, &mut trace)
                .await?;
        }

        Ok(trace)
    }

    /// Processes a message through only the Active workflows registered for a given channel.
    ///
    /// Workflows are processed in priority order (lowest first), same as process_message().
    /// If the channel does not exist or has no Active workflows, this is a no-op.
    ///
    /// # Arguments
    /// * `channel` - The channel name to route the message through
    /// * `message` - The message to process
    pub async fn process_message_for_channel(
        &self,
        channel: &str,
        message: &mut Message,
    ) -> Result<()> {
        set_processing_metadata(&mut message.context, Some(channel));

        if let Some(indices) = self.channel_index.get(channel) {
            for &idx in indices {
                self.workflow_executor
                    .execute(&self.workflows[idx], message)
                    .await?;
            }
        }

        Ok(())
    }

    /// Processes a message through a channel with step-by-step tracing.
    ///
    /// # Arguments
    /// * `channel` - The channel name to route the message through
    /// * `message` - The message to process
    pub async fn process_message_for_channel_with_trace(
        &self,
        channel: &str,
        message: &mut Message,
    ) -> Result<ExecutionTrace> {
        use trace::ExecutionTrace;

        set_processing_metadata(&mut message.context, Some(channel));

        let mut trace = ExecutionTrace::new();

        if let Some(indices) = self.channel_index.get(channel) {
            for &idx in indices {
                self.workflow_executor
                    .execute_with_trace(&self.workflows[idx], message, &mut trace)
                    .await?;
            }
        }

        Ok(trace)
    }

    /// Get a reference to the workflows (pre-sorted by priority)
    pub fn workflows(&self) -> &Arc<Vec<Workflow>> {
        &self.workflows
    }

    /// Look up a workflow by its ID
    pub fn workflow_by_id(&self, id: &str) -> Option<&Workflow> {
        self.workflows.iter().find(|w| w.id == id)
    }

    /// Get a reference to the underlying datalogic v5 engine.
    pub fn datalogic(&self) -> &Arc<DatalogicEngine> {
        &self.datalogic
    }

    /// Get a reference to the compiled logic cache (Arc<Logic> entries).
    pub fn logic_cache(&self) -> &Vec<Arc<Logic>> {
        &self.logic_cache
    }
}

/// Stamp the standard processing metadata (`processed_at`, `engine_version`,
/// and optionally `channel`) into the message context. Uses
/// `set_nested_value` so callers can stay agnostic of the OwnedDataValue
/// mutation surface.
fn set_processing_metadata(context: &mut OwnedDataValue, channel: Option<&str>) {
    set_nested_value(
        context,
        "metadata.processed_at",
        OwnedDataValue::String(Utc::now().to_rfc3339()),
    );
    set_nested_value(
        context,
        "metadata.engine_version",
        OwnedDataValue::String(env!("CARGO_PKG_VERSION").to_string()),
    );
    if let Some(channel) = channel {
        set_nested_value(
            context,
            "metadata.channel",
            OwnedDataValue::String(channel.to_string()),
        );
    }
}
