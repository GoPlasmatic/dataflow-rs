# Rules (Workflows)

A Rule (also called Workflow) is a collection of actions that execute sequentially when a condition is met. This is the core **IF → THEN** unit: **IF** condition matches, **THEN** execute actions.

## Overview

Rules provide:

- **Conditional Execution** - Only run when JSONLogic conditions are met (against full context: `data`, `metadata`, `temp_data`)
- **Priority Ordering** - Control execution order across rules
- **Action Organization** - Group related processing steps
- **Error Handling** - Continue or stop on errors

## Rule Structure

```json
{
    "id": "premium_order",
    "name": "Premium Order Processing",
    "priority": 1,
    "condition": { ">=": [{"var": "data.order.total"}, 1000] },
    "continue_on_error": false,
    "tasks": [
        {
            "id": "apply_discount",
            "name": "Apply Discount",
            "function": { ... }
        },
        {
            "id": "notify_manager",
            "name": "Notify Manager",
            "function": { ... }
        }
    ]
}
```

## Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `id` | string | Yes | Unique rule identifier |
| `name` | string | No | Human-readable name |
| `priority` | number | No | Execution order (default: 0, lower = first) |
| `condition` | JSONLogic | No | When to execute rule (evaluated against full context) |
| `continue_on_error` | boolean | No | Continue on action failure (default: false) |
| `tasks` | array | Yes | Actions to execute |

## Creating Rules

### From JSON String

```rust
use dataflow_rs::Workflow;

let rule = Workflow::from_json(r#"{
    "id": "my_rule",
    "name": "My Rule",
    "tasks": [...]
}"#)?;
```

### Using the Convenience Constructor

```rust
use dataflow_rs::{Rule, Task};
use serde_json::json;

let rule = Rule::rule(
    "premium_discount",
    "Premium Discount",
    json!({">=": [{"var": "data.order.total"}, 1000]}),
    vec![/* actions */],
);
```

### From File

```rust
let rule = Workflow::from_file("rules/my_rule.json")?;
```

## Priority Ordering

Rules execute in priority order (lowest first). This enables the **THAT** (chaining) in the IF → THEN → THAT model:

```json
// Executes first (priority 1) — validate input
{
    "id": "validation",
    "priority": 1,
    "tasks": [...]
}

// Executes second (priority 2) — transform data
{
    "id": "transformation",
    "priority": 2,
    "tasks": [...]
}

// Executes last (priority 10) — send notifications
{
    "id": "notification",
    "priority": 10,
    "tasks": [...]
}
```

## Conditional Execution

Use JSONLogic conditions to control when rules run. Conditions evaluate against the **full message context** — `data`, `metadata`, and `temp_data`:

```json
{
    "id": "premium_user_rule",
    "condition": {
        "and": [
            {">=": [{"var": "data.order.total"}, 500]},
            {"==": [{"var": "data.user.is_vip"}, true]}
        ]
    },
    "tasks": [...]
}
```

### Common Condition Patterns

```json
// Match on data fields
{">=": [{"var": "data.order.total"}, 1000]}

// Check data exists
{"!!": {"var": "data.email"}}

// Multiple conditions
{"and": [
    {">=": [{"var": "data.amount"}, 100]},
    {"==": [{"var": "data.currency"}, "USD"]}
]}

// Either condition
{"or": [
    {"==": [{"var": "metadata.source"}, "api"]},
    {"==": [{"var": "metadata.source"}, "webhook"]}
]}
```

## Error Handling

### Stop on Error (Default)

```json
{
    "id": "strict_rule",
    "continue_on_error": false,
    "tasks": [...]
}
```

If any action fails, the rule stops and the error is recorded.

### Continue on Error

```json
{
    "id": "resilient_rule",
    "continue_on_error": true,
    "tasks": [...]
}
```

Actions continue executing even if previous actions fail. Errors are collected in `message.errors`.

## Action Dependencies

Actions within a rule execute sequentially, allowing later actions to depend on earlier results:

```json
{
    "id": "pipeline",
    "tasks": [
        {
            "id": "fetch_data",
            "function": {
                "name": "map",
                "input": {
                    "mappings": [
                        {"path": "temp_data.fetched", "logic": {"var": "data.source"}}
                    ]
                }
            }
        },
        {
            "id": "process_data",
            "function": {
                "name": "map",
                "input": {
                    "mappings": [
                        {"path": "data.result", "logic": {"var": "temp_data.fetched"}}
                    ]
                }
            }
        }
    ]
}
```

## Try It

> **Want more features?** Try the [Full Debugger UI](/dataflow-rs/debugger/) with step-by-step execution and rule visualization.

<div class="playground-widget" data-workflows='[{"id":"parse_workflow","name":"Parse Input","priority":1,"tasks":[{"id":"parse","name":"Parse Payload","function":{"name":"parse_json","input":{"source":"payload","target":"input"}}}]},{"id":"conditional_workflow","name":"Conditional Rule","priority":2,"condition":{"==":[{"var":"data.input.role"},"admin"]},"tasks":[{"id":"greet","name":"Greet User","function":{"name":"map","input":{"mappings":[{"path":"data.greeting","logic":{"cat":["Welcome, ",{"var":"data.input.name"},"!"]}}]}}}]}]' data-payload='{"name":"Alice","role":"admin"}'>
</div>

Try changing `role` to something other than "admin" to see the conditional rule skip.
