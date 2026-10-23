# Engine

The `Engine` executes messages against a compiled `Program`. It is the central component that selects matching workflows and orchestrates task execution.

## Overview

The `Engine` is responsible for:

- Running messages through active workflows in priority order
- Evaluating workflow and task conditions against the full message context (`data`, `metadata`, `temp_data`)
- Applying task and workflow outcome policies and recording audit entries and errors
- Filtering workflows by `channel` and evaluating canary `rollout` buckets
- Reporting execution outcomes via `Run`, emitting events to `Observer`s, and capturing `Trace`s when requested
- Providing a pre-configured `Compiler` (`engine.compiler()`) to build updated programs for hot-reloading

Compilation happens ahead of time: workflow definitions are parsed and linted by `def::parse`, compiled by a `Compiler` created from a `Registry`, and linked into an immutable `Program`. The `Engine` pairs that `Program` with an `EngineConfig`.

## Creating an Engine

`Engine::builder()` handles parsing, compilation, and linking in one step. If any workflow definition is invalid, `build()` returns `Err` with all collected issues.

```rust
use dataflow_rs::Engine;
use serde_json::json;

# fn main() -> Result<(), Vec<dataflow_rs::Issue>> {
let first = json!({
    "id": "first",
    "priority": 1,
    "tasks": [
        {"id": "t", "function": {"name": "map", "input": {
            "mappings": [{"path": "data.first", "logic": true}]}}}
    ]
});
let second = json!({
    "id": "second",
    "priority": 2,
    "tasks": [
        {"id": "t", "function": {"name": "map", "input": {
            "mappings": [{"path": "data.second", "logic": true}]}}}
    ]
});

let engine = Engine::builder()
    .workflow_json(&first)
    .workflow_json(&second)
    // .register("my_function", MyFunction)  // register custom host functions here
    .build()?;

// All expressions and function configs are compiled upfront.
println!("Loaded {} workflows", engine.program().workflows().len());
# assert_eq!(engine.program().workflows().len(), 2);
# Ok(()) }
```

### Builder Methods

- **`.workflow_json(&value)` / `.workflow(def)`** — `.workflow_json` parses a JSON definition with 3.x compatibility enabled; `.workflow` adds a `WorkflowDef` you parsed yourself.
- **`.register("name", function)` / `.layer(layer)` / `.registry(registry)`** — `.register` adds a custom [`Function`](../advanced/custom-functions.md); `.layer` wraps host calls in a middleware layer; `.registry` replaces the entire `Registry` (including its `ExprEngine` operators, secrets, operation budget, and any previously registered functions—so call `.registry(...)` before `.register(...)`).
- **`.options(CompileOptions)` / `.config(EngineConfig)`** — Configure compilation limits and runtime engine settings.
- **`.build()`** — Returns `Result<Engine, Vec<Issue>>`. If any workflow is rejected, `build()` returns all issues from the rejected definitions in input order (parse issues followed by compile issues). If all workflows compile, non-fatal `Defect` and `Advisory` issues are discarded; use `Compiler::check` when you want to inspect warnings on valid definitions.

```rust
use dataflow_rs::Engine;
use dataflow_rs::program::IssueCode;
use serde_json::json;

let issues = Engine::builder()
    .workflow_json(&json!({"id": "w", "tasks": [
        {"id": "t", "function": {"name": "fetch"}}
    ]}))
    .build()
    .unwrap_err();
assert_eq!(issues[0].code, IssueCode::UnknownFunction);
assert_eq!(issues[0].path.to_string(), "tasks[0].function.name");
println!("{}", issues[0]);
```

### Quarantining Invalid Definitions

When loading workflows from a database or external store, you often want valid workflows to run even if one definition is broken. Use `Compiler::compile_one` and `Engine::new(program, config)` to compile workflows individually and skip any that fail:

