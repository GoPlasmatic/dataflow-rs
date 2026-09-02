# Custom Functions

Extend dataflow-rs with your own custom processing logic by implementing
the `AsyncFunctionHandler` trait.

## Overview

Custom functions allow you to:

- Add domain-specific processing logic
- Integrate with external systems
- Perform async operations (HTTP, database, etc.)
- Implement complex transformations

The trait has three moving parts:

- **`type Input`** — your typed config shape. The engine deserializes each
  task's `FunctionConfig::Custom { input }` JSON into this type once at
  `Engine::builder().build()`, not per message. Misshapen config fails at
  startup.
- **`TaskContext`** — handed to every call. Read the message context
  (`ctx.data()`, `ctx.metadata()`, `ctx.temp_data()`, `ctx.get(path)`),
  read a secret by name (`ctx.secret(name)`), mutate the context through
  `ctx.set(path, value)` which records audit-trail changes automatically,
  and append errors via `ctx.add_error(...)`.
- **`TaskOutcome`** — the return value: `Success`, `Status(u16)`,
  `Skip`, or `Halt`. Replaces the magic-number `usize` of earlier
  versions.

## Implementing AsyncFunctionHandler

```rust
use async_trait::async_trait;
use dataflow_rs::prelude::*;
use dataflow_rs::datavalue::OwnedDataValue;
use serde::Deserialize;
use serde_json::json;

/// Typed config for the handler. The engine deserializes the task's
/// `FunctionConfig::Custom { input }` JSON into this struct at startup;
/// misshapen config fails there, not on first message.
#[derive(Deserialize)]
pub struct MyInput {
    target: String,
}

pub struct MyCustomFunction;

#[async_trait]
impl AsyncFunctionHandler for MyCustomFunction {
    type Input = MyInput;

    async fn execute(
        &self,
        ctx: &mut TaskContext<'_>,
        input: &MyInput,
    ) -> Result<TaskOutcome> {
        // Write into the context. `ctx.set` auto-creates intermediate
        // objects/arrays and records a `Change` on the audit trail
        // when `message.capture_changes` is on.
        ctx.set(&input.target, OwnedDataValue::from(&json!(true)));
        Ok(TaskOutcome::Success)
    }
}
```

Three concrete things the new shape removes:

1. No `match config { Custom { input, .. } => ..., _ => Err(...) }`
   block — `input` is the typed parameter directly.
2. No hand-built `Change` entries — `ctx.set` does that.
3. No magic `Ok((200, vec![]))` return — `TaskOutcome::Success` is
   self-documenting.

## Registering Custom Functions

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
# fn _demo(workflows: Vec<Workflow>) -> Result<()> {
let engine = Engine::builder()
    .with_workflows(workflows)
    .register("my_custom_function", MyCustomFunction)
    .build()?;
# Ok(()) }
```

`register("name", handler)` accepts any `AsyncFunctionHandler` and boxes
it internally. The dyn-trait name (`BoxedFunctionHandler`) stays out of
user code.

## Using Custom Functions in Rules

```json
{
    "id": "custom_rule",
    "name": "Custom Rule",
    "tasks": [
        {
            "id": "custom_action",
            "name": "Custom action",
            "function": {
                "name": "my_custom_function",
                "input": {
                    "target": "data.processed"
                }
            }
        }
    ]
}
```

The `input` shape on the wire must match your handler's `Input` struct.
serde does the parse at engine init time.

## Accessing Configuration

Because the engine pre-parses the JSON, configuration is just the
`input` parameter — no extraction step. For freeform JSON, set
`type Input = serde_json::Value;`:

```rust,ignore
use serde_json::Value;

#[async_trait]
impl AsyncFunctionHandler for FreeformHandler {
    type Input = Value;

    async fn execute(
        &self,
        ctx: &mut TaskContext<'_>,
        input: &Value,
    ) -> Result<TaskOutcome> {
        let option1 = input.get("option1").and_then(Value::as_str).unwrap_or("default");
        let option2 = input.get("option2").and_then(Value::as_i64).unwrap_or(0);
        // ...
        Ok(TaskOutcome::Success)
    }
}
```

## Evaluating JSONLogic from a handler

`TaskContext` has a value-returning evaluation surface — `eval`, `eval_json` and
`eval_to_plain_string` — that runs on the worker thread's pooled bump arena, so a
handler never has to manage a `Bump` or walk `ctx.message().context` itself:

```rust,ignore
use dataflow_rs::prelude::*;
use serde_json::json;

#[async_trait]
impl AsyncFunctionHandler for EvalDemo {
    type Input = serde_json::Value;

