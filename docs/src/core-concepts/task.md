# Task

A Task is an individual processing unit within a workflow that executes a function.

## Overview

Tasks are the building blocks of workflows. Each task:

- Executes a single function (built-in or custom)
- Can have a condition for conditional execution
- Can modify message data
- Records changes in the audit trail

## Task Structure

```json
{
    "id": "transform_data",
    "name": "Transform Data",
    "condition": { "!!": {"var": "data.input"} },
    "continue_on_error": false,
    "function": {
        "name": "map",
        "input": {
            "mappings": [
                {
                    "path": "data.output",
                    "logic": {"var": "data.input"}
                }
            ]
        }
    }
}
```

## Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `id` | string | Yes | Unique task identifier within workflow |
| `name` | string | No | Human-readable name |
| `condition` | JSONLogic | No | When to execute task |
| `continue_on_error` | boolean | No | Continue workflow on failure |
| `function` | object | Yes | Function to execute |

## Function Configuration

The `function` object specifies what the task does:

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

### Custom Functions

Register custom functions when creating the engine:

```rust
let engine = Engine::new(workflows, Some(custom_functions));
```

Then reference them by name in tasks:

```json
{
    "function": {
        "name": "my_custom_function",
        "input": { ... }
    }
}
```

## Conditional Execution

Tasks can have conditions that determine if they should run:

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

### Task-Level Error Handling

```json
{
    "id": "optional_task",
    "continue_on_error": true,
    "function": { ... }
}
```

When `continue_on_error` is true:
- Task errors are recorded in `message.errors`
- Workflow continues to the next task

### Workflow-Level Error Handling

The workflow's `continue_on_error` setting applies to all tasks unless overridden.

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

## Try It

<div class="playground-widget" data-workflows='[{"id":"conditional_tasks","name":"Conditional Tasks","tasks":[{"id":"check_premium","name":"Check Premium","condition":{"==":[{"var":"data.tier"},"premium"]},"function":{"name":"map","input":{"mappings":[{"path":"data.discount","logic":20}]}}},{"id":"check_standard","name":"Check Standard","condition":{"==":[{"var":"data.tier"},"standard"]},"function":{"name":"map","input":{"mappings":[{"path":"data.discount","logic":5}]}}},{"id":"apply_discount","name":"Apply Discount","function":{"name":"map","input":{"mappings":[{"path":"data.final_price","logic":{"-":[{"var":"data.price"},{"/":{"*":[{"var":"data.price"},{"var":"data.discount"}]},100}]}}]}}}]}]' data-message='{"data":{"tier":"premium","price":100},"metadata":{}}'>
</div>

Try changing `tier` to "standard" to see different discount applied.

## Best Practices

1. **Unique IDs** - Use descriptive, unique IDs for debugging
2. **Single Responsibility** - Each task should do one thing well
3. **Use temp_data** - Store intermediate results in `temp_data`
4. **Conditions** - Add conditions to skip unnecessary processing
5. **Error Handling** - Use `continue_on_error` for optional tasks
