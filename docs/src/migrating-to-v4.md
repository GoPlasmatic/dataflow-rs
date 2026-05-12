# Migrating from v3 to v4

dataflow-rs v4 is a breaking API release focused on ergonomics. Every change
has a mechanical migration; nothing requires re-architecting your code.

JSON wire formats (Message, AuditTrail, Change, ErrorInfo, Workflow,
FunctionConfig) are **unchanged** — v3 messages deserialize into v4 structs
and vice versa. Performance is neutral on the realistic ISO 20022 →
SwiftMT-103 workload.

## TL;DR

| If you do this in v3 | Change to in v4 |
|---|---|
| `impl AsyncFunctionHandler for X { async fn execute(&self, message: &mut Message, config: &FunctionConfig, engine: Arc<DatalogicEngine>) -> Result<(usize, Vec<Change>)> { ... } }` | `impl AsyncFunctionHandler for X { type Input = MyInput; async fn execute(&self, ctx: &mut TaskContext<'_>, input: &MyInput) -> Result<TaskOutcome> { ... } }` |
| `Engine::new(workflows, Some(custom_functions))?` | `Engine::builder().with_workflows(workflows).register("name", H).build()?` |
| `Engine::new(workflows, None)?` | `Engine::builder().with_workflows(workflows).build()?` |
| `Message::from_value(&json)` | unchanged (also: `Message::builder().payload_json(&json).build()`) |
| `Message::with_id("x", payload)` | `Message::builder().id("x").payload(payload).build()` |
| `&message.errors` | `message.errors()` |
| `message.audit_trail.iter()` | `message.audit_trail().iter()` |
| `message.errors.push(e)` | `message.add_error(e)` |
| `Ok((200, vec![Change { ... }]))` (from a handler) | `ctx.set("data.x", v); Ok(TaskOutcome::Success)` |
| `set_nested_value(&mut message.context, path, v)` (from a handler) | `ctx.set(path, v)` |

## 1. Custom function handlers

The trait redesign is the biggest change. The good news: every concept moves
1:1, the new shape just removes boilerplate.

### Before (v3)

```rust
use async_trait::async_trait;
use dataflow_rs::engine::{
    AsyncFunctionHandler, FunctionConfig,
    error::{DataflowError, Result},
    message::{Change, Message},
    utils::set_nested_value,
};
use datalogic_rs::Engine as DatalogicEngine;
use datavalue::OwnedDataValue;
use serde_json::{Value, json};
use std::sync::Arc;

struct MyHandler;

#[async_trait]
impl AsyncFunctionHandler for MyHandler {
    async fn execute(
        &self,
        message: &mut Message,
        config: &FunctionConfig,
        _engine: Arc<DatalogicEngine>,
    ) -> Result<(usize, Vec<Change>)> {
        let input = match config {
            FunctionConfig::Custom { input, .. } => input,
            _ => {
                return Err(DataflowError::Validation(
                    "wrong config".to_string(),
                ));
            }
        };
        let target = input
            .get("target")
            .and_then(Value::as_str)
            .unwrap_or("data.out");

        let value = OwnedDataValue::from(&json!({"hello": "world"}));
        set_nested_value(&mut message.context, target, value.clone());

        Ok((
            200,
            vec![Change {
                path: Arc::from(target),
                old_value: OwnedDataValue::Null,
                new_value: value,
            }],
        ))
    }
}
```

### After (v4)

```rust
use async_trait::async_trait;
use dataflow_rs::prelude::*;
use datavalue::OwnedDataValue;
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize)]
struct MyInput {
    target: String,
}

struct MyHandler;

#[async_trait]
impl AsyncFunctionHandler for MyHandler {
    type Input = MyInput;

    async fn execute(
        &self,
        ctx: &mut TaskContext<'_>,
        input: &MyInput,
    ) -> Result<TaskOutcome> {
        ctx.set(
            &input.target,
            OwnedDataValue::from(&json!({"hello": "world"})),
        );
        Ok(TaskOutcome::Success)
    }
}
```

### Migration checklist

1. Define a `Deserialize` struct for your handler's input. Use
   `serde_json::Value` if you genuinely need freeform JSON.
2. Add `type Input = YourStruct;` to the impl.
3. Change the `execute` signature to
   `(&self, ctx: &mut TaskContext<'_>, input: &Self::Input) -> Result<TaskOutcome>`.
