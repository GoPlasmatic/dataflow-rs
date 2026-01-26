# Workflow

A Workflow is a collection of tasks that execute sequentially to process data.

## Overview

Workflows provide:

- **Task Organization** - Group related processing steps
- **Priority Ordering** - Control execution order across workflows
- **Conditional Execution** - Only run when conditions are met
- **Error Handling** - Continue or stop on errors

## Workflow Structure

```json
{
    "id": "user_processor",
    "name": "User Processor",
    "priority": 1,
    "condition": { "==": [{"var": "metadata.type"}, "user"] },
    "continue_on_error": false,
    "tasks": [
        {
            "id": "validate_user",
            "name": "Validate User",
            "function": { ... }
        },
        {
            "id": "enrich_user",
            "name": "Enrich User Data",
            "function": { ... }
        }
    ]
}
```

## Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `id` | string | Yes | Unique workflow identifier |
| `name` | string | No | Human-readable name |
| `priority` | number | No | Execution order (default: 0) |
| `condition` | JSONLogic | No | When to execute workflow |
| `continue_on_error` | boolean | No | Continue on task failure (default: false) |
| `tasks` | array | Yes | Tasks to execute |

## Creating Workflows

### From JSON String

```rust
use dataflow_rs::Workflow;

let workflow = Workflow::from_json(r#"{
    "id": "my_workflow",
    "name": "My Workflow",
    "tasks": [...]
}"#)?;
```

### From File

```rust
let workflow = Workflow::from_file("workflows/my_workflow.json")?;
```

## Priority Ordering

Workflows execute in priority order (lowest first):

```json
// Executes first (priority 1)
{
    "id": "validation",
    "priority": 1,
    "tasks": [...]
}

// Executes second (priority 2)
{
    "id": "transformation",
    "priority": 2,
    "tasks": [...]
}

// Executes last (priority 10)
{
    "id": "notification",
    "priority": 10,
    "tasks": [...]
}
```

## Conditional Execution

Use JSONLogic conditions to control when workflows run:

```json
{
    "id": "premium_user_workflow",
    "condition": {
        "and": [
            {"==": [{"var": "metadata.type"}, "user"]},
            {"==": [{"var": "data.premium"}, true]}
        ]
    },
    "tasks": [...]
}
```

### Common Condition Patterns

```json
// Match metadata type
{"==": [{"var": "metadata.type"}, "order"]}

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
    "id": "strict_workflow",
    "continue_on_error": false,
    "tasks": [...]
}
```

If any task fails, the workflow stops and the error is recorded.

### Continue on Error

```json
{
    "id": "resilient_workflow",
    "continue_on_error": true,
    "tasks": [...]
}
```

Tasks continue executing even if previous tasks fail. Errors are collected in `message.errors`.

## Task Dependencies

Tasks within a workflow execute sequentially, allowing later tasks to depend on earlier results:

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

> **Want more features?** Try the [Full Debugger UI](/dataflow-rs/debugger/) with step-by-step execution and workflow visualization.

<div class="playground-widget" data-workflows='[{"id":"conditional_workflow","name":"Conditional Workflow","condition":{"==":[{"var":"metadata.type"},"user"]},"tasks":[{"id":"greet","name":"Greet User","function":{"name":"map","input":{"mappings":[{"path":"data.greeting","logic":{"cat":["Welcome, ",{"var":"data.name"},"!"]}}]}}}]}]' data-message='{"data":{"name":"Alice"},"metadata":{"type":"user"}}'>
</div>

Try changing `metadata.type` to something other than "user" to see the workflow skip.
