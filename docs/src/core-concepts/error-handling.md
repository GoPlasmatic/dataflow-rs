# Error Handling

Dataflow-rs handles errors at the task, workflow and run levels.

## One Error List and a Run Report

Every error recorded during a run lands in one place, and the run reports
how it ended:

- **`message.errors()`** holds every error recorded: validation failures,
  `5xx` outcomes, a host function's `Err`, and any record a function added
  itself. Each record carries its `code`, its `message`, and the
  `workflow_id` and `task_id` it came from.
- **`Run::status()`** says how the run ended. `Completed` and
  `CompletedWithErrors` mean every considered workflow finished, the second
  with records on the message. `Stopped(error)` means a task's failure
  stopped the run before every workflow was considered, and carries the
  error that stopped it. `TimedOut` and `Cancelled` are the run's own
  limits.

A run that stops adds no extra record: the failing task's own record carries
the workflow id, and the `Run` carries the error.

```rust
# use dataflow_rs::{Engine, Message};
# async fn _demo(engine: Engine, mut message: Message) {
use dataflow_rs::run::RunStatus;

let run = engine.run(&mut message).await;

for error in message.errors() {
    println!(
        "{} in {}/{}: {}",
        error.code,
        error.workflow_id.as_deref().unwrap_or("-"),
        error.task_id.as_deref().unwrap_or("-"),
        error.message
    );
}

// The fail-fast signal: the engine stopped before every workflow ran.
if let RunStatus::Stopped(error) = run.status() {
    eprintln!("stopped early: {error} ({})", error.code());
}
// Or, for a caller that wants a `?`:
let run = run.into_result();
# let _ = run;
# }
```

`into_result()` is `Ok(run)` for `Completed` and `CompletedWithErrors`, and
`Err` for a stopped, timed-out or cancelled run.

## Failures and Rejections

A function call's result is classified before any policy applies:

| Result | Class | Recorded |
|---|---|---|
| `Ok(Outcome::Ok)`, or a status below `400` | success | nothing |
| a `4xx` status | **rejection** | nothing by the engine; a `validation` records its own failing rules |
| a `5xx` status | **failure** | `TASK_STATUS_ERROR`: `Task <id> returned status <s>` |
| `Err(error)` | **failure** | the error's code: `Task <id> error: <message>` |
| `Outcome::Skip` | | nothing, and no audit entry |
| `Outcome::Halt` | | nothing; the workflow ends |

Two policies on the task say what each class does, and one on the workflow
says what a failure that reaches it does.

## Task-Level Policies

### `on_failure`

```jsonc
{
    "id": "optional_task",
    "on_failure": "continue",
    "function": { ... }
}
```

- **`fail`** (default): the failure passes to the workflow's `on_failure`
- **`continue`**: the failure is recorded in `message.errors()` and the next
  task runs
- **`halt`**: the failure is recorded and the workflow ends; later workflows
  still run, and the workflow's exit is `Halted`

### `on_rejection`

```jsonc
{
    "id": "validate_input",
    "on_rejection": "halt",
    "function": { "name": "validation", "input": { ... } }
}
```

- **`continue`** (default): the next task runs
- **`halt`**: the workflow ends at the rejected task; later workflows still run

A rejection is not an error to the engine: a host function returning
`Outcome::Status(404)` leaves a `404` on the audit entry and nothing in
`message.errors()`. A failing `validation` is different only because it
records its own rules: `VALIDATION_ERROR` per failing rule, then `400`.

## Workflow-Level Policy

The workflow's `on_failure` answers a **different question** from the task's,
and is not a default for its tasks:

| Key | Question it answers |
|------|--------------------|
| `on_failure` on a **task** | When this task fails, do the *remaining tasks in this workflow* still run? |
| `on_failure` on a **workflow** | When this workflow fails, do the *later workflows* still run, or does the run stop? |

