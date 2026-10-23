# Tasks

A task is an individual processing step within a workflow that calls a single function. Tasks represent the **THEN** in the IF → THEN model.

## Overview

Each task in a workflow:

- Calls one function (either a built-in function or a registered custom host function)
- Optionally evaluates a JSONLogic `condition` to decide whether to run
- Can read and modify the message context (`data`, `metadata`, `temp_data`)
- Records its changes in the message's audit trail
- Configures how failures and rejections are handled via `on_failure` and `on_rejection`

## Task Structure

```json
{
    "id": "apply_discount",
    "name": "Apply Discount",
    "condition": { ">=": [{"var": "data.order.total"}, 100] },
    "on_failure": "fail",
    "on_rejection": "continue",
    "function": {
        "name": "map",
        "input": {
            "mappings": [
                {
                    "path": "data.order.discount",
                    "logic": {"*": [{"var": "data.order.total"}, 0.1]}
                }
            ]
        }
    }
}
```

## Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `id` | string | Yes | Unique step identifier within the workflow (shared across tasks and task groups) |
| `name` | string | No | Human-readable display name |
| `description` | string | No | Free-text description |
| `condition` | JSONLogic | No | Condition evaluated against `{data, metadata, temp_data}`; when omitted, the task always runs |
| `function` | object | Yes | Function call specification: `{"name": "...", "input": {...}}` (`input` defaults to `{}`) |
| `on_failure` | string | No | Behavior on failure (`5xx` status or `Err`): `"fail"` (default) escalates to the workflow's `on_failure`, `"continue"` records the error and continues to the next task, `"halt"` stops the current workflow |
| `on_rejection` | string | No | Behavior on rejection (`4xx` status): `"continue"` (default) continues to the next task, `"halt"` stops the current workflow |
| `terminal` | boolean | No | If `true`, ends the current workflow after this task runs (default: `false`); see [Control Flow](../advanced/control-flow.md) |
| `for_each` | object | No | Fan-out configuration to run a host function once per array element; see [Fan-Out](../advanced/for-each.md) (not supported on built-in functions) |
| `ext` | object | No | Custom host or UI metadata preserved verbatim by the parser |

> **Task Groups:** An entry in a workflow's `tasks` array that contains a nested `tasks` key (instead of `function`) is parsed as a **task group** (`GroupDef`)—a sequence of steps gated by a shared condition. See [Control Flow → Groups](../advanced/control-flow.md#groups).

## Inspecting Parsed Tasks in Rust

`def::parse` produces a `WorkflowDef` whose `tasks()` slice contains `StepDef::Task(TaskDef)` and `StepDef::Group(GroupDef)` items. Use `WorkflowDef::walk` to iterate through all steps in pre-order along with their JSON paths:

```rust
use dataflow_rs::def::{FailurePolicy, ParseOptions, RejectionPolicy, StepDef, parse};
use serde_json::json;

let parsed = parse(
    &json!({"id": "w", "tasks": [
        {"id": "apply_discount", "on_failure": "continue",
         "function": {"name": "map", "input": {"mappings": []}}},
        {"id": "guard", "condition": {"var": "data.vip"}, "tasks": [
            {"id": "perk", "function": {"name": "log", "input": {"message": "vip"}}}
        ]}
    ]}),
    ParseOptions::default(),
);
let def = parsed.def.expect("nothing was rejected");

let StepDef::Task(task) = &def.tasks()[0] else { panic!("a task") };
assert_eq!(task.function().name(), "map");
assert_eq!(task.on_failure(), FailurePolicy::Continue);
assert_eq!(task.on_rejection(), RejectionPolicy::Continue);
assert!(task.condition().is_none());

// Walk every step in pre-order (each group is visited before its child steps).
let paths: Vec<String> = def.walk().map(|(path, _, step)| format!("{path} = {}", step.id())).collect();
assert_eq!(paths, ["tasks[0] = apply_discount", "tasks[1] = guard", "tasks[1].tasks[0] = perk"]);
```

Definition structs are marked `#[non_exhaustive]`; construct them by parsing JSON via `def::parse` so all structural validation rules are enforced.

## Function Configuration

The `function` block specifies which function to call and its `input` configuration:

```jsonc
{
    "function": {
        "name": "function_name",
        "input": { ... }
    }
}
```

### Built-in Functions

