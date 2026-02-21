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
| `name` | string | No | Human-readable name |
| `condition` | JSONLogic | No | When to execute action (evaluated against full context) |
| `continue_on_error` | boolean | No | Continue rule on failure |
| `function` | object | Yes | Function to execute |

## Creating Actions Programmatically

```rust
use dataflow_rs::{Action, FunctionConfig};

let action = Action::action(
    "apply_discount",
    "Apply Discount",
    function_config,
);
```

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
| `parse_json` | Parse JSON from payload into data context |
| `parse_xml` | Parse XML string into JSON data structure |
| `publish_json` | Serialize data to JSON string |
| `publish_xml` | Serialize data to XML string |

### Custom Functions

Register custom functions when creating the engine:

```rust
let engine = Engine::new(rules, Some(custom_functions));
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

Actions can have conditions that determine if they should run. Conditions evaluate against the **full context** (`data`, `metadata`, `temp_data`):

```json
{
    "id": "premium_greeting",
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
- Action errors are recorded in `message.errors`
- Rule continues to the next action

### Rule-Level Error Handling

The rule's `continue_on_error` setting applies to all actions unless overridden.

## Sequential Execution

Actions execute in order within a rule. Later actions can use results from earlier actions:

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

## Try It

> **Want more features?** Try the [Full Debugger UI](/dataflow-rs/debugger/) with step-by-step execution and rule visualization.

<div class="playground-widget" data-workflows='[{"id":"conditional_tasks","name":"Conditional Actions","tasks":[{"id":"parse","name":"Parse Payload","function":{"name":"parse_json","input":{"source":"payload","target":"input"}}},{"id":"check_premium","name":"Check Premium","condition":{"==":[{"var":"data.input.tier"},"premium"]},"function":{"name":"map","input":{"mappings":[{"path":"data.discount","logic":20}]}}},{"id":"check_standard","name":"Check Standard","condition":{"==":[{"var":"data.input.tier"},"standard"]},"function":{"name":"map","input":{"mappings":[{"path":"data.discount","logic":5}]}}},{"id":"apply_discount","name":"Apply Discount","function":{"name":"map","input":{"mappings":[{"path":"data.final_price","logic":{"-":[{"var":"data.input.price"},{"/":{"*":[{"var":"data.input.price"},{"var":"data.discount"}]},100}]}}]}}}]}]' data-payload='{"tier":"premium","price":100}'>
</div>

Try changing `tier` to "standard" to see different discount applied.

## Best Practices

1. **Unique IDs** - Use descriptive, unique IDs for debugging
2. **Single Responsibility** - Each action should do one thing well
3. **Use temp_data** - Store intermediate results in `temp_data`
4. **Conditions** - Add conditions to skip unnecessary processing
5. **Error Handling** - Use `continue_on_error` for optional actions
