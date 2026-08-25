# Rules Engine

The Engine (also available as `RulesEngine` type alias) is the central component that evaluates rules and orchestrates action execution.

## Overview

The Engine is responsible for:

- Compiling all JSONLogic expressions at initialization
- Pre-sorting rules by priority at startup (no per-message sorting)
- Evaluating rule conditions against the full message context
- Processing messages through matching rules
- Channel-based routing with O(1) lookup
- Coordinating action execution
- Hot-reloading workflows without losing custom functions

## Creating an Engine

```rust
# fn _demo() -> dataflow_rs::Result<()> {
use dataflow_rs::{Engine, Workflow};

// Parse rules from JSON
let rule1 = Workflow::from_json(r#"{
    "id": "rule1",
    "name": "First Rule",
    "priority": 1,
    "tasks": [...]
}"#)?;

let rule2 = Workflow::from_json(r#"{
    "id": "rule2",
    "name": "Second Rule",
    "priority": 2,
    "tasks": [...]
}"#)?;

// Builder is the recommended construction path.
let engine = Engine::builder()
    .with_workflow(rule1)
    .with_workflow(rule2)
    // .register("my_handler", MyHandler)  // chain custom handlers here
    .build()?;

// Engine is now ready — all JSONLogic compiled, Custom inputs typed.
println!("Loaded {} rules", engine.workflows().len());
# Ok(()) }
```

You can also use the `RulesEngine` type alias:

```rust
# use dataflow_rs::Workflow;
# fn _demo() -> dataflow_rs::Result<()> {
# let rule1 = Workflow::from_json(r#"{"id":"a","tasks":[]}"#)?;
# let rule2 = Workflow::from_json(r#"{"id":"b","tasks":[]}"#)?;
use dataflow_rs::RulesEngine;

let engine = RulesEngine::builder()
    .with_workflows([rule1, rule2])
    .build()?;
# Ok(()) }
```

## Processing Messages

```rust
# use dataflow_rs::Engine;
# async fn _demo(engine: Engine) -> dataflow_rs::Result<()> {
use dataflow_rs::engine::message::Message;
use serde_json::json;

// Bridge from serde_json::Value — handiest when payloads come from JSON
let mut message = Message::from_value(&json!({
    "user": "john",
    "action": "login"
}));

// Process through all matching rules
engine.process_message(&mut message).await?;

// Access results
println!("Processed data: {:?}", message.data());
println!("Audit trail: {:?}", message.audit_trail());
# Ok(()) }
```

If you already have an `Arc<OwnedDataValue>` payload, use `Message::new`
to skip the `serde_json` bridge:

```rust
# use dataflow_rs::Message;
# use serde_json::json;
# fn _demo() {
use dataflow_rs::datavalue::OwnedDataValue;
use std::sync::Arc;

let payload = Arc::new(OwnedDataValue::from(&json!({"user": "john"})));
let mut message = Message::new(payload);
# }
```

## Execution Tracing

For debugging, use `process_message_with_trace` to capture step-by-step execution:

```rust
# use dataflow_rs::{Engine, Message};
# async fn _demo(engine: Engine, mut message: Message)
#     -> dataflow_rs::Result<()> {
let trace = engine.process_message_with_trace(&mut message).await?;

println!("Steps executed: {}", trace.executed_count());
println!("Steps skipped: {}", trace.skipped_count());

for step in &trace.steps {
    println!("Rule: {}, Action: {:?}, Result: {:?}",
        step.workflow_id, step.task_id, step.result);
}
# Ok(()) }
```

### Tracing a run that fails

`process_message_with_trace` returns the trace by value, so the `?` above
discards it when the engine stops early — on a hard failure you get `Err` and no
steps at all, which is the opposite of what a debugging API should do.

When the run you need to inspect is the run that failed, pass a trace you own:

```rust
# use dataflow_rs::{Engine, ExecutionTrace, Message};
# async fn _demo(engine: Engine, mut message: Message) {
let mut trace = ExecutionTrace::new();
let result = engine.process_message_tracing(&mut message, &mut trace).await;

// Whether the run succeeded or stopped early, `trace` holds the steps that ran.
for step in &trace.steps {
    println!("{}: {:?}", step.workflow_id, step.result);
}

if let Err(e) = result {
    println!("stopped early after {} steps: {e}", trace.executed_count());
}
# }
```

Steps are **appended**, so one trace can accumulate across a chain of calls.
Note that the failing task's own step is not recorded — the engine propagates the
failure before appending it — so the trace ends at the last known-good step. The
error itself comes from the returned `Err` and from `message.errors()`.

`process_message_for_channel_tracing` is the channel-scoped equivalent.

### Bounding what a trace captures