There is no third level. A [task group](../advanced/control-flow.md#groups)
gates a span; it carries no policy of its own.

### The Policy Matrix

"Task fails" means it returned `Err` or a `5xx` status:

| task `on_failure` | workflow `on_failure` | Later tasks in the workflow | Later workflows | `Run::status()` |
|---|---|---|---|---|
| `fail` (default) | `fail` (default) | stop | stop | `Stopped(error)` |
| `fail` | `continue` | stop | run | `CompletedWithErrors`; the workflow's exit is `Failed` |
| `halt` | *(either)* | stop | run | `CompletedWithErrors`; the exit is `Halted` |
| `continue` | *(either)* | run | run | `CompletedWithErrors` |

The failure is recorded in `message.errors()` on every row.

So a workflow marked `on_failure: continue` whose tasks leave their policy
unset still stops at its first failing task, but it does not take the rest of
the run down with it:

```jsonc
{
    "id": "resilient_workflow",
    "on_failure": "continue",
    "tasks": [
        {"id": "task1", "on_failure": "continue", "function": { ... }},
        {"id": "task2", "on_failure": "continue", "function": { ... }},
        {"id": "task3", "function": { ... }}
    ]
}
```

Because each task opts in, the workflow runs to the end even if `task1` and
`task2` fail; `task3` leaves the policy unset, so a failure there ends the
workflow with the exit `Failed`. The workflow's `continue` keeps later
workflows running.

### Seeing the matrix run

A host function that fails shows the two rows a host meets most:

```rust
use dataflow_rs::run::{RunStatus, WorkflowExit};
use dataflow_rs::{Ctx, Engine, Error, Function, Message, Outcome, Result};
use serde_json::json;

/// Always fails with a code of its own.
struct Flaky;

impl Function for Flaky {
    type Config = ();

    async fn call(&self, _ctx: &mut Ctx<'_>, _cfg: &()) -> Result<Outcome> {
        Err(Error::service("PARTNER_DOWN", "the partner is down").with_retryable(true))
    }
}

# #[tokio::main]
# async fn main() {
let engine = |on_failure: &str| {
    Engine::builder()
        .register("flaky", Flaky)
        .workflow_json(&json!({"id": "w", "tasks": [
            {"id": "call", "on_failure": on_failure, "function": {"name": "flaky"}},
            {"id": "after", "function": {"name": "map", "input": {
                "mappings": [{"path": "data.after", "logic": true}]}}}
        ]}))
        .workflow_json(&json!({"id": "next", "priority": 1, "tasks": [
            {"id": "t", "function": {"name": "map", "input": {
                "mappings": [{"path": "data.next", "logic": true}]}}}
        ]}))
        .build()
        .expect("the workflows build")
};

// The default: the failure stops the run. `after` and `next` never run.
let mut message = Message::from_json(&json!({}));
let run = engine("fail").run(&mut message).await;
let RunStatus::Stopped(error) = run.status() else { panic!("stopped") };
assert_eq!(error.code(), "PARTNER_DOWN");
assert!(run.retryable() && run.partial());
assert_eq!(message.errors()[0].code, "PARTNER_DOWN");
assert_eq!(message.errors()[0].message, "Task call error: the partner is down");
assert_eq!(message.errors()[0].task_id.as_deref(), Some("call"));
assert!(message.context().get(&"data.next".parse().unwrap()).is_none());

// `continue`: recorded, and both `after` and `next` run.
let mut message = Message::from_json(&json!({}));
let run = engine("continue").run(&mut message).await;
assert!(matches!(run.status(), RunStatus::CompletedWithErrors));
assert_eq!(message.errors().len(), 1);
assert_eq!(serde_json::to_string(message.context().data()).unwrap(), r#"{"after":true,"next":true}"#);

// `halt`: recorded, the workflow ends, and `next` still runs.
let mut message = Message::from_json(&json!({}));
let run = engine("halt").run(&mut message).await;
assert_eq!(run.workflows().next().unwrap().exit, Some(WorkflowExit::Halted));
assert_eq!(serde_json::to_string(message.context().data()).unwrap(), r#"{"next":true}"#);
# }
```

## Error Codes

Common error codes you'll see in `message.errors()`:

- `VALIDATION_ERROR`: from the `validation` built-in, one per failing rule;
  also from `parse_xml`, `publish_json` and `publish_xml` when the source is
  not what they need
- `EVALUATION_ERROR`: a `validation` rule or a `filter` condition that failed
  to evaluate
- `MAPPING_ERROR`: a `map` mapping that failed, one per mapping, with the
  mapping's destination as the record's `path`; before the task's
  `TASK_STATUS_ERROR`
- `TASK_STATUS_ERROR`: a task returned `Outcome::Status(s)` with `s >= 500`

A host function's `Err` is recorded under its `Error::code()`:

- the code a **service error** chose (`Error::service(code, message)`), or
  `TASK_ERROR` when that code is empty
- `LOGIC_ERROR` (`ErrorKind::Eval`), `BUDGET_EXCEEDED` (`Budget`),
  `TIMEOUT_ERROR` (`TimedOut`), `CANCELLED`, `CONFIG_ERROR` (`Config`) and
  `COMPILATION_ERROR` (`Compile`)

When a `5xx` stops the run, `Run::status()` carries a `TASK_ERROR` reading
`Task <id> failed with status <s>`; when an `Err` stops it, the error itself.
A run cut short by its own deadline or token is `TimedOut` or `Cancelled`,
and the call it cut short is recorded as `TIMEOUT_ERROR` or `CANCELLED` on
its task.

That list is not closed: a function returning a service error contributes
its own code. Switch on `code` with a default arm.

## Service Errors

`ErrorKind` describes engine concerns. When your function fails for a reason
only your service understands (a circuit breaker opened, a tenant hit a rate
limit), classify it yourself:

```rust
use dataflow_rs::Error;
use dataflow_rs::ErrorKind;

let error = Error::service("circuit_open", "upstream unavailable")
    .with_retryable(true)
    .with_source(std::io::Error::other("connection refused"));

assert_eq!(error.code(), "circuit_open");
assert_eq!(error.to_string(), "upstream unavailable");
assert!(error.is_retryable());
assert!(matches!(error.kind(), ErrorKind::Service { .. }));
```

Three things this buys you:

- **The code is passed through verbatim** (not upper-cased) as the record's
  `code`, so the string you switch on is the string you wrote. An empty code
  falls back to `TASK_ERROR`.
- **`retryable` is declared, not inferred.** Only a timeout is retryable
  unless told otherwise. The engine never retries a task on its own, but a
  stop's retryability is what `Run::retryable()` reports, so a host's retry
  loop depends on you declaring it correctly.
- **The source is kept.** `with_source` attaches the cause for
  `std::error::Error::source`; `wrap(context)` prefixes the message and keeps
  the kind, the retryability and the source.

`Display` renders the message alone, so `to_string()` is safe to hand to a
caller. The built-ins record `VALIDATION_ERROR`, `EVALUATION_ERROR` and
`MAPPING_ERROR` themselves; every other code in the list above comes from the engine's
classification of a call's result.

## Branching on Why a Task Failed

`message.errors()` is host-side only: the JSONLogic evaluation context is
exactly `{data, metadata, temp_data}`, so `{"var": "errors"}` resolves to
nothing. To let a workflow branch on *why* a step failed, point the engine at
a context path:

```rust
use dataflow_rs::run::{EngineConfig, ErrorContext};
use dataflow_rs::Engine;

# fn main() -> dataflow_rs::Result<()> {
let config = EngineConfig::default()
    .error_context(ErrorContext::new("metadata.errors".parse()?, 32)?);
let builder = Engine::builder().config(config);
# let _ = builder;
# Ok(()) }
```

The option is off unless you configure it. With no path configured the
engine writes nothing, and the whole mechanism is one `Option` check on a
path that only runs after a task has already failed.

One record is appended per error a task contributes:

```json
{ "workflow_id": "place_order", "task_id": "charge_payment",
  "code": "TIMEOUT_ERROR", "status": 500 }
```

so a later task, or a later workflow, can route on the reason:

```json
{ "in": [ { "var": "metadata.errors.0.code" }, ["TIMEOUT_ERROR", "IO_ERROR"] ] }
```

End to end, with a validation that fails and a task that reads the record:

```rust
use dataflow_rs::run::{EngineConfig, ErrorContext};
use dataflow_rs::{Engine, Message};
use serde_json::json;

# #[tokio::main]
# async fn main() -> dataflow_rs::Result<()> {
let engine = Engine::builder()
    .config(EngineConfig::default()
        .error_context(ErrorContext::new("metadata.errors".parse()?, 32)?))
    .workflow_json(&json!({"id": "w", "tasks": [
        {"id": "check", "function": {"name": "validation", "input": {"rules": [
            {"logic": {"!!": {"var": "data.email"}}, "message": "Email is required"}
        ]}}},
        {"id": "route",
         "condition": {"in": [{"var": "metadata.errors.0.code"}, ["VALIDATION_ERROR"]]},
         "function": {"name": "map", "input": {
             "mappings": [{"path": "data.routed", "logic": "rejected"}]}}}
    ]}))
    .build()
    .expect("the workflow builds");

let mut message = Message::from_json(&json!({}));
engine.run(&mut message).await;

let records = message.context().get(&"metadata.errors".parse()?).expect("one record");
assert_eq!(
    serde_json::to_string(records).unwrap(),
    r#"[{"workflow_id":"w","task_id":"check","code":"VALIDATION_ERROR","status":400}]"#
);
let routed = message.context().get(&"data.routed".parse()?).expect("routed");
assert_eq!(serde_json::to_string(routed).unwrap(), r#""rejected""#);
# Ok(()) }
```

### What is recorded

Coverage matches `errors()`: a function returning `Err`, a task returning a
`5xx` outcome, each failing rule of the `validation` built-in, and anything a
function adds through `Ctx::record_error`. A task returning `Outcome::Skip`
opts out of the per-task record entirely: no audit entry, no
`metadata.progress` write, no record.

`status` is the task's own status: `500` when the function returned `Err`,
otherwise the status the outcome carried (`400` for `validation`, `200` for a
function that recorded an error and still succeeded).
`metadata.progress.status_code` carries the same value.

The engine does **not** record the error `message`. This value lands in the
message's context, which is serialized straight back to callers; read the
message from `message.errors()` host-side instead. This applies to
`temp_data` too: it is part of the context and ships on the wire like
everything else, so it is not private scratch space.

### Practical notes

- **The path is reset at run start.** The engine removes the key before any
  workflow runs, so a message never brings records in, and the key is
  **absent**, not `[]`, when nothing failed.
- The engine keeps at most 32 records by default
  (`ErrorContext::DEFAULT_LIMIT`), newest retained; the limit must be at least
  1. The bound keeps the cost independent of a looping workflow's iteration
  count.
- **The engine owns the configured path.** It replaces a non-array found
  there, and anything but an object on the way to it. `ErrorContext::new`
  refuses a bare root (`data`, `metadata` or `temp_data`) and any path at or
  below the `metadata` slots the engine writes itself: `progress`,
  `processed_at`, `engine_version` and `channel`.
- The append is engine bookkeeping, not a task mutation: the task's audit
  entry does not record it as a `Change`. A trace does, so a replay rebuilds
  the context.

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
                    "logic": {"!!": {"var": "data.email"}},
                    "message": "Email is required"
                }
            ]
        }
    }
}
```

Only a literal `true` passes a rule. The message is evaluated only when the
rule fails, and a rule whose logic fails to evaluate records
`EVALUATION_ERROR` instead.

### Evaluation Errors

A `log` expression that fails to evaluate is written as `<eval error>`, and
the task still returns `200`. A `map` mapping that fails to evaluate is
logged, the remaining mappings still run, and the call ends with status
`500`, a failure the task's `on_failure` decides. A workflow or task condition that fails to evaluate counts as
false; the `Run` reports a workflow's as `Selection::ConditionFailed(error)`.

### Host Function Errors

Return errors from a host function via `Result::Err`:

```rust
use dataflow_rs::{Ctx, Error, Function, Outcome, Result};

