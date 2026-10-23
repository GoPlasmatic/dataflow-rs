# Error Handling

Dataflow-rs provides structured error handling across tasks, workflows, and runs.

## Two Error Channels: `message.errors()` and `Run::status()`

Every run reports errors through two complementary places:

- **`message.errors()`** — Contains every `ErrorRecord` recorded during the run: failed `validation` rules, `5xx` status outcomes, `Err` returns from host functions, and errors explicitly recorded via `Ctx::record_error`. Each record includes `code`, `message`, `workflow_id`, `task_id`, and `timestamp`.
- **`Run::status()`** — Describes how the overall run ended:
  - `Completed`: All matching workflows finished with no error records on the message.
  - `CompletedWithErrors`: All matching workflows finished, and one or more errors were recorded in `message.errors()`.
  - `Stopped(error)`: A task failed and its failure policy stopped the run before all workflows finished.
  - `TimedOut` / `Cancelled`: The run hit its `RunOptions::deadline` or `CancelToken`.

When a run stops early, no duplicate wrapper error is added to `message.errors()`—the failing task's own `ErrorRecord` is on the message, and `RunStatus::Stopped(error)` carries the stopping `Error`.

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

// Check if an unhandled failure stopped the run early:
if let RunStatus::Stopped(error) = run.status() {
    eprintln!("stopped early: {error} ({})", error.code());
}
// Or convert fatal stops (`Stopped`, `TimedOut`, `Cancelled`) into `Result::Err`:
let run = run.into_result();
# let _ = run;
# }
```

`run.into_result()` returns `Ok(run)` for `Completed` and `CompletedWithErrors`, and `Err(error)` when the run was `Stopped`, `TimedOut`, or `Cancelled`.

## Failures vs. Rejections

Before any policy is applied, the engine classifies each task's return value:

| Task Return Value | Classification | Error Recorded on `message.errors()` |
|---|---|---|
| `Ok(Outcome::Ok)` or status `< 400` | **Success** | None |
| `Ok(Outcome::Status(4xx))` | **Rejection** | None by the engine (`validation` records its own failing rules before returning `400`) |
| `Ok(Outcome::Status(5xx))` | **Failure** | `TASK_STATUS_ERROR`: `Task <id> returned status <s>` |
| `Err(error)` | **Failure** | `error.code()`: `Task <id> error: <message>` |
| `Ok(Outcome::Skip)` | **Skipped** | None (and no audit entry is written) |
| `Ok(Outcome::Halt)` | **Halted** | None (ends the current workflow with status `299`) |

Tasks configure how rejections and failures are handled using `on_rejection` and `on_failure`, while workflows configure what happens when a task failure escalates to the workflow using workflow-level `on_failure`.

## Task-Level Policies

### `on_failure`

Controls what happens when a task returns `Err` or `Outcome::Status(5xx)`:

```jsonc
{
    "id": "optional_task",
    "on_failure": "continue",
    "function": { ... }
}
```

- **`"fail"`** (default) — Stops the current workflow and escalates the failure to the workflow's `on_failure` policy.
- **`"continue"`** — Records the failure in `message.errors()` and continues to the next task in the workflow.
- **`"halt"`** — Records the failure and ends the current workflow (`WorkflowExit::Halted`), while allowing subsequent workflows to run.

### `on_rejection`

Controls what happens when a task returns `Outcome::Status(4xx)`:

```jsonc
{
    "id": "validate_input",
    "on_rejection": "halt",
    "function": { "name": "validation", "input": { ... } }
}
```

- **`"continue"`** (default) — Continues to the next task in the workflow.
- **`"halt"`** — Ends the current workflow immediately (`WorkflowExit::Halted`), while allowing subsequent workflows to run.

> **Note:** A `4xx` rejection from a custom host function (such as `Outcome::Status(404)`) records status `404` on the audit trail without adding an entry to `message.errors()`. The built-in `validation` function both records a `VALIDATION_ERROR` per failing rule on `message.errors()` and returns `400`.

## Workflow-Level Policy

A workflow's `on_failure` field answers a **different question** than a task's `on_failure` field—it is not a default inherited by tasks:

| Policy Field | Question It Answers |
|------|--------------------|
| `on_failure` on a **task** | When this task fails, should the *remaining tasks in this workflow* still run? |
| `on_failure` on a **workflow** | When a task failure stops this workflow, should *subsequent workflows* still run, or should the entire run stop? |

([Task groups](../advanced/control-flow.md#groups) only share a condition; they do not have their own failure policy.)

### The Policy Matrix

When a task fails (returns `Err` or a `5xx` status), the combination of task `on_failure` and workflow `on_failure` determines what runs next:

| Task `on_failure` | Workflow `on_failure` | Remaining Tasks in Workflow | Subsequent Workflows | Final `Run::status()` |
|---|---|---|---|---|
| `"fail"` (default) | `"fail"` (default) | Stop | Stop | `Stopped(error)` |
| `"fail"` | `"continue"` | Stop | Run | `CompletedWithErrors` (workflow exit is `Failed`) |
| `"halt"` | *(either)* | Stop | Run | `CompletedWithErrors` (workflow exit is `Halted`) |
| `"continue"` | *(either)* | Run | Run | `CompletedWithErrors` |

In every case above, the failure is recorded in `message.errors()`.

For example, if a workflow sets `"on_failure": "continue"` while its tasks keep the default `"on_failure": "fail"`, a failure in `task1` stops that workflow immediately, but subsequent workflows in the engine still run:

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

Here, `task1` and `task2` explicitly opt into `"on_failure": "continue"`, so the workflow keeps running even if they fail. `task3` uses the default `"fail"`, so if `task3` fails, `resilient_workflow` exits with `Failed` and the engine moves on to the next workflow.

### Policy Matrix Example in Rust

```rust
use dataflow_rs::run::{RunStatus, WorkflowExit};
use dataflow_rs::{Ctx, Engine, Error, Function, Message, Outcome, Result};
use serde_json::json;

