# Error Handling

Dataflow-rs provides flexible error handling at multiple levels to build resilient automation rules.

## Two complementary error channels

Every error encountered during `process_message` flows through two
complementary channels:

- **`message.errors()`** — **always** contains every error encountered:
  validation failures, task panics, 5xx-status outcomes, workflow
  wrappers. Callers that want a uniform view scan this list.
- **`Result::Err` from `process_message`** — signals **only** that the
  engine stopped before processing every workflow. Callers that want
  fail-fast match on it; the error pushed to `message.errors()` for the
  same failure carries the workflow context that the bare `Err` doesn't.

A workflow with `continue_on_error: true` records its errors to
`message.errors()` and returns `Ok(())`. A workflow with
`continue_on_error: false` records to `message.errors()` *and* returns
`Result::Err` (which short-circuits the rest of `process_message`).

## Error Levels

Errors can be handled at three levels:

1. **Action Level** - Individual action (task) error handling
2. **Rule Level** - Rule-wide (workflow) error policy
3. **Engine Level** - Processing errors

## Action-Level Error Handling

### Stop on Error (Default)

```json
{
    "id": "critical_action",
    "continue_on_error": false,
    "function": { ... }
}
```

If the action fails:
- Error is recorded in `message.errors()`
- Rule execution stops
- No further actions execute

### Continue on Error

```json
{
    "id": "optional_action",
    "continue_on_error": true,
    "function": { ... }
}
```

If the action fails:
- Error is recorded in `message.errors()`
- Rule continues to next action

## Rule-Level Error Handling

The rule's `continue_on_error` applies to all actions by default:

```json
{
    "id": "resilient_rule",
    "continue_on_error": true,
    "tasks": [
        {"id": "action1", "function": { ... }},
        {"id": "action2", "function": { ... }},
        {"id": "action3", "function": { ... }}
    ]
}
```

All actions will continue even if earlier actions fail.

### Override at Action Level

```json
{
    "id": "mixed_rule",
    "continue_on_error": true,
    "tasks": [
        {"id": "optional_action", "function": { ... }},
        {
            "id": "critical_action",
            "continue_on_error": false,
            "function": { ... }
        }
    ]
}
```

## Accessing Errors

After processing, walk `message.errors()`:

```rust
# use dataflow_rs::{Engine, Message};
# async fn _demo(engine: Engine, mut message: Message) {
let result = engine.process_message(&mut message).await;

for error in message.errors() {
    println!("Error: {} in {}/{}",
        error.message,
        error.workflow_id.as_deref().unwrap_or("unknown"),
        error.task_id.as_deref().unwrap_or("unknown")
    );
}

// Fail-fast signal — true when the engine stopped before all workflows ran.
if let Err(e) = result {
    eprintln!("engine stopped early: {e}");
}
# }
```

Common error codes you'll see:

- `VALIDATION_ERROR` — from the `validation` built-in, or a handler returning
  `DataflowError::Validation`
- `TASK_ERROR` — handler returned `DataflowError::Task`
- `TASK_STATUS_ERROR` — handler returned `TaskOutcome::Status(s)` with `s >= 500`
- `WORKFLOW_ERROR` — wrapper recording workflow context for the failure above

Every other engine variant contributes its own code the same way:
`FUNCTION_NOT_FOUND`, `FUNCTION_ERROR`, `LOGIC_ERROR`, `HTTP_ERROR`,
`TIMEOUT_ERROR`, `IO_ERROR`, `DESERIALIZATION_ERROR`, `UNKNOWN_ERROR`.

> **Changed in 3.5.0.** Before this release every variant except
> `Service` collapsed to `TASK_ERROR` on the live path, so a timeout, a dropped
> connection and a rejected request were indistinguishable. If you were matching
> on `TASK_ERROR` to mean "the handler returned `Err`", match the specific codes
> instead — or return `DataflowError::Task`, which still maps to `TASK_ERROR`.

That list is not closed: a handler returning a **service-classified** error
contributes its own code (see below). Switch on `code` with a default arm.

## Service-classified errors

The engine's error variants describe engine concerns. When your handler fails for
a reason only your service understands — a circuit breaker opened, a tenant hit a
rate limit — classify it yourself:

