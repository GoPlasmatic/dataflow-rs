# Actions (Tasks)

An Action (also called Task) is an individual processing unit within a rule that executes a function. Actions are the **THEN** in the IF → THEN model.

## Overview

Actions are the building blocks of rules. Each action:

- Executes a single function (built-in or custom)
- Can have a condition for conditional execution
- Can modify message data
- Records changes in the audit trail

## Action Structure

```json
{
    "id": "apply_discount",
    "name": "Apply Discount",
    "condition": { ">=": [{"var": "data.order.total"}, 100] },
    "continue_on_error": false,
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
| `id` | string | Yes | Unique action identifier within rule |
| `name` | string | Yes | Human-readable name |
| `description` | string | No | Free-text description |
| `condition` | JSONLogic | No | When to execute action (evaluated against full context) |
| `continue_on_error` | boolean | No | Run the rule's remaining actions even if this one fails (default: `false`) |
| `terminal` | boolean | No | End the workflow once this action has run (default: `false`) — see [Control Flow](../advanced/control-flow.md) |
| `halt_on` | string | No | `"failure"` ends the workflow when this action *failed* (default: `"never"`) — see [Control Flow](../advanced/control-flow.md#halt_on) |
| `function` | object | Yes | Function to execute |

## Creating Actions Programmatically

```rust
# use dataflow_rs::FunctionConfig;
# fn _demo(function_config: FunctionConfig) {
use dataflow_rs::{Action, FunctionConfig};

let action = Action::action(
    "apply_discount",
    "Apply Discount",
    function_config,
);
# }
```

`Task` is `#[non_exhaustive]` as of 3.7.0, so a struct literal no longer
compiles from outside the crate: three of its fields — `id_arc`,
`compiled_condition`, `group_starts` — are engine internals that a literal
forced every caller to name. Field reads, writes and `..` patterns are
unaffected, so the migration is a constructor plus assignment:

```rust
# use dataflow_rs::{FunctionConfig, Task};
# use serde_json::json;
# fn _demo(function_config: FunctionConfig) {
let mut action = Task::action("apply_discount", "Apply Discount", function_config);
action.condition = json!({">=": [{"var": "data.order.total"}, 1000]});
action.continue_on_error = true;
action.terminal = true;
# }
```

`Workflow::new()`, `Workflow::rule()` and `Workflow::from_json()` are the
equivalents for a rule, which is `#[non_exhaustive]` for the same reason.
`TaskGroup` gets no constructor: groups are produced by the parser, and their
`end` field indexes the *flattened* task list, so building one by hand was never
meaningful.

## Function Configuration

The `function` object specifies what the action does:

```json
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
| `filter` | Pipeline control flow — halt workflow or skip task |
| `log` | Structured logging with JSONLogic expressions |
| `parse_json` | Parse JSON from payload into data context |
| `parse_xml` | Parse XML string into JSON data structure |
| `publish_json` | Serialize data to JSON string |
| `publish_xml` | Serialize data to XML string |

### Custom Functions

Register custom handlers via the engine builder:

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
# fn _demo(rules: Vec<Workflow>) -> Result<()> {
let engine = Engine::builder()
    .with_workflows(rules)
    .register("my_custom_function", MyCustomFunction)
    .build()?;
# Ok(()) }
```

Then reference them by name in actions:

```json
{
    "function": {
        "name": "my_custom_function",
        "input": { ... }
    }
}
```

## Conditional Execution

Actions can have conditions that determine if they should run. Conditions evaluate against the **full context** (`data`, `metadata`, `temp_data`), and may read `{"secret": "name"}` from the engine's [secret store](../advanced/secrets.md) — a condition collapses to a bool, so nothing of the value is recorded:

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

### Common Patterns

```json
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

## Error Handling

### Action-Level Error Handling

```json
{
    "id": "optional_action",
    "continue_on_error": true,
    "function": { ... }
}
```

When `continue_on_error` is true:
- Action errors are recorded in `message.errors()`
- Rule continues to the next action

### Rule-Level Error Handling

The rule's own `continue_on_error` is a separate switch, not a default for its
actions: it decides whether *later rules* still run once this rule has failed.
An action that omits the flag stops its rule on failure no matter what the rule
says. See [Error Handling](./error-handling.md#rule-level-error-handling).

## Sequential Execution

Actions execute in order within a rule. Later actions can use results from earlier actions:

```json
{
    "tasks": [
        {
            "id": "step1",
            "name": "Step1",
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
            "name": "Step2",
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

## Try It

> **Want more features?** Try the [Full Debugger UI](/dataflow-rs/debugger/) with step-by-step execution and rule visualization.

<div class="playground-widget" data-workflows='[{"id":"conditional_tasks","name":"Conditional Actions","tasks":[{"id":"parse","name":"Parse Payload","function":{"name":"parse_json","input":{"source":"payload","target":"input"}}},{"id":"check_premium","name":"Check Premium","condition":{"==":[{"var":"data.input.tier"},"premium"]},"function":{"name":"map","input":{"mappings":[{"path":"data.discount","logic":20}]}}},{"id":"check_standard","name":"Check Standard","condition":{"==":[{"var":"data.input.tier"},"standard"]},"function":{"name":"map","input":{"mappings":[{"path":"data.discount","logic":5}]}}},{"id":"apply_discount","name":"Apply Discount","function":{"name":"map","input":{"mappings":[{"path":"data.final_price","logic":{"-":[{"var":"data.input.price"},{"/":[{"*":[{"var":"data.input.price"},{"var":"data.discount"}]},100]}]}}]}}}]}]' data-payload='{"tier":"premium","price":100}'>
</div>

Try changing `tier` to "standard" to see different discount applied.

## Best Practices

1. **Unique IDs** - Use descriptive, unique IDs for debugging
2. **Single Responsibility** - Each action should do one thing well
3. **Use temp_data** - Store intermediate results in `temp_data`
4. **Conditions** - Add conditions to skip unnecessary processing
5. **Error Handling** - Use `continue_on_error` for optional actions