    async fn execute(
        &self,
        ctx: &mut TaskContext<'_>,
        _input: &serde_json::Value,
    ) -> Result<TaskOutcome> {
        // Compile once — Arc<Logic> so it can be cached/shared. `compile_arc`
        // is on the shared engine, still reachable via `ctx.datalogic()`.
        let compiled = ctx
            .datalogic()
            .compile_arc(&json!({"var": "data.input"}))
            .map_err(|e| DataflowError::LogicEvaluation(e.to_string()))?;

        // Evaluate against the current message context.
        let value: serde_json::Value = ctx.eval_json(&compiled)?;
        let _ = value;
        Ok(TaskOutcome::Success)
    }
}
```

`eval` returns `OwnedDataValue`, `eval_json` projects straight to
`serde_json::Value`, and `eval_to_plain_string` unquotes a string result —
`eval_to_plain_string` **deliberately disagrees** with datalogic-rs's own string
projection (`Session::eval_str` keeps the JSON quoting), so pick it when the
result is going into a URL path or similar. See [API Reference](../api/reference.md#taskcontext).

Compiling once per task rather than per message matters for a hot path. If your
config has a field the workflow author writes as JSONLogic — which, since 3.9,
is every parameter of every built-in — reach for `Template` instead of managing
the raw/compiled pair by hand.

## Config fields that are JSONLogic (`Template`)

A `Template` field deserializes from any JSON value, gets compiled once at
engine construction, and evaluates through `TaskContext` like any other
pre-compiled expression:

```rust,ignore
use dataflow_rs::prelude::*;
use dataflow_rs::{Template, TemplateCompiler};
use serde::Deserialize;

#[derive(Deserialize)]
struct GreetingInput {
    // Authored as JSONLogic in the workflow: {"cat": ["hello, ", {"var": "data.name"}]}
    greeting: Template,
}

struct GreetingHandler;

#[async_trait]
impl AsyncFunctionHandler for GreetingHandler {
    type Input = GreetingInput;

    // Called once per task at Engine::builder().build() time, right after
    // parse_input. The default is a no-op, so a handler with no Template
    // fields needs no override.
    fn compile_input(input: &mut Self::Input, c: &TemplateCompiler) -> Result<()> {
        input.greeting.compile(c, "greeting")
    }

    async fn execute(
        &self,
        ctx: &mut TaskContext<'_>,
        input: &Self::Input,
    ) -> Result<TaskOutcome> {
        let text: String = input.greeting.eval_into(ctx)?;
        ctx.set("data.greeting", OwnedDataValue::from(&serde_json::json!(text)));
        Ok(TaskOutcome::Success)
    }
}
```

A malformed expression fails at `compile_input` time — `Engine::builder().build()`
or `Engine::with_new_workflows` — not on the first message that reaches the task,
matching this crate's stance for its own built-in parameters.

Two things worth knowing:

- **Any config field may be a `Template`.** It used to be opt-in per field,
  because a single-key object whose key matched an operator name —
  `{"cat": ["a", "b"]}` — evaluated as that operator and a literal object was
  inexpressible. Since 3.9 the author writes `{"$cat": ["a", "b"]}` for the
  literal, so the restriction is gone. See
  [Literal keys and the `$` escape](./jsonlogic.md#literal-keys-and-the--escape).
- **A literal costs nothing.** A `Template` whose expression folds to a
  constant — which is what any statically-authored value does — is evaluated
  once at `build()` and cached, so per-message work happens only for a field
  that actually reads the message. `Template::is_constant` reports which.
- **`Template` fields nested inside a `Vec<T>` or a nested struct work fine** —
  walk the collection in `compile_input` and call `.compile(..)` on each one, as
  the example above's single field does trivially and a list of rules would do
  in a loop.
- **A `Template` may read `{"secret": "name"}`.** That is the intended way for
  a handler to receive a signing key or token: the value comes from the
  engine's store, is never part of the message, and appears in no trace. What
  the handler then does with it is the handler's business — the one rule is
  that it must not write a secret-derived value back into the message. See
  [Secrets](./secrets.md).

There is no derive macro for this — a hand-written `compile_input` is a few
lines, and this crate has no proc-macro dependency to add one.

## Knowing which task you are

A handler often needs to label what it produces — a log line, a metric, a
recorded call in a test harness — with the task that produced it.
`TaskContext` reports the executing identity directly:

```rust
# use async_trait::async_trait;
# use dataflow_rs::engine::functions::AsyncFunctionHandler;
# use dataflow_rs::{Result, TaskContext, TaskOutcome};
# use serde_json::Value;
struct Timed;

#[async_trait]
impl AsyncFunctionHandler for Timed {
    type Input = Value;

