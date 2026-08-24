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

pub mod authoring;
pub mod compiler;
pub mod error;
pub mod executor;
pub mod functions;
pub mod message;
pub mod observer;
/// Retrying a failed operation. Not available on `wasm32` — tokio's time
/// driver, which the backoff needs, does not run there.
#[cfg(not(target_arch = "wasm32"))]
pub mod retry;
pub mod rollout;
pub mod steps;
pub mod task;
pub mod task_context;
pub mod task_executor;
pub mod task_outcome;
pub mod trace;
pub mod utils;
pub mod workflow;
pub mod workflow_executor;

// Re-export key types for easier access
pub use authoring::{IssueCode, WorkflowIssue};
use error::{DEFAULT_ERROR_CONTEXT_LIMIT, ErrorContextConfig};
pub use error::{DataflowError, ErrorInfo, Result, ServiceErrorBuilder};
pub use functions::{
    AsyncFunctionHandler, BoxedFunctionHandler, CompiledCustomInput, DynAsyncFunctionHandler,
    FunctionConfig, Template, TemplateCompiler,
};
pub use message::Message;
pub use observer::{ExecutionObserver, TaskEvent};
#[cfg(not(target_arch = "wasm32"))]
pub use retry::{RetryPolicy, retry_with_attempts, retry_with_policy};
pub use rollout::{Rollout, RolloutError};
pub use steps::{
    AuthoredStep, AuthoredSteps, MAX_GROUP_DEPTH, StepKind, is_group, walk_authored_steps,
};
pub use task::{Task, TaskGroup};
pub use task_context::TaskContext;
pub use task_outcome::{HALT_STATUS_CODE, TaskOutcome};
pub use trace::{AuditTrailScope, ExecutionStep, ExecutionTrace, StepResult, TraceOptions};
pub use workflow::{ConnectorRef, Workflow, WorkflowStatus};

// `EngineBuilder` is defined further down in this file but exposed here so
// downstream paths can import it via `dataflow_rs::engine::EngineBuilder`.

use chrono::Utc;
use datalogic_rs::Engine as DatalogicEngine;
use datavalue::OwnedDataValue;
use std::collections::HashMap;
use std::sync::Arc;

use crate::engine::functions::config::{
    DispatchableFunction, can_dispatch_in, dispatchable_functions_in,
};

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
    /// Custom JSONLogic operators registered via
    /// [`EngineBuilder::with_datalogic_operator`]. Retained here — not just
    /// applied once — because [`Engine::with_new_workflows`] builds a fresh
    /// datalogic engine and must re-register them; holding only the built
    /// engine would silently drop every custom operator at the first hot
    /// reload.
    datalogic_operators: DatalogicOperators,
    /// Pre-built `Arc<OwnedDataValue::String>` of the engine version.
    /// Built once at construction. Note the per-message stamp still clones
    /// the inner `String` — the context owns its values, so the cached
    /// form only saves re-formatting, not the (small) allocation.
    engine_version: Arc<OwnedDataValue>,
}