| Function | Purpose |
|----------|---------|
| `map` | Transform data and write values to `data`, `metadata`, or `temp_data` |
| `validation` | Validate context fields against JSONLogic rules and record custom error messages |
| `filter` | Control pipeline execution: halt the workflow or skip the current task when a condition is false |
| `log` | Emit structured log messages with dynamic JSONLogic fields |
| `parse_json` | Parse or copy JSON from `payload` (or a context path) into `data` |
| `parse_xml` | Parse an XML string from `payload` (or a context path) into `data` |
| `publish_json` | Serialize a `data` object to a JSON string |
| `publish_xml` | Serialize a `data` object to an XML string |

All eight built-in functions execute inline on the engine's context view. Their names are reserved and cannot be overwritten by custom host functions.

### Custom Host Functions

Custom host functions implement the `Function` trait with a typed `Config` struct (derived with `#[derive(FunctionConfig)]`) that is compiled from the task's `input` at build time:

```rust
use dataflow_rs::datavalue::OwnedDataValue;
use dataflow_rs::expr::Output;
use dataflow_rs::function::FunctionConfig;
use dataflow_rs::{Ctx, Engine, Expr, Function, Outcome, Result};
use serde_json::json;

#[derive(FunctionConfig)]
struct GreetConfig {
    /// Name to greet (can be a literal string or a JSONLogic expression).
    name: Expr<String>,
    /// Destination path where the greeting is written.
    #[output(default = "data.greeting")]
    output: Output,
}

struct Greet;

impl Function for Greet {
    type Config = GreetConfig;

    async fn call(&self, ctx: &mut Ctx<'_>, cfg: &GreetConfig) -> Result<Outcome> {
        let name = ctx.eval(&cfg.name)?;
        ctx.emit(&cfg.output, OwnedDataValue::String(format!("Hello, {name}!")))?;
        Ok(Outcome::Ok)
    }
}

# fn main() -> std::result::Result<(), Vec<dataflow_rs::Issue>> {
let engine = Engine::builder()
    .register("greet", Greet)
    .workflow_json(&json!({"id": "w", "tasks": [
        {"id": "t", "function": {"name": "greet", "input": {
            "name": {"var": "data.user"}}}}
    ]}))
    .build()?;
# let _ = engine;
# Ok(()) }
```

Reference your registered function by name in any task:

```json
{
    "function": {
        "name": "greet",
        "input": { "name": {"var": "data.user"} }
    }
}
```

If a task references a `function.name` that is neither a built-in nor registered in the `Registry`, compilation rejects the workflow with an `UNKNOWN_FUNCTION` issue. See [Custom Functions](../advanced/custom-functions.md) for details.

## Conditional Execution

A task's `condition` is evaluated against `{data, metadata, temp_data}` (and can also read `{"secret": "name"}` from the engine's [secret store](../advanced/secrets.md)). Because condition evaluation produces only a boolean, secret values used in conditions are never written to the message or trace:

```json
{
    "id": "premium_greeting",
    "name": "Premium greeting",
    "condition": { "==": [{"var": "data.tier"}, "premium"] },
    "function": {
        "name": "map",
        "input": {
            "mappings": [
                {"path": "data.greeting", "logic": "Welcome, VIP member!"}
            ]
        }
    }
}
```

When a task's condition evaluates to `false`, the task is skipped: no audit entry is added, `metadata.progress` is not modified, and no `TaskFinished` event is sent to observers.

### Common Condition Patterns

```jsonc
// Run only if field exists and is truthy
{"!!": {"var": "data.email"}}

// Run only if field equals a value
{"==": [{"var": "data.status"}, "active"]}

// Run only if numeric threshold is met
{">=": [{"var": "data.amount"}, 100]}

// Combine multiple conditions
{"and": [
    {"!!": {"var": "data.email"}},
    {"==": [{"var": "data.verified"}, true]}
]}
```

## Outcomes and Policies

Every task execution finishes with an `Outcome` or an `Err`, which the engine classifies as follows:

| Function Result | Audit Status | Classification | Next Step |
|---|---|---|---|
| `Outcome::Ok` | `200` | Success | Run the next task |
| `Outcome::Status(2xx or 3xx)` | `2xx` / `3xx` | Success | Run the next task |
| `Outcome::Status(4xx)` | `4xx` | Rejection | Governed by `on_rejection` |
| `Outcome::Status(5xx)` | `5xx` | Failure | Records `TASK_STATUS_ERROR`; governed by `on_failure` |
| `Err(error)` | `500` | Failure | Records error under `error.code()`; governed by `on_failure` |
| `Outcome::Skip` | *(no entry)* | Skipped | Nothing recorded; run the next task |
| `Outcome::Halt` | `299` | Halted | Ends the current workflow; subsequent workflows still run |

### Failure Policy (`on_failure`)

