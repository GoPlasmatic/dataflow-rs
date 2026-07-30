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
    let engine = Engine::builder().with_workflows(workflows).build()?;

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
pub mod observer;
pub mod task;
pub mod task_context;
pub mod task_executor;
pub mod task_outcome;
pub mod trace;
pub mod utils;
pub mod workflow;
pub mod workflow_executor;

// Re-export key types for easier access
pub use error::{DataflowError, ErrorInfo, Result, ServiceErrorBuilder};
pub use functions::{
    AsyncFunctionHandler, BoxedFunctionHandler, CompiledCustomInput, DynAsyncFunctionHandler,
    FunctionConfig, Template, TemplateCompiler,
};
pub use message::Message;
pub use observer::{ExecutionObserver, TaskEvent};
pub use task::Task;
pub use task_context::TaskContext;
pub use task_outcome::TaskOutcome;
pub use trace::{AuditTrailScope, ExecutionStep, ExecutionTrace, StepResult, TraceOptions};
pub use workflow::{ConnectorRef, Rollout, Workflow, WorkflowStatus};

// `EngineBuilder` is defined further down in this file but exposed here so
// downstream paths can import it via `dataflow_rs::engine::EngineBuilder`.

use chrono::Utc;
use datalogic_rs::Engine as DatalogicEngine;
use datavalue::OwnedDataValue;
use std::collections::HashMap;
use std::sync::Arc;

