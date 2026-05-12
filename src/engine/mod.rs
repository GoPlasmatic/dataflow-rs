/*!
# Engine Module

This module implements the core async workflow engine for dataflow-rs. The engine provides
high-performance, asynchronous message processing through workflows composed of tasks.

## Architecture

The engine features a clean async-first architecture built on datalogic v5:
- **Compiler**: Pre-compiles JSONLogic expressions into `Arc<Logic>` via `Engine::compile_arc`
- **Executor**: Handles internal function execution (map, validation) with async support
- **Engine**: Orchestrates workflow processing with shared compiled logic
- **Thread-Safe**: Single `datalogic_rs::Engine` shared via `Arc`, with `Arc<Logic>` entries for zero-copy sharing

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
- **Bump-arena evaluation**: Per-worker thread-local `Bump` is rewound (not freed) between evals
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
    let engine = Engine::new(workflows, None)?;

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
use datalogic_rs::Engine as DatalogicEngine;
use datavalue::OwnedDataValue;
use std::collections::HashMap;
use std::sync::Arc;

use compiler::LogicCompiler;
use task_executor::TaskExecutor;
use utils::set_nested_value;
use workflow_executor::WorkflowExecutor;

/// High-performance async workflow engine for message processing.
///
/// ## Architecture
///
/// The engine is designed for async-first operation with Tokio:
/// - **Separation of Concerns**: Distinct executors for workflows and tasks
/// - **Shared datalogic engine**: Single `datalogic_rs::Engine` wrapped in `Arc` for thread-safe sharing
/// - **Arc<Logic>**: Pre-compiled logic shared across all async tasks
/// - **Async Functions**: Native async support for I/O-bound operations
///
/// ## Performance Characteristics
///
/// - **Zero Runtime Compilation**: All logic compiled during initialization
/// - **Zero-Copy Sharing**: Arc-wrapped compiled logic shared without cloning
/// - **Optimal for Mixed Workloads**: Async I/O with blocking CPU evaluation
/// - **Thread-Safe by Design**: All components safe to share across Tokio tasks
pub struct Engine {
    /// Registry of available workflows, pre-sorted by priority (immutable after initialization).
    /// Each workflow / task / function-config holds its own `Arc<Logic>` slots
    /// — there is no central logic cache anymore.
    workflows: Arc<Vec<Workflow>>,
    /// Channel index: maps channel name -> indices into workflows vec (only Active workflows)
    channel_index: Arc<HashMap<String, Vec<usize>>>,
    /// Workflow executor for orchestrating workflow execution
    workflow_executor: Arc<WorkflowExecutor>,
    /// Shared datalogic v5 engine for JSONLogic evaluation (Send + Sync)
    datalogic: Arc<DatalogicEngine>,
    /// Pre-built `Arc<OwnedDataValue::String>` of the engine version. Built
    /// once at construction; stamped into `metadata.engine_version` per
    /// message via an `Arc` refcount bump (the underlying `String` is never
    /// re-allocated for this stamp).
    engine_version: Arc<OwnedDataValue>,
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
    /// Creates a new Engine instance.
    ///
    /// Compiles every workflow / task / function-config JSONLogic expression
    /// up-front. Returns `Err(DataflowError)` if any required expression
    /// fails to compile — fail-loud at construction time instead of silently
    /// dropping broken workflows at runtime.
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
    /// let engine = Engine::new(workflows, None).unwrap();
    /// ```
    pub fn new(
        workflows: Vec<Workflow>,
        custom_functions: Option<HashMap<String, Box<dyn AsyncFunctionHandler + Send + Sync>>>,
    ) -> Result<Self> {
        // Compile workflows (sorted by priority at compile time). Each
        // workflow/task/config owns its own `Arc<Logic>` slots — no central
        // cache to return. Any compile failure bubbles up immediately.
        let compiler = LogicCompiler::new();
        let sorted_workflows = compiler.compile_workflows(workflows)?;
        let datalogic = compiler.into_engine();

        let mut task_functions = custom_functions.unwrap_or_default();

        // Add built-in async function handlers
        for (name, handler) in functions::builtins::get_all_functions() {
            task_functions.insert(name, handler);
        }

        let task_executor = Arc::new(TaskExecutor::new(
            Arc::new(task_functions),
            Arc::clone(&datalogic),
        ));

        let workflow_executor = Arc::new(WorkflowExecutor::new(
            task_executor,
            Arc::clone(&datalogic),
        ));

        // Build channel index for O(1) channel-based routing
        let channel_index = build_channel_index(&sorted_workflows);

        Ok(Self {
            workflows: Arc::new(sorted_workflows),
            channel_index: Arc::new(channel_index),
            workflow_executor,
            datalogic,
            engine_version: Arc::new(OwnedDataValue::String(
                env!("CARGO_PKG_VERSION").to_string(),
            )),
        })
    }

    /// Cached `OwnedDataValue::String` of the engine version.
    pub fn engine_version_value(&self) -> &OwnedDataValue {
        &self.engine_version
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
    pub fn with_new_workflows(&self, workflows: Vec<Workflow>) -> Result<Self> {
        // Extract the shared function registry from the existing executor
        let task_functions = self.workflow_executor.task_functions();

        // Compile new workflows with a fresh datalogic engine instance.
        let compiler = LogicCompiler::new();
        let sorted_workflows = compiler.compile_workflows(workflows)?;
        let datalogic = compiler.into_engine();

        // Rebuild the executor stack, reusing the existing function registry
        let task_executor = Arc::new(TaskExecutor::new(task_functions, Arc::clone(&datalogic)));

        let workflow_executor = Arc::new(WorkflowExecutor::new(
            task_executor,
            Arc::clone(&datalogic),
        ));

        // Build channel index for O(1) channel-based routing
        let channel_index = build_channel_index(&sorted_workflows);

        Ok(Self {
            workflows: Arc::new(sorted_workflows),
            channel_index: Arc::new(channel_index),
            workflow_executor,
            datalogic,
            engine_version: Arc::clone(&self.engine_version),
        })
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
        // Capture a single timestamp for the entire process_message call. The
        // workflow executor reads it back via Message metadata if it needs to
        // emit AuditTrail entries; this caps the number of `Utc::now()` syscalls
        // at 1 per message (down from 3+ — one stamp here, one per AuditTrail).
        let now = Utc::now();
        set_processing_metadata(&mut message.context, &self.engine_version, now, None);

        // Process each workflow in priority order (pre-sorted at construction)
        for workflow in self.workflows.iter() {
            self.workflow_executor
                .execute(workflow, message, now)
                .await?;
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

        let now = Utc::now();
        set_processing_metadata(&mut message.context, &self.engine_version, now, None);

        let mut trace = ExecutionTrace::new();

        // Process each workflow in priority order (pre-sorted at construction)
        for workflow in self.workflows.iter() {
            self.workflow_executor
                .execute_with_trace(workflow, message, &mut trace, now)
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
        let now = Utc::now();
        set_processing_metadata(
            &mut message.context,
            &self.engine_version,
            now,
            Some(channel),
        );

        if let Some(indices) = self.channel_index.get(channel) {
            for &idx in indices {
                self.workflow_executor
                    .execute(&self.workflows[idx], message, now)
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

        let now = Utc::now();
        set_processing_metadata(
            &mut message.context,
            &self.engine_version,
            now,
            Some(channel),
        );

        let mut trace = ExecutionTrace::new();

        if let Some(indices) = self.channel_index.get(channel) {
            for &idx in indices {
                self.workflow_executor
                    .execute_with_trace(&self.workflows[idx], message, &mut trace, now)
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
}

/// Stamp the standard processing metadata (`processed_at`, `engine_version`,
/// and optionally `channel`) into the message context.
///
/// `now` is captured once at the top of `process_message` and reused so the
/// timestamp on `metadata.processed_at` matches the one used for every
/// `AuditTrail` entry within the same call.
/// `engine_version` is the cached `Arc<OwnedDataValue::String>` owned by
/// `Engine`; the deref-and-clone here is one Arc-bump's worth of work, not
/// a `String` allocation.
fn set_processing_metadata(
    context: &mut OwnedDataValue,
    engine_version: &Arc<OwnedDataValue>,
    now: chrono::DateTime<Utc>,
    channel: Option<&str>,
) {
    set_nested_value(
        context,
        "metadata.processed_at",
        OwnedDataValue::String(now.to_rfc3339()),
    );
    set_nested_value(
        context,
        "metadata.engine_version",
        (**engine_version).clone(),
    );
    if let Some(channel) = channel {
        set_nested_value(
            context,
            "metadata.channel",
            OwnedDataValue::String(channel.to_string()),
        );
    }
}