The default policy takes a full `Message` snapshot on every executed step. That
is unbounded in message size and **quadratic in task count** — each snapshot
clones the accumulated audit trail, so an N-task workflow retains `N*(N+1)/2`
audit entries. Fine for a step debugger, ruinous for a service that persists a
trace per request.

`TraceOptions` bounds it at capture time, which is the only place it can be
bounded: trimming the result afterwards has already paid the peak memory.

```rust
# use dataflow_rs::{AuditTrailScope, Engine, Message, TraceOptions};
# async fn _demo(engine: Engine, mut message: Message) -> dataflow_rs::Result<()> {
let trace = engine
    .process_message_with_trace_options(
        &mut message,
        TraceOptions {
            // Bound retained snapshots. Approximate in-memory size, not
            // serialized length — 0 means unbounded.
            max_snapshot_bytes: 256 * 1024,
            // Drop the quadratic term while keeping the step view working.
            snapshot_audit_trail: AuditTrailScope::Own,
            // Never let these subtrees reach the trace. The live message keeps
            // its real values, so later tasks are unaffected.
            redact_paths: vec!["data.card.pan".to_string()],
            // Per-step diff attributed to the task that produced it.
            changes: true,
            ..Default::default()
        },
    )
    .await?;

if trace.truncated() {
    println!("snapshot budget hit — some steps carry no message");
}
# Ok(()) }
```

For metrics rather than debugging, `TraceOptions::timings_only()` drops snapshots
and mapping contexts entirely, leaving ids, result, timing and the diff — a step
costs a few hundred bytes regardless of message size:

```rust
# use dataflow_rs::{Engine, Message, TraceOptions};
# async fn _demo(engine: Engine, mut message: Message) -> dataflow_rs::Result<()> {
let trace = engine
    .process_message_with_trace_options(&mut message, TraceOptions::timings_only())
    .await?;

for step in &trace.steps {
    if let Some(us) = step.duration_us {
        println!("{}/{:?} took {us}us", step.workflow_id, step.task_id);
    }
}
# Ok(()) }
```

Two things to know about `snapshots: false`: `final_message()` returns `None` and
`is_success()` degenerates to `true` (read `Message::errors` on the message you
passed in instead), and the `dataflow-ui` step debugger cannot render a step view
without snapshots.

Timing covers the **sync built-ins too** — `map`, `validation`, `filter`, the
`parse_*` and `publish_*` pair and `log` are dispatched inside the executor and
cannot be wrapped from outside the crate, so this is the only place their
duration is observable. Trace mode reads the clock twice per executed task; the
non-trace `process_message` path is unchanged and still takes one `Utc::now()`
per message.

## Always-on per-task metrics

A trace is a per-request allocation you persist. For aggregation — counters,
histograms, spans — attach an `ExecutionObserver` instead. It fires once per
dispatched task on every `process_message` call, with no trace involved:

```rust
use dataflow_rs::{Engine, ExecutionObserver, TaskEvent, Workflow};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Default)]
struct Metrics {
    tasks: AtomicU64,
    total_us: AtomicU64,
}

impl ExecutionObserver for Metrics {
    fn task_finished(&self, event: &TaskEvent<'_>) {
        // Must be cheap and non-blocking — see the contract below.
        self.tasks.fetch_add(1, Ordering::Relaxed);
        self.total_us
            .fetch_add(event.duration.as_micros() as u64, Ordering::Relaxed);
    }
}

# fn _demo(workflow: Workflow) -> dataflow_rs::Result<()> {
let metrics = Arc::new(Metrics::default());
let engine = Engine::builder()
    .with_workflow(workflow)
    .with_observer(metrics.clone())
    .build()?;
# Ok(()) }
```

`TaskEvent` carries `workflow_id`, `task_id`, `function`, `status` and
`duration`. Notes on the edges:

- **`status`** is `None` when the handler returned `TaskOutcome::Skip` (the body
  ran, but no audit entry was recorded), and `Some(500)` when the task returned
  `Err`. The event is emitted *before* the error propagates, so failing tasks are
  reported rather than dropped.
- **A task whose condition evaluated false is not reported** — it was never
  dispatched, so there is nothing to time.
- **`function`** reports `"validate"` for both `validation` and `validate`
  configs; they share one variant.
- **`duration`** is the task body only — not the condition evaluation, the
  audit-trail push, or the `metadata.progress` write.

The callback runs **synchronously on the executor's thread**, and on the sync
built-in path inside the arena scope. So it must not block, must not re-enter the
engine, and must not panic — a panic unwinds out of `process_message`. Push to a
channel or bump an atomic.

With no observer attached the instrumentation stays out of the dispatch path
entirely, including its clock reads, so `process_message` keeps its one
`Utc::now()` per message. The observer is carried across
`with_new_workflows`, so a hot reload does not silently stop reporting.

If you build your handler map in one place rather than calling `register` per
name, `EngineBuilder::with_handlers` takes the whole `HashMap` so you can still
reach `with_observer`.