```rust
use dataflow_rs::def::{ParseOptions, parse};
use dataflow_rs::program::CompileOptions;
use dataflow_rs::run::EngineConfig;
use dataflow_rs::{Engine, Registry};
use serde_json::json;

let definitions = vec![
    json!({"id": "good", "tasks": [
        {"id": "t", "function": {"name": "map", "input": {
            "mappings": [{"path": "data.ok", "logic": true}]}}}
    ]}),
    json!({"id": "bad", "tasks": [
        {"id": "t", "function": {"name": "no_such_function"}}
    ]}),
];

let compiler = Registry::default().compiler(CompileOptions::default());
let mut workflows = Vec::new();
for raw in &definitions {
    // `parse` returns `Some(def)` whenever there are no `Rejected` parse issues.
    let parsed = parse(raw, ParseOptions::default());
    let Some(def) = parsed.def else {
        for issue in &parsed.issues {
            eprintln!("{issue}");
        }
        continue;
    };
    match compiler.compile_one(&def) {
        Ok(compiled) => workflows.push(compiled.workflow),
        Err(issues) => {
            for issue in &issues {
                eprintln!("{}: {issue}", def.id());
            }
        }
    }
}
let engine = Engine::new(compiler.link(workflows), EngineConfig::default());
assert_eq!(engine.program().workflows().len(), 1);
```

`compile_one` returns `Err(issues)` if any issue has `Severity::Rejected`, or `Ok(Compiled)` containing the compiled workflow along with any `Defect` and `Advisory` issues. `link` never fails: it sorts the compiled workflows by `priority` (preserving input order for ties) and indexes their tasks.

## Running Messages

```rust
# use dataflow_rs::Engine;
# async fn _demo(engine: Engine) {
use dataflow_rs::Message;
use dataflow_rs::run::RunStatus;
use serde_json::json;

// Create a message from a JSON payload.
let mut message = Message::from_json(&json!({
    "user": "john",
    "action": "login"
}));

// Process the message through all active workflows.
let run = engine.run(&mut message).await;

match run.status() {
    RunStatus::Completed => {}
    RunStatus::CompletedWithErrors => eprintln!("{} error(s)", message.errors().len()),
    RunStatus::Stopped(error) => eprintln!("stopped: {error}"),
    RunStatus::TimedOut | RunStatus::Cancelled => eprintln!("cut short"),
    _ => {}
}
println!("{} workflow(s) ran", run.matched());
println!("data: {:?}", message.context().data());
println!("audit entries: {}", message.audit().len());
# }
```

### What `Run` Reports

- **`status()`** — `Completed`, `CompletedWithErrors` (all considered workflows finished, with one or more errors recorded on `message.errors()`), `Stopped(error)` (an unhandled task failure stopped the run), `TimedOut`, or `Cancelled`.
- **`matched()`** — Number of workflows that matched and ran.
- **`workflows()`** — Per-workflow execution records (`WorkflowRecord`) in priority order up to the point the run ended, including `selection` (`Ran`, `SkippedByCondition`, `ConditionFailed(error)`, `SkippedByRollout`), `sweeps` (iteration count), and `exit` (`Completed`, `Halted`, `Failed`, `Stopped`).
- **`partial()` & `retryable()`** — `partial()` is `true` if the run stopped before considering all workflows; `retryable()` indicates whether retrying the message may succeed.
- **`trace()` & `take_trace()`** — Returns the execution `Trace` if tracing was enabled in `RunOptions`.
- **`into_result()`** — Converts `Run` into `Result<Run, Error>`, returning `Ok(run)` for `Completed` / `CompletedWithErrors` and `Err` for `Stopped`, `TimedOut`, or `Cancelled` (useful when you want to propagate fatal stops with `?`).

If you already have an `OwnedDataValue` (or `Arc<OwnedDataValue>`), pass it to `Message::new` to share the payload without cloning:

```rust
use dataflow_rs::Message;
use dataflow_rs::datavalue::OwnedDataValue;
use serde_json::json;
use std::sync::Arc;

let payload = Arc::new(OwnedDataValue::from(&json!({"user": "john"})));
let a = Message::new(Arc::clone(&payload));
let b = Message::new(payload);
assert_eq!(a.payload(), b.payload());
```