use compiler::LogicCompiler;
use task_executor::TaskExecutor;
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
    /// Pre-built `Arc<OwnedDataValue::String>` of the engine version.
    /// Built once at construction. Note the per-message stamp still clones
    /// the inner `String` — the context owns its values, so the cached
    /// form only saves re-formatting, not the (small) allocation.
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
    /// * `custom_functions` - Custom async function handlers (use
    ///   `HashMap::new()` for none, or prefer [`Engine::builder`])
    ///
    /// # Example
    ///
    /// ```
    /// use dataflow_rs::{Engine, Workflow};
    ///
    /// let workflows = vec![Workflow::from_json(r#"{"id": "test", "name": "Test", "priority": 0, "tasks": [{"id": "task1", "name": "Task 1", "function": {"name": "map", "input": {"mappings": []}}}]}"#).unwrap()];
    ///
    /// let engine = Engine::builder().with_workflows(workflows).build().unwrap();
    /// ```
    /// The recommended construction path is [`Engine::builder`]. `Engine::new`
    /// is the lower-level escape hatch — accepts handlers as a plain
    /// `HashMap` (use `HashMap::new()` for the no-handler case).
    pub fn new(
        workflows: Vec<Workflow>,
        custom_functions: HashMap<String, BoxedFunctionHandler>,
    ) -> Result<Self> {
        // Compile workflows (sorted by priority at compile time). Each
        // workflow/task/config owns its own `Arc<Logic>` slots — no central
        // cache to return. Any compile failure bubbles up immediately.
        let compiler = LogicCompiler::new();
        let mut sorted_workflows = compiler.compile_workflows(workflows)?;
        let datalogic = compiler.into_engine();

        let task_functions = custom_functions;

        // Pre-parse `FunctionConfig::Custom { input }` JSON into the
        // registered handler's typed `Self::Input`, caching the boxed value
        // on the task. Misshapen Custom configs fail here, not on first
        // message — matches the "fail loud at startup" stance for compiled
        // logic. Built-in async configs (HttpCall/Enrich/PublishKafka) are
        // already typed by serde and need no second pass.
        precompile_custom_inputs(&mut sorted_workflows, &task_functions, &datalogic)?;

        let task_executor = Arc::new(TaskExecutor::new(
            Arc::new(task_functions),
            Arc::clone(&datalogic),
        ));

        let workflow_executor =
            Arc::new(WorkflowExecutor::new(task_executor, Arc::clone(&datalogic)));

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

    /// Start building an engine. The recommended construction path —
    /// chains `register("name", handler)` and `with_workflow(w)` calls,
    /// then `build()` to produce a `Result<Engine>`.
    ///
    /// ```no_run
    /// use dataflow_rs::{Engine, Workflow};
    /// # let workflow: Workflow = unimplemented!();
    /// let engine = Engine::builder()
    ///     .with_workflow(workflow)
    ///     // .register("my_handler", MyHandler)  // any AsyncFunctionHandler
    ///     .build()
    ///     .unwrap();
    /// ```
    pub fn builder() -> EngineBuilder {
        EngineBuilder::new()
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
        let mut sorted_workflows = compiler.compile_workflows(workflows)?;
        let datalogic = compiler.into_engine();

        // Pre-parse Custom inputs against the existing handler registry —
        // hot-reload still validates the new workflow set against the
        // already-registered handlers.
        precompile_custom_inputs(&mut sorted_workflows, &task_functions, &datalogic)?;

        // Rebuild the executor stack, reusing the existing function registry
        let task_executor = Arc::new(TaskExecutor::new(task_functions, Arc::clone(&datalogic)));

        // Carry the observer across the reload. Dropping it here would stop
        // metrics silently at the first hot reload.
        let mut executor = WorkflowExecutor::new(task_executor, Arc::clone(&datalogic));
        if let Some(observer) = self.workflow_executor.observer() {
            executor = executor.with_observer(Arc::clone(observer));
        }
        let workflow_executor = Arc::new(executor);

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

    /// Attach a per-task [`ExecutionObserver`], returning the updated engine.
    ///
    /// The escape hatch matching [`Engine::new`] — [`EngineBuilder::with_observer`]
    /// is the recommended path. Rebuilds the executor stack around the existing
    /// handler registry and datalogic engine, so nothing is recompiled; the cost
    /// is a few `Arc` bumps.
    ///
    /// Carried across [`Engine::with_new_workflows`], so a hot reload does not
    /// silently stop reporting.
    pub fn with_observer(self, observer: Arc<dyn ExecutionObserver>) -> Self {
        let task_executor = Arc::new(TaskExecutor::new(
            self.workflow_executor.task_functions(),
            Arc::clone(&self.datalogic),
        ));
        let workflow_executor = Arc::new(
            WorkflowExecutor::new(task_executor, Arc::clone(&self.datalogic))
                .with_observer(observer),
        );
        Self {
            workflows: self.workflows,
            channel_index: self.channel_index,
            workflow_executor,
            datalogic: self.datalogic,
            engine_version: self.engine_version,
        }
    }

    /// Processes a message through workflows that match their conditions.
    ///
    /// This async method:
    /// 1. Iterates through workflows sequentially in priority order (pre-sorted at construction)
    /// 2. Delegates workflow execution to the WorkflowExecutor
    /// 3. Updates message metadata
    ///
    /// # Error contract
    ///
    /// Errors flow through two complementary channels:
    /// - `message.errors()` — **always** contains every error encountered
    ///   (validation failures, task panics, 5xx-status outcomes, workflow
    ///   wrappers). Callers that want a uniform view inspect this list.
    /// - `Result::Err` — signals **only** that the engine stopped before
    ///   processing every workflow. Callers that want fail-fast match on
    ///   this. The error pushed to `message.errors` for the same failure
    ///   carries the workflow context (id) that the bare `Err` doesn't.
    ///
    /// In particular: a workflow with `continue_on_error: true` records its
    /// errors to `message.errors` and returns `Ok(())` here. A workflow
    /// with `continue_on_error: false` records to `message.errors` *and*
    /// returns `Result::Err` (which short-circuits the rest of this call).
    ///
    /// # Arguments
    /// * `message` - The message to process through workflows
    ///
    /// # Returns
    /// * `Result<()>` — `Ok(())` if every workflow completed (each may have
    ///   pushed errors to `message.errors`); `Err(e)` if the engine
    ///   stopped early on a hard failure.
    pub async fn process_message(&self, message: &mut Message) -> Result<()> {
        // Capture a single timestamp for the entire process_message call. The
        // workflow executor reads it back via Message metadata if it needs to
        // emit AuditTrail entries; this caps the number of `Utc::now()` syscalls
        // at 1 per message (down from 3+ — one stamp here, one per AuditTrail).
        let now = Utc::now();
        set_processing_metadata(&mut message.context, &self.engine_version, now, None);

        // Process workflows in priority order (pre-sorted at construction).
        // `run_all_borrowed` groups consecutive fully-sync workflows into a
        // single shared-arena scope so the context is deep-walked once per
        // run rather than once per workflow. Passing the registry slice
        // directly avoids the former per-message `Vec<&Workflow>` collect.
        self.workflow_executor
            .run_all_borrowed(&self.workflows[..], message, None, now)
            .await
    }

    /// Processes a message through workflows with step-by-step tracing,
    /// recording into a caller-owned trace.
    ///
    /// Identical to [`Engine::process_message_with_trace`] except that the
    /// trace is borrowed rather than returned, so the steps completed before a
    /// hard failure survive the `Err`. That makes this the method to reach for
    /// when the run you want to inspect is the run that failed — a returned
    /// trace is dropped by the `?` at the call site, a borrowed one is not.
    ///
    /// Steps are **appended** to `trace`; any steps already present are
    /// preserved, so a caller can accumulate across a chain of calls.
    ///
    /// The error contract is unchanged: `Ok(())` means every workflow was
    /// processed (each may still have pushed to `message.errors`), and `Err(e)`
    /// means the engine stopped early. See [`Engine::process_message`] for the
    /// full contract.
    ///
    /// Note that the failing task's *own* step is not recorded — the engine
    /// propagates the failure before appending it — so the retained trace ends
    /// at the last known-good step rather than at the error. The error itself
    /// is available from the returned `Err` and from `message.errors()`.
    ///
    /// # Arguments
    /// * `message` - The message to process through workflows
    /// * `trace` - Caller-owned trace to append steps to
    ///
    /// # Returns
    /// * `Result<()>` — `Ok(())` if every workflow completed; `Err(e)` if the
    ///   engine stopped early. In both cases `trace` holds the steps that ran.
    pub async fn process_message_tracing(
        &self,
        message: &mut Message,
        trace: &mut ExecutionTrace,
    ) -> Result<()> {
        // The trace carries its own capture policy, so nothing to pass here.
        let now = Utc::now();
        set_processing_metadata(&mut message.context, &self.engine_version, now, None);

        // Process workflows in priority order (pre-sorted at construction).
        self.workflow_executor
            .run_all_borrowed(&self.workflows[..], message, Some(trace), now)
            .await
    }

    /// Processes a message through workflows with step-by-step tracing.
    ///
    /// This method is similar to `process_message` but captures an execution trace
    /// that can be used for debugging and step-by-step visualization.
    ///
    /// Because the trace is returned by value, a `?` at the call site discards
    /// it — on a hard failure this yields `Err` and no steps at all. Use
    /// [`Engine::process_message_tracing`] to keep the steps that ran.
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
        self.process_message_with_trace_options(message, TraceOptions::default())
            .await
    }

    /// Processes a message with tracing under an explicit capture policy.
    ///
    /// The default policy — what [`Engine::process_message_with_trace`] uses —
    /// takes a full [`Message`] snapshot per executed step, which is unbounded
    /// in message size and quadratic in task count. A host that *persists*
    /// traces should bound them here rather than trimming the result
    /// afterwards; by then the peak memory has already been paid.
    ///
    /// See [`TraceOptions`] for the knobs, and
    /// [`Engine::process_message_tracing`] if you also need the steps to survive
    /// a hard failure.
    ///
    /// # Arguments
    /// * `message` - The message to process through workflows
    /// * `options` - What to record for each step
    pub async fn process_message_with_trace_options(
        &self,
        message: &mut Message,
        options: TraceOptions,
    ) -> Result<ExecutionTrace> {
        let mut trace = ExecutionTrace::with_options(options);
        self.process_message_tracing(message, &mut trace).await?;
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
            // Channel-selected workflows are non-contiguous in the registry,
            // so the pointer collect stays on this path.
            let workflows: Vec<&Workflow> =
                indices.iter().map(|&idx| &self.workflows[idx]).collect();
            self.workflow_executor
                .run_all_borrowed(&workflows, message, None, now)
                .await?;
        }

        Ok(())
    }

    /// Channel-scoped variant of [`Engine::process_message_tracing`].
    ///
    /// As with [`Engine::process_message_for_channel`], an unknown channel — or
    /// a channel with no Active workflows — is a no-op: this returns `Ok(())`
    /// and leaves `trace` untouched. Steps are appended, matching
    /// [`Engine::process_message_tracing`].
    ///
    /// # Arguments
    /// * `channel` - The channel name to route the message through
    /// * `message` - The message to process
    /// * `trace` - Caller-owned trace to append steps to
    pub async fn process_message_for_channel_tracing(
        &self,
        channel: &str,
        message: &mut Message,
        trace: &mut ExecutionTrace,
    ) -> Result<()> {
        let now = Utc::now();
        set_processing_metadata(
            &mut message.context,
            &self.engine_version,
            now,
            Some(channel),
        );

        if let Some(indices) = self.channel_index.get(channel) {
            let workflows: Vec<&Workflow> =
                indices.iter().map(|&idx| &self.workflows[idx]).collect();
            self.workflow_executor
                .run_all_borrowed(&workflows, message, Some(trace), now)
                .await?;
        }

        Ok(())
    }

    /// Processes a message through a channel with step-by-step tracing.
    ///
    /// Because the trace is returned by value, a `?` at the call site discards
    /// it — on a hard failure this yields `Err` and no steps at all. Use
    /// [`Engine::process_message_for_channel_tracing`] to keep the steps that
    /// ran.
    ///
    /// # Arguments
    /// * `channel` - The channel name to route the message through
    /// * `message` - The message to process
    pub async fn process_message_for_channel_with_trace(
        &self,
        channel: &str,
        message: &mut Message,
    ) -> Result<ExecutionTrace> {
        self.process_message_for_channel_with_trace_options(
            channel,
            message,
            TraceOptions::default(),
        )
        .await
    }

    /// Channel-scoped variant of
    /// [`Engine::process_message_with_trace_options`].
    ///
    /// # Arguments
    /// * `channel` - The channel name to route the message through
    /// * `message` - The message to process
    /// * `options` - What to record for each step
    pub async fn process_message_for_channel_with_trace_options(
        &self,
        channel: &str,
        message: &mut Message,
        options: TraceOptions,
    ) -> Result<ExecutionTrace> {
        let mut trace = ExecutionTrace::with_options(options);
        self.process_message_for_channel_tracing(channel, message, &mut trace)
            .await?;
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

/// Builder for [`Engine`]. The recommended construction path — chain
/// `register("name", handler)` and `with_workflow(workflow)` calls, then
/// `build()` to produce a `Result<Engine>`. Empty registration is fine; an
/// engine with no custom handlers still resolves the built-in functions.
///
/// `register` takes any [`AsyncFunctionHandler`] and boxes it internally; the
/// `Box<dyn DynAsyncFunctionHandler + Send + Sync>` plumbing stays out of
/// user code.
///
/// ```no_run
/// use dataflow_rs::{Engine, Workflow};
/// # let workflow: Workflow = unimplemented!();
/// let engine = Engine::builder()
///     .with_workflow(workflow)
///     // .register("my_handler", MyHandler)
///     .build()
///     .unwrap();
/// ```
#[must_use = "EngineBuilder must be `.build()` to produce an Engine"]
#[derive(Default)]
pub struct EngineBuilder {
    workflows: Vec<Workflow>,
    handlers: HashMap<String, BoxedFunctionHandler>,
    observer: Option<Arc<dyn ExecutionObserver>>,
}

impl EngineBuilder {
    /// Create an empty builder. Equivalent to [`EngineBuilder::default`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a custom async handler under `name`. Accepts any
    /// `AsyncFunctionHandler`; boxing happens internally via the engine's
    /// blanket impl.
    pub fn register<F>(mut self, name: impl Into<String>, handler: F) -> Self
    where
        F: AsyncFunctionHandler,
    {
        self.handlers.insert(name.into(), Box::new(handler));
        self
    }

    /// Register a pre-boxed handler. Useful when handlers are constructed
    /// dynamically (e.g. plugin registries) and the concrete type isn't
    /// known at the call site.
    pub fn register_boxed(
        mut self,
        name: impl Into<String>,
        handler: BoxedFunctionHandler,
    ) -> Self {
        self.handlers.insert(name.into(), handler);
        self
    }

    /// Add a single workflow. Subsequent calls append.
    pub fn with_workflow(mut self, workflow: Workflow) -> Self {
        self.workflows.push(workflow);
        self
    }

    /// Append every workflow in `workflows`. Accepts anything iterable —
    /// `Vec<Workflow>`, an array, an iterator. Existing workflows on the
    /// builder are kept; subsequent registers/workflows still chain.
    pub fn with_workflows<I>(mut self, workflows: I) -> Self
    where
        I: IntoIterator<Item = Workflow>,
    {
        self.workflows.extend(workflows);
        self
    }

    /// Insert every handler in `handlers`, keeping any already registered.
    ///
    /// Same extend-not-replace semantics as [`EngineBuilder::with_workflows`].
    /// Exists because `register` is per-name, which pushed an embedder that
    /// builds a whole `HashMap<String, BoxedFunctionHandler>` in one place onto
    /// [`Engine::new`] and off the builder entirely — and therefore out of reach
    /// of [`EngineBuilder::with_observer`].
    pub fn with_handlers(mut self, handlers: HashMap<String, BoxedFunctionHandler>) -> Self {
        self.handlers.extend(handlers);
        self
    }

    /// Attach a per-task [`ExecutionObserver`]. Later calls replace the previous
    /// one.
    ///
    /// This is the only way to time the sync built-ins, which are dispatched
    /// inside the executor and never reach the function registry. With no
    /// observer attached the instrumentation — including its clock reads — stays
    /// out of the dispatch path entirely.
    pub fn with_observer(mut self, observer: Arc<dyn ExecutionObserver>) -> Self {
        self.observer = Some(observer);
        self
    }

    /// Compile the workflows, pre-parse Custom inputs, and produce the
    /// engine. Compile errors and missing handler references surface here —
    /// the engine never deserializes Custom config on the hot path.
    pub fn build(self) -> Result<Engine> {
        let engine = Engine::new(self.workflows, self.handlers)?;
        Ok(match self.observer {
            Some(observer) => engine.with_observer(observer),
            None => engine,
        })
    }
}

/// Walk every task in every workflow; for each `FunctionConfig::Custom`,
/// look up the registered handler and ask it to parse the raw `input` JSON
/// into its typed `Self::Input` (boxed as `dyn Any`). The cached result is
/// stored on the task — dispatch then hands the handler a `&dyn Any` it
/// downcasts in O(1).
///
/// Built-in async configs (`HttpCall`, `Enrich`, `PublishKafka`) are already
/// parsed by serde's `untagged` representation on `FunctionConfig`; they
/// need no second pass.
///
/// Returns `FunctionNotFound` when a Custom task references an unregistered
/// handler — moves the failure from "first message" to engine construction.
fn precompile_custom_inputs(
    workflows: &mut [Workflow],
    handlers: &HashMap<String, BoxedFunctionHandler>,
    datalogic: &Arc<DatalogicEngine>,
) -> Result<()> {
    let template_compiler = TemplateCompiler::new(Arc::clone(datalogic));
    for workflow in workflows {
        for task in &mut workflow.tasks {
            if let FunctionConfig::Custom {
                name,
                input,
                compiled_input,
            } = &mut task.function
            {
                let handler = handlers
                    .get(name)
                    .ok_or_else(|| function_not_found_error(name, handlers))?;
                let mut parsed = handler.parse_input_box(input)?;
                handler.compile_input_box(&mut *parsed, &template_compiler)?;
                *compiled_input = Some(CompiledCustomInput(Arc::from(parsed)));
            }
        }
    }
    Ok(())
}

/// Build a `FunctionNotFound` error that lists both the registered custom
/// handlers and the names of built-in functions, so a user with a typo
/// (e.g. `htttp_call`) can immediately spot the intended name.
///
/// **This message is free-form and deliberately unpinned.** It is a diagnostic
/// for humans; its wording and layout may change in any release. No test
/// asserts on it, and none should — a caller that needs the built-in vocabulary
/// programmatically should use [`crate::BUILTIN_FUNCTION_NAMES`] and
/// [`crate::builtin_function_kind`], which exist for exactly that purpose and
/// answer the sharper question of whether a name needs a registered handler.
fn function_not_found_error(
    name: &str,
    handlers: &HashMap<String, BoxedFunctionHandler>,
) -> DataflowError {
    use crate::engine::functions::config::BUILTIN_FUNCTION_NAMES;
    let mut registered: Vec<&str> = handlers.keys().map(String::as_str).collect();
    registered.sort_unstable();
    let registered_part = if registered.is_empty() {
        String::from("none")
    } else {
        registered.join(", ")
    };
    DataflowError::FunctionNotFound(format!(
        "{name} (registered handlers: {registered_part}; built-ins: {})",
        BUILTIN_FUNCTION_NAMES.join(", ")
    ))
}

/// Stamp the standard processing metadata (`processed_at`, `engine_version`,
/// and optionally `channel`) into the message context.
///
/// `now` is captured once at the top of `process_message` and reused so the
/// timestamp on `metadata.processed_at` matches the one used for every
/// `AuditTrail` entry within the same call.
///
/// Walks to the `metadata` object once and sets every key in a single pass,
/// instead of one full `"metadata.*"` path split + tree walk per key.
/// Mirrors `set_nested_value` semantics for the degenerate shapes: a
/// non-object context or a non-object existing `metadata` slot no-ops; a
/// missing `metadata` slot is created.
///
/// `(**engine_version).clone()` deep-clones the inner `String` — the
/// context owns its values, so one small allocation per message is
/// inherent; the cached `Arc` only saves re-formatting the version.
fn set_processing_metadata(
    context: &mut OwnedDataValue,
    engine_version: &Arc<OwnedDataValue>,
    now: chrono::DateTime<Utc>,
    channel: Option<&str>,
) {
    let OwnedDataValue::Object(top) = context else {
        return;
    };
    let metadata = match top.iter().position(|(k, _)| k == "metadata") {
        Some(i) => &mut top[i].1,
        None => {
            top.push(("metadata".to_string(), OwnedDataValue::Object(Vec::new())));
            &mut top.last_mut().expect("just pushed").1
        }
    };
    let OwnedDataValue::Object(meta) = metadata else {
        return;
    };

    let mut set_key = |key: &str, value: OwnedDataValue| {
        if let Some(slot) = meta.iter_mut().find(|(k, _)| k == key) {
            slot.1 = value;
        } else {
            meta.push((key.to_string(), value));
        }
    };
    set_key("processed_at", OwnedDataValue::String(now.to_rfc3339()));
    set_key("engine_version", (**engine_version).clone());
    if let Some(channel) = channel {
        set_key("channel", OwnedDataValue::String(channel.to_string()));
    }
}