4. Delete the `match config { Custom { input, .. } => ..., _ => Err(...) }`
   block — `input` is now the typed parameter directly.
5. Replace `set_nested_value(&mut message.context, path, v)` with
   `ctx.set(path, v)`. The `Change` is recorded automatically when
   `capture_changes` is on.
6. Replace `Ok((200, vec![]))` with `Ok(TaskOutcome::Success)`.
   Replace status codes with `TaskOutcome::Status(code)`,
   filter-skip with `TaskOutcome::Skip`, filter-halt with
   `TaskOutcome::Halt`.
7. Replace `&message.field` reads in the handler with `ctx.data()` /
   `ctx.metadata()` / `ctx.temp_data()` / `ctx.get(path)`.

### Misshapen Custom config now fails at startup

In v3, a malformed `FunctionConfig::Custom { input: <bad json> }` only
surfaced when the first message hit the handler. In v4, the engine
deserializes each task's `input` into the registered handler's typed
`Self::Input` at `Engine::new()`/`Engine::builder().build()` — bad config
fails immediately, in line with v3.0.0's "fail loud at startup" stance.

## 2. Engine construction

### Before (v3)

```rust
use dataflow_rs::engine::functions::AsyncFunctionHandler;
use std::collections::HashMap;

let mut custom_functions: HashMap<String, Box<dyn AsyncFunctionHandler + Send + Sync>> =
    HashMap::new();
custom_functions.insert("statistics".to_string(), Box::new(StatisticsFunction));
custom_functions.insert("enrich_data".to_string(), Box::new(EnrichmentFunction));

let engine = Engine::new(vec![workflow], Some(custom_functions))?;
```

### After (v4)

```rust
use dataflow_rs::prelude::*;

let engine = Engine::builder()
    .with_workflow(workflow)
    .register("statistics", StatisticsFunction)
    .register("enrich_data", EnrichmentFunction)
    .build()?;
```

`register` accepts any `AsyncFunctionHandler` and boxes it internally; the
verbose `Box<dyn AsyncFunctionHandler + Send + Sync>` type is gone from user
code.

### Engine::new still works

If you were calling `Engine::new(workflows, None)`:

```rust
// v3
let engine = Engine::new(workflows, None)?;

// v4 — either of these:
let engine = Engine::builder().with_workflows(workflows).build()?;
// or:
let engine = Engine::new(workflows, std::collections::HashMap::new())?;
```

The `Option` wrapper is gone. For the no-handler case, builder is the
recommended path.

## 3. Message access

`Message`'s mutable fields are now encapsulated. Read via accessor methods,
mutate via `add_error` / `TaskContext::set`. The `context` field stays
`pub` — it's the legitimate read surface for tests and consumers that want
to walk the full tree.

### Before (v3)

```rust
println!("processed message {}", message.id);
for err in &message.errors {
    eprintln!("{}: {}", err.code, err.message);
}
println!("audit count: {}", message.audit_trail.len());
let last = &message.audit_trail[message.audit_trail.len() - 1];

message.errors.push(ErrorInfo::simple(
    "MY_CODE".to_string(), "boom".to_string(), None,
));
```

### After (v4)

```rust
println!("processed message {}", message.id());
for err in message.errors() {
    eprintln!("{}: {}", err.code, err.message);
}
println!("audit count: {}", message.audit_trail().len());
let last = &message.audit_trail()[message.audit_trail().len() - 1];

message.add_error(ErrorInfo::simple(
    "MY_CODE".to_string(), "boom".to_string(), None,
));
```

### Message construction

Two shortcuts and one builder:

```rust
// Already-owned Arc payload (perf path, e.g. an HTTP server holding a
// pre-parsed payload). Unchanged from v3.
let m = Message::new(payload_arc);

// Convenience for serde_json (one bridge walk). Unchanged from v3.
let m = Message::from_value(&json!({"x": 1}));

// New in v4 — for everything richer (custom id, capture_changes off, ...).
let m = Message::builder()
    .id("correlation-123")
    .payload_json(&json!({"x": 1}))
    .capture_changes(false)
    .build();
```

`Message::with_id` and `Message::without_change_capture` were removed — use
the builder.

## 4. Error handling

In v3, errors flowed through two channels with different rules:

- A workflow failure pushed a `WORKFLOW_ERROR` wrapper to `message.errors`
  **only when `continue_on_error: true`**. In fail-fast mode (`false`),
  the wrapper was absent — callers got the bare `Result::Err` and had to
  walk task-level errors to find context.
