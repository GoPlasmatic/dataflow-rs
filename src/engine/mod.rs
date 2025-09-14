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
- **Workflow**: Collection of tasks with JSONLogic conditions (metadata-only access)
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
pub mod utils;
pub mod workflow;
pub mod workflow_executor;

// Re-export key types for easier access
pub use error::{DataflowError, ErrorInfo, Result};
pub use functions::{AsyncFunctionHandler, FunctionConfig};
pub use message::Message;
pub use task::Task;
pub use workflow::Workflow;

use chrono::Utc;
use datalogic_rs::{CompiledLogic, DataLogic};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;

use compiler::LogicCompiler;
use executor::InternalExecutor;
use task_executor::TaskExecutor;
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
    /// Registry of available workflows (immutable after initialization)
    workflows: Arc<HashMap<String, Workflow>>,
    /// Workflow executor for orchestrating workflow execution
    workflow_executor: Arc<WorkflowExecutor>,
    /// Shared DataLogic instance for JSONLogic evaluation
    datalogic: Arc<DataLogic>,
    /// Compiled logic cache with Arc for zero-copy sharing
    logic_cache: Vec<Arc<CompiledLogic>>,
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
        // Compile workflows with DataLogic v4
        let mut compiler = LogicCompiler::new();
        let workflow_map = compiler.compile_workflows(workflows);
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

        Self {
            workflows: Arc::new(workflow_map),
            workflow_executor,
            datalogic,
            logic_cache,
        }
    }

    /// Processes a message through workflows that match their conditions.
    ///
    /// This async method:
    /// 1. Iterates through workflows sequentially in deterministic order (sorted by ID)
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
        message.context["metadata"]["processed_at"] = json!(Utc::now().to_rfc3339());
        message.context["metadata"]["engine_version"] = json!(env!("CARGO_PKG_VERSION"));

        // Sort workflows by priority for proper execution order
        let mut workflows: Vec<_> = self.workflows.values().collect();
        workflows.sort_by_key(|w| w.priority);

        // Process each workflow in priority order
        for workflow in workflows {
            // Execute workflow through the workflow executor
            self.workflow_executor.execute(workflow, message).await?;
        }

        Ok(())
    }

    /// Get a reference to the workflows
    pub fn workflows(&self) -> &Arc<HashMap<String, Workflow>> {
        &self.workflows
    }

    /// Get a reference to the DataLogic instance
    pub fn datalogic(&self) -> &Arc<DataLogic> {
        &self.datalogic
    }

    /// Get a reference to the compiled logic cache
    pub fn logic_cache(&self) -> &Vec<Arc<CompiledLogic>> {
        &self.logic_cache
    }
}