### Run Options

Use `engine.run_with(&mut message, &RunOptions)` to customize a single run:

| Option | Default | Description |
|---|---|---|
| `channel(&str)` | All active workflows | Run only active workflows assigned to this channel (also stamps `metadata.channel`). |
| `routing_bucket(u8)` | None | Canary rollout bucket (`0..=99`). When omitted, all workflow rollouts match. |
| `capture_changes(bool)` | `true` | Record old and new field values (`Change`) in each task's audit entry. |
| `audit(AuditMode)` | `Full` | Which audit entries to keep on the message: `Full`, `Last(n)`, or `Off`. |
| `deadline(Instant)` | None | End the run with `TimedOut` once the deadline passes. Checked at every host call, fan-out element, loop sweep, and every 64th expression op; async host calls are raced against a Tokio timer. |
| `cancel(&CancelToken)` | None | End the run with `Cancelled` when the token is triggered (checked at the same points as `deadline`). |
| `layers(&[Arc<dyn CallLayer>])` | None | Wrap host calls for this run in additional middleware layers outside the registry's layers. |
| `observer(&dyn Observer)` | None | Send execution events for this run to an additional observer alongside the engine's observer. |
| `trace(TraceOptions)` | None | Record an execution trace accessible via `Run::trace()`. |

```rust
# use dataflow_rs::{Engine, Message};
# async fn _demo(engine: Engine, mut message: Message) {
use dataflow_rs::run::{AuditMode, RunOptions};

let opts = RunOptions::default()
    .channel("orders")
    .audit(AuditMode::Last(10));
let run = engine.run_with(&mut message, &opts).await;
# let _ = run;
# }
```

> **Note:** Using `RunOptions::deadline` requires a Tokio runtime with the `time` feature enabled, because async host calls are raced against `tokio::time::sleep_until`. Only the run's own deadline or cancel token sets `RunStatus::TimedOut` or `RunStatus::Cancelled`; if a custom host function returns its own timeout error, it is handled like any other task failure under the task's `on_failure` policy.

## Execution Tracing

Enable tracing on a run with `RunOptions::trace(TraceOptions)`. The returned `Run` will carry a `Trace` containing:
- `initial`: the message context right after initial run stamps (`processed_at`, `engine_version`, `channel`) and before any workflow runs.
- `steps`: one `TraceStep` for every task call, skipped step, failure, and engine write, along with the `changes` it made.

Traces are returned on every run status, including `Stopped`, `TimedOut`, and `Cancelled`.

```rust
# use dataflow_rs::{Engine, Message};
# async fn _demo(engine: Engine, mut message: Message) {
use dataflow_rs::observe::TraceOptions;
use dataflow_rs::run::RunOptions;

let opts = RunOptions::default().trace(TraceOptions::default());
let run = engine.run_with(&mut message, &opts).await;
let trace = run.trace().expect("a trace was asked for");

for step in &trace.steps {
    println!(
        "{}/{}: {:?}, status {:?}, {} change(s)",
        step.workflow_id,
        step.task_id.as_deref().unwrap_or("-"),
        step.result,
        step.status,
        step.changes.len()
    );
}

// Replay step changes starting from `trace.initial` to inspect the context after each step.
for (step, context) in trace.steps.iter().zip(trace.contexts()) {
    println!("after {:?}: {:?}", step.task_id, context.data());
}
# }
```

Each step's `result` is `Executed` (the task or engine write ran and execution continued), `Skipped` (skipped due to a false condition or rollout mismatch), or `Failed` (the failure was handled by the workflow policy or stopped the run). `step.errors` holds any error records added during that step. For `map` tasks, `step.mapping_ends` marks the boundary after each mapping so `Trace::mapping_contexts` can reconstruct the context after individual mappings.

### Controlling Trace Overhead and Redaction