/// The custom-operator registrations an engine carries across rebuilds.
pub type DatalogicOperators = Arc<HashMap<String, Arc<dyn datalogic_rs::CustomOperator>>>;

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
    /// * `task_functions` - Custom async function handlers (use
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
        task_functions: HashMap<String, BoxedFunctionHandler>,
    ) -> Result<Self> {
        Self::new_with_operators(workflows, task_functions, Arc::new(HashMap::new()))
    }

    /// As [`Engine::new`], with custom JSONLogic operators registered on the
    /// datalogic engine (and retained across [`Engine::with_new_workflows`]).
    /// The builder path is [`EngineBuilder::with_datalogic_operator`]; this is
    /// its escape-hatch twin, matching `new`.
    pub fn new_with_operators(
        workflows: Vec<Workflow>,
        task_functions: HashMap<String, BoxedFunctionHandler>,
        datalogic_operators: DatalogicOperators,
    ) -> Result<Self> {
        // Compile workflows (sorted by priority at compile time). Each
        // workflow/task/config owns its own `Arc<Logic>` slots — no central
        // cache to return. Any compile failure bubbles up immediately.
        let compiler = LogicCompiler::with_operators(&datalogic_operators);
        let mut sorted_workflows = compiler.compile_workflows(workflows)?;
        let datalogic = compiler.into_engine();

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
            datalogic_operators,
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

        // Compile new workflows with a fresh datalogic engine instance —
        // re-registering the retained custom operators, so a hot reload keeps
        // the same operator vocabulary as the engine it replaces.
        let compiler = LogicCompiler::with_operators(&self.datalogic_operators);
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
        // Same reasoning as the observer: dropping this would silently stop
        // recording failure codes at the first hot reload.
        if let Some(cfg) = self.workflow_executor.error_context() {
            executor = executor.with_error_context(Arc::clone(cfg));
        }
        let workflow_executor = Arc::new(executor);

        // Build channel index for O(1) channel-based routing
        let channel_index = build_channel_index(&sorted_workflows);

        Ok(Self {
            workflows: Arc::new(sorted_workflows),
            channel_index: Arc::new(channel_index),
            workflow_executor,
            datalogic,
            datalogic_operators: Arc::clone(&self.datalogic_operators),
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
        self.rebuild_executor(|executor| executor.with_observer(observer))
    }

    /// Mirror per-task failure codes into the message context, returning the
    /// updated engine.
    ///
    /// The escape hatch matching [`Engine::new`];
    /// [`EngineBuilder::with_error_context_path`] is the recommended path and the
    /// only one that validates the path. Carried across
    /// [`Engine::with_new_workflows`] and [`Engine::with_observer`].
    pub(crate) fn with_error_context(self, cfg: Arc<ErrorContextConfig>) -> Self {
        self.rebuild_executor(|executor| executor.with_error_context(cfg))
    }

    /// Rebuild the executor stack around the existing handler registry and
    /// datalogic engine, applying `configure` to the fresh executor.
    ///
    /// Nothing is recompiled; the cost is a few `Arc` bumps. Every knob the old
    /// executor held is re-applied first, because the rebuild otherwise drops
    /// them — that is what would make `.with_error_context(..)` followed by
    /// `.with_observer(..)` silently lose the former.
    fn rebuild_executor(
        self,
        configure: impl FnOnce(WorkflowExecutor) -> WorkflowExecutor,
    ) -> Self {
        let task_executor = Arc::new(TaskExecutor::new(
            self.workflow_executor.task_functions(),
            Arc::clone(&self.datalogic),
        ));
        let mut executor = WorkflowExecutor::new(task_executor, Arc::clone(&self.datalogic));
        if let Some(observer) = self.workflow_executor.observer() {
            executor = executor.with_observer(Arc::clone(observer));
        }
        if let Some(cfg) = self.workflow_executor.error_context() {
            executor = executor.with_error_context(Arc::clone(cfg));
        }
        Self {
            workflows: self.workflows,
            channel_index: self.channel_index,
            workflow_executor: Arc::new(configure(executor)),
            datalogic: self.datalogic,
            datalogic_operators: self.datalogic_operators,
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
        self.process_all(message, None, Utc::now()).await
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
        self.process_all(message, Some(trace), Utc::now()).await
    }

    /// Shared driver behind [`Self::process_message`] and
    /// [`Self::process_message_tracing`] — stamps processing metadata and runs
    /// every registered workflow in priority order. Mirrors [`Self::process_channel`]
    /// for the whole-registry case.
    ///
    /// `run_all_borrowed` groups consecutive fully-sync workflows into a
    /// single shared-arena scope so the context is deep-walked once per run
    /// rather than once per workflow. Passing the registry slice directly
    /// avoids a per-message `Vec<&Workflow>` collect.
    async fn process_all(
        &self,
        message: &mut Message,
        trace: Option<&mut ExecutionTrace>,
        now: chrono::DateTime<Utc>,
    ) -> Result<()> {
        set_processing_metadata(&mut message.context, &self.engine_version, now, None);
        self.workflow_executor
            .run_all_borrowed(&self.workflows[..], message, trace, now)
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
        self.process_channel(channel, message, None, Utc::now())
            .await
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
        self.process_channel(channel, message, Some(trace), Utc::now())
            .await
    }

    /// Shared driver behind [`Self::process_message_for_channel`] and
    /// [`Self::process_message_for_channel_tracing`] — stamps processing
    /// metadata and runs only the channel's Active workflows. An unknown
    /// channel, or one with no Active workflows, is a no-op.
    async fn process_channel(
        &self,
        channel: &str,
        message: &mut Message,
        trace: Option<&mut ExecutionTrace>,
        now: chrono::DateTime<Utc>,
    ) -> Result<()> {
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
                .run_all_borrowed(&workflows, message, trace, now)
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
    /// Every function this engine will dispatch: self-contained built-ins,
    /// plus [`BuiltinKind::RequiresHandler`] built-ins and custom names with a
    /// registered handler.
    ///
    /// This is the authoring-side vocabulary — what a host needs to screen a
    /// workflow definition, build a completion catalogue, or offer a
    /// did-you-mean on an unknown name, without keeping its own copy of the
    /// list.
    ///
    /// Aliases are grouped: `validate` is yielded once carrying
    /// `["validation"]`, not twice. [`Engine::can_dispatch`] does accept an
    /// alias, so the two are deliberately different sets.
    ///
    /// **Ordering is not meaningful** and may change without notice; treat the
    /// result as a set, and collect and sort if you need stable output.
    ///
    /// ```
    /// use dataflow_rs::{BuiltinKind, Engine};
    ///
    /// let engine = Engine::builder().build().unwrap();
    /// let mut names: Vec<&str> = engine.dispatchable_functions().map(|f| f.name).collect();
    /// names.sort_unstable();
    ///
    /// // Self-contained built-ins need no registration…
    /// assert!(names.contains(&"map"));
    /// // …but `enrich` ships as a config schema only, so with no handler
    /// // registered this engine cannot run it.
    /// assert!(!names.contains(&"enrich"));
    ///
    /// let validate = engine
    ///     .dispatchable_functions()
    ///     .find(|f| f.name == "validate")
    ///     .unwrap();
    /// assert_eq!(validate.kind, Some(BuiltinKind::SelfContained));
    /// assert_eq!(validate.aliases, &["validation"]);
    /// ```
    pub fn dispatchable_functions(&self) -> impl Iterator<Item = DispatchableFunction<'_>> {
        dispatchable_functions_in(self.workflow_executor.registry())
    }

    /// Whether this engine can actually run a task named `name`.
    ///
    /// `true` for a [`BuiltinKind::SelfContained`] built-in, which this crate
    /// executes itself, and for any name with a registered handler — including
    /// an alias such as `validation`.
    ///
    /// `false` means the opposite is guaranteed: a task naming it fails with
    /// [`DataflowError::FunctionNotFound`] on the first message that reaches
    /// it. That is the whole point of the method — `Engine::build` is
    /// deliberately permissive about `http_call` / `enrich` / `publish_kafka`,
    /// which deserialize into typed built-in variants and so pass construction
    /// even with no handler behind them.
    ///
    /// ```
    /// use dataflow_rs::Engine;
    ///
    /// let engine = Engine::builder().build().unwrap();
    ///
    /// assert!(engine.can_dispatch("map"));
    /// assert!(engine.can_dispatch("validation")); // alias of `validate`
    ///
    /// // Builds fine, would fail every message — this is the check that catches it.
    /// assert!(!engine.can_dispatch("enrich"));
    /// assert!(!engine.can_dispatch("never_registered"));
    /// ```
    pub fn can_dispatch(&self, name: &str) -> bool {
        can_dispatch_in(self.workflow_executor.registry(), name)
    }

    /// Check a workflow against this engine's registered handlers, without
    /// building anything.
    ///
    /// Answers the half of the question [`Workflow::validate_authored`] cannot:
    /// that method proves the definition *parses and validates*, but
    /// [`Engine::build`] also resolves every task to a handler and parses
    /// custom inputs. A definition can therefore be structurally perfect and
    /// still abort a build — which, in a host that builds one engine over many
    /// stored definitions, takes down every workflow in the process.
    ///
    /// Reports rather than aborts, so a host screens one definition at a time.
    /// Issues are anchored on [`WorkflowIssue::task_id`] — step ids are unique
    /// across tasks and groups — with a path relative to that task
    /// (`function.input`). Join it with the coordinate
    /// [`walk_authored_steps`](crate::walk_authored_steps) reports for that id
    /// to point at the authored document.
    ///
    /// `Workflow::tasks` is already flattened, so tasks inside groups are
    /// covered with no extra traversal.
    ///
    /// ```
    /// use dataflow_rs::{Engine, IssueCode, Workflow};
    ///
    /// let workflow = Workflow::from_json(r#"{
    ///     "id": "w", "name": "w", "priority": 0,
    ///     "tasks": [{"id": "lookup", "name": "lookup",
    ///                "function": {"name": "enrich",
    ///                             "input": {"connector": "c", "merge_path": "data.out"}}}]
    /// }"#).unwrap();
    ///
    /// // Builds cleanly — that permissiveness is deliberate.
    /// let engine = Engine::builder().build().unwrap();
    ///
    /// let issues = engine.check_workflow(&workflow);
    /// assert_eq!(issues[0].code, IssueCode::MissingHandler);
    /// assert_eq!(issues[0].task_id.as_deref(), Some("lookup"));
    /// ```
    pub fn check_workflow(&self, workflow: &Workflow) -> Vec<WorkflowIssue> {
        let compiler = TemplateCompiler::new(Arc::clone(&self.datalogic));
        authoring::check_against_registry(workflow, self.workflow_executor.registry(), &compiler)
    }

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
    datalogic_operators: HashMap<String, Arc<dyn datalogic_rs::CustomOperator>>,
    error_context_path: Option<String>,
    error_context_limit: Option<usize>,
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

    /// Every function this builder will dispatch once built.
    ///
    /// The pre-build twin of [`Engine::dispatchable_functions`], with identical
    /// semantics — the two agree by construction, since `build()` moves this
    /// registry into the engine unchanged. Takes `&self`, so screening a batch
    /// of definitions does not consume the builder.
    ///
    /// ```
    /// use dataflow_rs::Engine;
    ///
    /// let builder = Engine::builder();
    /// let names: Vec<&str> = builder.dispatchable_functions().map(|f| f.name).collect();
    ///
    /// assert!(names.contains(&"parse_json"));
    /// assert!(!names.contains(&"publish_kafka")); // config schema, no handler
    /// ```
    pub fn dispatchable_functions(&self) -> impl Iterator<Item = DispatchableFunction<'_>> {
        dispatchable_functions_in(&self.handlers)
    }

    /// Whether the engine this builder produces will run a task named `name`.
    ///
    /// The pre-build twin of [`Engine::can_dispatch`]. Screening a workflow is
    /// then a filter over its tasks — note that `Workflow::tasks` is already
    /// flattened, so this covers members of task groups too:
    ///
    /// ```
    /// use dataflow_rs::{Engine, Workflow};
    ///
    /// let workflow = Workflow::from_json(r#"{
    ///     "id": "w", "name": "w", "priority": 0,
    ///     "tasks": [
    ///         {"id": "a", "name": "a", "function": {"name": "map", "input": {"mappings": []}}},
    ///         {"id": "b", "name": "b",
    ///          "function": {"name": "enrich",
    ///                       "input": {"connector": "c", "merge_path": "data.out"}}}
    ///     ]
    /// }"#).unwrap();
    ///
    /// let builder = Engine::builder();
    /// let unrunnable: Vec<&str> = workflow
    ///     .tasks
    ///     .iter()
    ///     .map(|t| t.function.function_name())
    ///     .filter(|name| !builder.can_dispatch(name))
    ///     .collect();
    ///
    /// assert_eq!(unrunnable, vec!["enrich"]);
    /// ```
    pub fn can_dispatch(&self, name: &str) -> bool {
        can_dispatch_in(&self.handlers, name)
    }

    /// Check a workflow against this builder's registered handlers and
    /// operators, without consuming the builder or building an engine.
    ///
    /// The pre-build twin of [`Engine::check_workflow`], with identical
    /// semantics. Takes `&self`, so a host can screen a batch of definitions
    /// against the registrations it is about to build with.
    ///
    /// Templates are compiled against a datalogic engine configured exactly as
    /// [`Self::build`] will configure it — same custom operators, same
    /// templating mode — so a template that passes here compiles there.
    ///
    /// ```
    /// use dataflow_rs::{Engine, IssueCode, Workflow};
    ///
    /// let workflow = Workflow::from_json(r#"{
    ///     "id": "w", "name": "w", "priority": 0,
    ///     "tasks": [{"id": "t", "name": "t",
    ///                "function": {"name": "typo_handler", "input": {}}}]
    /// }"#).unwrap();
    ///
    /// let issues = Engine::builder().check_workflow(&workflow);
    /// assert_eq!(issues[0].code, IssueCode::UnknownFunction);
    /// assert_eq!(issues[0].task_id.as_deref(), Some("t"));
    /// ```
    pub fn check_workflow(&self, workflow: &Workflow) -> Vec<WorkflowIssue> {
        // Build the datalogic engine the same way `build()` does, so template
        // compilation here is the same operation it will be there — rather than
        // an approximation a caller has to keep in step by hand.
        let compiler = LogicCompiler::with_operators(&self.datalogic_operators);
        let template_compiler = TemplateCompiler::new(compiler.into_engine());
        authoring::check_against_registry(workflow, &self.handlers, &template_compiler)
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

    /// Mirror per-task failure codes into the message context at `path`, so a
    /// downstream `condition` or `map` can branch on *why* a task failed.
    ///
    /// Off unless called: with no path configured nothing is written and the
    /// mechanism costs one `Option` check on a path that only runs after a task
    /// has already failed.
    ///
    /// One record is appended per error a task contributes to
    /// [`Message::errors`](crate::engine::message::Message::errors):
    ///
    /// ```json
    /// { "workflow_id": "place_order", "task_id": "charge_payment",
    ///   "code": "TIMEOUT_ERROR", "status": 500 }
    /// ```
    ///
    /// so a later task can gate on the reason:
    ///
    /// ```json
    /// { "in": [ { "var": "metadata.errors.0.code" },
    ///           ["TIMEOUT_ERROR", "IO_ERROR"] ] }
    /// ```
    ///
    /// Coverage matches `errors()` exactly — a handler returning `Err`, a task
    /// returning a 5xx outcome, the `validation` built-in's per-rule failures, and
    /// anything a handler adds through
    /// [`TaskContext::add_error`](crate::engine::task_context::TaskContext::add_error)
    /// all appear. The workflow-level `WORKFLOW_ERROR` wrapper does not: it
    /// re-reports the same underlying failure, so mirroring it would double-count.
    ///
    /// `status` is the task's own status — `500` when the handler returned `Err`,
    /// otherwise the status the outcome carried (`400` for `validation`). That is
    /// the distinction `metadata.progress` cannot make, since its failure arm
    /// hard-codes `500`.
    ///
    /// The error `message` and the operator-only `detail` are deliberately **not**
    /// recorded: the context is serialized back to callers, and `detail` is
    /// documented as unsafe to hand to an untrusted one. Read those from
    /// `message.errors()` host-side.
    ///
    /// `path` must start with `data`, `metadata` or `temp_data` — the JSONLogic
    /// evaluation context is exactly those three slots — and may not be
    /// `metadata.progress`. Violations fail [`EngineBuilder::build`].
    pub fn with_error_context_path(mut self, path: impl Into<String>) -> Self {
        self.error_context_path = Some(path.into());
        self
    }

    /// Cap the number of records retained at the error-context path, keeping the
    /// most recent (default 32).
    ///
    /// The bound is what keeps the option's memory cost independent of a looping
    /// workflow's iteration count: `Message.context` is deep-cloned into every
    /// trace snapshot, so an uncapped list in a loop with a failing body grows the
    /// trace quadratically. Conditions overwhelmingly read the latest failure, so
    /// the oldest records are the ones dropped.
    ///
    /// Setting a limit without a path is inert, not an error. A limit of `0` fails
    /// [`EngineBuilder::build`].
    pub fn with_error_context_limit(mut self, limit: usize) -> Self {
        self.error_context_limit = Some(limit);
        self
    }

    /// Register a custom JSONLogic operator on the engine's internal datalogic
    /// instance, under `name`. Later calls with the same name replace the
    /// earlier registration.
    ///
    /// This is the host's door for domain operators: the engine builds (and on
    /// [`Engine::with_new_workflows`] *rebuilds*) its datalogic engine
    /// internally, where registration is builder-only — so operators must
    /// enter here to exist at all, and are retained on the engine so every
    /// hot reload re-registers them.
    ///
    /// Semantics follow `datalogic_rs`: arguments arrive pre-evaluated, and a
    /// built-in operator name always wins over a custom registration — pick
    /// names no built-in uses. Because the engine always runs in templating
    /// mode, a name that is *not* registered is not an error: the object
    /// echoes back as literal data, exactly like a disabled operator family.
    /// Registering a name therefore converts previously-inert values into
    /// live operator calls, the same caveat the cargo features carry.
    pub fn with_datalogic_operator<T>(mut self, name: impl Into<String>, operator: T) -> Self
    where
        T: datalogic_rs::CustomOperator + 'static,
    {
        self.datalogic_operators
            .insert(name.into(), Arc::new(operator));
        self
    }

    /// Compile the workflows, pre-parse Custom inputs, and produce the
    /// engine. Compile errors and missing handler references surface here —
    /// the engine never deserializes Custom config on the hot path.
    pub fn build(self) -> Result<Engine> {
        // Validated here rather than at the setter so an invalid path fails at
        // engine construction alongside every other config-shape error, instead
        // of on the first message that happens to fail a task.
        let error_context = match self.error_context_path {
            Some(path) => Some(Arc::new(ErrorContextConfig::new(
                path,
                self.error_context_limit
                    .unwrap_or(DEFAULT_ERROR_CONTEXT_LIMIT),
            )?)),
            None => None,
        };
        let engine = Engine::new_with_operators(
            self.workflows,
            self.handlers,
            Arc::new(self.datalogic_operators),
        )?;
        let engine = match error_context {
            Some(cfg) => engine.with_error_context(cfg),
            None => engine,
        };
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
