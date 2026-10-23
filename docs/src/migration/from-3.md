# Migrating from 3.x

Dataflow-rs 4.0 maintains backward compatibility for stored 3.x workflow
definitions and serialized messages while introducing a cleaner, compile-time
validated Rust API.

- **Workflow authors:** Read [Backward Compatibility for 3.x Workflows](#backward-compatibility-for-3x-workflows)
  and [Behavioral Changes for Workflow Authors](#behavioral-changes-for-workflow-authors).
- **Rust application developers:** Read those sections first, then follow
  [Rust API Migration Guide](#rust-api-migration-guide) and the
  [Pre-Migration Checklist](#pre-migration-checklist).

## Backward Compatibility for 3.x Workflows

By default, `def::parse` enables the 3.x compatibility reader
(`ParseOptions::compat(true)`). It automatically translates 3.x workflow
spellings into their 4.0 equivalents in memory and reports each translation as a
`V3_SPELLING` advisory at the authored JSON path. Your existing 3.x workflow
definitions load and run without requiring immediate database migrations.

### Automatic Rewrites

| 3.x Spelling | 4.0 Equivalent | Issue Reported |
|---|---|---|
| `continue_on_error: true` / `false` on a workflow | `on_failure: "continue"` / `"fail"` | `V3_SPELLING` (`Advisory`) |
| `continue_on_error` and `halt_on` on a task | `on_failure` and `on_rejection` (see table below) | `V3_SPELLING` (`Advisory`) |
| `"name": "validate"` on a task | `"name": "validation"` | `V3_SPELLING` (`Advisory`) |
| Unknown key on a workflow, task, or group (e.g., UI `"path"`) | Moved into that node's `ext` object (`ext.path`) | `UNKNOWN_KEY` (`Advisory`) |
| Unknown key close to a valid key name (e.g., `"conditon"`) | Moved into `ext` | `LIKELY_TYPO` (`Defect`) |
| Unknown key inside `loop`, `for_each`, or `rollout` | Moved into the owning node's `ext` as `loop.<key>`, etc. | `UNKNOWN_KEY` or `LIKELY_TYPO` |
| Unknown key inside a built-in's `input`, `map` mapping, or `validation` rule | Moved into the task's `ext` under its relative path (e.g., `function.input.mappings[0].note`) | `UNKNOWN_KEY` or `LIKELY_TYPO` |
| `continue_on_error` on a task group | Moved into the group's `ext` | `GROUP_CONTINUE_ON_ERROR` (`Advisory`) |
| Timestamp formatted as `2024-01-01 00:00:00 UTC` | Parsed cleanly and serialized as RFC 3339 | None |

An unknown key is flagged as a `LIKELY_TYPO` (`Severity::Defect`) when it is
within an edit distance of 1 (for keys up to 5 characters) or 2 (for longer
keys) of a known key at that level.

### Task Policy Mapping

In 3.x, tasks used two flags (`continue_on_error` and `halt_on`). In 4.0, tasks
use two explicit policies (`on_failure` for `5xx` statuses and `Err`, and
`on_rejection` for `4xx` statuses):

| 3.x Task Flags | 4.0 Task Policies | Behavior |
|---|---|---|
| Neither | `on_failure: "fail"`, `on_rejection: "continue"` | `5xx` or `Err` propagates to the workflow's `on_failure` policy; `4xx` is recorded and execution continues to the next task. |
| `continue_on_error: true` | `on_failure: "continue"`, `on_rejection: "continue"` | Both rejections and failures are recorded, and execution continues to the next task. |
| `halt_on: "failure"` | `on_failure: "fail"`, `on_rejection: "halt"` | `4xx` halts the current workflow; `5xx` or `Err` propagates to the workflow's `on_failure` policy. |
| Both `continue_on_error: true` and `halt_on: "failure"` | `on_failure: "halt"`, `on_rejection: "halt"` | Any `4xx`, `5xx`, or `Err` halts the current workflow while allowing subsequent workflows to run. |
| `terminal: true` | `terminal: true` | Unchanged (applied after outcome policies, so failures under `on_failure: "fail"` still propagate). |
| Workflow `continue_on_error: false` / `true` | Workflow `on_failure: "fail"` / `"continue"` | `"continue"` ends the current workflow (or advances to the next loop iteration) and moves on to the next workflow. |

Default 3.x values (`halt_on: "never"` and `continue_on_error: false`) map to
the 4.0 defaults and are omitted when rewritten by `migrate_v3`, while still
reporting `V3_SPELLING`.

**Do not mix 3.x and 4.0 policy keys on the same node.** Combining a 3.x flag
(`continue_on_error` or `halt_on`) with a 4.0 policy (`on_failure` or
`on_rejection`) on the same workflow or task is rejected with `MIXED_SPELLING`.

### Upgrading Stored Definitions to 4.0 JSON (`compat::migrate_v3`)

Use `compat::migrate_v3` to permanently convert stored 3.x workflow JSON into
canonical 4.0 JSON (which parses cleanly with `.compat(false)`). It returns the
rewritten JSON, an ordered list of `rewrites`, and any non-rejected `issues`:

```rust
use dataflow_rs::compat::migrate_v3;
use dataflow_rs::program::IssueCode;
use serde_json::json;

let stored = json!({
    "id": "checkout",
    "continue_on_error": true,
    "tasks": [
        {"id": "guard", "halt_on": "failure", "path": "guards/1",
         "function": {"name": "validate", "input": {"rules": [
             {"logic": {"!!": [{"var": "data.order"}]}, "message": "no order"}
         ]}}}
    ]
});

let migrated = migrate_v3(&stored).unwrap();
assert_eq!(migrated.json, json!({
    "id": "checkout",
    "on_failure": "continue",
    "tasks": [
        {"id": "guard", "on_rejection": "halt", "ext": {"path": "guards/1"},
         "function": {"name": "validation", "input": {"rules": [
             {"logic": {"!!": [{"var": "data.order"}]}, "message": "no order"}
         ]}}}
    ]
}));

let rewrites: Vec<(String, String)> = migrated.rewrites.iter()
    .map(|r| (r.path.to_string(), r.to_string()))
    .collect();
// Reported in document key order:
assert_eq!(rewrites, [
    (
        "continue_on_error".to_string(),
        "`continue_on_error: true` is a v3 spelling, read as `on_failure: continue`".to_string(),
    ),
    (
        "tasks[0].halt_on".to_string(),
        "`halt_on: failure` is a v3 spelling, read as `on_failure: fail, on_rejection: halt`".to_string(),
    ),
    (
        "tasks[0].function.name".to_string(),
        "`validate` is a v3 spelling, read as `validation`".to_string(),
    ),
]);

let codes: Vec<IssueCode> = migrated.issues.iter().map(|i| i.code).collect();
assert_eq!(codes, [
    IssueCode::V3Spelling, IssueCode::V3Spelling, IssueCode::V3Spelling, IssueCode::UnknownKey,
]);
assert_eq!(migrated.issues[3].path.to_string(), "tasks[0].path");
# // Verify that the migrated JSON parses with `.compat(false)` to the same definition:
# use dataflow_rs::def::{ParseOptions, parse};
# let off = parse(&migrated.json, ParseOptions::default().compat(false));
# assert_eq!(off.def, parse(&stored, ParseOptions::default()).def);
# assert!(off.issues.is_empty());
```

## Behavioral Changes for Workflow Authors

Core execution semantics remain unchanged: a failed `validation` task still
returns status `400` and only halts the workflow when `on_rejection: "halt"` is
set, `null` results in `map` are skipped by default, `terminal` and halt
policies apply after status classification, loops and `for_each` behave the same
way, group conditions evaluate once on entry, and `metadata.progress` is written
after every task.

The following edge cases and validations have been tightened in 4.0:

- **`map` paths must start with `data`, `metadata`, or `temp_data`:** A literal
  `path` outside the three context roots is rejected at compile time with
  `INVALID_PATH` (3.x created a fourth top-level key on the context). A dynamic
  path that resolves outside the three roots fails the mapping at runtime with
  status `500`.
- **Literal `payload` reads in `var` are flagged as a `Defect`:** Writing
  `{"var": "payload.x"}` is reported as `PAYLOAD_VAR` because `payload` is not
  in the evaluation context (it always resolved to `null` in 3.x as well). Parse
  `payload` into `data` first using `parse_json` or `parse_xml`.
- **No wrapper `WORKFLOW_ERROR` records:** Each task's `ErrorRecord` already
  carries `workflow_id` and `task_id`. Workflow-level errors (such as a
  `loop.over` expression that does not evaluate to an array) record a single
  `LOGIC_ERROR` with `workflow_id` set and `task_id: None`.
- **`name` is optional** on workflows, tasks, and groups.
- **Only `active` workflows execute:** Workflows with `status: "paused"` or
  `"archived"` are skipped regardless of whether a channel is specified, and do
  not appear in `Run::workflows()`.
- **Unregistered functions are reported per task:** Calling an unknown function
  reports `UNKNOWN_FUNCTION` at `tasks[i].function.name` and rejects only the
  affected workflow rather than failing the entire registry build on the first
  missing function.
- **Integration config types (`http_call`, `enrich`, `publish_kafka`) were
  removed from the core crate:** These are host functions that your application
  registers like any other `Function`. If your stored workflows still use 3.x
  field names (`path_logic`, `body_logic`, `key_logic`, `value_logic`), rename
  them in your loader or support them on your `FunctionConfig` struct with
  `#[config(rename = ...)]`.
- **Unknown keys in host function inputs are strictly rejected:** While unknown
  keys on workflow/task/group nodes are moved into `ext` when `compat` is on,
  unknown keys inside a custom host function's `input` are always rejected with
  `LIKELY_TYPO` or `UNKNOWN_KEY`.
- **Array index paths are capped at `1,048,575` (`MAX_ARRAY_INDEX`):** Numeric
  path segments above `1,048,575` are treated as object keys rather than growing
  an array without bound.
- **Context roots must always be objects:** Neither `map` nor a custom host
  function can replace `data`, `metadata`, or `temp_data` with a scalar or
  array.

## Rust API Migration Guide

### Type and Method Mapping

| 3.x API | 4.0 Equivalent |
|---|---|
| `Workflow`, `Task`, `TaskGroup`, `LoopConfig`, `ForEach` | `def::WorkflowDef`, `TaskDef`, `GroupDef`, `LoopDef`, `ForEachDef` (parsed via `def::parse`, accessed via methods like `def.id()`, `task.on_failure()`) |
| `Workflow::validate_authored`, `check_workflow` | `def::parse` + `Compiler::check` |
| `walk_authored_steps`, `is_group`, `MAX_GROUP_DEPTH` | `WorkflowDef::walk`, `StepDef::Group`, `def::MAX_GROUP_DEPTH` |
| `Engine::new`, `new_with_operators`, `builder`, `with_new_workflows` | `Registry` + `Compiler` + `Engine::new` (or `Engine::builder()`); reload via `engine.compiler()`, `link`, and `engine.with_program()` |
| `process_message`, `process_message_in_channel`, `process_message_tracing` | `Engine::run(&mut msg)` and `Engine::run_with(&mut msg, &opts)` |
| `AsyncFunctionHandler`, `type Input`, `execute` | `Function`, `#[derive(FunctionConfig)] type Config`, `async fn call` |
| `TaskContext` | `Ctx` (unit-tested with `function::testing::call`) |
| `Template`, `PathTemplate`, `TemplateCompiler` | `Expr<T>`, `Output`, `Interpolated` |
| `FunctionConfig` enum (`HttpCallConfig`, `EnrichConfig`, `PublishKafkaConfig`) | Custom `Function` implementations registered on `Registry` |
| `DataflowError`, `ErrorInfo`, `ServiceErrorBuilder` | `Error` and `ErrorKind`; `ErrorRecord` on `Message::errors()` |
| `ExecutionObserver` (5 trait methods) | `Observer` (`interest()` + `on(&Event)`) |
| `ExecutionTrace` (full message snapshots per step) | `Trace` (`initial` context + incremental `changes`; replay via `trace.contexts()`) |
| `with_error_context_path`, `with_error_context_limit` | `EngineConfig::error_context(ErrorContext::new(path, limit)?)` |
| `MessageBuilder` (`capture_changes`, `audit_mode`, `routing_bucket`) | Configured per run on `RunOptions` |
| `get_nested_value`, `set_nested_value`, `remove_nested_value` | `Path::get`, `Path::set`, `Path::remove` and `Context::get`, `set`, `remove` |
| `declared_secrets`, `operator_names`, `template_key_escape`, `datalogic` | `ExprEngine::secrets().names()`, `operators()`, `escape()`, `ExprEngine` |
| `Rollout::partition`, `validate_set` | Unchanged, in `dataflow_rs::def` |
| `RetryPolicy`, `retry_with_policy`, `retry_with_attempts` | Removed (check `Run::retryable()` or `Error::is_retryable()` in your application's retry loop) |
| `Rule`, `Action`, `RulesEngine` type aliases | Removed |

### 1. Building and Reloading the Engine

In 3.x, you parsed each `Workflow` separately and passed it to `Engine::builder()`:

```text
// 3.x
let workflow = Workflow::from_json(json_text)?;
let engine = Engine::builder()
    .with_workflow(workflow)
    .register("fetch", Fetch)
    .build()?;
```

In 4.0, if you want the build to fail when any workflow is invalid, pass JSON
directly to `Engine::builder()`:

```rust
use dataflow_rs::Engine;
# use dataflow_rs::{Ctx, Function, Outcome, Result};
# use serde_json::json;
# struct Fetch;
# impl Function for Fetch {
#     type Config = ();
#     async fn call(&self, _ctx: &mut Ctx<'_>, _cfg: &()) -> Result<Outcome> { Ok(Outcome::Ok) }
# }
# let json = json!({"id": "w", "tasks": [{"id": "t", "function": {"name": "fetch"}}]});

let engine = Engine::builder()
    .register("fetch", Fetch)
    .workflow_json(&json)
    .build()
    .map_err(|issues| issues.len())?;
# Ok::<(), usize>(())
```

If you load workflows from a database and want to quarantine invalid workflows
while running the valid ones, use `def::parse` and `Compiler`:

```rust
use dataflow_rs::def::{ParseOptions, parse};
use dataflow_rs::program::CompileOptions;
use dataflow_rs::run::EngineConfig;
use dataflow_rs::{Engine, Registry};
# use serde_json::json;
# let stored = vec![
#     json!({"id": "a", "tasks": [{"id": "t", "function": {"name": "log", "input": {"message": "a"}}}]}),
#     json!({"id": "b", "tasks": [{"id": "t", "function": {"name": "nowhere"}}]}),
# ];

let compiler = Registry::default().compiler(CompileOptions::default());
let mut workflows = Vec::new();
for json in &stored {
    let Some(def) = parse(json, ParseOptions::default()).def else { continue };
    match compiler.compile_one(&def) {
        Ok(compiled) => workflows.push(compiled.workflow),
        Err(issues) => eprintln!("{}: {} issues, quarantined", def.id(), issues.len()),
    }
}
let engine = Engine::new(compiler.link(workflows), EngineConfig::default());
# assert_eq!(engine.program().workflows().len(), 1);

// Hot-reload workflows while preserving the registry and engine configuration:
let reloaded = engine.with_program(engine.compiler().link([]));
# assert_eq!(reloaded.program().workflows().len(), 0);
```

### 2. Custom Host Functions

In 3.x, custom functions implemented `AsyncFunctionHandler` with `#[async_trait]`,
a `serde::Deserialize` input struct, and `TaskContext`:

```text
// 3.x
#[derive(Deserialize)]
struct FetchInput { url: Template, target: String }

#[async_trait]
impl AsyncFunctionHandler for Fetch {
    type Input = FetchInput;
    async fn execute(&self, ctx: &mut TaskContext<'_>, input: &FetchInput)
        -> Result<TaskOutcome> {
        let url = input.url.resolve_str(ctx)?;
        ctx.set(&input.target, response);
        Ok(TaskOutcome::Success)
    }
}
```

In 4.0, custom functions implement `Function` with a `#[derive(FunctionConfig)]`
struct:

```rust
use dataflow_rs::expr::Output;
use dataflow_rs::function::FunctionConfig;
use dataflow_rs::datavalue::OwnedDataValue;
use dataflow_rs::{Ctx, Expr, Function, Outcome, Result};
# use serde_json::json;

#[derive(FunctionConfig)]
struct FetchConfig {
    url: Expr<String>,
    #[config(default = 5000)]
    timeout_ms: Expr<u64>,
    #[output(default = "data.response")]
    output: Output,
}

struct Fetch;

impl Function for Fetch {
    type Config = FetchConfig;

    async fn call(&self, ctx: &mut Ctx<'_>, cfg: &FetchConfig) -> Result<Outcome> {
        let url = ctx.eval(&cfg.url)?;
        let timeout_ms = ctx.eval(&cfg.timeout_ms)?;
        let response = OwnedDataValue::from(&json!({"url": &*url, "timeout_ms": timeout_ms}));
        ctx.emit(&cfg.output, response)?;
        Ok(Outcome::Ok)
    }
}
# let _ = Fetch;
```

`TaskContext` methods map to `Ctx` as follows:

| 3.x `TaskContext` | 4.0 `Ctx` |
|---|---|
| `ctx.get("data.x")` | `ctx.get(&path)` (where `path` is a pre-parsed `Path`) |
| `ctx.set("data.x", value)` | `ctx.set(&path, value)?` |
| `ctx.eval(&logic)` | `ctx.eval(&expr)` (typed by `Expr<T>`) |
| Writing to `input.target` | `ctx.emit(&cfg.output, value)?` via an `Output` field |
| `ctx.add_error(info)` | `ctx.record_error(record)` |
| `ctx.workflow_id()`, `task_id()`, `loop_counter()`, `element_index()` | `ctx.info().workflow_id`, `task_id`, `loop_counter`, `element_index` |
| `ctx.secret(name)` | `ctx.secret(name)` (unchanged) |
| `ctx.message_mut()` | Removed (perform all context writes through `set`, `remove`, and `emit`) |
| `TaskOutcome::Success` | `Outcome::Ok` (`Status`, `Skip`, and `Halt` keep the same names) |

Remember that context writes in `Ctx` are buffered in an overlay and committed
only when `call` returns `Ok`; if `call` returns `Err`, all context writes from
that call are discarded. Use `function::testing::call` to unit-test functions
directly without building a full `Engine`.

### 3. Running Messages and Checking Results

In 3.x, `process_message` returned `Result<(), DataflowError>`:

```text
// 3.x
match engine.process_message(&mut message).await {
    Ok(()) => {}
    Err(e) => eprintln!("stopped: {e}"),
}
```

In 4.0, `engine.run` and `engine.run_with` always return a `Run` report, and
you inspect `run.status()` (or call `run.into_result()`):

```rust
# tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
use dataflow_rs::run::RunStatus;
use dataflow_rs::{Engine, Message, RunOptions};
# use serde_json::json;
# let engine = Engine::builder()
#     .workflow_json(&json!({"id": "w", "tasks": [
#         {"id": "t", "function": {"name": "validation", "input": {"rules": [
#             {"logic": false, "message": "always"}]}}}
#     ]}))
#     .build().unwrap();

let mut message = Message::from_json(&json!({}));
let run = engine.run_with(&mut message, &RunOptions::default().channel("default")).await;
match run.status() {
    RunStatus::Completed => {}
    RunStatus::CompletedWithErrors => eprintln!("{} records", message.errors().len()),
    RunStatus::Stopped(e) => eprintln!("stopped: {}", e.code()),
    RunStatus::TimedOut | RunStatus::Cancelled => eprintln!("cut short"),
    _ => {}
}
# assert!(matches!(run.status(), RunStatus::CompletedWithErrors));
# assert_eq!(message.errors()[0].code, "VALIDATION_ERROR");
# });
```

### 4. Observers, Traces, and Error Context

#### Observers

In 3.x, `ExecutionObserver` had five separate callback methods. In 4.0,
`Observer` has a single `on(&Event)` method and an `interest()` method that
controls which events and timings are collected:

```rust
# tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
use dataflow_rs::observe::{Event, Interest, Observer};
use dataflow_rs::run::{EngineConfig, RunStatus};
use dataflow_rs::{Engine, Message};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
# use serde_json::json;

#[derive(Default)]
struct Metrics { tasks: AtomicUsize, stopped: AtomicUsize }

impl Observer for Metrics {
    fn interest(&self) -> Interest {
        Interest::TASKS | Interest::TIMINGS
    }

    fn on(&self, event: &Event<'_>) {
        match event {
            Event::TaskFinished { info, status, elapsed, .. } => {
                let _ = (info.function, status, elapsed.expect("TIMINGS was asked for"));
                self.tasks.fetch_add(1, Ordering::Relaxed);
            }
            Event::RunFinished { status, .. } => {
                if !matches!(status, RunStatus::Completed | RunStatus::CompletedWithErrors) {
                    self.stopped.fetch_add(1, Ordering::Relaxed);
                }
            }
            _ => {}
        }
    }
}

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
# assert_eq!(metrics.stopped.load(Ordering::Relaxed), 0);
# });
```

#### Execution Traces

Instead of calling a separate `process_message_tracing` method that stores full
message snapshots per step, pass `TraceOptions` on `RunOptions` and retrieve the
`Trace` from `Run`:

```rust
# tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
use dataflow_rs::observe::{StepResult, TraceOptions};
use dataflow_rs::{Engine, Message, RunOptions};
# use serde_json::json;
# let engine = Engine::builder()
#     .workflow_json(&json!({"id": "w", "tasks": [
#         {"id": "t", "function": {"name": "map", "input": {
#             "mappings": [{"path": "data.a", "logic": 1}, {"path": "data.b", "logic": 2}]}}}
#     ]}))
#     .build().unwrap();

let mut message = Message::from_json(&json!({}));
let opts = RunOptions::default().trace(TraceOptions::default().timings(true));
let mut run = engine.run_with(&mut message, &opts).await;

let trace = run.take_trace().expect("a trace was asked for");
for (step, context) in trace.steps.iter().zip(trace.contexts()) {
    // `context` is the reconstructed message context after this step:
    let _ = (step.result, step.status, &step.changes, &step.errors, &step.mapping_ends, context);
}
# assert_eq!(trace.steps.len(), 1);
# assert_eq!(trace.steps[0].result, StepResult::Executed);
# assert_eq!(trace.steps[0].mapping_ends, [1, 2]);
# assert!(trace.steps[0].elapsed.is_some());
# let last = trace.contexts().last().unwrap();
# assert_eq!(last.data(), message.context().data());
# });
```

#### Error Context

Configure `ErrorContext` on `EngineConfig` rather than directly on
`EngineBuilder`:

```rust
use dataflow_rs::run::{EngineConfig, ErrorContext};
use dataflow_rs::Engine;

let config = EngineConfig::default()
    .error_context(ErrorContext::new("metadata.errors".parse()?, 50)?);
let builder = Engine::builder().config(config);
# let _ = builder;
# Ok::<(), dataflow_rs::Error>(())
```

### 5. Constructing and Inspecting Errors

Replace `DataflowError::service(...).retryable(true).build()` with
`Error::service(code, message).with_retryable(true)`:

```rust
use dataflow_rs::{Error, ErrorKind};

let e = Error::service("HTTP_ERROR", "upstream 502").with_retryable(true);
assert_eq!(e.code(), "HTTP_ERROR");
assert!(matches!(e.kind(), ErrorKind::Service { .. }));
assert!(e.is_retryable());
```

See [Error Codes](../api/reference.md#error-codes) for the full list of error
codes in 4.0.

### 6. Run Options and Context Paths

`capture_changes`, `audit_mode`, and `routing_bucket` have moved from
`MessageBuilder` to `RunOptions`:

```rust
use dataflow_rs::run::AuditMode;
use dataflow_rs::RunOptions;

let opts = RunOptions::default()
    .capture_changes(false)
    .audit(AuditMode::Last(10))
    .routing_bucket(7);
# assert_eq!(opts.routing_bucket, Some(7));
```

Replace string-based `get_nested_value` / `set_nested_value` /
`remove_nested_value` calls with pre-parsed `Path` operations on
`message.context()` / `message.context_mut()`:

```rust
use dataflow_rs::datavalue::OwnedDataValue;
use dataflow_rs::{Message, Path};
use serde_json::json;

let mut message = Message::from_json(&json!({}));
let name: Path = "data.user.name".parse()?;
message.context_mut().set(&name, OwnedDataValue::from(&json!("Ada")))?;
assert_eq!(message.context().get(&name), Some(&OwnedDataValue::from(&json!("Ada"))));
let removed = message.context_mut().remove(&name)?;
assert!(removed.is_some());

// Every Path must start with `data`, `metadata`, or `temp_data`:
assert!("payload.x".parse::<Path>().is_err());
# Ok::<(), dataflow_rs::Error>(())
```

### Wire Format for Messages

The serialized JSON format of `Message` (`{id, payload, context, audit_trail, errors}`)
is unchanged, and every valid 3.x message deserializes cleanly in 4.0. Note that
4.0's `Message` deserializer strictly enforces that `context` contains only the
three object roots (`data`, `metadata`, and `temp_data`) and rejects duplicate
JSON keys:

```rust
use dataflow_rs::Message;

let text = r#"{"id":"m","payload":{"a":1},"context":{"data":{},"metadata":{},"temp_data":{}},"audit_trail":[],"errors":[]}"#;
let message: Message = serde_json::from_str(text)?;
assert_eq!(serde_json::to_string(&message)?, text);

let fourth = text.replace(r#""temp_data":{}"#, r#""temp_data":{},"secrets":{}"#);
let refused = serde_json::from_str::<Message>(&fourth).unwrap_err().to_string();
assert!(refused.starts_with("a context holds only `data`, `metadata` and `temp_data`, not `secrets`"));
# Ok::<(), serde_json::Error>(())
```

### WASM and UI Packages

- `@goplasmatic/dataflow-wasm` 4.0 provides a single `WasmEngine` class
  constructed with `new WasmEngine(workflows, { secrets })` and executed via
  `await engine.run(payload, options)` (returning `{ message, run, trace? }`).
  See [WASM Package](../wasm/overview.md).
- `@goplasmatic/dataflow-ui` 4.0 uses the generated TypeScript bindings from the
  WASM package and replays step contexts from incremental traces. See
  [UI Package](../ui/overview.md).

## Pre-Migration Checklist

Before deploying 4.0 in production, run this validation pass over all stored
workflow definitions:

1. **Parse and check every stored workflow** with `def::parse` and
   `Compiler::check` against your production `Registry`.
2. **Fix all `Rejected` issues and `LIKELY_TYPO` defects.** Any workflow with a
   `Rejected` issue will not load, and any `LIKELY_TYPO` points to a misspelled
   field that was previously ignored.
3. **Review `Advisory` issues** (`V3_SPELLING` shows what was rewritten;
   `UNKNOWN_KEY` shows what moved into `ext`).
4. **Persist the output of `compat::migrate_v3`** so your stored definitions use
   canonical 4.0 spellings and can be parsed with `.compat(false)`.
5. **Record each workflow's `Fingerprint`** (`compiled.workflow.fingerprint()`)
   to track which operators and context paths each workflow depends on.
6. **Check stored messages (if applicable)** to ensure no historical serialized
   `Message` has a fourth key in `context` or a non-object context root.

```rust
use dataflow_rs::compat::migrate_v3;
use dataflow_rs::def::{ParseOptions, parse};
use dataflow_rs::program::{CompileOptions, IssueCode, Severity};
use dataflow_rs::Registry;
# use serde_json::json;
# let stored = vec![
#     json!({"id": "a", "continue_on_error": true, "tasks": [
#         {"id": "t", "function": {"name": "log", "input": {"message": "a"}}}]}),
#     json!({"id": "b", "tasks": [{"id": "t", "conditon": true, "function": {"name": "nowhere"}}]}),
# ];

let compiler = Registry::default().compiler(CompileOptions::default());
let mut ready = Vec::new();
for json in &stored {
    let parsed = parse(json, ParseOptions::default());
    let mut issues = parsed.issues;
    if let Some(def) = &parsed.def {
        issues.extend(compiler.check(def));
    }
    let blocking = issues.iter().any(|i| {
        i.severity == Severity::Rejected || i.code == IssueCode::LikelyTypo
    });
    if blocking {
        eprintln!("{json:#}: {issues:#?}");
        continue;
    }
    let def = parsed.def.expect("nothing was Rejected");
    let migrated = migrate_v3(json).expect("nothing was Rejected");
    let fingerprint = compiler.compile_one(&def).expect("checked").workflow.fingerprint();
    ready.push((migrated.json, fingerprint.to_string()));
}
# assert_eq!(ready.len(), 1);
# assert_eq!(ready[0].0["on_failure"], "continue");
# assert!(ready[0].1.starts_with("v1:"));
```