`TraceOptions` provides four settings to bound and sanitize traces:

```rust
# use dataflow_rs::{Engine, Message};
# async fn _demo(engine: Engine, mut message: Message) -> dataflow_rs::Result<()> {
use dataflow_rs::observe::TraceOptions;
use dataflow_rs::run::RunOptions;

let trace = TraceOptions::default()
    // Measure each task's execution duration in `step.elapsed` (off by default).
    .timings(true)
    // Redact sensitive subtrees in the trace: `initial` and any changes at or
    // below these paths are replaced with `null` in the trace while the live
    // message retains the real values.
    .redact(vec!["data.card.pan".parse()?])
    // Cap the total byte size of recorded changes. Once exceeded, subsequent
    // steps keep their metadata and status but omit `changes`, and `trace.truncated` is set.
    .max_bytes(256 * 1024);
let run = engine.run_with(&mut message, &RunOptions::default().trace(trace)).await;

if run.trace().is_some_and(|t| t.truncated) {
    println!("byte budget hit: later steps carry no changes");
}
# Ok(()) }
```

The fourth setting, `.changes(false)`, records step statuses and timings without capturing context diffs (useful for lightweight profiling, though the resulting trace cannot replay contexts). Note that trace change capture is independent of `RunOptions::capture_changes`, which controls only `message.audit()`.

## Always-On Metrics with `Observer`

While a `Trace` allocates a per-run log for debugging, an `Observer` provides zero-allocation, inline callbacks for metrics (counters, histograms, tracing spans) across every run:

```rust
use dataflow_rs::observe::{Event, Interest, Observer};
use dataflow_rs::run::EngineConfig;
use dataflow_rs::{Engine, Message};
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Default)]
struct Metrics {
    tasks: AtomicU64,
    total_us: AtomicU64,
}

impl Observer for Metrics {
    // Include `Interest::TIMINGS` only when you need `elapsed` durations;
    // omitting it avoids per-task clock reads.
    fn interest(&self) -> Interest {
        Interest::TASKS | Interest::TIMINGS
    }

    fn on(&self, event: &Event<'_>) {
        // Runs synchronously on the caller's thread: keep it fast and non-blocking.
        if let Event::TaskFinished { elapsed, .. } = event {
            self.tasks.fetch_add(1, Ordering::Relaxed);
            if let Some(elapsed) = elapsed {
                self.total_us
                    .fetch_add(elapsed.as_micros() as u64, Ordering::Relaxed);
            }
        }
    }
}

# #[tokio::main]
# async fn main() {
let metrics = Arc::new(Metrics::default());
let engine = Engine::builder()
    .config(EngineConfig::default().observer(metrics.clone()))
    .workflow_json(&json!({"id": "w", "tasks": [
        {"id": "t", "function": {"name": "map", "input": {
            "mappings": [{"path": "data.a", "logic": 1}]}}}
    ]}))
    .build()
    .unwrap();
engine.run(&mut Message::from_json(&json!({}))).await;
assert_eq!(metrics.tasks.load(Ordering::Relaxed), 1);
# }
```

An observer configured on `EngineConfig::observer` receives events for every run of the engine, while `RunOptions::observer(&obs)` attaches an observer to a single run. `interest()` is checked once at the start of each run:
- `Interest::WORKFLOWS`: enables `WorkflowStarted` and `WorkflowFinished`.
- `Interest::TASKS`: enables `TaskFinished`.
- `Interest::TIMINGS`: populates `elapsed` on workflow and task finish events.
- `RunStarted` and `RunFinished` are always delivered to every active observer.
- The default `Interest` is `WORKFLOWS | TASKS` (no per-step clock reads).

### Event Details

- **`Event::TaskFinished`** carries `info` (`workflow_id`, `task_id`, `function`, `loop_counter`, `element_index`), `status`, and `elapsed`:
  - `status` is `None` when the function returned `Outcome::Skip`, and `Some(500)` when the function returned `Err`.
  - Tasks skipped because their condition evaluated to `false` do not emit `TaskFinished`.
  - A `for_each` fan-out task emits `TaskFinished` once for each element it folds.
  - `elapsed` measures the function call itself and is `Some` only when `Interest::TIMINGS` is set.
