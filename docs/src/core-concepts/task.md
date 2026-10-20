# Tasks

A task is an individual processing unit within a workflow that calls one function. Tasks are the **THEN** in the IF → THEN model.

## Overview

Tasks are the building blocks of workflows. Each task:

- Calls a single function (built-in or host)
- Can have a condition for conditional execution
- Can modify the message context
- Records its writes in the audit trail
- Says what its outcome does, through `on_failure` and `on_rejection`

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
| `id` | string | Yes | Step identifier, unique across the workflow's steps (groups share the namespace) |
| `name` | string | No | Human-readable name |
| `description` | string | No | Free-text description |
| `condition` | JSONLogic | No | Whether the task runs (evaluated against the full context); absent means always |
| `function` | object | Yes | The function to call: `{"name": ..., "input": {...}}`; `input` defaults to `{}` |
| `on_failure` | string | No | What a failure (a `5xx` or an `Err`) does: `fail` (default) passes it to the workflow's `on_failure`, `continue` records it and runs the next task, `halt` ends the workflow |
| `on_rejection` | string | No | What a rejection (a `4xx`) does: `continue` (default) runs the next task, `halt` ends the workflow |
| `terminal` | boolean | No | End the workflow once this task has run (default: `false`); see [Control Flow](../advanced/control-flow.md) |
| `for_each` | object | No | Run the function once per element of an array; see [Fan-Out](../advanced/for-each.md). Refused on a built-in |
| `ext` | object | No | Host and UI data, kept verbatim and never read |

