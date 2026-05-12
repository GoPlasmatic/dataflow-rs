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
  mutate it through `ctx.set(path, value)` which records audit-trail
  changes automatically, and append errors via `ctx.add_error(...)`.
- **`TaskOutcome`** — the return value: `Success`, `Status(u16)`,
  `Skip`, or `Halt`. Replaces the magic-number `usize` of earlier
  versions.

## Implementing AsyncFunctionHandler

```rust
use async_trait::async_trait;
use dataflow_rs::prelude::*;
use datavalue::OwnedDataValue;
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
let engine = Engine::builder()
    .with_workflows(workflows)
    .register("my_custom_function", MyCustomFunction)
    .build()?;
```

`register("name", handler)` accepts any `AsyncFunctionHandler` and boxes
it internally. The dyn-trait name (`BoxedFunctionHandler`) stays out of
user code.

## Using Custom Functions in Rules

```json
{
    "id": "custom_rule",
    "tasks": [
        {
            "id": "custom_action",
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

Custom handlers can compile and evaluate ad-hoc JSONLogic using the
shared datalogic engine exposed by `TaskContext::datalogic()`:

```rust,ignore
use bumpalo::Bump;
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
        // Compile the expression — Arc<Logic> so it can be cached/shared.
        let compiled = ctx
            .datalogic()
            .compile_arc(&json!({"var": "data.input"}))
            .map_err(|e| DataflowError::LogicEvaluation(e.to_string()))?;

        // Evaluate against the current message context.
        let arena = Bump::new();
        let av = ctx.message().context.to_arena(&arena);
        let result = ctx
            .datalogic()
            .evaluate(&compiled, av, &arena)
            .map_err(|e| DataflowError::LogicEvaluation(e.to_string()))?;

        // `result` is a `DataValue<'_>` borrowed from the arena.
        let _owned = result.to_owned();
        Ok(TaskOutcome::Success)
    }
}
```

If your handler evaluates many expressions against the same context,
build the `DataValue<'_>` once via `to_arena` and reuse it.

## Async Operations

The trait is async/await all the way through. Real I/O works naturally:

```rust,ignore
use async_trait::async_trait;
use dataflow_rs::prelude::*;
use datavalue::OwnedDataValue;
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
use datavalue::OwnedDataValue;
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