/// A custom function that always fails with a service error code.
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

// 1. Default ("fail"): stops the entire run. Neither `after` nor `next` runs.
let mut message = Message::from_json(&json!({}));
let run = engine("fail").run(&mut message).await;
let RunStatus::Stopped(error) = run.status() else { panic!("stopped") };
assert_eq!(error.code(), "PARTNER_DOWN");
assert!(run.retryable() && run.partial());
assert_eq!(message.errors()[0].code, "PARTNER_DOWN");
assert_eq!(message.errors()[0].message, "Task call error: the partner is down");
assert_eq!(message.errors()[0].task_id.as_deref(), Some("call"));
assert!(message.context().get(&"data.next".parse().unwrap()).is_none());

// 2. "continue": records the error and runs both `after` and `next`.
let mut message = Message::from_json(&json!({}));
let run = engine("continue").run(&mut message).await;
assert!(matches!(run.status(), RunStatus::CompletedWithErrors));
assert_eq!(message.errors().len(), 1);
assert_eq!(serde_json::to_string(message.context().data()).unwrap(), r#"{"after":true,"next":true}"#);

// 3. "halt": records the error, ends workflow `w` (skipping `after`), and runs `next`.
let mut message = Message::from_json(&json!({}));
let run = engine("halt").run(&mut message).await;
assert_eq!(run.workflows().next().unwrap().exit, Some(WorkflowExit::Halted));
assert_eq!(serde_json::to_string(message.context().data()).unwrap(), r#"{"next":true}"#);
# }
```

## Error Codes

Common error codes recorded in `message.errors()`:

- **`VALIDATION_ERROR`** — Recorded by `validation` for each failing rule, and by `parse_xml`, `publish_json`, and `publish_xml` when input data is missing or invalid.
- **`EVALUATION_ERROR`** — Recorded when a `validation` rule or `filter` condition fails to evaluate.
- **`MAPPING_ERROR`** — Recorded for each individual `map` mapping that fails (with the mapping's target path in `record.path`), followed by `TASK_STATUS_ERROR` for the task.
- **`TASK_STATUS_ERROR`** — Recorded when a task returns `Outcome::Status(s)` with `s >= 500`.

When a custom host function returns `Err(error)`, the record uses `error.code()`:
- The custom code passed to **`Error::service(code, message)`** (or `"TASK_ERROR"` if `code` is empty).
- Built-in engine error codes: `LOGIC_ERROR` (`ErrorKind::Eval`), `BUDGET_EXCEEDED` (`Budget`), `TIMEOUT_ERROR` (`TimedOut`), `CANCELLED`, `CONFIG_ERROR` (`Config`), and `COMPILATION_ERROR` (`Compile`).

Because custom functions can define their own service error codes, always include a wildcard/default arm when matching on `error.code`.

## Custom Service Errors (`Error::service`)

Built-in `ErrorKind` variants cover engine-level errors. When a custom host function fails for a domain- or service-specific reason (such as an open circuit breaker, upstream 503, or rate limit), construct an error with `Error::service`:

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

Key behaviors:
- **Exact code preservation:** The engine keeps your `code` string as-is (without altering case) in `ErrorRecord::code` and `ErrorContext`. An empty string falls back to `"TASK_ERROR"`.
- **Explicit retryability:** Only engine timeouts default to `retryable = true`. For service errors, call `.with_retryable(true)` when the caller can safely retry the run (`Run::retryable()` reflects the stopping error's flag).
- **Error chaining:** Use `.with_source(err)` to attach an underlying `std::error::Error` cause, and `.wrap("context")` to prefix the message while preserving the error kind, code, retryability, and source.

## Branching on Task Failures (`ErrorContext`)

By default, `message.errors()` is only accessible from Rust after (or outside) JSONLogic evaluation, because JSONLogic expressions only see `{data, metadata, temp_data}`.

If you want subsequent tasks or workflows to branch on *why* an earlier task failed, configure an `ErrorContext` path on `EngineConfig`:

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

When `ErrorContext` is configured, each error recorded by a task is mirrored as a compact object at the configured path:

```json
{ "workflow_id": "place_order", "task_id": "charge_payment",
  "code": "TIMEOUT_ERROR", "status": 500 }
```

Subsequent task or workflow conditions can then inspect the error code directly:

```json
{ "in": [ { "var": "metadata.errors.0.code" }, ["TIMEOUT_ERROR", "IO_ERROR"] ] }
```

### End-to-End Example

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

### `ErrorContext` Details and Rules

- **Mirrored events:** Mirrors the exact same events as `message.errors()` (`Err` returns, `5xx` outcomes, failed `validation` rules, and `Ctx::record_error` calls).
- **Omitted message text:** Mirrored records include `{workflow_id, task_id, code, status}` but intentionally omit the free-text error `message` so internal details are not exposed in serialized message contexts. Read full error messages from `message.errors()` in Rust.
- **Reset at run start:** The engine clears the configured path at the start of every run. If no errors occur during a run, the path remains absent rather than `[]`.
- **Bounded capacity:** Retains up to `limit` newest records (`ErrorContext::DEFAULT_LIMIT` is `32`; minimum is `1`), preventing unbounded growth in loops.
- **Reserved path validation:** `ErrorContext::new` rejects bare roots (`data`, `metadata`, `temp_data`) and reserved `metadata` keys (`progress`, `processed_at`, `engine_version`, `channel`).
- **Audit vs. Trace:** Mirrored error-context writes do not clutter task `AuditEntry::changes`, but are recorded in `TraceStep::changes` so trace replay accurately reconstructs `metadata.errors`.

## Error Behavior by Task Type

### Validation Errors
Generated by the `validation` built-in when rules do not evaluate to `true`:

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

A rule passes only when `logic` evaluates to boolean `true`. The `message` expression is evaluated only when the rule fails. If `logic` itself fails to evaluate, `EVALUATION_ERROR` is recorded instead.

### Expression Evaluation Errors
- **`log`**: If a `message` or `fields` expression fails to evaluate, `"<eval error>"` is logged and the task still succeeds with status `200`.
- **`map`**: If a mapping fails, `MAPPING_ERROR` is recorded on `message.errors()` (with `path` set to the mapping's target), remaining mappings still execute, and the task finishes with status `500` (governed by `on_failure`).
- **`filter`**: If the condition fails to evaluate, `EVALUATION_ERROR` is recorded and the filter treats the condition as rejected (`299` under `halt`, or `Skip` under `skip`).
- **Operation budget (`ops_budget`)**: Exceeding the configured operation budget in `validation`, `map`, or `filter` fails the task with `BUDGET_EXCEEDED` under `on_failure`.
- **Workflow & task conditions**: If a workflow or task `condition` fails to evaluate, it is treated as `false` (and recorded on `Run` as `Selection::ConditionFailed(error)` for workflows).

### Custom Host Function Errors
Return `Err(...)` for failures or `Ok(Outcome::Status(4xx))` for rejections:

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
            // Return a 4xx rejection (governed by `on_rejection`).
            return Ok(Outcome::Status(422));
        }
        Ok(Outcome::Ok)
    }
}
```

When a host function returns `Err`, all context writes made during that call are discarded, while any error records added via `Ctx::record_error` are kept alongside the returned `Err`.

## Common Error Recovery Patterns

### Fallback Values

Combine `"on_failure": "continue"` with a conditional fallback task:

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

Because a failed host function call discards its writes, `temp_data.result` remains absent when `try_primary` fails, causing `use_fallback` to run.

### Halting on Failed Validation

Set `"on_rejection": "halt"` on a `validation` task to prevent subsequent tasks in the workflow from running on invalid data:

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

## Try It

> Open the [Full Debugger UI](/dataflow-rs/debugger/) for step-by-step execution and workflow visualization.

<div class="playground-widget" data-workflows='[{"id":"error_demo","name":"Error Demo","on_failure":"continue","tasks":[{"id":"parse","name":"Parse Payload","function":{"name":"parse_json","input":{"source":"payload","target":"input"}}},{"id":"validate_email","name":"Validate Email","function":{"name":"validation","input":{"rules":[{"logic":{"!!":[{"var":"data.input.email"}]},"message":"Email is required"}]}}},{"id":"greet","name":"Greet User","function":{"name":"map","input":{"mappings":[{"path":"data.greeting","logic":{"cat":["Hello, ",{"var":"data.input.name"},"!"]}}]}}}]}]' data-payload='{"name":"John"}'>
</div>

Notice how `validate_email` records a `VALIDATION_ERROR`, and `greet` still runs because `on_rejection` defaults to `"continue"`.

## Best Practices

1. **Validate Early and Set `on_rejection: "halt"`** — Place validation tasks at the beginning of a workflow and set `"on_rejection": "halt"` whenever invalid data should block downstream tasks.
2. **Use `on_failure: "continue"` Selectively** — Reserve `"continue"` for optional enrichment or non-critical side effects.
3. **Check Both `Run::status()` and `message.errors()`** — Inspect `Run::status()` to see if execution stopped early, and iterate over `message.errors()` to see all recorded validation and task errors.

## Migrating from 3.x

In 3.x, `process_message` returned `Result<(), DataflowError>`, stopping runs added an extra `WORKFLOW_ERROR` wrapper record to `message.errors`, and error policies were configured with `continue_on_error` and `halt_on`.

In 4.0:
- `Engine::run` always returns a `Run` (use `run.status()` or `run.into_result()`).
- No duplicate `WORKFLOW_ERROR` record is added when a run stops.
- Policies use `on_failure` and `on_rejection` (3.x keys are automatically rewritten by the compatibility reader).
- Errors use `Error` and `ErrorKind` instead of `DataflowError`.

See [Migrating from 3.x](../migration/from-3.md) for full details.