struct Charge;

impl Function for Charge {
    type Config = ();

    async fn call(&self, ctx: &mut Ctx<'_>, _cfg: &()) -> Result<Outcome> {
        let amount = ctx.get(&"data.amount".parse()?).and_then(|v| v.as_f64());
        let Some(amount) = amount else {
            return Err(Error::service("MISSING_AMOUNT", "data.amount is not a number"));
        };
        if amount > 10_000.0 {
            // A rejection, not a failure: `on_rejection` decides.
            return Ok(Outcome::Status(422));
        }
        Ok(Outcome::Ok)
    }
}
```

An `Err` drops every write the call made and keeps the records it added
through `Ctx::record_error`; the engine then records the error under its
code, as `Task <id> error: <message>`.

## Error Recovery Patterns

### Fallback Values

Use `on_failure: continue` and a condition to provide fallback values:

```json
{
    "tasks": [
        {
            "id": "try_primary",
            "on_failure": "continue",
            "function": {
                "name": "fetch_primary",
                "input": { "output": "temp_data.result" }
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

A host function's writes are dropped when it returns `Err`, so
`temp_data.result` is absent after a failed `fetch_primary`, and the fallback
runs.

### Validation Before Processing

Validate data before critical operations, and gate on the result:

```jsonc
{
    "tasks": [
        {
            "id": "validate_input",
            "on_rejection": "halt",
            "function": {
                "name": "validation",
                "input": {
                    "rules": [
                        {"logic": {"!!": {"var": "data.required_field"}}, "message": "Required field missing"}
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

A failing `validation` returns status `400`, a rejection, and the default
`on_rejection: continue` lets the `process` task run. `on_rejection: "halt"`
stops the workflow at the failed validation. See
[Control Flow](../advanced/control-flow.md).

## Try It

> **Want more features?** Try the [Full Debugger UI](/dataflow-rs/debugger/) with step-by-step execution and workflow visualization.

<div class="playground-widget" data-workflows='[{"id":"error_demo","name":"Error Demo","on_failure":"continue","tasks":[{"id":"parse","name":"Parse Payload","function":{"name":"parse_json","input":{"source":"payload","target":"input"}}},{"id":"validate_email","name":"Validate Email","function":{"name":"validation","input":{"rules":[{"logic":{"!!":[{"var":"data.input.email"}]},"message":"Email is required"}]}}},{"id":"greet","name":"Greet User","function":{"name":"map","input":{"mappings":[{"path":"data.greeting","logic":{"cat":["Hello, ",{"var":"data.input.name"},"!"]}}]}}}]}]' data-payload='{"name":"John"}'>
</div>

The engine records the validation error, and processing continues.

## Best Practices

1. **Validate Early**
   - Add validation tasks at the start of workflows
   - Set `on_rejection: "halt"` on a validation that must stop the workflow; a
     failing validation alone records `400` and lets the next task run

2. **Use `on_failure: continue` Wisely**
   - Only for optional tasks
   - Critical operations should stop on failure

3. **Check Both the Message and the Run**
   - Walk `message.errors()` after every run
   - Match `Run::status()` for a run that stopped early, and `retryable()` before retrying

4. **Provide Context**
   - Include meaningful error messages
   - Include field paths in validation messages

## From 3.x

In 3.x the two channels were `message.errors()` and the `Err` that
`process_message` returned, every stop added a `WORKFLOW_ERROR` wrapper
record, the policies were `continue_on_error` and `halt_on`, and errors were
`DataflowError` variants. In 4.0 the `Run` replaces the `Err`, no wrapper is
added, the policies are `on_failure` and `on_rejection`, and errors are
`Error` with an `ErrorKind`. The compat reader rewrites the old keys when a
3.x definition loads. See [Migrating from 3.x](../migration/from-3.md).