```rust
# use dataflow_rs::DataflowError;
# fn _demo() -> DataflowError {
DataflowError::service("circuit_open", "upstream unavailable")
    .detail("connector 'billing' breaker open since 12:04")
    .retryable(true)
    .build()
# }
```

Three things this buys you:

- **`kind` becomes the `ErrorInfo::code`** on `message.errors()`, passed through
  **verbatim** — not upper-cased — so the string you switch on is the string you
  wrote. An empty `kind` falls back to `TASK_ERROR`.
- **`detail` is a separate, operator-only channel.** `Display` renders `message`
  alone, so `to_string()` is always safe to hand to an untrusted caller; the detail
  is reachable through `Debug`, `DataflowError::detail()` and `ErrorInfo::detail`.
  It is omitted from the serialized form when absent, so nothing changes for errors
  that do not carry one.
- **`retryable` is declared, not inferred** from the variant. Note the engine does
  not act on it — no code path in this crate reads `retryable()`. It is carriage
  for your own retry policy.

Everything else is unchanged: `continue_on_error`, the audit-trail entry, and the
`Result::Err` short-circuit behave exactly as for any other error. The
`WORKFLOW_ERROR` wrapper still records workflow context and keeps its own code, so
counting errors by code does not double-count. No built-in ever returns this
variant.

## Branching on why a task failed

`message.errors()` is host-side only — the JSONLogic evaluation context is
exactly `{data, metadata, temp_data}`, so `{"var": "errors"}` resolves to
nothing. To let a workflow branch on *why* a step failed, point the engine at a
context path:

```rust
# use dataflow_rs::{Engine, Workflow};
# fn _demo(workflows: Vec<Workflow>) -> dataflow_rs::Result<()> {
let engine = Engine::builder()
    .with_workflows(workflows)
    .with_error_context_path("metadata.errors")
    .build()?;
# Ok(()) }
```

Off unless called. With no path configured nothing is written, and the whole
mechanism is one `Option` check on a path that only runs after a task has
already failed.

One record is appended per error a task contributes:

```json
{ "workflow_id": "place_order", "task_id": "charge_payment",
  "code": "TIMEOUT_ERROR", "status": 500 }
```

so a later task — or a later workflow — can route on the reason:

```json
{ "in": [ { "var": "metadata.errors.0.code" }, ["TIMEOUT_ERROR", "IO_ERROR"] ] }
```

### What is recorded

Coverage matches `errors()`: a handler returning `Err`, a task returning a 5xx
outcome, each failing rule of the `validation` built-in, and anything a handler
adds through `TaskContext::add_error`. Two deliberate exclusions:

- **The `WORKFLOW_ERROR` wrapper.** It re-reports the same underlying failure, so
  mirroring it would double-count. A task failure with `continue_on_error: false`
  therefore puts *two* entries on `message.errors()` but *one* record here.
- **Tasks returning `TaskOutcome::Skip`.** Skip opts out of the per-task record
  entirely — no audit entry, no `metadata.progress` write, no record.

`status` is the task's own status: `500` when the handler returned `Err`,
otherwise the status the outcome carried (`400` for `validation`, `200` for a
handler that recorded an error and still succeeded). That is the distinction
`metadata.progress` cannot make — its failure arm hard-codes `500`.

The error `message` and the operator-only `detail` are **not** recorded. This
value lands in `Message.context`, which is serialized straight back to callers;
read those from `message.errors()` host-side instead. Note this applies to
`temp_data` too — it is part of `context` and ships on the wire like everything
else, so it is not private scratch space.

### Practical notes

- The key is **absent**, not `[]`, when nothing failed — a clean message keeps
  the exact wire shape it had before the option existed.
- At most 32 records are kept by default, newest retained; change it with
  `.with_error_context_limit(n)`. The bound is what keeps the cost independent of
  a looping workflow's iteration count, since `Message.context` is deep-cloned
  into every trace snapshot.
- **The engine owns the configured path.** A non-array found there is replaced.
  `metadata.progress` is rejected at `build()`, as is any path that does not start
  with `data`, `metadata` or `temp_data` — such a path would write somewhere the
  evaluation context cannot see, giving you a condition that is silently never
  true.