### Message and rule lifecycle

`ExecutionObserver` carries four more callbacks, all defaulted to no-ops so an
existing observer keeps compiling: `message_started`, `message_finished`,
`workflow_started` and `workflow_finished`.

They make engine overhead directly measurable rather than a host-side residual:
`workflow_finished.duration` minus the task durations inside that workflow is
its condition evaluation, group gating, loop bookkeeping, audit writes and arena
management.

```rust
use dataflow_rs::{ExecutionObserver, MessageFinished, TaskEvent, WorkflowFinished};
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Default)]
struct Overhead {
    workflow_us: AtomicU64,
    task_us: AtomicU64,
}

impl ExecutionObserver for Overhead {
    fn workflow_finished(&self, event: &WorkflowFinished<'_>) {
        self.workflow_us
            .fetch_add(event.duration.as_micros() as u64, Ordering::Relaxed);
        if event.halted {
            println!("{} halted after {} sweep(s)", event.workflow_id, event.sweeps);
        }
    }

    fn message_finished(&self, event: &MessageFinished<'_>) {
        println!(
            "{}: {} error(s), stopped_early={}",
            event.message_id, event.errors, event.stopped_early
        );
    }

    fn task_finished(&self, event: &TaskEvent<'_>) {
        self.task_us
            .fetch_add(event.duration.as_micros() as u64, Ordering::Relaxed);
    }
}
```

The edges mirror `task_finished`:

- A rule that its **rollout gate or its condition rejected never starts** — no
  `workflow_started`, no `workflow_finished`, exactly as a skipped task is not
  reported.
- `message_finished` fires whether the run completed or stopped early;
  `stopped_early` distinguishes them, and `errors` is
  `message.errors().len()` at the end of the run.
- A **looping** rule reports **one** `workflow_finished` for the whole loop,
  carrying `sweeps` — per-sweep events would explode cardinality.
- `MessageStarted::workflows_considered` is how many rules are about to be
  *considered*, not how many will run. How many actually ran is the number of
  `workflow_started` callbacks in between.

All four event types are `#[non_exhaustive]`, so matching on them uses field
access rather than a struct pattern.

## Rule Execution Order

Rules execute in priority order (lowest priority number first):

```rust
# use dataflow_rs::Workflow;
# fn _demo() -> dataflow_rs::Result<()> {
// Priority 1 executes first
let high_priority = Workflow::from_json(r#"{
    "id": "high",
    "priority": 1,
    "tasks": [...]
}"#)?;

// Priority 10 executes later
let low_priority = Workflow::from_json(r#"{
    "id": "low",
    "priority": 10,
    "tasks": [...]
}"#)?;
# Ok(()) }
```

## Rule Conditions

Rules have conditions that determine if they should execute. Conditions are evaluated against the **full message context** — `data`, `metadata`, and `temp_data`:

```json
{
    "id": "premium_order",
    "name": "Premium Order Processing",
    "condition": { ">=": [{"var": "data.order.total"}, 1000] },
    "tasks": [...]
}
```

The rule only executes if the condition evaluates to true.

## Custom Functions

Register custom action handlers via the builder. `register("name", handler)`
accepts any [`AsyncFunctionHandler`](../advanced/custom-functions.md) and
boxes it internally; the engine pre-parses each `FunctionConfig::Custom`
input JSON into the handler's typed `Self::Input` at `.build()` time, so
mis-shaped configs fail at startup, not on first message.

```rust
# use async_trait::async_trait;
# use dataflow_rs::prelude::*;
# struct MyCustomFunction;
# #[async_trait]
# impl AsyncFunctionHandler for MyCustomFunction {
#     type Input = ();
#     async fn execute(&self, _c: &mut TaskContext<'_>, _i: &())
#         -> Result<TaskOutcome> { Ok(TaskOutcome::Success) }
# }
# fn _demo(rules: Vec<Workflow>) -> Result<()> {
let engine = Engine::builder()
    .with_workflows(rules)
    .register("my_function", MyCustomFunction)
    .build()?;
# Ok(()) }
```

## Thread Safety

The Engine is designed for concurrent use:

- Rules are immutable after creation
- Compiled logic is shared via `Arc`
- Each message is processed independently

```rust
# use dataflow_rs::{Engine, Message, Workflow};
# async fn _demo(rules: Vec<Workflow>, messages: Vec<Message>)
#     -> std::result::Result<(), Box<dyn std::error::Error>> {
use std::sync::Arc;
use tokio::task;

let engine = Arc::new(Engine::builder().with_workflows(rules).build()?);

// Process multiple messages concurrently
let handles: Vec<_> = messages.into_iter().map(|mut msg| {
    let engine = Arc::clone(&engine);
    task::spawn(async move {
        engine.process_message(&mut msg).await
    })
}).collect();

// Wait for all to complete
for handle in handles {
    handle.await??;
}
# Ok(()) }
```