    async fn execute(&self, ctx: &mut TaskContext<'_>, _input: &Value) -> Result<TaskOutcome> {
        // Both are `Some` whenever the engine is running the task.
        let workflow = ctx.workflow_id().unwrap_or("<none>");
        let task = ctx.task_id().unwrap_or("<none>");

        // `Some(n)` on sweep `n` of a workflow carrying a `loop`, else `None`.
        match ctx.loop_counter() {
            Some(sweep) => println!("{workflow}/{task} sweep {sweep}"),
            None => println!("{workflow}/{task}"),
        }

        Ok(TaskOutcome::Success)
    }
}
```

Three things are worth knowing:

- **`task_id` is always a leaf task.** Handlers dispatch only on leaf tasks; a
  task group is evaluated on entry and recorded as a span, never dispatched. A
  group's id can never appear here.
- **All three are `None` for a context you built yourself** with
  `TaskContext::new`, which is the supported way to drive a handler from a test
  or benchmark. There is no workflow run to describe, and the `Option` says so
  rather than inventing an id.
- **`loop_counter` is the only way to see the sweep index** when the workflow's
  `loop` has no `counter` name. A named counter is written to
  `temp_data.<name>`, but an unnamed one is written nowhere — the engine still
  tracks it, and this is where it surfaces.

## Async Operations

The trait is async/await all the way through. Real I/O works naturally:

```rust,ignore
use async_trait::async_trait;
use dataflow_rs::prelude::*;
use dataflow_rs::datavalue::OwnedDataValue;
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
pub struct HttpFetchInput {
    url: String,
}

pub struct HttpFetchFunction;

#[async_trait]
impl AsyncFunctionHandler for HttpFetchFunction {
    type Input = HttpFetchInput;

    async fn execute(
        &self,
        ctx: &mut TaskContext<'_>,
        input: &HttpFetchInput,
    ) -> Result<TaskOutcome> {
        let response = reqwest::get(&input.url)
            .await
            .map_err(|e| DataflowError::http(0, e.to_string()))?;

        let body: Value = response
            .json()
            .await
            .map_err(|e| DataflowError::http(0, e.to_string()))?;

        ctx.set("data.fetched", OwnedDataValue::from(&body));
        Ok(TaskOutcome::Success)
    }
}
```

## Error Handling

Return appropriate errors for different failure modes:

```rust,ignore
async fn execute(
    &self,
    ctx: &mut TaskContext<'_>,
    _input: &Self::Input,
) -> Result<TaskOutcome> {
    if some_validation_fails {
        return Err(DataflowError::Validation("Invalid input".to_string()));
    }

    if some_operation_fails {
        return Err(DataflowError::Task("Operation failed".to_string()));
    }

    if downstream_call_failed {
        return Err(DataflowError::function_execution(
            "HTTP call failed",
            Some(DataflowError::http(503, "Service Unavailable")),
        ));
    }

    // Or return a status code for an HTTP-style outcome that isn't an Err:
    // 200 for success, 400 for validation failure, 500 for processing failure.
    Ok(TaskOutcome::Status(500))
}
```

The engine routes errors and 5xx statuses through `message.errors()` —
see [Error Handling](../core-concepts/error-handling.md) for the
unified-channel contract.

## Complete Example

```rust
use async_trait::async_trait;
use dataflow_rs::prelude::*;
use dataflow_rs::datavalue::OwnedDataValue;
use serde::Deserialize;
use serde_json::json;

/// Calculates statistics from numeric array data
#[derive(Deserialize)]
pub struct StatisticsInput {
    /// Field inside `data` whose value is the array to summarize.
    field: String,
}

pub struct StatisticsFunction;

#[async_trait]
impl AsyncFunctionHandler for StatisticsFunction {
    type Input = StatisticsInput;

    async fn execute(
        &self,
        ctx: &mut TaskContext<'_>,
        input: &StatisticsInput,
    ) -> Result<TaskOutcome> {
        let numbers: Vec<f64> = ctx
            .data()
            .get(input.field.as_str())
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_f64()).collect())
            .unwrap_or_default();

        if numbers.is_empty() {
            return Err(DataflowError::Validation(format!(
                "Field '{}' has no numeric values",
                input.field
            )));
        }

        let sum: f64 = numbers.iter().sum();
        let count = numbers.len() as f64;
        let mean = sum / count;
        let min = numbers.iter().cloned().fold(f64::INFINITY, f64::min);
        let max = numbers.iter().cloned().fold(f64::NEG_INFINITY, f64::max);

        ctx.set(
            "data.statistics",
            OwnedDataValue::from(&json!({
                "count": count,
                "sum": sum,
                "mean": mean,
                "min": min,
                "max": max,
            })),
        );
        Ok(TaskOutcome::Success)
    }
}
```

## Best Practices

1. **Use a typed Input** — let serde validate at startup. Reach for
   `serde_json::Value` only when the input genuinely is freeform.
2. **Mutate via `ctx.set`** — it auto-records the audit trail. Reaching
   into `message.context` directly bypasses change capture.
3. **Return TaskOutcome cleanly** — `Success` for the happy path,
   `Status(u16)` for HTTP-like codes (5xx pushes a `TASK_STATUS_ERROR`
   to `message.errors()`), `Skip` for "did nothing, continue",
   `Halt` for "stop this workflow".
4. **Use the right error type** — `DataflowError::retryable` looks at
   the variant to decide whether transient errors are worth retrying.
5. **Document** — your handler's `Input` struct is its contract;
   docstring it.
6. **Test** — drive the handler with `TaskContext::new(&mut message,
   &datalogic)` and assert on the outcome and `ctx.into_changes()`.