- **`Event::WorkflowStarted` & `Event::WorkflowFinished`** fire only for workflows that actually run (workflows skipped by condition or rollout do not emit events). A looping workflow emits a single `WorkflowFinished` event covering all sweeps.
- **`Event::RunStarted`** reports `considered` (the number of active workflows in the target channel before rollout or condition checks). **`Event::RunFinished`** reports the final `RunStatus` and total error count on the message.

> **Important:** `Observer::on` runs synchronously on the execution path. Keep it non-blocking, do not call back into the engine, and do not panic. Always include `..` when pattern-matching `Event` variants, as they are marked `#[non_exhaustive]`.

## Workflow Execution Order

Workflows execute in ascending `priority` order (lowest number first), with ties broken by the order they were added to the program. Only `active` workflows run; `paused` and `archived` workflows remain in the compiled program for inspection but are skipped during execution.

```jsonc
[
    {"id": "high", "priority": 1, "tasks": [...]},
    {"id": "low", "priority": 10, "tasks": [...]}
]
```

When `RunOptions::channel` is set, only active workflows in that channel are considered. When a workflow specifies a `rollout` range, it is skipped if the run's `routing_bucket` falls outside that range (runs without a `routing_bucket` match all rollouts). See [Workflows](./workflow.md#workflow-lifecycle) for details.

## Workflow Conditions

A workflow's `condition` determines whether it runs. The engine evaluates it against the full message context (`data`, `metadata`, and `temp_data`):

```jsonc
{
    "id": "premium_order",
    "name": "Premium Order Processing",
    "condition": { ">=": [{"var": "data.order.total"}, 1000] },
    "tasks": [...]
}
```

A condition matches only when it evaluates to boolean `true` (truthy values like `1` or non-empty strings do not select the workflow). If a condition fails with an evaluation error, it is treated as `false` and recorded on `Run` as `Selection::ConditionFailed(error)`.

## Custom Host Functions

Register custom async functions on `EngineBuilder` with `.register("name", func)`. Each task's `input` is compiled into your function's `Function::Config` type at build time, catching configuration errors before any message is processed:

```rust
use dataflow_rs::{Ctx, Engine, Function, Outcome, Result};
use serde_json::json;

/// A simple host function that takes no config and makes no writes.
struct Ping;

impl Function for Ping {
    type Config = ();

    async fn call(&self, _ctx: &mut Ctx<'_>, _cfg: &()) -> Result<Outcome> {
        Ok(Outcome::Ok)
    }
}

# fn main() -> std::result::Result<(), Vec<dataflow_rs::Issue>> {
let engine = Engine::builder()
    .register("ping", Ping)
    .workflow_json(&json!({"id": "w", "tasks": [
        {"id": "t", "function": {"name": "ping"}}
    ]}))
    .build()?;
# let _ = engine;
# Ok(()) }
```

During execution, a host function's writes are buffered in a per-call overlay and committed to the message only when `call` returns `Ok`. Returning `Err` discards all context writes from that call while preserving any explicit error records added via `Ctx::record_error`. Note that the 8 built-in function names are reserved; attempting to register a custom function under a built-in name will panic.

## Thread Safety & Concurrency

`Engine` is `Send + Sync`, and `Engine::clone` is a cheap `Arc` reference count increment that shares the compiled program and configuration across threads. Because all mutable state lives on the `Message`, you can process many messages concurrently across Tokio tasks:

```rust
# use dataflow_rs::{Engine, Message};
# async fn _demo(engine: Engine, messages: Vec<Message>) {
let handles: Vec<_> = messages
    .into_iter()
    .map(|mut message| {
        let engine = engine.clone();
        tokio::spawn(async move {
            let run = engine.run(&mut message).await;
            (message, run)
        })
    })
    .collect();

for handle in handles {
    let (message, run) = handle.await.expect("the task ran");
    println!("{}: {:?}", message.id(), run.status());
}
# }
```