- A handler returning a 5xx status was recorded on the audit trail but
  **not** pushed to `message.errors`.

In v4, `message.errors()` is the always-on channel:

- Every workflow failure pushes a `WORKFLOW_ERROR` wrapper, regardless of
  `continue_on_error`. The wrapper carries workflow id; the underlying
  task error doesn't.
- Handler statuses `>= 500` push a new `TASK_STATUS_ERROR` entry (in
  addition to the audit-trail status).
- `Result::Err` from `process_message` only signals "the engine stopped
  before processing further workflows". Callers that want fail-fast match
  on it; callers that want the uniform list inspect `message.errors()`.

### Before (v3)

```rust
match engine.process_message(&mut message).await {
    Ok(_) => { /* might still have errors in continue mode */ }
    Err(e) => {
        // The bare `e` doesn't carry workflow context.
        // To find it, walk message.errors looking for TASK_ERROR
        // — workflow context wrapper is only present in continue mode.
        eprintln!("hard fail: {e}");
    }
}
```

### After (v4)

```rust
let result = engine.process_message(&mut message).await;

// The unified channel — always present, regardless of continue_on_error
// or Result::Err. Walk this for a complete error picture.
for err in message.errors() {
    eprintln!("[{}] {}: {}", err.code, err.workflow_id.as_deref().unwrap_or(""), err.message);
}

// Fail-fast signal — true when the engine stopped before all workflows ran.
if let Err(e) = result {
    eprintln!("engine stopped early: {e}");
}
```

If you were inspecting `message.errors` after every `process_message`
already (most callers), nothing changes — `message.errors()` is now a
strict superset of what v3 exposed.

## 5. Filter status codes

`FILTER_STATUS_PASS` (200), `FILTER_STATUS_SKIP` (298), and
`FILTER_STATUS_HALT` (299) are gone. `FilterConfig` returns `TaskOutcome`
variants directly:

| v3 status | v4 outcome |
|---|---|
| 200 (PASS)  | `TaskOutcome::Success` |
| 298 (SKIP)  | `TaskOutcome::Skip` |
| 299 (HALT)  | `TaskOutcome::Halt` |

The on-the-wire halt code stays at 299 (audit-trail consumers like
dataflow-ui keep working). It's exposed as
`dataflow_rs::engine::task_outcome::HALT_STATUS_CODE` if you need the
constant.

## 6. Imports

The `dataflow_rs::prelude` module re-exports the 14 types you need for the
90% case:

```rust
use dataflow_rs::prelude::*;
// brings in: Engine, EngineBuilder, Workflow, Task, Message,
// MessageBuilder, AuditTrail, Change, AsyncFunctionHandler,
// TaskContext, TaskOutcome, Result, DataflowError, ErrorInfo,
// WorkflowStatus
```

If you were importing piece-by-piece (`use dataflow_rs::{Engine, Workflow,
...}`), you can usually replace those imports with `use
dataflow_rs::prelude::*;`. Less-common types — `BoxedFunctionHandler`,
`DynAsyncFunctionHandler`, the named `*Config` structs, the trace surface
(`ExecutionTrace`, `ExecutionStep`, `StepResult`), the rules-engine aliases
(`Rule`, `Action`, `RulesEngine`) — stay at the crate root.

## Performance notes

- Realistic ISO 20022 → SwiftMT-103 workload (500K msgs × 38 ops):
  unchanged within run-to-run noise (227K → 230K msg/s, P50 23 μs).
- New `examples/async_handler_benchmark.rs` measures the framework
  overhead of the typed `AsyncFunctionHandler::Input` dispatch path:
  ~1.2 μs/call (TaskContext alloc + downcast_ref + ctx.set with audit).
  Real-world handlers doing HTTP/DB I/O dwarf this; for in-memory
  transformations the built-in `map` function stays the right primitive.

## Help

If you hit a migration case not covered here, please file an issue:
[https://github.com/GoPlasmatic/dataflow-rs/issues](https://github.com/GoPlasmatic/dataflow-rs/issues).

The four v4 commits — for cross-referencing while migrating:

- `a0e564e` — typed `AsyncFunctionHandler::Input` + `TaskContext` + `TaskOutcome`
- `492dab9` — `EngineBuilder`, `MessageBuilder`, encapsulate `Message`, prelude
- `e69d6f8` — single error channel
- `62accb0` — async-handler benchmark