- Prefer `metadata.*` or `temp_data.*` over `data.*`: the first append into a
  `data.*` path costs a one-time re-arena of the whole `data` subtree, which is
  the heavy payload side.
- The append is engine bookkeeping, not a task mutation, so it is not recorded as
  an audit-trail `Change`.

## Error Types

### Validation Errors

Generated by the `validation` function when rules fail:

```json
{
    "function": {
        "name": "validation",
        "input": {
            "rules": [
                {
                    "condition": {"!!": {"var": "data.email"}},
                    "error_message": "Email is required"
                }
            ]
        }
    }
}
```

### Execution Errors

Generated when function execution fails:

- JSONLogic evaluation errors
- Data type mismatches
- Missing required fields

### Custom Function Errors

Return errors from custom functions via `Result::Err`:

```rust,ignore
use dataflow_rs::prelude::*;

impl AsyncFunctionHandler for MyFunction {
    type Input = serde_json::Value;

    async fn execute(
        &self,
        ctx: &mut TaskContext<'_>,
        _input: &serde_json::Value,
    ) -> Result<TaskOutcome> {
        if some_condition {
            return Err(DataflowError::Task(
                "Custom error message".to_string()
            ));
        }
        Ok(TaskOutcome::Success)
    }
}
```

`DataflowError` provides typed variants for the most common cases —
`Validation`, `Task`, `Workflow`, `FunctionExecution`, `FunctionNotFound`,
`Http`, `Timeout`, `Io`, `LogicEvaluation`, `Deserialization`, `Unknown`.
See the [API reference](../api/reference.md#dataflowerror) for the full list.

## Error Recovery Patterns

### Fallback Values

Use conditions to provide fallback values:

```json
{
    "tasks": [
        {
            "id": "try_primary",
            "continue_on_error": true,
            "function": {
                "name": "map",
                "input": {
                    "mappings": [
                        {"path": "temp_data.result", "logic": {"var": "data.primary"}}
                    ]
                }
            }
        },
        {
            "id": "use_fallback",
            "condition": {"!": {"var": "temp_data.result"}},
            "function": {
                "name": "map",
                "input": {
                    "mappings": [
                        {"path": "data.result", "logic": "default_value"}
                    ]
                }
            }
        }
    ]
}
```

### Validation Before Processing

Validate data before critical operations:

```json
{
    "tasks": [
        {
            "id": "validate",
            "function": {
                "name": "validation",
                "input": {
                    "rules": [
                        {"condition": {"!!": {"var": "data.required_field"}}, "error_message": "Required field missing"}
                    ]
                }
            }
        },
        {
            "id": "process",
            "function": { ... }
        }
    ]
}
```

If validation fails, the rule stops before further processing.

## Try It

> **Want more features?** Try the [Full Debugger UI](/dataflow-rs/debugger/) with step-by-step execution and workflow visualization.

<div class="playground-widget" data-workflows='[{"id":"error_demo","name":"Error Demo","continue_on_error":true,"tasks":[{"id":"parse","name":"Parse Payload","function":{"name":"parse_json","input":{"source":"payload","target":"input"}}},{"id":"validate_email","name":"Validate Email","function":{"name":"validation","input":{"rules":[{"logic":{"!!":[{"var":"data.input.email"}]},"message":"Email is required"}]}}},{"id":"greet","name":"Greet User","function":{"name":"map","input":{"mappings":[{"path":"data.greeting","logic":{"cat":["Hello, ",{"var":"data.input.name"},"!"]}}]}}}]}]' data-payload='{"name":"John"}'>
</div>

Notice the validation error is recorded but processing continues.

## Best Practices

1. **Validate Early**
   - Add validation actions at the start of rules
   - Fail fast on invalid data

2. **Use continue_on_error Wisely**
   - Only for truly optional actions
   - Critical operations should stop on error

3. **Check Errors**
   - Always check `message.errors()` after processing
   - Log errors for monitoring

4. **Provide Context**
   - Include meaningful error messages
   - Include field paths in validation errors