The future returned by `engine.run` / `engine.run_with` is `Send`.

## Hot-Reloading Workflows

Because a `Program` is immutable, you reload workflows by compiling a new `Program` with `engine.compiler()`—which reuses the engine's `Registry` (custom functions, layers, operators, and secrets) and `CompileOptions`—and swapping it in with `engine.with_program`:

```rust
# use dataflow_rs::Engine;
# use serde_json::Value;
# fn _demo(engine: Engine, new_definitions: Vec<Value>) -> Result<(), Vec<dataflow_rs::Issue>> {
use dataflow_rs::def::{ParseOptions, parse};

let compiler = engine.compiler();
let mut workflows = Vec::new();
for raw in &new_definitions {
    let parsed = parse(raw, ParseOptions::default());
    let def = parsed.def.ok_or(parsed.issues)?;
    workflows.push(compiler.compile_one(&def)?.workflow);
}
let reloaded = engine.with_program(compiler.link(workflows));

// `engine` is unchanged and finishes any in-flight runs; `reloaded`
// serves new runs with the updated program and the same `EngineConfig`.
# let _ = reloaded;
# Ok(()) }
```

If any new definition fails to parse or compile, you catch the issues before swapping, so a bad update never disrupts a running engine.

## API Summary

### `Engine::builder()`
Returns an `EngineBuilder`. Chain `.workflow_json(&value)`, `.workflow_json_owned(value)`, `.workflow(def)`, `.register("name", function)`, `.layer(layer)`, `.registry(registry)`, `.options(CompileOptions)`, and `.config(EngineConfig)`, then call `.build() -> Result<Engine, Vec<Issue>>`.

### `EngineConfig`
Holds engine-wide runtime settings preserved across reloads:
- `.observer(Arc<dyn Observer>)`: attaches an always-on observer to every run.
- `.error_context(ErrorContext::new(path, limit)?)`: mirrors task failure codes into `path` on the message context (retaining up to `limit` newest records). See [Error Handling](./error-handling.md#branching-on-task-failures).

### `Engine::new(program, config)`
Constructs an `Engine` from a pre-linked `Program` and an `EngineConfig`.

### `engine.run(&mut message)` & `engine.run_with(&mut message, &opts)`
Executes `message` through active workflows in priority order and returns a `Run`. Mutates `message` in place (updating its context, audit trail, and error list).

### `engine.compiler()`
Returns a `Compiler` sharing this engine's `Registry` and `CompileOptions`.

### `engine.with_program(program)`
Returns a new `Engine` that pairs `program` with this engine's `EngineConfig`.

### `engine.program()`
Returns a reference to the compiled `Program`. `program().workflows()` lists metadata for all linked workflows (`id`, `channel`, `priority`, `status`, `version`, including `paused` and `archived` workflows), and `program().tasks()` lists all compiled tasks:

```rust
# fn _demo(engine: dataflow_rs::Engine) {
for workflow in engine.program().workflows() {
    println!("{} on {} at priority {}", workflow.id, workflow.channel, workflow.priority);
}
# }
```

## Migrating from 3.x

In 3.x, the engine was constructed from `Workflow` structs and executed via the `process_message*` methods (`process_message_with_trace`, `process_message_for_channel`, etc.), observed via a five-method `ExecutionObserver` trait, traced with full-message snapshots, and reloaded with `with_new_workflows`.

In 4.0:
- Every execution goes through `run` or `run_with` and returns a `Run`.
- Channels, traces, deadlines, and per-run observers are configured on `RunOptions`.
- Traces store compact per-step diffs instead of full message snapshots.
- Hot-reloading uses `engine.compiler()`, `compiler.link(...)`, and `engine.with_program(...)`.

See [Migrating from 3.x](../migration/from-3.md) for the complete migration guide.