```jsonc
{
    "id": "optional_task",
    "on_failure": "continue",
    "function": { ... }
}
```

- **`"fail"`** (default) — Escalates the failure to the workflow's `on_failure` policy (which either stops the run or moves to the next workflow).
- **`"continue"`** — Records the failure in `message.errors()` and continues to the next task in the workflow.
- **`"halt"`** — Records the failure and ends the current workflow (`WorkflowExit::Halted`), while allowing subsequent workflows to run.

### Rejection Policy (`on_rejection`)

```jsonc
{
    "id": "validate_input",
    "on_rejection": "halt",
    "function": { "name": "validation", "input": { ... } }
}
```

- **`"continue"`** (default) — Continues to the next task. Note that a failed `validation` task returns status `400` (a rejection) and records `VALIDATION_ERROR` entries; if you leave `on_rejection` at `"continue"` and have tasks after it, those tasks will still run (`Compiler::check` flags this pattern with an `UNGUARDED_VALIDATION` advisory).
- **`"halt"`** — Stops the current workflow when the task returns a `4xx` rejection, while allowing subsequent workflows to run.

> **Interaction with `terminal: true`:** `terminal` is evaluated after outcome policies. If a `terminal: true` task fails, it still follows its `on_failure` policy; if the task succeeds (or continues after a rejection/failure), `terminal: true` halts the rest of the current workflow. See [Error Handling](./error-handling.md) for the complete policy matrix.

## Sequential Execution

Tasks execute sequentially in the order they appear in `tasks`. Each task immediately sees writes committed by earlier tasks:

```json
{
    "tasks": [
        {
            "id": "step1",
            "function": {
                "name": "map",
                "input": {
                    "mappings": [
                        {"path": "temp_data.intermediate", "logic": {"var": "data.raw"}}
                    ]
                }
            }
        },
        {
            "id": "step2",
            "function": {
                "name": "map",
                "input": {
                    "mappings": [
                        {"path": "data.final", "logic": {"var": "temp_data.intermediate"}}
                    ]
                }
            }
        }
    ]
}
```

Inside a custom host function, any write made via `Ctx` is immediately readable by later `ctx.get` / `ctx.eval` calls within the same invocation, and is committed to the shared message context once `call` returns `Ok`.

## Try It

> Open the [Full Debugger UI](/dataflow-rs/debugger/) for step-by-step execution and workflow visualization.

<div class="playground-widget" data-workflows='[{"id":"conditional_tasks","name":"Conditional Tasks","tasks":[{"id":"parse","name":"Parse Payload","function":{"name":"parse_json","input":{"source":"payload","target":"input"}}},{"id":"check_premium","name":"Check Premium","condition":{"==":[{"var":"data.input.tier"},"premium"]},"function":{"name":"map","input":{"mappings":[{"path":"data.discount","logic":20}]}}},{"id":"check_standard","name":"Check Standard","condition":{"==":[{"var":"data.input.tier"},"standard"]},"function":{"name":"map","input":{"mappings":[{"path":"data.discount","logic":5}]}}},{"id":"apply_discount","name":"Apply Discount","function":{"name":"map","input":{"mappings":[{"path":"data.final_price","logic":{"-":[{"var":"data.input.price"},{"/":[{"*":[{"var":"data.input.price"},{"var":"data.discount"}]},100]}]}}]}}}]}]' data-payload='{"tier":"premium","price":100}'>
</div>

Try changing `"tier"` to `"standard"` in the payload to see a different discount task execute.

## Best Practices

1. **Use Descriptive, Unique IDs** — Task IDs appear in audit entries, error records, `metadata.progress`, and traces.
2. **Keep Tasks Focused** — Give each task a clear, single responsibility.
3. **Use `temp_data` for Scratch State** — Store intermediate values in `temp_data` rather than cluttering `data`.
4. **Set Explicit Policies** — Use `"on_failure": "continue"` for non-critical enrichment tasks, and `"on_rejection": "halt"` on `validation` tasks that should block subsequent steps in the workflow.

## Migrating from 3.x

In 3.x, task error handling used `continue_on_error` and `halt_on`, and tasks were constructed in Rust via `Task::action`. In 4.0, tasks use `on_failure` and `on_rejection` and are parsed via `def::parse` into `TaskDef`. The compatibility reader automatically rewrites 3.x task keys:
- `continue_on_error: true` → `on_failure: "continue"`
- `halt_on: "failure"` → `on_rejection: "halt"`
- Both together → `on_failure: "halt"` and `on_rejection: "halt"`

See [Migrating from 3.x](../migration/from-3.md) for details.