An element of `tasks` carrying a `tasks` key is a group, not a task: a span of
steps sharing one condition, evaluated once on entry. See
[Control Flow](../advanced/control-flow.md#groups).

## Reading a Parsed Task

A definition is plain data. `def::parse` gives a `WorkflowDef` whose `tasks`
are `StepDef`s, each a `Task(TaskDef)` or a `Group(GroupDef)`, and
`WorkflowDef::walk` visits every step with its authored path:

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

// Every step, pre-order, each group before its members.
let paths: Vec<String> = def.walk().map(|(path, _, step)| format!("{path} = {}", step.id())).collect();
assert_eq!(paths, ["tasks[0] = apply_discount", "tasks[1] = guard", "tasks[1].tasks[0] = perk"]);
```

The definition types are `#[non_exhaustive]`: read their fields, but build
one through `parse`, which is where every structural rule is checked.

## Function Configuration

The `function` object specifies what the task does:

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
| `map` | Data transformation and field mapping |
| `validation` | Data validation with custom error messages |
| `filter` | Pipeline control flow: halt the workflow or skip the task |
| `log` | Structured logging with JSONLogic expressions |
| `parse_json` | Parse JSON from the payload, or a context path, into `data` |
| `parse_xml` | Parse an XML string into a JSON data structure |
| `publish_json` | Serialize data to a JSON string |
| `publish_xml` | Serialize data to an XML string |

The built-ins run inline, over the run's single view of the context. Their
names are reserved: a host function cannot be registered under one.

### Host Functions

A host function implements `Function` with a typed `Config`, compiled from
the task's `input` when the workflow is built:

```rust
use dataflow_rs::datavalue::OwnedDataValue;
use dataflow_rs::expr::Output;
use dataflow_rs::function::FunctionConfig;
use dataflow_rs::{Ctx, Engine, Expr, Function, Outcome, Result};
use serde_json::json;

#[derive(FunctionConfig)]
struct GreetConfig {
    /// Who to greet: a literal or JSONLogic.
    name: Expr<String>,
    /// Where the greeting is written.
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

Then reference it by name in a task:

```json
{
    "function": {
        "name": "greet",
        "input": { "name": {"var": "data.user"} }
    }
}
```

A task whose `function.name` is neither a built-in nor a registered function
is an `UNKNOWN_FUNCTION` issue, and the workflow is rejected. See
[Custom Functions](../advanced/custom-functions.md).

## Conditional Execution

Tasks can have conditions that determine if they should run. Conditions evaluate against the **full context** (`data`, `metadata`, `temp_data`), and may read `{"secret": "name"}` from the engine's [secret store](../advanced/secrets.md). A condition collapses to a bool, so nothing of the value is recorded:

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

A task whose condition is false is skipped: no audit entry, no
`metadata.progress` write, and no event to an observer.

### Common Patterns

```jsonc
// Only if field exists
{"!!": {"var": "data.email"}}

// Only if field equals value
{"==": [{"var": "data.status"}, "active"]}

// Only if numeric condition
{">=": [{"var": "data.amount"}, 100]}

// Combine conditions
{"and": [
    {"!!": {"var": "data.email"}},
    {"==": [{"var": "data.verified"}, true]}
]}
```

## Outcomes and Policies

A function call ends in one of four `Outcome`s, or an `Err`:

| Result | Audit status | Class | Then |
|---|---|---|---|
| `Outcome::Ok` | `200` | success | the next task runs |
| `Outcome::Status(2xx or 3xx)` | that status | success | the next task runs |
| `Outcome::Status(4xx)` | that status | rejection | `on_rejection` decides |
| `Outcome::Status(5xx)` | that status | failure | `TASK_STATUS_ERROR` is recorded; `on_failure` decides |
| `Err(error)` | `500` | failure | the error is recorded under its code; `on_failure` decides |
| `Outcome::Skip` | no entry | | nothing is recorded; the next task runs |
| `Outcome::Halt` | `299` | | the workflow ends; later workflows still run |

### Failure

```jsonc
{
    "id": "optional_task",
    "on_failure": "continue",
    "function": { ... }
}
```

- `fail` (default): the failure passes to the workflow's `on_failure`, which
  stops the run or, with `continue`, lets the next workflow run
- `continue`: the failure is recorded in `message.errors()` and the next task
  runs
- `halt`: the failure is recorded and the workflow ends; later workflows run

### Rejection

```jsonc
{
    "id": "validate_input",
    "on_rejection": "halt",
    "function": { "name": "validation", "input": { ... } }
}
```

- `continue` (default): the next task runs. A failing `validation` returns
  `400` and records a `VALIDATION_ERROR` per failing rule, so with the default
  the pipeline proceeds as if it had passed; `Compiler::check` reports that
  shape as the `UNGUARDED_VALIDATION` advisory
- `halt`: the workflow ends at the rejected task; later workflows run

`terminal: true` is applied after the classification: a terminal task that
fails still records its failure and still follows `on_failure`. It upgrades
only a task that would otherwise go on to a halt. See
[Error Handling](./error-handling.md) for the full matrix.

## Sequential Execution

Tasks execute in order within a workflow. Later tasks can use results from earlier tasks:

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

A host function's writes are visible to its own later reads at once, and to
the next task once the call returns `Ok`.

## Try It

> Open the [Full Debugger UI](/dataflow-rs/debugger/) for step-by-step execution and workflow visualization.

<div class="playground-widget" data-workflows='[{"id":"conditional_tasks","name":"Conditional Tasks","tasks":[{"id":"parse","name":"Parse Payload","function":{"name":"parse_json","input":{"source":"payload","target":"input"}}},{"id":"check_premium","name":"Check Premium","condition":{"==":[{"var":"data.input.tier"},"premium"]},"function":{"name":"map","input":{"mappings":[{"path":"data.discount","logic":20}]}}},{"id":"check_standard","name":"Check Standard","condition":{"==":[{"var":"data.input.tier"},"standard"]},"function":{"name":"map","input":{"mappings":[{"path":"data.discount","logic":5}]}}},{"id":"apply_discount","name":"Apply Discount","function":{"name":"map","input":{"mappings":[{"path":"data.final_price","logic":{"-":[{"var":"data.input.price"},{"/":[{"*":[{"var":"data.input.price"},{"var":"data.discount"}]},100]}]}}]}}}]}]' data-payload='{"tier":"premium","price":100}'>
</div>

Try changing `tier` to "standard" to see a different discount applied.

## Best Practices

1. **Unique IDs** - Use descriptive, unique IDs for debugging; they appear in every audit entry, error record and trace step
2. **Single Responsibility** - Each task should do one thing well
3. **Use temp_data** - Store intermediate results in `temp_data`
4. **Conditions** - Add conditions to skip unnecessary processing
5. **Policies** - Use `on_failure: continue` for optional tasks, and `on_rejection: halt` on a validation that must gate the tasks after it

## From 3.x

In 3.x a task's error handling was `continue_on_error` and `halt_on`, and
tasks were built in Rust through `Task::action` on a `#[non_exhaustive]`
`Task`. In 4.0 the two policies are `on_failure` and `on_rejection`, and a
task is plain data (`TaskDef`) that enters through `def::parse`. The compat
reader rewrites the 3.x keys: `continue_on_error: true` becomes
`on_failure: continue`, `halt_on: "failure"` becomes `on_rejection: halt`,
and both together become `on_failure: halt` with `on_rejection: halt`. See
[Migrating from 3.x](../migration/from-3.md).