## API Reference

### `Engine::builder()`

Returns an [`EngineBuilder`](../api/reference.md). Chain
`.register("name", handler)`, `.register_boxed(name, boxed)`,
`.with_workflow(w)`, `.with_workflows(iter)`, `.with_handlers(map)`,
`.with_observer(obs)`, `.with_datalogic_operator(name, op)`,
`.with_error_context_path(path)`, `.with_error_context_limit(n)`, then
`.build() -> Result<Engine>`. Recommended construction path.

### `EngineBuilder::with_error_context_path(path)`

Mirror per-task failure codes into `path` inside the message context, so a
downstream `condition` or `map` can branch on *why* a task failed. Off unless
called. See [Error Handling](./error-handling.md#branching-on-why-a-task-failed)
for the record shape and the coverage rules; `.with_error_context_limit(n)` caps
how many records are retained (default 32, newest kept).

### `Engine::new(workflows, custom_functions)`

Lower-level escape hatch — accepts rules and a plain handler `HashMap`
(use `HashMap::new()` for no custom handlers, or — preferred — go
through the builder).

- `workflows: Vec<Workflow>` — Rules to register
- `custom_functions: HashMap<String, BoxedFunctionHandler>` — Custom
  action implementations

### `engine.process_message(&mut message)`

Processes a message through all matching rules.

- Returns `Result<()>` - Ok if processing succeeded
- Message is modified in place with results and audit trail

### `engine.process_message_with_trace(&mut message)`

Processes a message and returns an execution trace for debugging.

- Returns `Result<ExecutionTrace>` - Contains all execution steps with message snapshots
- Useful for step-by-step debugging and visualization
- On `Err` the trace is discarded — use `process_message_tracing` to keep it

### `engine.process_message_tracing(&mut message, &mut trace)`

Same as `process_message_with_trace`, but records into a caller-owned trace so
the steps survive a hard failure.

- Returns `Result<()>` - the trace is borrowed rather than returned
- Steps are appended; any already present are preserved
- The failing task's own step is not recorded (see [Tracing a run that fails](#tracing-a-run-that-fails))

### `engine.workflows()`

Returns a reference to the registered rules (sorted by priority).

```rust
# fn _demo(engine: dataflow_rs::Engine) {
let count = engine.workflows().len();
# }
```

### `engine.workflow_by_id(id)`

Find a specific workflow by its ID.

```rust
# fn _demo(engine: dataflow_rs::Engine) {
if let Some(workflow) = engine.workflow_by_id("my_rule") {
    println!("Found: {}", workflow.name);
}
# }
```

### `engine.process_message_for_channel(channel, message)`

Processes a message through only the active workflows on a specific channel. Uses O(1) channel index lookup.

```rust
# use dataflow_rs::{Engine, Message};
# async fn _demo(engine: Engine, mut message: Message)
#     -> dataflow_rs::Result<()> {
engine.process_message_for_channel("orders", &mut message).await?;
# Ok(()) }
```

Only workflows with `status: "active"` are included in channel routing.

### `engine.process_message_for_channel_with_trace(channel, message)`

Same as `process_message_for_channel` but returns an execution trace for debugging.

```rust
# use dataflow_rs::{Engine, Message};
# async fn _demo(engine: Engine, mut message: Message)
#     -> dataflow_rs::Result<()> {
let trace = engine.process_message_for_channel_with_trace("orders", &mut message).await?;
# Ok(()) }
```

### `engine.process_message_for_channel_tracing(channel, message, trace)`

Channel-scoped `process_message_tracing`. An unknown channel is a no-op: returns
`Ok(())` and leaves the trace untouched.

```rust
# use dataflow_rs::{Engine, ExecutionTrace, Message};
# async fn _demo(engine: Engine, mut message: Message) {
let mut trace = ExecutionTrace::new();
let result = engine
    .process_message_for_channel_tracing("orders", &mut message, &mut trace)
    .await;
# let _ = result;
# }
```

### `engine.with_new_workflows(workflows)`

Creates a new engine with different workflows while preserving custom function registrations. Useful for hot-reloading workflow definitions at runtime.

It returns `Result<Engine>`, not `Engine`: the new definitions are compiled and
validated here, so a bad reload surfaces as an error instead of replacing a
working engine with a broken one.

```rust
# use dataflow_rs::{Engine, Workflow};
# fn _demo(engine: Engine) -> dataflow_rs::Result<()> {
let new_workflows = vec![Workflow::from_json(r#"{ ... }"#)?];
let new_engine = engine.with_new_workflows(new_workflows)?;

// Old engine is still valid for in-flight messages
// New engine has freshly compiled logic + same custom functions
# Ok(()) }
```
